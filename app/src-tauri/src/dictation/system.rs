//! What connects the controller to this machine: the clock, the microphone, the panel windows, and the controller that is
//! built from them and from the rest of the app.

use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDateTime};
use dictate_services::audio::capture::{Capture, CaptureError, CaptureEvent};
use dictate_services::system;
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::dictation::backend::AppBackend;
use crate::dictation::controller::{
    CaptureFactory, Clock, Controller, Deps, PanelSink, RunningCapture,
};
use crate::dictation::save::SavePaths;
use crate::dictation::stt::WhisperStt;
use crate::hotkey::{HotkeyEvent, Hotkeys};
use crate::panel::{self, PanelState};
use crate::platform::paste::{Paster, SystemClock};
use crate::platform::rules::Os;
use crate::platform::{self, Rect};
use crate::state::AppState;
use crate::tray;

/// The time of the system.
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }

    fn wall(&self) -> NaiveDateTime {
        Local::now().naive_local()
    }
}

/// The default microphone.
pub struct Microphone;

struct Recording(Capture);

impl RunningCapture for Recording {
    fn sample_rate(&self) -> u32 {
        self.0.sample_rate()
    }

    fn stop(self: Box<Self>) {
        self.0.stop();
    }
}

impl CaptureFactory for Microphone {
    fn start(&self, tx: Sender<CaptureEvent>) -> Result<Box<dyn RunningCapture>, CaptureError> {
        Capture::start(tx).map(|capture| Box::new(Recording(capture)) as Box<dyn RunningCapture>)
    }
}

/// A debug build can listen to a WAV file instead of the microphone, to try dictation without a person to talk: set
/// `DICTATE_DEBUG_MIC_WAV` to the path of a WAV file (16-bit or 32-bit float, any rate), and each recording hears it, in real time,
/// from its start. A release build has none of this.
#[cfg(debug_assertions)]
pub struct WavReplay {
    samples: Vec<f32>,
    /// How long each buffer of [`REPLAY_BUFFER`] samples takes: a tenth of a second, in real time.
    pace: Duration,
}

/// What the replay sends at a time: a tenth of a second at 16 kHz.
#[cfg(debug_assertions)]
const REPLAY_BUFFER: usize = 1_600;

#[cfg(debug_assertions)]
impl WavReplay {
    pub fn new(samples: Vec<f32>, pace: Duration) -> Self {
        Self { samples, pace }
    }

    /// The file at `path`, heard in real time.
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let samples =
            dictate_services::whisper::local::read_wav_16k_mono(path).map_err(|e| e.to_string())?;
        Ok(Self::new(samples, Duration::from_millis(100)))
    }
}

/// The replay of a file, until it is stopped.
#[cfg(debug_assertions)]
struct Replaying {
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(debug_assertions)]
impl RunningCapture for Replaying {
    fn sample_rate(&self) -> u32 {
        16_000
    }

    fn stop(self: Box<Self>) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(debug_assertions)]
impl CaptureFactory for WavReplay {
    fn start(&self, tx: Sender<CaptureEvent>) -> Result<Box<dyn RunningCapture>, CaptureError> {
        use std::sync::atomic::Ordering;
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (samples, pace, flag) = (self.samples.clone(), self.pace, stopped.clone());
        thread::Builder::new()
            .name("debug-replay".into())
            .spawn(move || {
                for buffer in samples.chunks(REPLAY_BUFFER) {
                    if flag.load(Ordering::SeqCst) {
                        return;
                    }
                    if tx.send(CaptureEvent::Samples(buffer.to_vec())).is_err() {
                        return;
                    }
                    thread::sleep(pace);
                }
                // The file is over and the microphone is not: the channel stays open until the recording is stopped.
                while !flag.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(10));
                }
            })
            .map_err(|e| CaptureError::Stream(e.to_string()))?;
        Ok(Box::new(Replaying { stopped }))
    }
}

/// What a recording listens to: the microphone, or in a debug build the file that `DICTATE_DEBUG_MIC_WAV` names.
fn capture_source() -> Arc<dyn CaptureFactory> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("DICTATE_DEBUG_MIC_WAV") {
        match WavReplay::open(std::path::Path::new(&path)) {
            Ok(replay) => {
                log::warn!("Recordings listen to a WAV file, not to the microphone");
                return Arc::new(replay);
            }
            Err(e) => log::error!("Could not read DICTATE_DEBUG_MIC_WAV: {e}"),
        }
    }
    Arc::new(Microphone)
}

/// The two floating windows.
pub struct PanelWindows {
    app: AppHandle,
}

impl PanelSink for PanelWindows {
    fn show(&self, state: &PanelState) {
        panel::emit_state(&self.app, state);
    }

    fn set_anchor(&self, anchor: Option<Rect>) {
        panel::set_anchor(&self.app, anchor);
    }

    fn open_settings(&self) {
        tray::show_settings(&self.app);
    }

    fn language_changed(&self, lang: &str) {
        crate::commands::emit_language(&self.app, lang);
    }

    fn own_window_focused(&self) -> bool {
        self.app
            .webview_windows()
            .values()
            .any(|window| window.is_focused().unwrap_or(false))
    }
}

/// Where the commands of the panels find the controller. It exists from the start of the app, and the controller is put in
/// when it is built: it needs the keys to have been read, which can wait for the person to answer the key store's prompt.
#[derive(Clone, Default)]
pub struct DictationHandle(Arc<OnceLock<Arc<Controller>>>);

impl DictationHandle {
    /// The controller, once there is one.
    pub fn get(&self) -> Option<Arc<Controller>> {
        self.0.get().cloned()
    }

    pub fn set(&self, controller: Arc<Controller>) {
        if self.0.set(controller).is_err() {
            log::warn!("The dictation controller was built twice");
        }
    }
}

/// The controller of this app. Call it on a thread that may block, after the key flags of `state` were read (see
/// `AppState::loaded_keys`): the controller reads them without asking the key store again.
pub fn build(app: &AppHandle, state: &AppState) -> Result<Arc<Controller>, String> {
    let files = SavePaths::system().ok_or("the system has no folder for the app's data")?;
    let hotkeys = app.state::<Hotkeys>().inner().clone();
    let settings = state.settings.clone();
    Ok(Controller::new(Deps {
        platform: app.state::<platform::SystemPlatform>().0.clone(),
        clipboard: platform::current_clipboard(),
        keys: platform::current_keys(app),
        paster: Arc::new(Paster::new(Arc::new(SystemClock))),
        monitor: platform::current_monitor(app),
        capture: capture_source(),
        whisper: Arc::new(WhisperStt::new(
            settings.clone(),
            state.models.clone(),
            state.whisper.clone(),
            system::total_ram_gb(),
        )),
        backend: Arc::new(AppBackend::new(
            settings.clone(),
            state.secrets.clone(),
            state.keys.clone(),
        )),
        settings,
        keys_flags: state.keys.clone(),
        jobs: state.jobs.clone(),
        ui: Arc::new(PanelWindows { app: app.clone() }),
        files,
        clock: Arc::new(RealClock),
        hotkey_reset: Arc::new(move || hotkeys.reset()),
        os: Os::current(),
        own_pid: std::process::id(),
    }))
}

/// Builds the controller on a thread of its own, once the key store has been asked which keys exist, and then hands it what the
/// shortcut, the tray and `dictate --toggle` say. The events that came meanwhile wait in the channel.
pub fn start(
    app: AppHandle,
    state: AppState,
    handle: DictationHandle,
    events: UnboundedReceiver<HotkeyEvent>,
) {
    tauri::async_runtime::spawn_blocking(move || {
        // The first call asks the key store, which may wait for the person; the controller reads only the flags.
        state.loaded_keys();
        let controller = match build(&app, &state) {
            Ok(controller) => controller,
            Err(e) => {
                log::error!("Dictation is not available: {e}");
                return;
            }
        };
        handle.set(controller.clone());
        log::info!("Dictation is ready");
        if let Err(e) = listen_for_the_shortcut(controller, events) {
            log::error!("Could not start listening for the shortcut: {e}");
        }
    });
}

/// Hands what the shortcut, the tray and `dictate --toggle` say to the controller, one at a time, until the channel is closed.
/// It has a thread of its own: handling a press shows windows, which asks the main thread and may wait for it, and that is not
/// work for a thread that the rest of the app's async work shares.
fn listen_for_the_shortcut(
    controller: Arc<Controller>,
    mut events: UnboundedReceiver<HotkeyEvent>,
) -> std::io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("dictation-shortcut".into())
        .spawn(move || {
            while let Some(event) = events.blocking_recv() {
                controller.on_hotkey(event);
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictation::controller::fakes::Fixture;

    #[test]
    fn the_real_clock_only_goes_forward_and_sleeps_for_real() {
        let clock = RealClock;
        let before = clock.now();
        clock.sleep(Duration::from_millis(20));
        assert!(clock.now().duration_since(before) >= Duration::from_millis(20));
    }

    #[test]
    fn the_wall_clock_is_the_local_time() {
        let wall = RealClock.wall();
        let local = Local::now().naive_local();
        assert!(
            (local - wall).num_seconds().abs() < 5,
            "{wall} against {local}"
        );
    }

    #[test]
    fn the_shortcut_is_heard_on_a_thread_of_its_own_and_goes_on_after_a_bug() {
        use std::sync::atomic::Ordering;
        use tokio::sync::mpsc::unbounded_channel;

        let fixture = Fixture::new();
        let (tx, rx) = unbounded_channel();
        let listening = listen_for_the_shortcut(fixture.ctl.clone(), rx).unwrap();

        // The first press meets a bug; the one after it is heard all the same.
        fixture.ui.panic_next.store(true, Ordering::SeqCst);
        tx.send(HotkeyEvent::Pressed).unwrap();
        tx.send(HotkeyEvent::Released).unwrap();
        tx.send(HotkeyEvent::Pressed).unwrap();
        fixture.wait_for("a recording", || {
            fixture.mic.starts.load(Ordering::SeqCst) == 1
        });

        // The thread ends with the channel.
        drop(tx);
        listening.join().unwrap();
    }

    #[test]
    fn the_handle_has_no_controller_until_it_is_set_and_gives_the_same_one_after() {
        let handle = DictationHandle::default();
        let clone = handle.clone();
        assert!(handle.get().is_none());

        let fixture = Fixture::new();
        handle.set(fixture.ctl.clone());
        // A clone, which is what a command holds, sees what was set through the other.
        assert!(Arc::ptr_eq(&clone.get().unwrap(), &fixture.ctl));
        // A second controller does not replace the first.
        let other = Fixture::new();
        handle.set(other.ctl.clone());
        assert!(Arc::ptr_eq(&handle.get().unwrap(), &fixture.ctl));
    }

    #[cfg(debug_assertions)]
    mod replay {
        use super::super::*;
        use std::sync::mpsc;

        fn samples(n: usize) -> Vec<f32> {
            (0..n).map(|i| (i as f32 / 100.0).sin() * 0.5).collect()
        }

        #[test]
        fn a_wav_file_is_heard_in_order_at_16_khz_and_the_recording_stays_open_until_it_is_stopped()
        {
            let audio = samples(5_000);
            let replay = WavReplay::new(audio.clone(), Duration::ZERO);
            let (tx, rx) = mpsc::channel();

            let capture = replay.start(tx).unwrap();
            assert_eq!(capture.sample_rate(), 16_000);
            let mut heard = Vec::new();
            while heard.len() < audio.len() {
                match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                    CaptureEvent::Samples(buffer) => heard.extend(buffer),
                    other => panic!("{other:?}"),
                }
            }
            assert_eq!(heard, audio);

            // The file is over, and the microphone is not: the channel stays open, so that the controller does not take the end of
            // the file for a microphone that broke off.
            assert!(matches!(
                rx.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            capture.stop();
            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(5)),
                Err(mpsc::RecvTimeoutError::Disconnected)
            ));
        }

        #[test]
        fn a_wav_file_is_read_from_disk_as_the_samples_it_holds() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("speech.wav");
            dictate_services::audio::wav::write_16k_mono(&path, &samples(1_600)).unwrap();
            let replay = WavReplay::open(&path).unwrap();
            let (tx, rx) = mpsc::channel();
            let capture = replay.start(tx).unwrap();
            let CaptureEvent::Samples(first) = rx.recv_timeout(Duration::from_secs(5)).unwrap()
            else {
                panic!("not samples")
            };
            assert_eq!(first.len(), 1_600);
            capture.stop();
            assert!(WavReplay::open(&dir.path().join("missing.wav")).is_err());
        }
    }
}
