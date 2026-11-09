//! The dictation controller: the hotkey, the microphone, the live text, the stop pipeline, the paste into the focused field
//! and the panels, connected. Everything that touches the system is behind a trait in [`Deps`], so that the tests run it all
//! with fakes and a clock that only moves when the test says so.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use chrono::NaiveDateTime;
use dictate_services::audio::capture::{CaptureError, CaptureEvent};
use dictate_services::audio::{level, resample};
use dictate_services::dictation::lang::display_hotkey;
use dictate_services::dictation::machine::{self, Action, Phase};
use dictate_services::dictation::pipeline::{
    self, Backend, Notice, NoticeKey, Outcome, PipelineConfig, Progress, StopInput,
};
use dictate_services::presets::{find_edit_preset, EDIT_PRESETS};
use dictate_services::secrets::Account;
use dictate_services::settings::{self, Settings, SettingsStore};
use tokio_util::sync::CancellationToken;

use crate::config;
use crate::dictation::chip::{self, ChipTimer};
use crate::dictation::live::LiveText;
use crate::dictation::save::{self, SavePaths, Take};
use crate::dictation::target::{self, CardReason, Facts, Target};
use crate::hotkey::HotkeyEvent;
use crate::panel::{ErrorAction, PanelError, PanelState, View, WorkKind, Working};
use crate::platform::monitor::InputMonitor;
use crate::platform::paste::{landed, Clipboard, CopyReason, KeySender, PasteResult, Paster};
use crate::platform::rules::Os;
use crate::platform::{AppId, FieldLookup, Permission, Platform, Rect};
use crate::state::{lock, Job, JobGuard, Jobs, KeyFlags};

/// What the controller tells the time with, and waits with.
pub trait Clock: Send + Sync {
    /// A reading of the clock that only goes forward.
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
    /// The local date and time, for the names of saved files.
    fn wall(&self) -> NaiveDateTime;
}

/// A recording that is running. It is made, used and stopped by one thread: the microphone of some systems can't be moved
/// to another.
pub trait RunningCapture {
    /// The rate in Hz of the samples that the recording sends.
    fn sample_rate(&self) -> u32;
    /// Ends the recording. What was sent already stays in the channel.
    fn stop(self: Box<Self>);
}

/// What starts a recording from the microphone.
pub trait CaptureFactory: Send + Sync {
    /// Starts sending [`CaptureEvent`]s to `tx`.
    fn start(&self, tx: Sender<CaptureEvent>) -> Result<Box<dyn RunningCapture>, CaptureError>;
}

/// Where the state of the panels goes: the two floating windows, or a fake.
pub trait PanelSink: Send + Sync {
    /// Draws this state, and shows or hides the windows to fit it.
    fn show(&self, state: &PanelState);
    /// The next panels go next to this rectangle (the caret or the field), or at the bottom centre of the screen.
    fn set_anchor(&self, anchor: Option<Rect>);
    /// Opens Settings, where a Whisper model is downloaded.
    fn open_settings(&self);
}

/// Local speech recognition.
pub trait LocalStt: Send + Sync {
    /// The text of 16 kHz mono audio. `language` is a Whisper language code ("en"), empty to have it detected; `prompt`
    /// is the vocabulary. Stops soon after `cancel` is cancelled, with an error. Blocks until it is done.
    fn transcribe(
        &self,
        samples_16k: &[f32],
        language: &str,
        prompt: &str,
        cancel: &CancellationToken,
    ) -> Result<String, String>;
    /// Whether there is a model to transcribe with (downloaded; not necessarily loaded yet).
    fn available(&self) -> bool;
}

/// Everything the controller touches that isn't its own.
pub struct Deps {
    pub platform: Arc<dyn Platform>,
    pub clipboard: Arc<dyn Clipboard>,
    pub keys: Arc<dyn KeySender>,
    /// The one paster of the app: it remembers the user's clipboard while a restore waits.
    pub paster: Arc<Paster>,
    pub monitor: Arc<dyn InputMonitor>,
    pub capture: Arc<dyn CaptureFactory>,
    pub whisper: Arc<dyn LocalStt>,
    pub backend: Arc<dyn Backend>,
    pub settings: Arc<SettingsStore>,
    /// Which keys exist. Read only after the key store was asked once (see `AppState::loaded_keys`).
    pub keys_flags: Arc<KeyFlags>,
    pub jobs: Arc<Jobs>,
    pub ui: Arc<dyn PanelSink>,
    pub files: SavePaths,
    pub clock: Arc<dyn Clock>,
    /// Forgets that the hotkey is down; called when a dictation ends (see `Hotkeys::reset`).
    pub hotkey_reset: Arc<dyn Fn() + Send + Sync>,
    pub os: Os,
    /// The process of this app: when it is the one in front, there is no field to write into.
    pub own_pid: u32,
}

/// How long the recording loop waits for the next buffer of audio before it looks at what else has happened.
const POLL: Duration = Duration::from_millis(20);
/// How often the live text is asked for again.
const LIVE_TICK: Duration = Duration::from_secs(1);
/// The most often that the level of the meter goes to the panels.
const METER_EVERY: Duration = Duration::from_millis(50);
/// How long after the paste the field is looked at, and how long after that once more.
const LANDING_FIRST: Duration = Duration::from_millis(350);
const LANDING_SECOND: Duration = Duration::from_millis(400);
/// The shortest recording that is transcribed or kept: a fifth of a second at 16 kHz. Whisper takes nothing shorter, and a
/// recording that short is a tap of the shortcut, not speech.
const MIN_TAKE_SAMPLES: usize = 3_200;
/// The key of the error that the microphone gives only silence.
const SILENT: &str = "audio.silent";
/// How long an app is given to come forward again, in real time: how often it is asked, and how many times.
const FOCUS_POLL: Duration = Duration::from_millis(25);
const FOCUS_POLLS: u32 = 20;
/// The most that the end of a dictation waits for the work it has to wait for (in real time).
const WAIT_LIMIT: Duration = Duration::from_secs(5);
/// What the answer to the question about a field is, when it doesn't come.
const NO_ANSWER: Target = Target::Card(CardReason::Unknown);

/// The answer to "where does the text go", and the app that had the focus.
#[derive(Debug, Clone)]
struct Lookup {
    target: Target,
    previous: Option<AppId>,
    /// macOS without the Accessibility permission, with the setting that writes into fields on.
    accessibility_missing: bool,
}

/// One piece of work for a dictation: the recording and what follows it up to the text in the field, or a rewrite. A newer
/// one replaces it.
struct Run {
    id: u64,
    /// Cancelled when the work is replaced, dropped or told to quit.
    token: CancellationToken,
    /// Cancelled when the person asks for the text as it is, while the AI edits it.
    skip: CancellationToken,
    /// Ends the live transcription once the recording is over.
    live_cancel: CancellationToken,
    stop: bool,
    lookup: Option<Lookup>,
}

/// The result chip that is up.
struct ChipRun {
    id: u64,
    timer: ChipTimer,
    hover: bool,
    menu: bool,
}

/// What the app has written into a field: what the chip and the card are about.
#[derive(Debug, Clone)]
struct Written {
    text: String,
    original: Option<String>,
    style: Option<String>,
    app: AppId,
    terminal: bool,
    anchor: Option<Rect>,
    duration_s: f64,
}

struct Inner {
    next_id: u64,
    /// What the panels show, except what the settings give (the language, the shortcut).
    panel: PanelState,
    /// A paste is going on and can't be taken back.
    writing: bool,
    /// The text is in the field already (a rewrite is under way, or the chip is up).
    written: bool,
    run: Option<Run>,
    chip: Option<ChipRun>,
    last: Option<Written>,
    /// The app that had the focus when the dictation began: it has it back when the card closes.
    previous: Option<AppId>,
    /// The text of the box of the card, as the card last said it.
    card_text: String,
    last_meter: Option<Instant>,
    /// The number of the state that was made last.
    seq: u64,
}

impl Inner {
    fn new() -> Self {
        Inner {
            next_id: 1,
            panel: PanelState::hidden(),
            writing: false,
            written: false,
            run: None,
            chip: None,
            last: None,
            previous: None,
            card_text: String::new(),
            last_meter: None,
            seq: 0,
        }
    }

    fn is_current(&self, id: u64) -> bool {
        self.run.as_ref().is_some_and(|run| run.id == id)
    }
}

/// How the recording ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// The person stopped it.
    Stopped,
    /// The microphone broke off: it is stopped as if the person had.
    Broken,
    /// Dropped or replaced.
    Gone,
}

struct Recording {
    raw: Vec<f32>,
    rate: u32,
    started: Instant,
    ended: Instant,
    end: End,
    /// What the system said when the stream broke off.
    broken: Option<String>,
    /// The microphone gave nothing but silence at the start.
    silent: bool,
}

/// Where the text was written, and what the chip then says.
struct Delivery {
    outcome: Outcome,
    duration_s: f64,
    audio_error: Option<PanelError>,
}

/// The dictation controller.
pub struct Controller {
    deps: Deps,
    me: Weak<Controller>,
    inner: Mutex<Inner>,
    /// Wakes whoever waits for something in `inner` (the field was found).
    changed: Condvar,
    /// The number of the state that went out last, so that an older one never follows a newer one.
    emitted: Mutex<u64>,
}

impl Controller {
    pub fn new(deps: Deps) -> Arc<Controller> {
        // What an earlier run left in the temporary folder is of no use to anyone.
        let swept = save::sweep_temp(&deps.files.temp);
        if swept > 0 {
            log::info!("Deleted {swept} temporary recording(s) of an earlier run");
        }
        Arc::new_cyclic(|me| Controller {
            deps,
            me: me.clone(),
            inner: Mutex::new(Inner::new()),
            changed: Condvar::new(),
            emitted: Mutex::new(0),
        })
    }

    fn this(&self) -> Arc<Controller> {
        self.me
            .upgrade()
            .expect("the controller is alive while it is in use")
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        lock(&self.inner)
    }

    /// Runs `work` on a thread of its own. Work that waits for the system, or for the network, never runs on the thread of
    /// the shortcut or of a command.
    fn spawn(&self, name: &str, work: impl FnOnce(Arc<Controller>) + Send + 'static) {
        let me = self.this();
        if let Err(e) = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || work(me))
        {
            log::error!("Could not start the thread {name}: {e}");
        }
    }

    // ---- what the panels are told -----------------------------------------------------------------------------------

    /// The panel as it is now: what the dictation made of it, and the settings' words (the language, the shortcut). `new_state`
    /// numbers it as a state that is about to be sent.
    fn snapshot(&self, new_state: bool) -> (PanelState, u64) {
        // The settings are read before the lock is taken: they are locked while a file is written.
        let settings = self.deps.settings.get();
        let mut inner = self.inner();
        if new_state {
            inner.seq += 1;
        }
        let mut state = inner.panel.clone();
        state.language = settings.dictation_language;
        state.hotkey = display_hotkey(&settings.hotkey, self.deps.os == Os::Mac);
        state.push_to_talk = settings.push_to_talk;
        (state, inner.seq)
    }

    /// Sends the panel as it is now to the windows. Never with a lock held: showing a window asks the system. Of two states
    /// that come from two threads, the older is dropped if the newer went out already.
    fn publish(&self) {
        let (state, seq) = self.snapshot(true);
        let mut emitted = lock(&self.emitted);
        if seq > *emitted {
            *emitted = seq;
            self.deps.ui.show(&state);
        }
    }

    /// The panel as it is now, for a page that has just loaded.
    pub fn state(&self) -> PanelState {
        self.snapshot(false).0
    }

    // ---- the hotkey ---------------------------------------------------------------------------------------------------

    /// What the shortcut, the tray or `dictate --toggle` says. A bug in the handling ends the dictation that is current, and no
    /// more: the shortcut works for the next event.
    pub fn on_hotkey(&self, event: HotkeyEvent) {
        if catch_unwind(AssertUnwindSafe(|| self.handle_hotkey(event))).is_err() {
            log::error!("The shortcut was handled with a bug: ending the dictation");
            self.end_whatever_is_going_on();
        }
    }

    /// Everything is dropped and the panels hide: the way out of a state that can't be trusted. It may fail on its own, which
    /// must not take the thread down either.
    fn end_whatever_is_going_on(&self) {
        let ended = catch_unwind(AssertUnwindSafe(|| {
            {
                let mut inner = self.inner();
                self.drop_everything(&mut inner);
            }
            self.deps.monitor.stop();
            self.publish();
            (self.deps.hotkey_reset)();
        }));
        if ended.is_err() {
            log::error!("Could not end the dictation after a bug");
        }
    }

    fn handle_hotkey(&self, event: HotkeyEvent) {
        let hold_mode = self.deps.settings.get().push_to_talk;
        let action = {
            let inner = self.inner();
            let phase = inner.panel.phase;
            match event {
                HotkeyEvent::Pressed => {
                    machine::on_press(phase, hold_mode, inner.writing, inner.written)
                }
                // `dictate --toggle` is a press of a toggle, whatever the mode.
                HotkeyEvent::Toggle => {
                    machine::on_press(phase, false, inner.writing, inner.written)
                }
                HotkeyEvent::Released => machine::on_release(phase, hold_mode),
            }
        };
        log::debug!("The shortcut says {event:?}: {action:?}");
        match action {
            Action::Start => self.start(),
            // The press that stops a toggle dictation: the key is still down, the gate of the shortcut knows it, and telling it
            // otherwise would make the repeats of a held key presses.
            Action::Stop => self.stop_recording(event != HotkeyEvent::Pressed),
            Action::WriteNow => self.write_now(),
            Action::Ignore => {}
        }
    }

    /// Starts a recording: whatever else is up goes away, and a thread finds the text field while another listens.
    fn start(&self) {
        let (id, guard, give_back) = {
            let mut inner = self.inner();
            // The pill and the card made this app the active one: the dictation that starts now is about the app before it.
            let give_back = match inner.panel.view {
                View::Pill | View::Card => inner.previous.take(),
                _ => None,
            };
            let (id, guard) = self.begin_run(&mut inner);
            inner.chip = None;
            inner.last = None;
            inner.previous = None;
            inner.card_text.clear();
            inner.writing = false;
            inner.written = false;
            inner.last_meter = None;
            inner.panel = PanelState {
                phase: Phase::Recording,
                ..PanelState::hidden()
            };
            (id, guard, give_back)
        };
        let Some(guard) = guard else {
            log::error!("Could not register the dictation");
            return;
        };
        // The chip's watch on the keyboard goes with the chip.
        self.deps.monitor.stop();
        // Our own panels first: the question about the field is about the app behind them.
        self.publish();

        let settings = self.deps.settings.get();
        self.spawn("dictation-lookup", move |me| {
            if let Some(app) = give_back {
                me.take_the_focus_back_from_this_app(&app);
            }
            me.look_for_the_field(id, settings);
        });
        self.spawn("dictation", move |me| me.dictation(id, guard));
    }

    /// Replaces the work that is going on with new work. The old one is told to stop and finds that out by its number.
    fn begin_run(&self, inner: &mut Inner) -> (u64, Option<JobGuard>) {
        if let Some(old) = inner.run.take() {
            old.token.cancel();
        }
        let id = inner.next_id;
        inner.next_id += 1;
        let guard = self.deps.jobs.start(Job::Dictation(id)).ok();
        let token = guard.as_ref().map(JobGuard::token).unwrap_or_default();
        inner.run = Some(Run {
            id,
            live_cancel: token.child_token(),
            token,
            skip: CancellationToken::new(),
            stop: false,
            lookup: None,
        });
        (id, guard)
    }

    /// The Stop button of the pill and Return: stops the recording and writes.
    pub fn stop(&self) {
        self.stop_recording(true);
    }

    /// Stops the recording. `reset_shortcut` tells the gate of the shortcut that its key is up (see `Hotkeys::reset`).
    fn stop_recording(&self, reset_shortcut: bool) {
        {
            let mut inner = self.inner();
            if inner.panel.phase != Phase::Recording {
                return;
            }
            let Some(run) = inner.run.as_mut() else {
                return;
            };
            run.stop = true;
            inner.panel.phase = Phase::Finishing;
        }
        if reset_shortcut {
            (self.deps.hotkey_reset)();
        }
        self.publish();
    }

    /// The shortcut again, while the AI edits and nothing is written: write what was heard as it is.
    fn write_now(&self) {
        if let Some(run) = self.inner().run.as_ref() {
            run.skip.cancel();
        }
    }

    /// The cross of the bubble, Esc and Discard: drops the dictation. The audio that is saved stays, without its text.
    pub fn cancel(&self) {
        let (show_again, previous) = {
            let mut inner = self.inner();
            // A paste can't be taken back.
            if inner.writing {
                return;
            }
            let anything =
                inner.panel.phase != Phase::Hidden || inner.run.is_some() || inner.chip.is_some();
            if !anything {
                return;
            }
            let on_the_card = matches!(inner.panel.view, View::Pill | View::Card);
            let previous = inner.previous.take();
            self.drop_everything(&mut inner);
            (on_the_card, previous)
        };
        self.deps.monitor.stop();
        self.publish();
        (self.deps.hotkey_reset)();
        if show_again {
            self.give_the_focus_back(previous);
        }
    }

    /// Nothing is going on: the panels hide, and the work that is under way is told to stop.
    fn drop_everything(&self, inner: &mut Inner) {
        if let Some(run) = inner.run.take() {
            run.token.cancel();
        }
        inner.chip = None;
        inner.last = None;
        inner.writing = false;
        inner.written = false;
        inner.card_text.clear();
        inner.panel = PanelState::hidden();
    }

    /// The card made this app the active one: the app that had the focus before it is asked to take it back.
    fn give_the_focus_back(&self, previous: Option<AppId>) {
        let Some(app) = previous else {
            return;
        };
        if app.id.is_empty() || app.pid == Some(self.deps.own_pid) {
            return;
        }
        let keys = self.deps.keys.clone();
        self.spawn("dictation-focus", move |_| {
            keys.bring_forward(&app);
        });
    }

    // ---- finding the field --------------------------------------------------------------------------------------------

    /// Asks the app that had the focus before the card to take it back, and waits a moment for it to do so: the field to write
    /// into is the one in that app, not the card.
    fn take_the_focus_back_from_this_app(&self, app: &AppId) {
        if app.id.is_empty() || app.pid == Some(self.deps.own_pid) {
            return;
        }
        for _ in 0..FOCUS_POLLS {
            if self.deps.keys.bring_forward(app)
                || self
                    .deps
                    .platform
                    .frontmost_app()
                    .is_some_and(|front| crate::platform::paste::is_app(&front, app))
            {
                return;
            }
            thread::sleep(FOCUS_POLL);
        }
    }

    /// Finds out where the text will go, on a thread of its own: asking another app can take its time, and the thread of
    /// the shortcut must not wait for it. Then the bubble or the pill comes up.
    fn look_for_the_field(&self, id: u64, settings: Settings) {
        let platform = &self.deps.platform;
        let front = platform.frontmost_app();
        let facts = Facts {
            os: self.deps.os,
            auto_write: settings.auto_write,
            accessibility: platform.accessibility(),
            own_pid: self.deps.own_pid,
        };
        let target = target::decide(
            facts,
            front.as_ref(),
            || platform.focused_field(),
            || platform.mouse_position(),
        );
        let (anchor, previous) = match &target {
            Target::Inline { app, anchor, .. } => (*anchor, Some(app.clone())),
            Target::Card(_) => (None, front),
        };
        let accessibility_missing = settings.auto_write
            && self.deps.os == Os::Mac
            && facts.accessibility != Permission::Granted;
        {
            let mut inner = self.inner();
            if !inner.is_current(id) {
                return;
            }
            inner.previous = previous.clone();
            if matches!(inner.panel.phase, Phase::Recording | Phase::Finishing) {
                inner.panel.view = if target.is_inline() {
                    View::Bubble
                } else {
                    View::Pill
                };
            }
            if let Some(run) = inner.run.as_mut() {
                run.lookup = Some(Lookup {
                    target,
                    previous,
                    accessibility_missing,
                });
            }
        }
        self.changed.notify_all();
        self.deps.ui.set_anchor(anchor);
        self.publish();
    }

    /// Waits until the field is found. `None` when the dictation was dropped meanwhile. A lookup that never ends is taken
    /// for no answer.
    fn wait_for_lookup(&self, id: u64) -> Option<Lookup> {
        let until = Instant::now() + WAIT_LIMIT;
        let mut inner = self.inner();
        loop {
            let run = inner.run.as_ref().filter(|run| run.id == id)?;
            if let Some(lookup) = &run.lookup {
                return Some(lookup.clone());
            }
            let Some(left) = until.checked_duration_since(Instant::now()) else {
                return Some(Lookup {
                    target: NO_ANSWER,
                    previous: None,
                    accessibility_missing: false,
                });
            };
            inner = self
                .changed
                .wait_timeout(inner, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    // ---- one dictation -----------------------------------------------------------------------------------------------

    /// The whole of a dictation, on its own thread: the checks, the recording, and everything after it.
    fn dictation(&self, id: u64, guard: JobGuard) {
        let outcome = catch_unwind(AssertUnwindSafe(|| self.dictate(id, &guard.token())));
        if outcome.is_err() {
            log::error!("The dictation stopped because of a bug");
            self.end_without_text(id);
        }
        // The job is over when the thread is: quitting waits for it.
        drop(guard);
    }

    /// The dictation ends with nothing to show: the panels must not stay up for a recording that is not running (a bug in the
    /// work, or a shortcut pressed while the system asks for the microphone). Does nothing when the work was replaced.
    fn end_without_text(&self, id: u64) {
        let mine = {
            let mut inner = self.inner();
            let mine = inner.is_current(id);
            if mine {
                self.drop_everything(&mut inner);
            }
            mine
        };
        if mine {
            self.deps.monitor.stop();
            self.publish();
            (self.deps.hotkey_reset)();
        }
    }

    fn dictate(&self, id: u64, token: &CancellationToken) {
        let settings = self.deps.settings.get();
        let flags = &self.deps.keys_flags;
        let local = self.deps.whisper.available();
        let api = config::dictation_api_plan(
            &settings,
            flags.get(Account::Ai),
            flags.get(Account::Transcribe),
        );
        // Neither a model to listen with nor a service to send the audio to: no recording for no one to hear.
        if !local && api.is_none() {
            return self.fail(
                id,
                PanelError {
                    action: Some(ErrorAction::OpenSettings),
                    ..PanelError::new("dictation.no_model")
                },
            );
        }
        match self.microphone_allowed(id) {
            Allowed::Yes => {}
            Allowed::No => return self.fail(id, PanelError::new("audio.denied")),
            Allowed::Gone => return self.end_without_text(id),
        }

        let (tx, rx) = mpsc::channel();
        let capture = match self.deps.capture.start(tx) {
            Ok(capture) => capture,
            Err(CaptureError::NoDevice) => {
                return self.fail(id, PanelError::new("audio.no_device"));
            }
            Err(CaptureError::Stream(detail)) => {
                return self.fail(id, PanelError::with_detail("audio.start_failed", detail));
            }
            Err(CaptureError::Silent) => return self.fail(id, PanelError::new(SILENT)),
        };
        let live = Arc::new(Mutex::new(LiveText::new()));
        let recording = self.record(id, token, capture, &rx, &live, local);
        self.finish(id, token, recording, &live, local);
    }

    /// Whether the microphone may be used: asks for it when the person has not been asked yet.
    fn microphone_allowed(&self, id: u64) -> Allowed {
        match self.deps.platform.microphone() {
            Permission::Granted | Permission::NotNeeded => Allowed::Yes,
            Permission::Denied => Allowed::No,
            Permission::NotDetermined => {
                let (answer, wait) = mpsc::channel();
                self.deps
                    .platform
                    .request_microphone(Box::new(move |granted| {
                        let _ = answer.send(granted);
                    }));
                loop {
                    match wait.recv_timeout(Duration::from_millis(100)) {
                        Ok(true) => return Allowed::Yes,
                        Ok(false) | Err(RecvTimeoutError::Disconnected) => return Allowed::No,
                        Err(RecvTimeoutError::Timeout) => {
                            let inner = self.inner();
                            // The shortcut again while the prompt is up: nothing was recorded, so there is nothing to write.
                            let stopped = inner.run.as_ref().is_some_and(|run| run.stop);
                            if !inner.is_current(id) || stopped {
                                return Allowed::Gone;
                            }
                        }
                    }
                }
            }
        }
    }

    /// The card with a reason why there is nothing to record or to write.
    fn fail(&self, id: u64, error: PanelError) {
        let Some(lookup) = self.wait_for_lookup(id) else {
            return;
        };
        {
            let mut inner = self.inner();
            if !inner.is_current(id) {
                return;
            }
            inner.run = None;
            inner.previous = lookup.previous;
            inner.card_text.clear();
            inner.panel = PanelState {
                view: View::Card,
                phase: Phase::Review,
                error: Some(error),
                accessibility_missing: lookup.accessibility_missing,
                ..PanelState::hidden()
            };
        }
        (self.deps.hotkey_reset)();
        self.publish();
    }

    // ---- recording ---------------------------------------------------------------------------------------------------

    /// Listens until the dictation is stopped, dropped or broken off, and keeps the live text up to date.
    fn record(
        &self,
        id: u64,
        token: &CancellationToken,
        capture: Box<dyn RunningCapture>,
        rx: &mpsc::Receiver<CaptureEvent>,
        live: &Arc<Mutex<LiveText>>,
        local: bool,
    ) -> Recording {
        let rate = capture.sample_rate();
        let started = self.deps.clock.now();
        let mut raw: Vec<f32> = Vec::new();
        let mut next_tick = started + LIVE_TICK;
        let mut broken = None;
        let mut silent = false;

        let end = loop {
            match rx.recv_timeout(POLL) {
                Ok(CaptureEvent::Samples(buffer)) => {
                    let now = self.deps.clock.now();
                    if self.meter(id, level::level(&buffer), now, started) {
                        silent = false;
                    }
                    raw.extend_from_slice(&buffer);
                }
                Ok(CaptureEvent::Error(CaptureError::Silent)) => {
                    silent = true;
                    self.set_error(id, Some(PanelError::new(SILENT)));
                }
                Ok(CaptureEvent::Error(error)) => {
                    broken = Some(match error {
                        CaptureError::Stream(detail) => detail,
                        other => other.to_string(),
                    });
                    self.stop_by_itself(id);
                    break End::Broken;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    broken = Some("the microphone stopped sending".to_owned());
                    self.stop_by_itself(id);
                    break End::Broken;
                }
            }
            match self.flow(id, token) {
                Flow::Going => {}
                Flow::Stop => break End::Stopped,
                Flow::Gone => break End::Gone,
            }
            let now = self.deps.clock.now();
            if local && now >= next_tick {
                next_tick = now + LIVE_TICK;
                self.live_tick(id, &raw, rate, live);
            }
        };
        let ended = self.deps.clock.now();
        capture.stop();
        // What the microphone sent before it was stopped is still in the channel.
        while let Ok(event) = rx.try_recv() {
            if let CaptureEvent::Samples(buffer) = event {
                raw.extend_from_slice(&buffer);
            }
        }
        Recording {
            raw,
            rate,
            started,
            ended,
            end,
            broken,
            silent,
        }
    }

    fn flow(&self, id: u64, token: &CancellationToken) -> Flow {
        let inner = self.inner();
        match inner.run.as_ref().filter(|run| run.id == id) {
            None => Flow::Gone,
            Some(_) if token.is_cancelled() => Flow::Gone,
            Some(run) if run.stop => Flow::Stop,
            Some(_) => Flow::Going,
        }
    }

    /// The microphone broke off: the dictation ends as if the person had stopped it.
    fn stop_by_itself(&self, id: u64) {
        {
            let mut inner = self.inner();
            if !inner.is_current(id) {
                return;
            }
            if let Some(run) = inner.run.as_mut() {
                run.stop = true;
            }
            if inner.panel.phase == Phase::Recording {
                inner.panel.phase = Phase::Finishing;
            }
        }
        (self.deps.hotkey_reset)();
        self.publish();
    }

    /// The level and the time of the recording go to the panels, not more than 20 times a second. A sound above the level of
    /// silence takes the notice that the microphone hears only silence away, at once; whether it did is the answer.
    fn meter(&self, id: u64, level: f32, now: Instant, started: Instant) -> bool {
        let mut heard_again = false;
        {
            let mut inner = self.inner();
            if !inner.is_current(id) || inner.panel.phase != Phase::Recording {
                return false;
            }
            inner.panel.level = level;
            inner.panel.duration_s = now.saturating_duration_since(started).as_secs_f64();
            if level > 0.0 && inner.panel.error.as_ref().is_some_and(|e| e.key == SILENT) {
                inner.panel.error = None;
                heard_again = true;
            }
            let due = inner
                .last_meter
                .is_none_or(|last| now.saturating_duration_since(last) >= METER_EVERY);
            if !due && !heard_again {
                return false;
            }
            inner.last_meter = Some(now);
        }
        self.publish();
        heard_again
    }

    fn set_error(&self, id: u64, error: Option<PanelError>) {
        {
            let mut inner = self.inner();
            if !inner.is_current(id) {
                return;
            }
            inner.panel.error = error;
        }
        self.publish();
    }

    /// Asks local speech recognition for the live text, if the last time's job is over.
    fn live_tick(&self, id: u64, raw: &[f32], rate: u32, live: &Arc<Mutex<LiveText>>) {
        let (job, samples, cancel) = {
            let mut text = lock(live);
            text.feed(raw, rate);
            let Some((job, samples)) = text.next_job() else {
                return;
            };
            let cancel = self
                .inner()
                .run
                .as_ref()
                .filter(|run| run.id == id)
                .map(|run| run.live_cancel.clone());
            let Some(cancel) = cancel else {
                text.finished(&job, Err("dropped".into()), "");
                return;
            };
            (job, samples, cancel)
        };
        let live = live.clone();
        self.spawn("dictation-live", move |me| {
            // The language is the one of this moment: the person may change it while talking.
            let settings = me.deps.settings.get();
            let lang = settings.dictation_language.clone();
            let code = config::whisper_language(&lang);
            let result = catch_unwind(AssertUnwindSafe(|| {
                me.deps
                    .whisper
                    .transcribe(&samples, &code, &settings.vocabulary, &cancel)
            }))
            .unwrap_or_else(|_| Err("the transcription stopped because of a bug".to_owned()))
            .map(|text| text.trim().to_owned());
            let (committed, partial) = {
                let mut text = lock(&live);
                text.finished(&job, result, &lang);
                let (committed, partial) = text.text();
                (committed.to_owned(), partial.to_owned())
            };
            {
                let mut inner = me.inner();
                if !inner.is_current(id) || inner.panel.phase != Phase::Recording {
                    return;
                }
                inner.panel.committed = committed;
                inner.panel.partial = partial;
            }
            me.publish();
        });
    }

    // ---- after the recording ---------------------------------------------------------------------------------------------

    /// The recording is over. The audio is kept, the whole of it is transcribed, the pipeline runs, and the text goes where
    /// the field was found.
    fn finish(
        &self,
        id: u64,
        token: &CancellationToken,
        recording: Recording,
        live: &Arc<Mutex<LiveText>>,
        local: bool,
    ) {
        let settings = self.deps.settings.get();
        let duration_s = recording
            .ended
            .saturating_duration_since(recording.started)
            .as_secs_f64();
        let mut samples = resample::to_16k(&recording.raw, recording.rate);
        if samples.len() < MIN_TAKE_SAMPLES {
            samples.clear();
        }

        // 1. The audio first: whatever happens next, what was said is kept (when the setting is on).
        let take = if samples.is_empty() {
            None
        } else {
            self.write_audio(&settings, &samples)
        };
        if recording.end == End::Gone || self.flow(id, token) == Flow::Gone {
            return self.abandon(take);
        }

        // 2. The live transcription ends, the field is found, and the whole take is transcribed once more.
        self.end_live(id, live);
        let Some(lookup) = self.wait_for_lookup(id) else {
            return self.abandon(take);
        };
        self.show_finishing(id, &lookup);

        let lang = settings.dictation_language.clone();
        let mut local_error = None;
        let local_text = if local && !samples.is_empty() {
            let code = config::whisper_language(&lang);
            match self
                .deps
                .whisper
                .transcribe(&samples, &code, &settings.vocabulary, token)
            {
                Ok(text) => text.trim().to_owned(),
                Err(_) if token.is_cancelled() => return self.abandon(take),
                Err(detail) => {
                    log::warn!("Local speech recognition failed: {detail}");
                    local_error = Some(detail);
                    lock(live).full(&lang)
                }
            }
        } else {
            String::new()
        };
        if self.flow(id, token) == Flow::Gone {
            return self.abandon(take);
        }

        // 3. The pipeline: the speech API hears it again, the AI edits it.
        let flags = &self.deps.keys_flags;
        let config = PipelineConfig {
            api: config::dictation_api_plan(
                &settings,
                flags.get(Account::Ai),
                flags.get(Account::Transcribe),
            ),
            ai_configured: settings::is_ai_configured(&settings, flags.get(Account::Ai)),
            auto_refine: settings.auto_refine,
            style: find_edit_preset(&settings.default_edit_preset),
        };
        let skip = self.skip_token(id);
        let Some(skip) = skip else {
            return self.abandon(take);
        };
        self.show_working(id, &lookup, &local_text);
        let guarded = Guarded::new(&*self.deps.backend, token.clone(), skip.clone());
        let outcome = tauri::async_runtime::block_on(pipeline::run(
            StopInput {
                local_text,
                duration_s,
                wav: take.as_ref().map(|take| take.wav.clone()),
            },
            &config,
            &guarded,
            &skip,
            |progress| self.progress(id, progress, &lookup, config.style.id),
        ));
        let mut outcome = outcome;
        if guarded.was_skipped() {
            // The call was dropped on purpose, for the text to be written at once: that is no failure to report.
            outcome.notice = outcome
                .notice
                .take()
                .filter(|notice| notice.key != NoticeKey::ApiFailed)
                .or_else(|| {
                    outcome.text.trim().is_empty().then_some(Notice {
                        key: NoticeKey::NothingHeard,
                        detail: None,
                    })
                });
        }
        if self.flow(id, token) == Flow::Gone {
            return self.abandon(take);
        }

        // 4. What was heard is kept with the audio, and the audio that is not kept goes.
        if let Some(take) = &take {
            if let Err(e) = save::write_text(take, &outcome.saved_text) {
                log::warn!("Could not save the text of the recording: {e}");
            }
        }
        self.abandon(take);

        let audio_error = if let Some(detail) = recording.broken {
            Some(PanelError::with_detail("audio.stream", detail))
        } else if recording.silent && outcome.text.trim().is_empty() {
            Some(PanelError::new(SILENT))
        } else {
            None
        };
        if outcome.text.trim().is_empty() {
            if let Some(detail) = local_error {
                return self.fail_with_text(
                    id,
                    &lookup,
                    PanelError::with_detail("dictation.local_failed", detail),
                );
            }
        }
        self.deliver(
            id,
            lookup,
            Delivery {
                outcome,
                duration_s,
                audio_error,
            },
        );
    }

    /// Writes the audio of the take where `save_recordings` says. The recording is lost only if no folder can be written to.
    fn write_audio(&self, settings: &Settings, samples: &[f32]) -> Option<Take> {
        match save::write_take(
            &self.deps.files,
            settings.save_recordings,
            self.deps.clock.wall(),
            samples,
        ) {
            Ok(take) => Some(take),
            Err(e) => {
                log::warn!("Could not write the recording: {e}");
                None
            }
        }
    }

    /// The take is no longer needed: a temporary file is deleted, a saved one stays.
    fn abandon(&self, take: Option<Take>) {
        if let Some(take) = take {
            save::discard_if_temporary(&take);
        }
    }

    /// The live transcription is stopped, and waited for: the whole take is transcribed next, and two transcriptions of one
    /// model at once would only slow each other down.
    fn end_live(&self, id: u64, live: &Arc<Mutex<LiveText>>) {
        if let Some(run) = self.inner().run.as_ref().filter(|run| run.id == id) {
            run.live_cancel.cancel();
        }
        let until = Instant::now() + WAIT_LIMIT;
        while lock(live).is_busy() && Instant::now() < until {
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn skip_token(&self, id: u64) -> Option<CancellationToken> {
        self.inner()
            .run
            .as_ref()
            .filter(|run| run.id == id)
            .map(|run| run.skip.clone())
    }

    /// The recording is over and the text is being made: the bubble says so (the pill too, once it knows).
    fn show_finishing(&self, id: u64, lookup: &Lookup) {
        {
            let mut inner = self.inner();
            if !inner.is_current(id) {
                return;
            }
            inner.panel.phase = Phase::Finishing;
            inner.panel.view = if lookup.target.is_inline() {
                View::Bubble
            } else {
                View::Pill
            };
            inner.panel.level = 0.0;
        }
        self.publish();
    }

    /// The pipeline begins, and the text as it was heard is what the panel holds: the bubble shows it as it goes on, and the card
    /// (which comes up when the AI or the API starts on the text, see `progress`) shows it until the result replaces it. With no
    /// field the pill stays until then.
    fn show_working(&self, id: u64, lookup: &Lookup, text: &str) {
        {
            let mut inner = self.inner();
            if !inner.is_current(id) {
                return;
            }
            inner.panel.text = text.to_owned();
            if lookup.target.is_inline() {
                inner.panel.phase = Phase::Working;
                inner.panel.working = Some(Working {
                    kind: WorkKind::Writing,
                    style: None,
                    model: None,
                    editing: false,
                });
            }
        }
        self.publish();
    }

    /// What the pipeline is doing now.
    fn progress(&self, id: u64, progress: Progress, lookup: &Lookup, style: &str) {
        let working = match progress {
            Progress::Retranscribing { model, editing } => Working {
                kind: WorkKind::Retranscribing,
                style: editing.then(|| style.to_owned()),
                model: Some(model),
                editing,
            },
            Progress::Editing { style_id } => Working {
                kind: WorkKind::Editing,
                style: Some(style_id.to_owned()),
                model: None,
                editing: false,
            },
        };
        {
            let mut inner = self.inner();
            if !inner.is_current(id) {
                return;
            }
            if !lookup.target.is_inline() {
                // The card comes up now and shows the AI at work in its header.
                inner.panel.view = View::Card;
                inner.panel.phase = Phase::Refining;
            }
            inner.panel.working = Some(working);
        }
        self.publish();
    }

    // ---- delivering the text ---------------------------------------------------------------------------------------------

    /// The text goes where the field was found: pasted with the chip after it, or into the card.
    fn deliver(&self, id: u64, lookup: Lookup, delivery: Delivery) {
        let Delivery {
            outcome,
            duration_s,
            audio_error,
        } = delivery;
        let empty = outcome.text.trim().is_empty();
        let notice = outcome.notice.clone();
        let written = |app: AppId, terminal: bool, anchor: Option<Rect>| Written {
            text: outcome.text.clone(),
            original: outcome.original.clone(),
            style: outcome.style.map(str::to_owned),
            app,
            terminal,
            anchor,
            duration_s,
        };

        match lookup.target {
            Target::Inline {
                app,
                terminal,
                anchor,
            } => {
                if empty {
                    // Nothing was heard: a chip for four seconds, and nothing is written.
                    return self.show_chip(id, written(app, terminal, anchor), notice, audio_error);
                }
                match self.write_into_field(id, &outcome.text, &app, terminal) {
                    Ok(()) => {
                        self.show_chip(id, written(app, terminal, anchor), notice, audio_error)
                    }
                    Err(error) => self.show_card(
                        id,
                        CardContent {
                            text: outcome.text.clone(),
                            original: outcome.original.clone(),
                            style: outcome.style.map(str::to_owned),
                            notice,
                            error: Some(error),
                            duration_s,
                            ..CardContent::default()
                        },
                        lookup.previous,
                        lookup.accessibility_missing,
                    ),
                }
            }
            Target::Card(_) => {
                // The card is where the text can be edited by hand or by a style, so it is not told that the AI wasn't
                // set up; and a card without text says what the pipeline found out about that.
                let notice = notice.filter(|n| n.key != NoticeKey::NotEditedNoKey);
                self.show_card(
                    id,
                    CardContent {
                        text: outcome.text.clone(),
                        original: outcome.original.clone(),
                        style: outcome.style.map(str::to_owned),
                        notice,
                        error: audio_error,
                        duration_s,
                        ..CardContent::default()
                    },
                    lookup.previous,
                    lookup.accessibility_missing,
                );
            }
        }
    }

    /// The card with no text and an error: the local transcription failed, which "nothing heard" would hide.
    fn fail_with_text(&self, id: u64, lookup: &Lookup, error: PanelError) {
        self.show_card(
            id,
            CardContent {
                error: Some(error),
                ..CardContent::default()
            },
            lookup.previous.clone(),
            lookup.accessibility_missing,
        );
    }

    /// Pastes the text into the field, and checks that the field took it. The panels are out of the way first. Then the app comes
    /// forward and its field is looked at: it may have become a password field since the dictation began, and the person may
    /// have gone to another app, so what is looked at must be the app that takes the paste, once it is in front. An app that
    /// will not come forward is left to [`Controller::paste`], which says so.
    fn write_into_field(
        &self,
        id: u64,
        text: &str,
        app: &AppId,
        terminal: bool,
    ) -> Result<(), PanelError> {
        {
            let mut inner = self.inner();
            if !inner.is_current(id) {
                return Ok(());
            }
            inner.writing = true;
            inner.panel.phase = Phase::Working;
            inner.panel.view = View::Hidden;
            inner.panel.working = Some(Working {
                kind: WorkKind::Writing,
                style: None,
                model: None,
                editing: false,
            });
        }
        self.publish();
        let in_front = self.deps.paster.bring_forward(app, &*self.deps.keys);
        let result = match in_front.then(|| self.field_turned_away()).flatten() {
            Some(reason) => {
                log::info!("Not pasting: the field is {reason:?} now");
                Err(PanelError::new("dictation.not_written"))
            }
            None => self.paste(text, app, terminal),
        };
        self.inner().writing = false;
        result
    }

    /// Why the text must not be pasted into the field after all, if it must not: what is in focus in the app that is in front
    /// is not a field that takes text (or the system can't say, where it is asked to).
    fn field_turned_away(&self) -> Option<CardReason> {
        match self.deps.platform.focused_field() {
            FieldLookup::Field(_) => None,
            FieldLookup::NoField => Some(CardReason::NoField),
            FieldLookup::Unknown if self.deps.os == Os::Linux => None,
            FieldLookup::Unknown => Some(CardReason::Unknown),
        }
    }

    /// The paste, with the check that it landed where the system can read the field.
    fn paste(&self, text: &str, app: &AppId, terminal: bool) -> Result<(), PanelError> {
        let check = self.deps.os != Os::Linux && !terminal;
        let before = if check {
            self.deps.platform.field_snapshot()
        } else {
            None
        };
        let result =
            self.deps
                .paster
                .paste(text, app, terminal, &self.deps.clipboard, &*self.deps.keys);
        if let PasteResult::CopiedOnly(reason) = result {
            return Err(paste_error(&reason));
        }
        let Some(before) = before else {
            // No reading of the field before it (Windows, a document of 50 000 characters): the paste is taken as it is.
            return Ok(());
        };
        for wait in [LANDING_FIRST, LANDING_SECOND] {
            self.deps.clock.sleep(wait);
            if landed(before, self.deps.platform.field_snapshot()) {
                return Ok(());
            }
        }
        Err(PanelError::new("dictation.not_landed"))
    }

    // ---- the card --------------------------------------------------------------------------------------------------------

    /// The review card, with what it shows.
    fn show_card(
        &self,
        id: u64,
        content: CardContent,
        previous: Option<AppId>,
        accessibility_missing: bool,
    ) {
        {
            let mut inner = self.inner();
            if !inner.is_current(id) {
                return;
            }
            inner.run = None;
            inner.writing = false;
            inner.written = false;
            inner.previous = previous.or(inner.previous.take());
            inner.card_text = content.text.clone();
            inner.panel = content.into_state(accessibility_missing);
        }
        self.deps.monitor.stop();
        (self.deps.hotkey_reset)();
        self.publish();
    }

    // ---- the chip ---------------------------------------------------------------------------------------------------------

    /// The chip by the text that was written: it stays for a while, and goes when the person goes on with something else.
    fn show_chip(
        &self,
        id: u64,
        written: Written,
        notice: Option<Notice>,
        error: Option<PanelError>,
    ) {
        let anchor = self.deps.platform.caret_anchor().or(written.anchor);
        let now = self.deps.clock.now();
        let chip_id = {
            let mut inner = self.inner();
            if !inner.is_current(id) {
                return;
            }
            inner.run = None;
            inner.writing = false;
            inner.written = true;
            let chip_id = inner.next_id;
            inner.next_id += 1;
            inner.chip = Some(ChipRun {
                id: chip_id,
                timer: ChipTimer::new(now, written.text.trim().is_empty()),
                hover: false,
                menu: false,
            });
            inner.previous = Some(written.app.clone());
            inner.panel = PanelState {
                view: View::Chip,
                phase: Phase::Written,
                text: written.text.clone(),
                original: written.original.clone(),
                style: written.style.clone(),
                duration_s: written.duration_s,
                notice,
                error,
                can_write: true,
                ..PanelState::hidden()
            };
            inner.last = Some(written.clone());
            chip_id
        };
        self.deps.ui.set_anchor(anchor);
        // Our own keys, those of a rewrite, are not the person's: the monitor starts after them.
        let weak = self.me.clone();
        self.deps.monitor.start(Box::new(move || {
            if let Some(me) = weak.upgrade() {
                me.on_input(chip_id);
            }
        }));
        (self.deps.hotkey_reset)();
        self.publish();
        let app = written.app;
        self.spawn("dictation-chip", move |me| me.watch_chip(chip_id, app));
    }

    /// A key went down or a button was pressed somewhere: the chip goes, unless the key was ours. Quick, because the system
    /// reports it on a thread that must not wait.
    fn on_input(&self, chip_id: u64) {
        if chip::is_own_input(self.deps.clock.now(), self.deps.keys.last_post()) {
            return;
        }
        self.spawn("dictation-chip-hide", move |me| me.hide_chip(chip_id));
    }

    /// Hides the chip `chip_id`, if it is still the one that is up.
    fn hide_chip(&self, chip_id: u64) {
        {
            let mut inner = self.inner();
            if inner.chip.as_ref().map(|chip| chip.id) != Some(chip_id) {
                return;
            }
            self.drop_everything(&mut inner);
        }
        self.deps.monitor.stop();
        self.publish();
    }

    /// Hides the chip when its time is up or the person has gone to another app.
    fn watch_chip(&self, chip_id: u64, app: AppId) {
        loop {
            self.deps.clock.sleep(chip::FRONT_POLL);
            let now = self.deps.clock.now();
            {
                let inner = self.inner();
                let Some(chip) = inner.chip.as_ref().filter(|chip| chip.id == chip_id) else {
                    return;
                };
                if chip.timer.expired(now) {
                    drop(inner);
                    return self.hide_chip(chip_id);
                }
            }
            let front = self.deps.platform.frontmost_app();
            if chip::switched_away(front.as_ref(), &app, self.deps.own_pid) {
                return self.hide_chip(chip_id);
            }
        }
    }

    /// The mouse is over the chip or has left it. The bubble is the chip's window, and a mouse over the bubble while it
    /// listens is no business of the chip.
    pub fn chip_hover(&self, hovering: bool) {
        self.hold(|chip| chip.hover = hovering);
    }

    /// The menu of the chip is open or closed.
    pub fn menu_open(&self, open: bool) {
        self.hold(|chip| chip.menu = open);
    }

    /// Changes what holds the chip, and stops its timer while anything does.
    fn hold(&self, change: impl FnOnce(&mut ChipRun)) {
        let now = self.deps.clock.now();
        let mut inner = self.inner();
        if inner.panel.view != View::Chip {
            return;
        }
        let Some(chip) = inner.chip.as_mut() else {
            return;
        };
        change(chip);
        if chip.hover || chip.menu {
            chip.timer.pause(now);
        } else {
            chip.timer.resume(now);
        }
    }

    // ---- writing the text again ------------------------------------------------------------------------------------------

    /// Writes the text again in a style (a preset id): as fresh text at the caret from the chip, in the card's box from the
    /// card.
    pub fn rewrite(&self, style: &str) -> Result<(), String> {
        let preset = EDIT_PRESETS
            .iter()
            .find(|preset| preset.id == style)
            .ok_or_else(|| format!("There is no style called {style}"))?;
        self.edit(preset.instruction.to_owned(), preset.id.to_owned());
        Ok(())
    }

    /// The card's custom request: its words are the instruction, and the title of the result.
    pub fn custom(&self, instruction: &str) {
        let instruction = instruction.trim();
        if !instruction.is_empty() {
            self.edit(instruction.to_owned(), instruction.to_owned());
        }
    }

    /// Applies `instruction` to the text that the chip or the card has, by the AI. `title` is the style that the result has.
    fn edit(&self, instruction: String, title: String) {
        let settings = self.deps.settings.get();
        let ai_ready = settings::is_ai_configured(&settings, self.deps.keys_flags.get(Account::Ai));
        enum Where {
            Chip(Written),
            Card(String),
        }
        let place = {
            let inner = self.inner();
            match (inner.panel.view, inner.panel.phase) {
                (View::Chip, Phase::Written) => inner
                    .last
                    .clone()
                    .filter(|last| !last.text.trim().is_empty())
                    .map(Where::Chip),
                (View::Card, Phase::Review) => {
                    let text = inner.card_text.clone();
                    (!text.trim().is_empty()).then_some(Where::Card(text))
                }
                _ => None,
            }
        };
        let Some(place) = place else {
            return;
        };
        if !ai_ready {
            // The edit needs the model: the person is told, and the text is as it was.
            let mut inner = self.inner();
            inner.panel.notice = Some(Notice {
                key: NoticeKey::NotEditedNoKey,
                detail: None,
            });
            drop(inner);
            return self.publish();
        }
        match place {
            Where::Chip(last) => self.rewrite_in_field(last, instruction, title),
            Where::Card(text) => self.edit_in_card(text, instruction, title),
        }
    }

    /// The chip's rewrite: the AI makes the new text, and it is pasted as fresh text at the caret.
    fn rewrite_in_field(&self, last: Written, instruction: String, title: String) {
        let (id, guard) = {
            let mut inner = self.inner();
            let (id, guard) = self.begin_run(&mut inner);
            // The chip is over while the bubble says what is going on; the chip that follows watches anew.
            inner.chip = None;
            inner.written = true;
            inner.panel = PanelState {
                view: View::Bubble,
                phase: Phase::Working,
                text: last.text.clone(),
                working: Some(Working {
                    kind: WorkKind::Rewriting,
                    style: Some(title.clone()),
                    model: None,
                    editing: false,
                }),
                duration_s: last.duration_s,
                ..PanelState::hidden()
            };
            (id, guard)
        };
        self.deps.monitor.stop();
        self.publish();
        let Some(guard) = guard else {
            return;
        };
        self.spawn("dictation-rewrite", move |me| {
            let token = guard.token();
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                me.rewrite_and_paste(id, &token, last, &instruction, title)
            }));
            if outcome.is_err() {
                log::error!("The rewrite stopped because of a bug");
                me.end_without_text(id);
            }
            drop(guard);
        });
    }

    fn rewrite_and_paste(
        &self,
        id: u64,
        token: &CancellationToken,
        last: Written,
        instruction: &str,
        title: String,
    ) {
        let refined = tauri::async_runtime::block_on(async {
            token
                .run_until_cancelled(self.deps.backend.refine(&last.text, instruction))
                .await
        });
        let Some(refined) = refined else {
            return;
        };
        match refined {
            Err(detail) => {
                // The field still has the text that was written, and the chip says why it wasn't made again.
                let notice = Notice {
                    key: NoticeKey::EditFailed,
                    detail: Some(detail),
                };
                self.show_chip(id, last, Some(notice), None);
            }
            Ok(text) => {
                let next = Written {
                    original: last.original.clone().or_else(|| Some(last.text.clone())),
                    style: Some(title),
                    text,
                    ..last
                };
                self.paste_written(id, next);
            }
        }
    }

    /// Pastes the text of `written` as fresh text, and shows the chip, or the card if it didn't go.
    fn paste_written(&self, id: u64, written: Written) {
        match self.write_into_field(id, &written.text, &written.app, written.terminal) {
            Ok(()) => self.show_chip(id, written, None, None),
            Err(error) => {
                let previous = Some(written.app.clone());
                self.show_card(
                    id,
                    CardContent {
                        text: written.text,
                        original: written.original,
                        style: written.style,
                        error: Some(error),
                        duration_s: written.duration_s,
                        ..CardContent::default()
                    },
                    previous,
                    false,
                );
            }
        }
    }

    /// The card's style buttons: the AI works on the text of the box, and the result goes back into it.
    fn edit_in_card(&self, text: String, instruction: String, title: String) {
        let (id, guard) = {
            let mut inner = self.inner();
            let (id, guard) = self.begin_run(&mut inner);
            inner.panel.phase = Phase::Refining;
            inner.panel.notice = None;
            inner.panel.error = None;
            inner.panel.working = Some(Working {
                kind: WorkKind::Editing,
                style: Some(title.clone()),
                model: None,
                editing: false,
            });
            (id, guard)
        };
        self.publish();
        let Some(guard) = guard else {
            return;
        };
        self.spawn("dictation-card-edit", move |me| {
            let token = guard.token();
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                me.edit_and_show(id, &token, text, &instruction, title)
            }));
            if outcome.is_err() {
                log::error!("The edit stopped because of a bug");
                me.end_without_text(id);
            }
            drop(guard);
        });
    }

    fn edit_and_show(
        &self,
        id: u64,
        token: &CancellationToken,
        text: String,
        instruction: &str,
        title: String,
    ) {
        let refined = tauri::async_runtime::block_on(async {
            token
                .run_until_cancelled(self.deps.backend.refine(&text, instruction))
                .await
        });
        let Some(refined) = refined else {
            return;
        };
        {
            let mut inner = self.inner();
            if !inner.is_current(id) {
                return;
            }
            inner.run = None;
            inner.panel.phase = Phase::Review;
            inner.panel.working = None;
            match refined {
                Ok(edited) => {
                    inner.panel.original = inner.panel.original.take().or(Some(text));
                    inner.panel.text = edited.clone();
                    inner.panel.style = Some(title);
                    inner.card_text = edited;
                }
                Err(detail) => {
                    inner.panel.notice = Some(Notice {
                        key: NoticeKey::EditFailed,
                        detail: Some(detail),
                    });
                }
            }
        }
        self.publish();
    }

    /// The card's box has this text now.
    pub fn card_text(&self, text: &str) {
        let mut inner = self.inner();
        if inner.panel.view == View::Card {
            inner.card_text = text.to_owned();
        }
    }

    /// The card's "Undo": the text from before the edit is in the box again.
    pub fn undo(&self) {
        {
            let mut inner = self.inner();
            if inner.panel.view != View::Card || inner.panel.phase != Phase::Review {
                return;
            }
            let Some(original) = inner.panel.original.take() else {
                return;
            };
            inner.panel.text = original.clone();
            inner.panel.style = None;
            inner.panel.notice = None;
            inner.card_text = original;
        }
        self.publish();
    }

    /// The chip's "Edit…" and "Custom request…": the review card on what was written.
    pub fn open_card(&self, custom: bool) {
        {
            let mut inner = self.inner();
            if inner.panel.view != View::Chip {
                return;
            }
            let Some(last) = inner.last.clone() else {
                return;
            };
            inner.chip = None;
            inner.previous = Some(last.app.clone());
            inner.card_text = last.text.clone();
            inner.panel = CardContent {
                text: last.text,
                original: last.original,
                style: last.style,
                from_chip: true,
                can_write: true,
                open_custom: custom,
                duration_s: last.duration_s,
                ..CardContent::default()
            }
            .into_state(false);
        }
        self.deps.monitor.stop();
        self.publish();
        // The card was asked to open its box for a request, once: what is sent next is no such request again.
        self.inner().panel.open_custom = false;
    }

    /// The card that was opened from the chip: its text is pasted as fresh text.
    pub fn paste_card(&self, text: &str) {
        let (id, guard, written) = {
            let mut inner = self.inner();
            if inner.panel.view != View::Card || !inner.panel.from_chip {
                return;
            }
            let Some(last) = inner.last.clone() else {
                return;
            };
            if text.trim().is_empty() {
                return;
            }
            let written = Written {
                text: text.to_owned(),
                original: inner.panel.original.clone(),
                style: inner.panel.style.clone(),
                ..last
            };
            let (id, guard) = self.begin_run(&mut inner);
            inner.written = true;
            inner.panel = PanelState {
                view: View::Hidden,
                phase: Phase::Working,
                text: text.to_owned(),
                ..PanelState::hidden()
            };
            (id, guard, written)
        };
        self.publish();
        let Some(guard) = guard else {
            return;
        };
        self.spawn("dictation-paste", move |me| {
            let outcome = catch_unwind(AssertUnwindSafe(|| me.paste_written(id, written)));
            if outcome.is_err() {
                log::error!("The paste stopped because of a bug");
                me.end_without_text(id);
            }
            drop(guard);
        });
    }

    /// Puts the text on the clipboard and closes the chip or the card.
    pub fn copy(&self, text: &str) {
        if let Err(e) = self.deps.clipboard.copy(text) {
            log::warn!("Could not copy the text: {e}");
            return;
        }
        let (on_the_card, previous) = {
            let mut inner = self.inner();
            if !matches!(inner.panel.view, View::Chip | View::Card) {
                return;
            }
            let on_the_card = inner.panel.view == View::Card;
            let previous = inner.previous.take();
            self.drop_everything(&mut inner);
            (on_the_card, previous)
        };
        self.deps.monitor.stop();
        self.publish();
        (self.deps.hotkey_reset)();
        if on_the_card {
            self.give_the_focus_back(previous);
        }
    }

    // ---- the rest of the commands ----------------------------------------------------------------------------------------

    /// The language menu: saves the language and applies it to what is said from now on.
    pub fn set_language(&self, lang: &str) -> Result<(), String> {
        if !is_language_code(lang) {
            return Err(format!("{lang:?} is not a language code like en-US"));
        }
        self.deps
            .settings
            .update(|settings| settings.dictation_language = lang.to_owned())
            .map_err(|e| format!("Could not save the settings: {e}"))?;
        self.publish();
        Ok(())
    }

    /// The card's "Grant access…".
    pub fn grant_accessibility(&self) {
        self.deps.platform.request_accessibility();
    }

    /// The error's "Open Settings".
    pub fn open_settings(&self) {
        self.deps.ui.open_settings();
    }
}

/// What the person must be told about a text that was not pasted. Every reason has words of its own in the locale files
/// (the platform's English explanation goes to the log only), and "already copied" is only said when it is true.
fn paste_error(reason: &CopyReason) -> PanelError {
    match reason {
        CopyReason::Failed(detail) | CopyReason::NotCopied(detail) => {
            log::warn!("Could not paste: {detail}");
        }
        CopyReason::AppNotForward | CopyReason::Elevated | CopyReason::NoWaylandTool => {}
    }
    let key = if !reason.on_clipboard() {
        "dictation.not_copied"
    } else {
        match reason {
            CopyReason::Elevated => "dictation.copied_elevated",
            CopyReason::NoWaylandTool => "dictation.copied_no_wayland_tool",
            _ => "dictation.copied_only",
        }
    };
    PanelError::new(key)
}

/// Whether the text is a language code of the kind the menu offers (`en-US`, `vi`, `yue-HK`): a language of two or three
/// letters, and up to two parts after it of letters and digits. The code is written to the settings and handed to Whisper.
fn is_language_code(code: &str) -> bool {
    let mut parts = code.split('-');
    let language = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    (2..=3).contains(&language.len())
        && language.chars().all(|c| c.is_ascii_alphabetic())
        && rest.len() <= 2
        && rest.iter().all(|part| {
            (2..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

/// What the card shows.
#[derive(Default)]
struct CardContent {
    text: String,
    original: Option<String>,
    style: Option<String>,
    notice: Option<Notice>,
    error: Option<PanelError>,
    from_chip: bool,
    can_write: bool,
    open_custom: bool,
    duration_s: f64,
}

impl CardContent {
    fn into_state(self, accessibility_missing: bool) -> PanelState {
        PanelState {
            view: View::Card,
            phase: Phase::Review,
            text: self.text,
            original: self.original,
            style: self.style,
            notice: self.notice,
            error: self.error,
            from_chip: self.from_chip,
            can_write: self.can_write,
            open_custom: self.open_custom,
            duration_s: self.duration_s,
            accessibility_missing,
            ..PanelState::hidden()
        }
    }
}

enum Allowed {
    Yes,
    No,
    Gone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Going,
    Stop,
    Gone,
}

/// The network calls of the pipeline, with the two ways that the person has of ending one early: dropping the dictation
/// ends every call, and asking for the text as it is ends the ones that hear the recording again (the edit has the token
/// of its own inside the pipeline). A call that ends early answers with an error, which the pipeline takes for a failed one:
/// [`Guarded::was_skipped`] tells that it was dropped on purpose.
struct Guarded<'a> {
    inner: &'a dyn Backend,
    cancel: CancellationToken,
    skip: CancellationToken,
    skipped: AtomicBool,
}

impl<'a> Guarded<'a> {
    fn new(inner: &'a dyn Backend, cancel: CancellationToken, skip: CancellationToken) -> Self {
        Self {
            inner,
            cancel,
            skip,
            skipped: AtomicBool::new(false),
        }
    }

    /// Whether a call was dropped because the person asked for the text as it is.
    fn was_skipped(&self) -> bool {
        self.skipped.load(Ordering::SeqCst)
    }

    /// Runs `call` unless the dictation is dropped, or (for a call that hears the recording) the text is asked for as it is.
    async fn guard<T>(
        &self,
        call: impl std::future::Future<Output = Result<T, String>>,
        skippable: bool,
    ) -> Result<T, String> {
        let ended = if skippable {
            self.cancel
                .run_until_cancelled(self.skip.run_until_cancelled(call))
                .await
                .map(|inner| inner.ok_or(()))
        } else {
            self.cancel.run_until_cancelled(call).await.map(Ok::<_, ()>)
        };
        match ended {
            Some(Ok(result)) => result,
            Some(Err(())) => {
                self.skipped.store(true, Ordering::SeqCst);
                Err("skipped".to_owned())
            }
            None => Err("cancelled".to_owned()),
        }
    }
}

#[async_trait::async_trait]
impl Backend for Guarded<'_> {
    async fn transcribe(&self, wav: &Path, timeout: Duration) -> Result<String, String> {
        self.guard(self.inner.transcribe(wav, timeout), true).await
    }

    async fn transcribe_and_edit(
        &self,
        wav: &Path,
        instruction: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        self.guard(
            self.inner.transcribe_and_edit(wav, instruction, timeout),
            true,
        )
        .await
    }

    async fn refine(&self, text: &str, instruction: &str) -> Result<String, String> {
        self.guard(self.inner.refine(text, instruction), false)
            .await
    }
}

#[cfg(test)]
pub(crate) mod fakes;
#[cfg(test)]
mod tests;
