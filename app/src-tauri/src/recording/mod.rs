//! Recording in the app: the recorder that turns what `dictate_services::record` captures into a meeting of the library.
//!
//! A recording begins as a folder of the library (its `meta.json` has the source that the screen recordings of the Swift app have)
//! and ends, when it is stopped or ends by itself, as a meeting whose job starts at the audio. The recorder holds the one
//! recording that can go on at a time, tells the windows how it goes (`recording-state`), and keeps what is recorded safe: a
//! recording that the app was ended in the middle of is finished at the next start (`recover_library`), and quitting while one goes
//! on waits for it to be put together.
//!
//! What can take long is done on the thread of the caller, which is a blocking one: `Session::start` (ffmpeg is asked which of its
//! encoders work, and the desktop may ask the person what to share), `Session::stop` (the mix of the sound has no bound) and
//! `Session::discard`. The session reports its state on a thread of its own, which does no more than record it.

pub mod commands;
pub mod hud;
#[cfg(test)]
mod tests;
mod token;
pub mod views;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use chrono::Local;
use dictate_services::media::tools::{install_command, Tool, Tools};
use dictate_services::record::capture::{list_sources, OsCapturer, Sources};
use dictate_services::record::marker::MARKER;
use dictate_services::record::session::{recover, Levels, Session, SessionState};
use dictate_services::record::video::pick_codec;
use dictate_services::record::{Capturer, EndReason, Mode, RecordOptions, StartError};
use dictate_services::settings::SettingsStore;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::meetings::jobs::Hold;
use crate::meetings::library::Meetings;
use crate::meetings::notify::{Notice, NoticeKind, Notifier};
use crate::meetings::views::ErrorView;
use crate::platform::{Permission, Platform, SystemPlatform};
use crate::state::{AppState, Job, JobGuard, Jobs};
use crate::tray;
pub use views::{Phase, RecordingView, ScreenAvailability, SourcesView};

/// The event that tells the windows how the recording goes: a [`RecordingView`] each time that changes, and at most every 50 ms
/// while it only goes on.
pub const STATE_EVENT: &str = "recording-state";

/// How often the state of a recording that goes on is told, at the most.
const REPORT_EVERY: Duration = Duration::from_millis(50);

/// What the log of a recording says that was found cut off at the start of the app.
const INTERRUPTED: &str = "The recording was interrupted; what was recorded is kept.";

/// How long quitting waits for a recording to be put together.
pub const QUIT_WAIT: Duration = Duration::from_secs(60);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the recorder shows what it does. It must not call back into the recorder: it is called with the state locked.
pub trait Surface: Send + Sync {
    /// The state changed.
    fn state(&self, view: &RecordingView);
    /// A recording has begun: shows the bar.
    fn show_hud(&self);
    /// The recording is over: hides it.
    fn hide_hud(&self);
}

/// The windows of the app: the meetings window and the bar hear the state.
pub struct AppSurface<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> Surface for AppSurface<R> {
    fn state(&self, view: &RecordingView) {
        for label in [crate::meetings::window::LABEL, hud::LABEL] {
            if let Err(e) = self.0.emit_to(label, STATE_EVENT, view) {
                log::warn!("Could not tell the {label} window how the recording goes: {e}");
            }
        }
        // The tray says how to stop the recording, and on macOS counts it. This only notes it: nothing here waits.
        tray::recording_changed(&self.0, view);
    }

    fn show_hud(&self) {
        hud::show(&self.0);
    }

    fn hide_hud(&self) {
        hud::hide(&self.0);
    }
}

/// Why the shortcut or `--record` did not do what it was asked.
#[derive(Debug, PartialEq)]
pub enum ToggleError {
    /// A window is what was recorded last, and which window is not kept (it may be gone): only the picker can say. Nothing was begun.
    NeedsPicker,
    /// The recording could not begin: the picker shows why.
    Start(ErrorView),
    /// The recording could not be put together: a banner says so.
    Stop(ErrorView),
}

/// What the shortcut and `--record` do, for the recorder of the app (there is none until the app has started it).
pub fn toggle<R: Runtime>(app: &AppHandle<R>) {
    match app.try_state::<Recorder>() {
        Some(recorder) => recorder.toggle(app),
        None => log::info!("A recording was asked for before the recorder is ready"),
    }
}

/// What the recorder works with, which a test replaces.
pub struct Deps {
    pub meetings: Meetings,
    pub jobs: Arc<Jobs>,
    pub settings: Arc<SettingsStore>,
    pub platform: Arc<dyn Platform>,
    pub surface: Arc<dyn Surface>,
    /// Where a banner is shown: a recording that stopped by itself, and a start from the shortcut that failed.
    pub notifier: Arc<dyn Notifier>,
    /// Which tools of the system are installed.
    pub tools: Arc<dyn Fn() -> Tools + Send + Sync>,
    /// The sources of a recording, given where ffmpeg is.
    pub capturer: Arc<dyn Fn(Option<PathBuf>) -> Arc<dyn Capturer> + Send + Sync>,
    /// What the system offers to record.
    pub sources: Arc<dyn Fn() -> Result<Sources, StartError> + Send + Sync>,
    /// Whether this ffmpeg has a video encoder that works here (it starts programs to find out).
    pub encoder_works: Arc<dyn Fn(&Path) -> bool + Send + Sync>,
}

impl Deps {
    /// The recorder of the app, on the system's own sources.
    pub fn system<R: Runtime>(
        app: &AppHandle<R>,
        state: &AppState,
        meetings: Meetings,
        notifier: Arc<dyn Notifier>,
        platform: Arc<dyn Platform>,
    ) -> Deps {
        let tokens = token::for_settings(state.settings.clone());
        Deps {
            meetings,
            jobs: state.jobs.clone(),
            settings: state.settings.clone(),
            platform,
            surface: Arc::new(AppSurface(app.clone())),
            notifier,
            tools: Arc::new(Tools::find),
            capturer: Arc::new(move |ffmpeg| -> Arc<dyn Capturer> {
                Arc::new(OsCapturer::new(ffmpeg).with_token_store(tokens.clone()))
            }),
            sources: Arc::new(list_sources),
            encoder_works: Arc::new(|ffmpeg| pick_codec(ffmpeg).is_some()),
        }
    }
}

/// Makes the recorder of the app and keeps it as state of the app, for the commands, the tray and the shortcut.
pub fn start<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    meetings: &Meetings,
    notifier: Arc<dyn Notifier>,
) -> Recorder {
    let platform = app.state::<SystemPlatform>().0.clone();
    let recorder = Recorder::new(Deps::system(
        app,
        state,
        meetings.clone(),
        notifier,
        platform,
    ));
    app.manage(recorder.clone());
    recorder
}

/// A recording that goes on.
struct Running {
    session: Session,
    /// The id of the meeting, which is the name of its folder.
    id: String,
    folder: PathBuf,
    title: String,
    /// The place of the meeting in the registry: no job can start on a folder that is being written.
    hold: Hold,
    /// The recording as a job of the app: quitting knows of it.
    job: JobGuard,
}

/// Where the recorder is. The slot is held only for as long as it takes to look at it or change it: nothing slow is done with it.
enum Slot {
    Idle,
    /// The sources are being started.
    Starting,
    Running(Box<Running>),
    /// Someone has the recording in hand, to put it together or to throw it away.
    Finishing,
}

/// What `take_running` found.
enum Taken {
    Running(Box<Running>),
    /// The recording is in the hands of someone else already.
    Finishing,
    Nothing,
}

/// The state that windows are told of, and when they were last told of one that only went on.
struct Shown {
    view: RecordingView,
    last_told: Option<Instant>,
}

impl Shown {
    /// Takes `next` as the state. Whether the windows are to be told: always for a change of anything but the time and the levels
    /// (the phase, a note), and for those only when `REPORT_EVERY` has gone by since they were last told, if `throttled`.
    fn change(&mut self, next: RecordingView, throttled: bool, now: Instant) -> bool {
        let big = self.view.phase != next.phase
            || self.view.notes != next.notes
            || self.view.mode != next.mode
            || self.view.meeting_id != next.meeting_id;
        let due = self
            .last_told
            .is_none_or(|at| now.saturating_duration_since(at) >= REPORT_EVERY);
        self.view = next;
        let tell = big || due || !throttled;
        if tell {
            self.last_told = Some(now);
        }
        tell
    }
}

struct Inner {
    deps: Deps,
    slot: Mutex<Slot>,
    /// Told when a start ends, and when the recorder is idle again.
    changed: Condvar,
    shown: Mutex<Shown>,
    /// Whether the video encoder of an ffmpeg works, for the ffmpeg that was asked about last: asking is slow.
    encoders: Mutex<Option<(PathBuf, bool)>>,
    /// The app has waited as long as it would for a recording, and is going: asked to exit again (which is how it goes) it is let
    /// through, and does not wait another time.
    exiting: AtomicBool,
}

/// The recorder: one recording at a time, started, paused, stopped or thrown away by the windows, the tray and the shortcut.
#[derive(Clone)]
pub struct Recorder {
    inner: Arc<Inner>,
}

impl Recorder {
    pub fn new(deps: Deps) -> Recorder {
        Recorder {
            inner: Arc::new(Inner {
                deps,
                slot: Mutex::new(Slot::Idle),
                changed: Condvar::new(),
                shown: Mutex::new(Shown {
                    view: RecordingView::idle(),
                    last_told: None,
                }),
                encoders: Mutex::new(None),
                exiting: AtomicBool::new(false),
            }),
        }
    }

    /// How the recording is now.
    pub fn view(&self) -> RecordingView {
        lock(&self.inner.shown).view.clone()
    }

    /// What the picker offers: the displays and the windows, and whether the screen can be recorded at all. This may take a
    /// moment (it asks the system, and ffmpeg for its encoders the first time): call it on a blocking thread.
    pub fn sources(&self) -> SourcesView {
        let deps = &self.inner.deps;
        let mut view = SourcesView {
            displays: Vec::new(),
            windows: Vec::new(),
            picker_is_the_system: false,
            screen: ScreenAvailability::Ready,
            ffmpeg_command: install_command(Tool::Ffmpeg).to_owned(),
            last: views::options_from(&deps.settings.get()),
        };
        let Some(ffmpeg) = (deps.tools)().ffmpeg else {
            view.screen = ScreenAvailability::NeedsFfmpeg;
            return view;
        };
        if !self.has_video_encoder(&ffmpeg) {
            view.screen = ScreenAvailability::NoVideoEncoder;
            return view;
        }
        // Listing the windows is what makes macOS show its prompt: it is not asked for until the permission is there.
        if deps.platform.screen_recording() == Permission::Denied {
            view.screen = ScreenAvailability::NeedsPermission;
            return view;
        }
        match (deps.sources)() {
            Ok(found) => {
                view.displays = found.displays;
                view.windows = found.windows;
                view.picker_is_the_system = found.picker_is_the_system;
            }
            Err(StartError::NeedsScreenPermission) => {
                view.screen = ScreenAvailability::NeedsPermission;
            }
            Err(StartError::PortalUnavailable) => {
                view.screen = ScreenAvailability::PortalUnavailable;
            }
            // The start says what is wrong, in words: the picker is not a dead end for a list that could not be made.
            Err(other) => log::warn!("Could not list what can be recorded: {other:?}"),
        }
        view
    }

    /// Whether `ffmpeg` has a video encoder that works, asked once for each ffmpeg.
    fn has_video_encoder(&self, ffmpeg: &Path) -> bool {
        let mut known = lock(&self.inner.encoders);
        if let Some((path, works)) = known.as_ref() {
            if path == ffmpeg {
                return *works;
            }
        }
        let works = (self.inner.deps.encoder_works)(ffmpeg);
        *known = Some((ffmpeg.to_path_buf(), works));
        works
    }

    /// Starts a recording with what the person chose. This waits for the sources to start, which can be as long as a person takes
    /// to answer the question of the desktop: call it on a blocking thread. A recording that is going on already says so
    /// (`ErrorView::busy`); one that cannot start leaves no folder, and says why with the key of a text.
    pub fn start(&self, options: RecordOptions) -> Result<(), ErrorView> {
        let job = {
            let mut slot = lock(&self.inner.slot);
            if !matches!(*slot, Slot::Idle) {
                return Err(ErrorView::busy());
            }
            let job = self
                .inner
                .deps
                .jobs
                .start(Job::Recording)
                .map_err(|_| ErrorView::busy())?;
            *slot = Slot::Starting;
            job
        };
        self.inner.publish(
            RecordingView {
                phase: Phase::Starting,
                mode: Some(options.mode.clone()),
                levels: quiet(&options),
                ..RecordingView::idle()
            },
            false,
        );
        match self.begin(options, job) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.inner.become_idle(false);
                Err(error)
            }
        }
    }

    /// The part of `start` that can fail: what is left behind when it does is taken away here, and the job is let go with it.
    fn begin(&self, options: RecordOptions, job: JobGuard) -> Result<(), ErrorView> {
        let inner = &self.inner;
        let deps = &inner.deps;
        // What the person chose is kept whatever comes of it: they chose it.
        let kept = options.clone();
        if let Err(e) = deps.settings.update(|s| views::remember(s, &kept)) {
            log::warn!("Could not keep the choices of the recording: {e}");
        }
        let ffmpeg = (deps.tools)().ffmpeg;
        // What would be refused anyway is refused before a folder is made.
        if matches!(options.mode, Mode::Audio) && !options.mic && !options.system_audio {
            return Err(views::start_error(StartError::NothingToRecord));
        }
        if !matches!(options.mode, Mode::Audio) && ffmpeg.is_none() {
            return Err(views::start_error(StartError::NeedsFfmpeg));
        }

        let lang = tray::language_for(&deps.settings.get());
        let when = Local::now().format("%H:%M %d/%m").to_string();
        let title = views::title(lang, &options.mode, &when);
        let (id, folder) = deps
            .meetings
            .begin_recording(&title, views::expected_media(&options.mode))
            .map_err(|e| views::start_error(StartError::Io(e.to_string())))?;
        let hold = match deps.meetings.registry.hold(&id) {
            Ok(hold) => hold,
            Err(_) => {
                deps.meetings.discard_folder(&folder);
                return Err(views::start_error(StartError::Io(
                    "the new recording already has a job".to_owned(),
                )));
            }
        };
        inner.publish_with(|view| view.meeting_id = Some(id.clone()), false);

        let (ended, ends) = mpsc::channel::<EndReason>();
        let weak = Arc::downgrade(inner);
        let on_state = Box::new(move |state: SessionState| {
            if let Some(inner) = weak.upgrade() {
                inner.session_reported(state, &ended);
            }
        });
        let capturer = (deps.capturer)(ffmpeg.clone());
        let session = match Session::start(options, &folder, capturer, ffmpeg, on_state) {
            Ok(session) => session,
            Err(error) => {
                drop(hold);
                deps.meetings.discard_folder(&folder);
                deps.meetings.library_changed();
                return Err(views::start_error(error));
            }
        };

        // The bar goes up while the slot still says that the sources are being started: nobody can take the recording to end it until
        // it is running, so the bar is shown before whoever ends the recording hides it. After the slot is running a recording that
        // ended at once would be put together, and the bar hidden, before this showed it, and it would stay over an idle recorder.
        deps.surface.show_hud();
        *lock(&inner.slot) = Slot::Running(Box::new(Running {
            session,
            id,
            folder,
            title,
            hold,
            job,
        }));
        inner.changed.notify_all();
        // The session reports as it starts; until it has, the recording is under way all the same.
        inner.publish_with(
            |view| {
                if view.phase == Phase::Starting {
                    view.phase = Phase::Recording;
                }
            },
            false,
        );
        self.watch_for_an_end(ends);
        deps.meetings.library_changed();
        Ok(())
    }

    /// Waits, on a thread of its own, for the session to say that it ended by itself, and then puts the recording together as a
    /// stop would. The thread ends with the session: when that is stopped or thrown away nothing more comes.
    fn watch_for_an_end(&self, ends: mpsc::Receiver<EndReason>) {
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let spawned = std::thread::Builder::new()
            .name("recording-end".into())
            .spawn(move || {
                let Ok(reason) = ends.recv() else { return };
                let Some(inner) = weak.upgrade() else { return };
                let recorder = Recorder { inner };
                // The person may have stopped it at the same moment: whoever takes the recording first puts it together.
                if let Taken::Running(running) = recorder.take_running() {
                    let _ = recorder.finish(*running, Some(reason));
                }
            });
        if let Err(e) = spawned {
            log::warn!("Could not watch for the end of the recording: {e}");
        }
    }

    /// The clock stops: what the sources deliver is not recorded until `resume`.
    pub fn pause(&self) {
        if let Slot::Running(running) = &*lock(&self.inner.slot) {
            running.session.pause();
        }
    }

    pub fn resume(&self) {
        if let Slot::Running(running) = &*lock(&self.inner.slot) {
            running.session.resume();
        }
    }

    /// Takes the recording that goes on to put it together or to throw it away: the one who asks first has it. The state says
    /// that it is being finished.
    fn take_running(&self) -> Taken {
        let taken = {
            let mut slot = lock(&self.inner.slot);
            take_from(&mut slot)
        };
        if matches!(taken, Taken::Running(_)) {
            self.inner
                .publish_with(|view| view.phase = Phase::Finishing, false);
        }
        taken
    }

    /// Ends the recording and makes a meeting of it: the job starts at the audio. This waits for the sound to be mixed, which is as
    /// long as a long recording takes: call it on a blocking thread. A recording that has nothing in it is told so, and its folder
    /// is removed; one that has already been taken to be put together is not an error.
    pub fn stop(&self) -> Result<(), ErrorView> {
        match self.take_running() {
            Taken::Running(running) => self.finish(*running, None),
            Taken::Finishing => Ok(()),
            Taken::Nothing => Err(ErrorView::busy()),
        }
    }

    /// Puts the recording together. `ended` is why it stopped when it did that by itself: the person is told with a banner.
    fn finish(&self, running: Running, ended: Option<EndReason>) -> Result<(), ErrorView> {
        let Running {
            session,
            id,
            folder,
            title,
            hold,
            job,
        } = running;
        let meetings = &self.inner.deps.meetings;
        let made = session.stop();
        // The place of the meeting is let go of: its job takes it.
        drop(hold);
        let result = match made {
            Ok(finished) => meetings
                .finish_recording(&id, &folder, &finished)
                .map_err(|e| stop_failed(&e)),
            // Nothing was recorded: there is nothing to keep, and no job to start.
            Err(e) if nothing_was_recorded(&e, &folder) => {
                meetings.discard_folder(&folder);
                meetings.library_changed();
                Err(views::error(
                    "Other",
                    "recording.error.nothing_recorded",
                    None,
                    None,
                ))
            }
            // What is on the disk stays, with its marker: the next start of the app finishes it.
            Err(e) => Err(stop_failed(&e)),
        };
        drop(job);
        self.inner.become_idle(true);
        if let Some(reason) = ended {
            let (key, detail) = match &result {
                Ok(()) => {
                    let (key, detail) = views::ended(&reason);
                    (key.to_owned(), detail)
                }
                Err(error) => (error.key.clone(), error.detail.clone().unwrap_or_default()),
            };
            self.inner.deps.notifier.notify(Notice {
                id,
                title,
                kind: NoticeKind::Recording { key, detail },
            });
        }
        result
    }

    /// Throws the recording away: the sources are stopped, and the meeting goes to the Trash (it is removed when the Trash will not
    /// take it). No job starts and no banner is shown. Refused when no recording goes on.
    pub fn discard(&self) -> Result<(), ErrorView> {
        let Taken::Running(running) = self.take_running() else {
            return Err(ErrorView::busy());
        };
        let Running {
            session,
            folder,
            hold,
            job,
            ..
        } = *running;
        session.discard();
        let result = self
            .inner
            .deps
            .meetings
            .trash_recording(&folder)
            .map_err(|why| {
                views::error("Other", "recording.error.discard_failed", Some(why), None)
            });
        drop(hold);
        drop(job);
        self.inner.become_idle(true);
        result
    }

    /// What the shortcut and `--record` do: a recording that goes on is stopped, and with none a new one starts with the choices that
    /// were made last. A recording that is being started or put together is left to finish. A window is not among the choices that
    /// are kept (it may be gone): with a window as the last choice no recording is begun, which is `NeedsPicker`.
    pub fn toggle_now(&self) -> Result<(), ToggleError> {
        match self.view().phase {
            Phase::Idle => {
                let options = views::options_from(&self.inner.deps.settings.get());
                if matches!(&options.mode, Mode::Window { id } if id.is_empty()) {
                    return Err(ToggleError::NeedsPicker);
                }
                self.start(options).map_err(ToggleError::Start)
            }
            Phase::Recording | Phase::Paused => self.stop().map_err(ToggleError::Stop),
            Phase::Starting | Phase::Finishing => Ok(()),
        }
    }

    /// `toggle_now` off the thread of the shortcut that asked. What goes wrong is told by `toggle_failed`.
    pub fn toggle<R: Runtime>(&self, app: &AppHandle<R>) {
        let (recorder, app) = (self.clone(), app.clone());
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = recorder.toggle_now() {
                recorder.toggle_failed(&app, error);
            }
        });
    }

    /// Tells what went wrong with a toggle. A recording that could not begin has the picker opened for it, with the reason shown
    /// there: what the shortcut could not choose, the person can. A recording that could not be put together is a banner, since
    /// there is no picker to choose in.
    fn toggle_failed<R: Runtime>(&self, app: &AppHandle<R>, error: ToggleError) {
        match error {
            ToggleError::NeedsPicker => crate::meetings::window::show_recorder(app, None),
            ToggleError::Start(error) => {
                log::warn!("The recording shortcut could not start: {}", error.key);
                crate::meetings::window::show_recorder(app, Some(&error));
            }
            ToggleError::Stop(error) => {
                log::warn!("The recording shortcut could not stop: {}", error.key);
                self.tell(&error);
            }
        }
    }

    /// What the item of the tray does: with no recording going on it opens the meetings window with the picker, and with one it
    /// stops it and opens the window, as the Swift app does, so that the meeting it became is there. A recording that could not be
    /// put together is a banner. This may wait for the sound to be mixed: call it on a blocking thread.
    pub fn tray_item<R: Runtime>(&self, app: &AppHandle<R>) {
        match self.view().phase {
            Phase::Recording | Phase::Paused => match self.stop() {
                // A recording that ended by itself at that moment has told so already (and its meeting is there to see).
                Ok(()) => crate::meetings::window::show(app),
                Err(error) if error.key == crate::meetings::views::key::BUSY => {
                    crate::meetings::window::show(app);
                }
                Err(error) => {
                    log::warn!("The tray could not stop the recording: {}", error.key);
                    self.tell(&error);
                }
            },
            // Starting or being put together, a recording is not to be stopped and none can begin: the picker says as much.
            Phase::Idle | Phase::Starting | Phase::Finishing => {
                crate::meetings::window::show_recorder(app, None);
            }
        }
    }

    /// Shows `error` as a banner, with what is behind it.
    fn tell(&self, error: &ErrorView) {
        self.inner.deps.notifier.notify(Notice {
            id: String::new(),
            title: String::new(),
            kind: NoticeKind::Recording {
                key: error.key.clone(),
                detail: error.detail.clone().unwrap_or_default(),
            },
        });
    }

    /// What quitting does: when a recording goes on, or is being started or put together, it is put together first (waiting for
    /// a start that is under way), and `then` is called when that is done, or after `wait` at the longest. Returns whether quitting
    /// has to wait for that: `false` with no recording, and then `then` is not called.
    pub fn finish_before_exit(&self, wait: Duration, then: impl FnOnce() + Send + 'static) -> bool {
        // `then` is what asks for the exit again: that one is not to wait, whatever the recording is doing (a mix that takes longer
        // than the app waits, a dialog of the desktop that nobody answers).
        if self.inner.exiting.load(Ordering::SeqCst)
            || matches!(*lock(&self.inner.slot), Slot::Idle)
        {
            return false;
        }
        let (done, finished) = mpsc::channel::<()>();
        let worker = self.clone();
        let started = std::thread::Builder::new()
            .name("recording-quit".into())
            .spawn(move || {
                worker.finish_all(wait);
                let _ = done.send(());
            });
        if let Err(e) = started {
            log::warn!("Could not put the recording together before quitting: {e}");
            return false;
        }
        let waiter = self.clone();
        let waiting = std::thread::Builder::new()
            .name("recording-quit-wait".into())
            .spawn(move || {
                let _ = finished.recv_timeout(wait);
                // Done, or given up on: the app goes now, and what is still being put together is finished by the next start.
                waiter.inner.exiting.store(true, Ordering::SeqCst);
                then();
            });
        if let Err(e) = waiting {
            log::warn!("Could not wait for the recording before quitting: {e}");
            return false;
        }
        true
    }

    /// Puts the recording together when there is one (and one that is being started becomes one), and waits for the one that is
    /// being put together by someone else, for `wait` at the most.
    fn finish_all(&self, wait: Duration) {
        let deadline = Instant::now() + wait;
        let mut slot = lock(&self.inner.slot);
        loop {
            match &*slot {
                Slot::Idle => return,
                Slot::Running(_) => {
                    let Taken::Running(running) = take_from(&mut slot) else {
                        return;
                    };
                    drop(slot);
                    self.inner
                        .publish_with(|view| view.phase = Phase::Finishing, false);
                    if let Err(error) = self.finish(*running, None) {
                        log::warn!(
                            "The recording could not be put together at quit: {}",
                            error.key
                        );
                    }
                    return;
                }
                Slot::Starting | Slot::Finishing => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return;
                    }
                    slot = self
                        .inner
                        .changed
                        .wait_timeout(slot, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
        }
    }
}

/// The meters of a recording that has not heard anything yet: quiet for each track that is on, and none for one that is off (which is
/// what the session says of them too).
fn quiet(options: &RecordOptions) -> Levels {
    Levels {
        mic: options.mic.then_some(0.0),
        system: options.system_audio.then_some(0.0),
    }
}

/// What the app does when it is asked to exit, `code` being how (`None`: the last window was closed, which does not end a tray app).
/// A quit while a recording goes on, or is being started or put together, waits for it to be put together (for `wait` at the most)
/// and then calls `exit` with the code; the caller then does not exit now, which is what `true` says. With no recording, or no quit,
/// nothing is done and `exit` is not called. `exit` asks for the exit again, and that call is let through (`false`) once the wait is
/// over, finished or given up: the app does not wait twice.
pub fn before_exit(
    recorder: &Recorder,
    wait: Duration,
    code: Option<i32>,
    exit: impl FnOnce(i32) + Send + 'static,
) -> bool {
    let Some(code) = code else { return false };
    recorder.finish_before_exit(wait, move || exit(code))
}

/// Takes the recording out of `slot`, if it is there, leaving the slot saying that someone has it.
fn take_from(slot: &mut Slot) -> Taken {
    match slot {
        Slot::Running(_) => match std::mem::replace(slot, Slot::Finishing) {
            Slot::Running(running) => Taken::Running(running),
            _ => Taken::Nothing,
        },
        Slot::Finishing => Taken::Finishing,
        Slot::Idle | Slot::Starting => Taken::Nothing,
    }
}

impl Inner {
    /// Makes `next` the state, and tells the windows.
    fn publish(&self, next: RecordingView, throttled: bool) {
        let mut shown = lock(&self.shown);
        if shown.change(next, throttled, Instant::now()) {
            self.deps.surface.state(&shown.view);
        }
    }

    /// Changes the state with `change`.
    fn publish_with(&self, change: impl FnOnce(&mut RecordingView), throttled: bool) {
        let mut shown = lock(&self.shown);
        let mut next = shown.view.clone();
        change(&mut next);
        if shown.change(next, throttled, Instant::now()) {
            self.deps.surface.state(&shown.view);
        }
    }

    /// What the session says, on its own thread: it is recorded and told on. After the recording is taken to be finished what the
    /// session still reports is of no interest. `ended` hears of an end by itself.
    fn session_reported(&self, state: SessionState, ended: &Sender<EndReason>) {
        let live = |view: &RecordingView| {
            matches!(
                view.phase,
                Phase::Starting | Phase::Recording | Phase::Paused
            )
        };
        let mut shown = lock(&self.shown);
        if !live(&shown.view) {
            return;
        }
        let mut next = shown.view.clone();
        let throttled = match state {
            SessionState::Recording {
                active,
                levels,
                notes,
            } => {
                next.phase = Phase::Recording;
                next.elapsed_ms = active.as_millis() as u64;
                next.levels = levels;
                next.notes = notes;
                true
            }
            SessionState::Paused {
                active,
                levels,
                notes,
            } => {
                next.phase = Phase::Paused;
                next.elapsed_ms = active.as_millis() as u64;
                next.levels = levels;
                next.notes = notes;
                true
            }
            SessionState::Ended(reason) => {
                next.phase = Phase::Finishing;
                // Whoever takes the recording puts it together: this thread does none of that.
                let _ = ended.send(reason);
                false
            }
        };
        if shown.change(next, throttled, Instant::now()) {
            self.deps.surface.state(&shown.view);
        }
    }

    /// The recorder is free: a new recording can start. `hide` when the bar was shown.
    fn become_idle(&self, hide: bool) {
        {
            let mut slot = lock(&self.slot);
            *slot = Slot::Idle;
        }
        self.publish(RecordingView::idle(), false);
        if hide {
            self.deps.surface.hide_hud();
        }
        self.changed.notify_all();
    }
}

/// Whether `error` of putting together the recording in `folder` says that nothing was recorded, which is when its folder has nothing
/// to keep. `InvalidData` is also what a stem that cannot be read gives, and then the files are all that is left of the recording: the
/// one case of nothing recorded is the one in which the marker was removed (`finalize` removes it so that no start-up tries the folder
/// again), and any other error leaves it, for another try.
fn nothing_was_recorded(error: &io::Error, folder: &Path) -> bool {
    error.kind() == io::ErrorKind::InvalidData && !folder.join(MARKER).exists()
}

/// A recording that could not be put together (its files are kept, and the next start of the app tries again).
fn stop_failed(error: &io::Error) -> ErrorView {
    views::error(
        "Other",
        "recording.error.stop_failed",
        Some(error.to_string()),
        None,
    )
}

/// Finishes the recordings that the app left cut off (a crash, or a quit that did not wait): each folder of the library that has a
/// `recording.json` is put together from what is on the disk, and its job starts. A folder that has nothing in it is removed, and
/// one that could not be put together is left as it is, for the next start. Returns how many became meetings.
///
/// This must run before the start-up sweep of the library, which removes every `*.part` file, among them the video that was not
/// finished; and before the jobs are let go.
pub fn recover_library(meetings: &Meetings, ffmpeg: Option<&Path>) -> usize {
    let Ok(entries) = fs::read_dir(&meetings.library.directory) else {
        return 0;
    };
    let mut recovered = 0;
    for entry in entries.flatten() {
        let folder = entry.path();
        if !folder.join(MARKER).is_file() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        // The recording that goes on has its marker too: whoever holds the place of the meeting owns the folder, and a folder that is
        // owned is not touched (a recording may be started while this runs, which is as long as the biggest mix takes). The place is
        // held while the recording is put together, and let go for its job.
        let Ok(hold) = meetings.registry.hold(&id) else {
            log::info!("The recording {id} is in hand: not recovered");
            continue;
        };
        let found = recover(&folder, ffmpeg);
        drop(hold);
        match found {
            None => {}
            Some(Ok(mut finished)) => {
                finished.notes.insert(0, INTERRUPTED.to_owned());
                match meetings.finish_recording(&id, &folder, &finished) {
                    Ok(()) => {
                        log::info!("Finished the recording {id} that was cut off");
                        recovered += 1;
                    }
                    Err(e) => log::warn!("Could not start the job of the recording {id}: {e}"),
                }
            }
            Some(Err(e)) if nothing_was_recorded(&e, &folder) => {
                log::info!("The recording {id} that was cut off has nothing in it: removed");
                meetings.discard_folder(&folder);
            }
            Some(Err(e)) => log::warn!("Could not finish the recording {id}: {e}; it is kept"),
        }
    }
    recovered
}
