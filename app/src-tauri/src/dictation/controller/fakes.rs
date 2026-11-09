//! The system, as the tests of the controller have it: a platform that answers what it is told to, a clipboard and keys that
//! write down what was done to them, a microphone that the test speaks into, and a clock that moves when the test says.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{NaiveDate, NaiveDateTime};
use dictate_services::audio::capture::{CaptureError, CaptureEvent};
use dictate_services::dictation::pipeline::Backend;
use dictate_services::presets::AiProvider;
use dictate_services::secrets::Account;
use dictate_services::settings::SettingsStore;
use tokio_util::sync::CancellationToken;

use super::{CaptureFactory, Clock, Controller, Deps, LocalStt, PanelSink, RunningCapture};
use crate::dictation::save::SavePaths;
use crate::hotkey::HotkeyEvent;
use crate::panel::{PanelState, View};
use crate::platform::monitor::InputMonitor;
use crate::platform::paste::{
    self, ChangeMark, ClipSnapshot, Clipboard, Contents, KeySender, PasteError, Paster,
};
use crate::platform::rules::Os;
use crate::platform::{
    AppId, FieldInfo, FieldLookup, FieldSnapshot, Permission, Platform, Rect, DEFAULT_SCREEN,
};
use crate::state::{lock, Jobs, KeyFlags};

pub const OWN_PID: u32 = 4242;

/// The order in which the system was touched: what a paste does is to be told by what came before it.
#[derive(Default)]
pub struct CallLog(Mutex<Vec<&'static str>>);

impl CallLog {
    pub fn add(&self, what: &'static str) {
        lock(&self.0).push(what);
    }

    pub fn all(&self) -> Vec<&'static str> {
        lock(&self.0).clone()
    }
}

// ---- time ------------------------------------------------------------------------------------------------------------

/// A clock that stands still until the test moves it. A thread that sleeps on it wakes when the time it waits for has come.
pub struct FakeClock {
    base: Instant,
    wall: NaiveDateTime,
    state: Mutex<ClockState>,
    moved: Condvar,
}

struct ClockState {
    elapsed: Duration,
    closed: bool,
    sleeps: Vec<Duration>,
}

impl FakeClock {
    pub fn new() -> Arc<FakeClock> {
        Arc::new(FakeClock {
            base: Instant::now(),
            wall: NaiveDate::from_ymd_opt(2026, 11, 9)
                .unwrap()
                .and_hms_opt(10, 15, 30)
                .unwrap(),
            state: Mutex::new(ClockState {
                elapsed: Duration::ZERO,
                closed: false,
                sleeps: Vec::new(),
            }),
            moved: Condvar::new(),
        })
    }

    pub fn advance(&self, by: Duration) {
        lock(&self.state).elapsed += by;
        self.moved.notify_all();
    }

    /// How long each sleep that was asked for was meant to last.
    pub fn sleeps(&self) -> Vec<Duration> {
        lock(&self.state).sleeps.clone()
    }

    /// The test is over: whatever still sleeps gets up, and sleeps in short real steps from now on.
    pub fn close(&self) {
        lock(&self.state).closed = true;
        self.moved.notify_all();
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.base + lock(&self.state).elapsed
    }

    fn sleep(&self, duration: Duration) {
        let mut state = lock(&self.state);
        state.sleeps.push(duration);
        let until = state.elapsed + duration;
        while state.elapsed < until {
            if state.closed {
                drop(state);
                thread::sleep(Duration::from_millis(20));
                return;
            }
            state = self
                .moved
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap()
                .0;
        }
    }

    fn wall(&self) -> NaiveDateTime {
        self.wall + chrono::Duration::from_std(lock(&self.state).elapsed).unwrap()
    }
}

/// The paster's own clock: its waits take no time.
pub struct NoWait;

impl paste::Clock for NoWait {
    fn sleep(&self, _: Duration) {}
}

/// A door that a fake blocks at until the test opens it.
#[derive(Default)]
pub struct Gate {
    open: AtomicBool,
}

impl Gate {
    pub fn closed() -> Arc<Gate> {
        Arc::new(Gate::default())
    }

    pub fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    /// Waits at the door, for at most `limit`; `cancel` ends the wait too. Whether the door is open.
    pub fn wait(&self, cancel: Option<&CancellationToken>, limit: Duration) -> bool {
        let until = Instant::now() + limit;
        while !self.is_open() && Instant::now() < until {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return false;
            }
            thread::sleep(Duration::from_millis(2));
        }
        self.is_open()
    }
}

// ---- the platform ---------------------------------------------------------------------------------------------------------

/// Answers in order, and goes on giving the last one.
pub struct Script<T: Clone>(Mutex<VecDeque<T>>);

impl<T: Clone> Script<T> {
    pub fn new(first: T) -> Self {
        Script(Mutex::new(VecDeque::from([first])))
    }

    pub fn set(&self, answers: impl IntoIterator<Item = T>) {
        *lock(&self.0) = answers.into_iter().collect();
    }

    fn next(&self) -> T {
        let mut answers = lock(&self.0);
        if answers.len() > 1 {
            answers.pop_front().unwrap()
        } else {
            answers.front().cloned().expect("a script with an answer")
        }
    }
}

pub fn text_edit() -> AppId {
    AppId {
        id: "com.apple.TextEdit".into(),
        pid: Some(10),
    }
}

pub fn caret() -> Rect {
    Rect {
        x: 300.0,
        y: 200.0,
        w: 2.0,
        h: 18.0,
    }
}

pub fn a_field() -> FieldLookup {
    FieldLookup::Field(FieldInfo {
        app: text_edit(),
        terminal: false,
        anchor: Some(caret()),
    })
}

/// What the system calls with the person's answer to the prompt for the microphone.
type MicAnswer = Box<dyn FnOnce(bool) + Send>;

pub struct FakePlatform {
    pub front: Mutex<Option<AppId>>,
    pub lookups: Script<FieldLookup>,
    /// What the text field is like, one answer for each time it is looked at. Without a script the field grows with every
    /// look, as a field does that text is pasted into.
    snapshots: Mutex<Option<Script<Option<FieldSnapshot>>>>,
    growth: AtomicUsize,
    pub caret: Mutex<Option<Rect>>,
    pub mouse: Mutex<Option<(f64, f64)>>,
    pub accessibility: Mutex<Permission>,
    pub microphone: Mutex<Permission>,
    /// What the system answers when asked for the microphone.
    pub mic_grant: AtomicBool,
    /// Whether it answers at all: with `false` the prompt stays up, and its callback is kept as the system keeps it.
    pub mic_answers: AtomicBool,
    pending_prompt: Mutex<Option<MicAnswer>>,
    /// While it is closed, looking for the field doesn't finish.
    pub lookup_gate: Mutex<Option<Arc<Gate>>>,
    pub field_lookups: AtomicUsize,
    log: Arc<CallLog>,
    pub accessibility_requests: AtomicUsize,
    pub mic_requests: AtomicUsize,
}

impl FakePlatform {
    pub(crate) fn new(os: Os, log: Arc<CallLog>) -> Arc<FakePlatform> {
        Arc::new(FakePlatform {
            front: Mutex::new(Some(text_edit())),
            lookups: Script::new(a_field()),
            snapshots: Mutex::new(None),
            growth: AtomicUsize::new(0),
            caret: Mutex::new(Some(Rect {
                x: 360.0,
                ..caret()
            })),
            mouse: Mutex::new(Some((50.0, 60.0))),
            accessibility: Mutex::new(if os == Os::Mac {
                Permission::Granted
            } else {
                Permission::NotNeeded
            }),
            microphone: Mutex::new(Permission::Granted),
            mic_grant: AtomicBool::new(true),
            mic_answers: AtomicBool::new(true),
            pending_prompt: Mutex::new(None),
            lookup_gate: Mutex::new(None),
            field_lookups: AtomicUsize::new(0),
            log,
            accessibility_requests: AtomicUsize::new(0),
            mic_requests: AtomicUsize::new(0),
        })
    }

    pub fn set_front(&self, app: Option<AppId>) {
        *lock(&self.front) = app;
    }

    /// The field answers like this, one answer for each look, and then goes on giving the last.
    pub fn script_snapshots(&self, answers: impl IntoIterator<Item = Option<FieldSnapshot>>) {
        let script = Script::new(None);
        script.set(answers);
        *lock(&self.snapshots) = Some(script);
    }
}

impl Platform for FakePlatform {
    fn frontmost_app(&self) -> Option<AppId> {
        lock(&self.front).clone()
    }

    fn focused_field(&self) -> FieldLookup {
        self.log.add("look");
        self.field_lookups.fetch_add(1, Ordering::SeqCst);
        let gate = lock(&self.lookup_gate).clone();
        if let Some(gate) = gate {
            gate.wait(None, Duration::from_secs(10));
        }
        self.lookups.next()
    }

    fn field_snapshot(&self) -> Option<FieldSnapshot> {
        self.log.add("snapshot");
        if let Some(script) = lock(&self.snapshots).as_ref() {
            return script.next();
        }
        let len = 5 + self.growth.fetch_add(1, Ordering::SeqCst);
        Some(FieldSnapshot {
            len,
            sel_loc: len,
            sel_len: 0,
        })
    }

    fn caret_anchor(&self) -> Option<Rect> {
        *lock(&self.caret)
    }

    fn mouse_position(&self) -> Option<(f64, f64)> {
        *lock(&self.mouse)
    }

    fn screen_visible_frame(&self, _near: Option<(f64, f64)>) -> Rect {
        DEFAULT_SCREEN
    }

    fn accessibility(&self) -> Permission {
        *lock(&self.accessibility)
    }

    fn request_accessibility(&self) {
        self.accessibility_requests.fetch_add(1, Ordering::SeqCst);
    }

    fn microphone(&self) -> Permission {
        *lock(&self.microphone)
    }

    fn request_microphone(&self, done: Box<dyn FnOnce(bool) + Send>) {
        self.mic_requests.fetch_add(1, Ordering::SeqCst);
        if self.mic_answers.load(Ordering::SeqCst) {
            done(self.mic_grant.load(Ordering::SeqCst));
        } else {
            *lock(&self.pending_prompt) = Some(done);
        }
    }
}

// ---- the clipboard and the keys ---------------------------------------------------------------------------------

#[derive(Default)]
pub struct FakeClipboard {
    /// What was written for a paste, in order.
    pub writes: Mutex<Vec<String>>,
    /// What was copied for the user to paste.
    pub copies: Mutex<Vec<String>>,
    pub restores: AtomicUsize,
    pub fail_writes: AtomicBool,
}

impl Clipboard for FakeClipboard {
    fn snapshot(&self) -> ClipSnapshot {
        ClipSnapshot::Contents(Contents::default())
    }

    fn write_transient(&self, text: &str) -> Result<ChangeMark, String> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err("the clipboard is busy".into());
        }
        let mut writes = lock(&self.writes);
        writes.push(text.to_owned());
        Ok(ChangeMark(writes.len() as u64))
    }

    fn changed_since(&self, _: &ChangeMark) -> bool {
        false
    }

    fn restore(&self, _: ClipSnapshot) {
        self.restores.fetch_add(1, Ordering::SeqCst);
    }

    fn copy(&self, text: &str) -> Result<(), String> {
        lock(&self.copies).push(text.to_owned());
        Ok(())
    }
}

pub struct FakeKeys {
    clock: Arc<FakeClock>,
    monitor: Arc<FakeMonitor>,
    platform: Arc<FakePlatform>,
    log: Arc<CallLog>,
    /// The apps that were asked to come forward, in order.
    pub forwarded: Mutex<Vec<AppId>>,
    /// One for each paste: whether it was for a terminal, and whether the monitor of the keyboard was running while it
    /// was sent.
    pub pastes: Mutex<Vec<(bool, bool)>>,
    pub last_post: Mutex<Option<Instant>>,
    pub forward_ok: AtomicBool,
    pub paste_error: Mutex<Option<PasteError>>,
}

impl KeySender for FakeKeys {
    fn bring_forward(&self, app: &AppId) -> bool {
        self.log.add("forward");
        lock(&self.forwarded).push(app.clone());
        let comes = self.forward_ok.load(Ordering::SeqCst);
        if comes {
            self.platform.set_front(Some(app.clone()));
        }
        comes
    }

    fn modifiers_released(&self) -> bool {
        true
    }

    fn send_paste(&self, terminal: bool) -> Result<(), PasteError> {
        if let Some(error) = lock(&self.paste_error).clone() {
            return Err(error);
        }
        self.log.add("paste");
        lock(&self.pastes).push((terminal, self.monitor.running.load(Ordering::SeqCst)));
        *lock(&self.last_post) = Some(self.clock.now());
        Ok(())
    }

    fn last_post(&self) -> Option<Instant> {
        *lock(&self.last_post)
    }
}

/// Hears keys and clicks when the test says it did.
#[derive(Default)]
pub struct FakeMonitor {
    pub running: AtomicBool,
    pub starts: AtomicUsize,
    handler: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl FakeMonitor {
    /// A key goes down somewhere. Nothing happens when nobody listens.
    pub fn press_a_key(&self) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }
        let handler = lock(&self.handler).clone();
        if let Some(handler) = handler {
            handler();
        }
    }
}

impl InputMonitor for FakeMonitor {
    fn start(&self, on_input: Box<dyn Fn() + Send + Sync>) {
        self.starts.fetch_add(1, Ordering::SeqCst);
        *lock(&self.handler) = Some(Arc::from(on_input));
        self.running.store(true, Ordering::SeqCst);
    }

    fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

// ---- the microphone ---------------------------------------------------------------------------------------------

/// What the test speaks into.
pub struct FakeMic {
    pub rate: u32,
    pub starts: AtomicUsize,
    pub stopped: Arc<AtomicBool>,
    pub start_error: Mutex<Option<CaptureError>>,
    tx: Mutex<Option<Sender<CaptureEvent>>>,
}

struct FakeCapture {
    rate: u32,
    stopped: Arc<AtomicBool>,
}

impl RunningCapture for FakeCapture {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn stop(self: Box<Self>) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}

impl CaptureFactory for FakeMic {
    fn start(&self, tx: Sender<CaptureEvent>) -> Result<Box<dyn RunningCapture>, CaptureError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = lock(&self.start_error).clone() {
            return Err(error);
        }
        self.stopped.store(false, Ordering::SeqCst);
        *lock(&self.tx) = Some(tx);
        Ok(Box::new(FakeCapture {
            rate: self.rate,
            stopped: self.stopped.clone(),
        }))
    }
}

impl FakeMic {
    /// A sound of `seconds` seconds, in buffers of a quarter of a second. The last buffer is a little louder than the others:
    /// when the meter shows [`FakeMic::MARKER_LEVEL`], everything that was said has been heard.
    pub fn speak(&self, seconds: f64) {
        let quarter = (f64::from(self.rate) / 4.0) as usize;
        let buffers = ((seconds * 4.0).round() as usize).max(1);
        for _ in 1..buffers {
            self.send(CaptureEvent::Samples(vec![0.1; quarter]));
        }
        self.send(CaptureEvent::Samples(vec![Self::MARKER; quarter]));
    }

    /// The amplitude of the last buffer of a sound.
    pub const MARKER: f32 = 0.125;

    /// What the meter shows for it.
    pub fn marker_level() -> f32 {
        dictate_services::audio::level::level(&[Self::MARKER; 64])
    }

    pub fn send(&self, event: CaptureEvent) {
        if let Some(tx) = lock(&self.tx).as_ref() {
            let _ = tx.send(event);
        }
    }
}

// ---- the speech ---------------------------------------------------------------------------------------------------

/// One call to local speech recognition.
#[derive(Debug, Clone, PartialEq)]
pub struct SttCall {
    pub samples: usize,
    pub language: String,
    pub prompt: String,
}

type Answer = Box<dyn Fn(usize) -> Result<String, String> + Send + Sync>;

pub struct FakeStt {
    pub available: AtomicBool,
    pub calls: Mutex<Vec<SttCall>>,
    /// What the nth call (from 0) answers.
    pub answer: Mutex<Answer>,
    /// While it is closed, a call waits (and ends when it is cancelled).
    pub gate: Mutex<Option<Arc<Gate>>>,
}

impl FakeStt {
    pub fn say(&self, text: &str) {
        let text = text.to_owned();
        *lock(&self.answer) = Box::new(move |_| Ok(text.clone()));
    }

    pub fn call_count(&self) -> usize {
        lock(&self.calls).len()
    }
}

impl LocalStt for FakeStt {
    fn transcribe(
        &self,
        samples_16k: &[f32],
        language: &str,
        prompt: &str,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        let nth = {
            let mut calls = lock(&self.calls);
            calls.push(SttCall {
                samples: samples_16k.len(),
                language: language.to_owned(),
                prompt: prompt.to_owned(),
            });
            calls.len() - 1
        };
        let gate = lock(&self.gate).clone();
        if let Some(gate) = gate {
            gate.wait(Some(cancel), Duration::from_secs(10));
        }
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        (lock(&self.answer))(nth)
    }

    fn available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }
}

// ---- the network ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum BackendCall {
    Transcribe { wav: PathBuf },
    TranscribeAndEdit { wav: PathBuf, instruction: String },
    Refine { text: String, instruction: String },
}

pub struct FakeBackend {
    pub calls: Mutex<Vec<BackendCall>>,
    pub transcribe: Mutex<Result<String, String>>,
    pub transcribe_and_edit: Mutex<Result<String, String>>,
    pub refine: Mutex<Result<String, String>>,
    /// While one is closed, the call does not answer.
    pub transcribe_gate: Mutex<Option<Arc<Gate>>>,
    pub refine_gate: Mutex<Option<Arc<Gate>>>,
}

impl FakeBackend {
    pub fn calls(&self) -> Vec<BackendCall> {
        lock(&self.calls).clone()
    }

    pub fn uploads(&self) -> usize {
        self.calls()
            .iter()
            .filter(|call| !matches!(call, BackendCall::Refine { .. }))
            .count()
    }

    pub fn refines(&self) -> Vec<(String, String)> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                BackendCall::Refine { text, instruction } => Some((text, instruction)),
                _ => None,
            })
            .collect()
    }
}

async fn wait_at(gate: Option<Arc<Gate>>) {
    if let Some(gate) = gate {
        while !gate.is_open() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
}

#[async_trait::async_trait]
impl Backend for FakeBackend {
    async fn transcribe(&self, wav: &Path, _timeout: Duration) -> Result<String, String> {
        lock(&self.calls).push(BackendCall::Transcribe {
            wav: wav.to_owned(),
        });
        let gate = lock(&self.transcribe_gate).clone();
        wait_at(gate).await;
        lock(&self.transcribe).clone()
    }

    async fn transcribe_and_edit(
        &self,
        wav: &Path,
        instruction: &str,
        _timeout: Duration,
    ) -> Result<String, String> {
        lock(&self.calls).push(BackendCall::TranscribeAndEdit {
            wav: wav.to_owned(),
            instruction: instruction.to_owned(),
        });
        let gate = lock(&self.transcribe_gate).clone();
        wait_at(gate).await;
        lock(&self.transcribe_and_edit).clone()
    }

    async fn refine(&self, text: &str, instruction: &str) -> Result<String, String> {
        lock(&self.calls).push(BackendCall::Refine {
            text: text.to_owned(),
            instruction: instruction.to_owned(),
        });
        let gate = lock(&self.refine_gate).clone();
        wait_at(gate).await;
        lock(&self.refine).clone()
    }
}

// ---- the panels ----------------------------------------------------------------------------------------------------

#[derive(Default)]
pub struct FakeUi {
    /// The next state that is shown panics, as a bug in showing a window might.
    pub panic_next: AtomicBool,
    pub states: Mutex<Vec<PanelState>>,
    pub anchors: Mutex<Vec<Option<Rect>>>,
    pub settings_opened: AtomicUsize,
}

impl FakeUi {
    pub fn last(&self) -> PanelState {
        lock(&self.states)
            .last()
            .cloned()
            .unwrap_or_else(PanelState::hidden)
    }

    pub fn history(&self) -> Vec<PanelState> {
        lock(&self.states).clone()
    }
}

impl PanelSink for FakeUi {
    fn show(&self, state: &PanelState) {
        if self.panic_next.swap(false, Ordering::SeqCst) {
            panic!("a bug in showing the panels");
        }
        lock(&self.states).push(state.clone());
    }

    fn set_anchor(&self, anchor: Option<Rect>) {
        lock(&self.anchors).push(anchor);
    }

    fn open_settings(&self) {
        self.settings_opened.fetch_add(1, Ordering::SeqCst);
    }
}

// ---- all of it together --------------------------------------------------------------------------------------------

pub struct Fixture {
    pub ctl: Arc<Controller>,
    pub platform: Arc<FakePlatform>,
    pub clipboard: Arc<FakeClipboard>,
    pub keys: Arc<FakeKeys>,
    pub monitor: Arc<FakeMonitor>,
    pub mic: Arc<FakeMic>,
    pub stt: Arc<FakeStt>,
    pub backend: Arc<FakeBackend>,
    pub settings: Arc<SettingsStore>,
    pub flags: Arc<KeyFlags>,
    pub jobs: Arc<Jobs>,
    pub ui: Arc<FakeUi>,
    pub clock: Arc<FakeClock>,
    pub resets: Arc<AtomicUsize>,
    pub log: Arc<CallLog>,
    pub dir: tempfile::TempDir,
}

impl Fixture {
    /// A Mac with a text field in front, a microphone at 16 kHz, a Whisper model that hears "hello world", and an AI model
    /// that makes it "Hello, world.". Dictations are saved.
    pub fn new() -> Fixture {
        Fixture::on(Os::Mac)
    }

    pub fn on(os: Os) -> Fixture {
        Fixture::build(os, |_| {})
    }

    /// A fixture whose temporary folder already holds what `leftovers` puts there: what an earlier run left behind.
    pub fn with_leftovers(leftovers: impl FnOnce(&Path)) -> Fixture {
        Fixture::build(Os::Mac, leftovers)
    }

    fn build(os: Os, in_temp_folder: impl FnOnce(&Path)) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let (settings, warning) = SettingsStore::open(dir.path().join("settings.json"));
        assert!(warning.is_none());
        let settings = Arc::new(settings);
        let flags = Arc::new(KeyFlags::default());
        flags.set(Account::Ai, true);
        // Anthropic needs a key and has one: the AI edit is configured.
        assert_eq!(settings.get().ai_provider, AiProvider::Anthropic);

        let clock = FakeClock::new();
        let log = Arc::new(CallLog::default());
        let platform = FakePlatform::new(os, log.clone());
        let clipboard = Arc::new(FakeClipboard::default());
        let monitor = Arc::new(FakeMonitor::default());
        let keys = Arc::new(FakeKeys {
            clock: clock.clone(),
            monitor: monitor.clone(),
            platform: platform.clone(),
            log: log.clone(),
            forwarded: Mutex::default(),
            pastes: Mutex::default(),
            last_post: Mutex::new(None),
            forward_ok: AtomicBool::new(true),
            paste_error: Mutex::new(None),
        });
        let mic = Arc::new(FakeMic {
            rate: 16_000,
            starts: AtomicUsize::new(0),
            stopped: Arc::new(AtomicBool::new(false)),
            start_error: Mutex::new(None),
            tx: Mutex::new(None),
        });
        let stt = Arc::new(FakeStt {
            available: AtomicBool::new(true),
            calls: Mutex::default(),
            answer: Mutex::new(Box::new(|_| Ok("hello world".to_string()))),
            gate: Mutex::new(None),
        });
        let backend = Arc::new(FakeBackend {
            calls: Mutex::default(),
            transcribe: Mutex::new(Ok("hello from the api".into())),
            transcribe_and_edit: Mutex::new(Ok("Hello from the API.".into())),
            refine: Mutex::new(Ok("Hello, world.".into())),
            transcribe_gate: Mutex::new(None),
            refine_gate: Mutex::new(None),
        });
        let ui = Arc::new(FakeUi::default());
        let jobs = Arc::new(Jobs::default());
        let resets = Arc::new(AtomicUsize::new(0));
        let reset = resets.clone();
        let files = SavePaths {
            dir: dir.path().join("saved"),
            temp: dir.path().join("temp"),
        };
        std::fs::create_dir_all(&files.temp).unwrap();
        in_temp_folder(&files.temp);

        let ctl = Controller::new(Deps {
            platform: platform.clone(),
            clipboard: clipboard.clone(),
            keys: keys.clone(),
            paster: Arc::new(Paster::new(Arc::new(NoWait))),
            monitor: monitor.clone(),
            capture: mic.clone(),
            whisper: stt.clone(),
            backend: backend.clone(),
            settings: settings.clone(),
            keys_flags: flags.clone(),
            jobs: jobs.clone(),
            ui: ui.clone(),
            files,
            clock: clock.clone(),
            hotkey_reset: Arc::new(move || {
                reset.fetch_add(1, Ordering::SeqCst);
            }),
            os,
            own_pid: OWN_PID,
        });
        Fixture {
            ctl,
            platform,
            clipboard,
            keys,
            monitor,
            mic,
            stt,
            backend,
            settings,
            flags,
            jobs,
            ui,
            clock,
            resets,
            log,
            dir,
        }
    }

    pub fn saved_dir(&self) -> PathBuf {
        self.dir.path().join("saved")
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.dir.path().join("temp")
    }

    /// The files of a folder, by name; none when it isn't there.
    pub fn files_in(&self, dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    pub fn press(&self) {
        self.ctl.on_hotkey(HotkeyEvent::Pressed);
    }

    pub fn release(&self) {
        self.ctl.on_hotkey(HotkeyEvent::Released);
    }

    pub fn change_settings(&self, change: impl FnOnce(&mut dictate_services::settings::Settings)) {
        self.settings.update(change).unwrap();
    }

    /// Waits, in real time, until `condition` holds.
    pub fn wait_for(&self, what: &str, condition: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < until, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Waits until `condition` holds, and lets the clock run on while it waits: for what sleeps (the landing check, the
    /// timer of the chip) to get on.
    pub fn run_until(&self, what: &str, condition: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < until, "timed out waiting for {what}");
            self.clock.advance(Duration::from_millis(50));
            thread::sleep(Duration::from_millis(2));
        }
    }

    pub fn wait_for_view(&self, view: View) {
        self.wait_for(&format!("the view {view:?}"), || {
            self.ui.last().view == view
        });
    }

    /// Lets what is running get on for a moment: for a test that asserts that something did not happen.
    pub fn settle(&self) {
        thread::sleep(Duration::from_millis(60));
    }

    /// A toggle dictation up to the point where it listens: pressed, and the bubble is up.
    pub fn start_listening(&self) {
        self.press();
        self.wait_for("the first state of a listening bubble", || {
            let last = self.ui.last();
            last.phase == dictate_services::dictation::machine::Phase::Recording
                && last.view != View::Hidden
        });
        self.wait_for("the microphone", || {
            self.mic.starts.load(Ordering::SeqCst) > 0
        });
    }

    /// Says something for `seconds` seconds, and waits until the controller has heard all of it.
    pub fn say(&self, seconds: f64) {
        self.mic.speak(seconds);
        let marker = FakeMic::marker_level();
        self.wait_for("the sound to be heard", || {
            (self.ctl.state().level - marker).abs() < 1e-6
        });
    }

    /// Says something for `seconds` seconds, a second at a time, and lets as many seconds go by on the clock, so that live
    /// text is asked for after each: the loop of the recording looks at the clock when audio comes in.
    pub fn talk(&self, seconds: u32) {
        for _ in 0..seconds {
            self.say(1.0);
            self.clock.advance(Duration::from_secs(1));
            self.say(0.25);
            self.settle();
        }
    }

    /// Goes on talking, a second at a time, until `condition` holds: for what is asked for by the tick of the clock and answered on
    /// a thread of its own, which may be slow.
    pub fn talk_until(&self, what: &str, condition: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < until, "timed out waiting for {what}");
            self.talk(1);
        }
    }

    pub fn chip_is_up(&self) -> bool {
        self.ui.last().view == View::Chip
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.ctl.cancel();
        self.clock.close();
    }
}
