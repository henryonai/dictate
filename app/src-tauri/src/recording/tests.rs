//! The recorder against scripted sources (`FakeCapturer`), a scripted pipeline and a library in a temporary folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use dictate_core::meetings::library::{files, MeetingLibrary, Meta, SCREEN_SOURCE};
use dictate_services::media::tools::Tools;
use dictate_services::meetings::fakes::{gate, FakeBackend, GateControl};
use dictate_services::record::capture::{SourceInfo, Sources};
use dictate_services::record::fakes::FakeCapturer;
use dictate_services::record::marker::{Marker, MARKER};
use dictate_services::record::quality::Quality;
use dictate_services::record::session::Levels;
use dictate_services::record::stem::StemWriter;
use dictate_services::record::{
    Capturer, EndReason, Mode, Note, RecordOptions, RunningSource, SourceEvent, StartError, Track,
};
use dictate_services::settings::{SettingsStore, UiLanguage};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::{Listener, Manager};

use super::views::{Phase, RecordingView, ScreenAvailability};
use super::*;
use crate::dictation::controller::fakes::{CallLog, FakePlatform};
use crate::meetings::jobs::{Events, Registry};
use crate::meetings::library::{Meetings, Trash};
use crate::meetings::notify::{Notice, NoticeKind, Notifier};
use crate::meetings::views::JobView;
use crate::platform::rules::Os;
use crate::platform::Permission;
use crate::state::{lock, Job, Jobs};

/// A safety net, not a way to wait for a time: what a test waits for happens in a moment, or the test is wrong.
const PATIENCE: Duration = Duration::from_secs(30);

/// Waits until `done` is true.
fn wait_until(what: &str, done: impl Fn() -> bool) {
    let give_up = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < give_up, "waited for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// What the window would hear of the library.
#[derive(Default)]
struct Heard {
    changed: AtomicUsize,
}

impl Events for Heard {
    fn job(&self, _: &JobView) {}
    fn changed(&self) {
        self.changed.fetch_add(1, Ordering::SeqCst);
    }
}

/// The banners that were shown.
#[derive(Default)]
struct Notices(Mutex<Vec<Notice>>);

impl Notifier for Notices {
    fn notify(&self, notice: Notice) {
        lock(&self.0).push(notice);
    }
}

/// A Trash that takes a folder away as the real one does.
#[derive(Default)]
struct FakeTrash {
    moved: Mutex<Vec<PathBuf>>,
}

impl Trash for FakeTrash {
    fn delete(&self, folder: &Path) -> Result<(), String> {
        lock(&self.moved).push(folder.to_path_buf());
        fs::remove_dir_all(folder).map_err(|e| e.to_string())
    }
}

/// What the recorder shows: the states it sent, and how often the bar was shown and hidden.
#[derive(Default)]
struct FakeSurface {
    views: Mutex<Vec<RecordingView>>,
    shown: AtomicUsize,
    hidden: AtomicUsize,
}

impl Surface for FakeSurface {
    fn state(&self, view: &RecordingView) {
        lock(&self.views).push(view.clone());
    }
    fn show_hud(&self) {
        self.shown.fetch_add(1, Ordering::SeqCst);
    }
    fn hide_hud(&self) {
        self.hidden.fetch_add(1, Ordering::SeqCst);
    }
}

struct Rig {
    recorder: Recorder,
    fake: Arc<FakeCapturer>,
    backend: Arc<FakeBackend>,
    trash: Arc<FakeTrash>,
    surface: Arc<FakeSurface>,
    notices: Arc<Notices>,
    heard: Arc<Heard>,
    jobs: Arc<Jobs>,
    settings: Arc<SettingsStore>,
    meetings: Meetings,
    tools: Arc<Mutex<Tools>>,
    /// How many times a video encoder was looked for.
    probes: Arc<AtomicUsize>,
    /// Whether the video encoder that is looked for works.
    encoder: Arc<AtomicBool>,
    library: PathBuf,
    _dir: tempfile::TempDir,
}

impl Rig {
    /// A recorder whose microphone delivers a tone.
    fn new() -> Rig {
        Rig::using(FakeCapturer::new().tone(Track::Mic, 440.0))
    }

    fn using(fake: FakeCapturer) -> Rig {
        Rig::with(fake, |_| {})
    }

    /// A recorder whose meetings are held at the audio step of their job: what the recording wrote in `meta.json` is read before the
    /// pipeline writes its own (a length, a title), which it does as soon as it is let go. The job is let go with the control.
    fn gated(fake: FakeCapturer) -> (Rig, GateControl) {
        let (held, control) = gate();
        let mut backend = FakeBackend::new();
        backend.gates.prepare = Some(held);
        (Rig::build(fake, backend, |_| {}), control)
    }

    fn with(fake: FakeCapturer, customize: impl FnOnce(&mut Deps)) -> Rig {
        Rig::build(fake, FakeBackend::new(), customize)
    }

    fn build(fake: FakeCapturer, backend: FakeBackend, customize: impl FnOnce(&mut Deps)) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("library");
        fs::create_dir(&library).unwrap();
        let (store, _) = SettingsStore::open(dir.path().join("settings.json"));
        let settings = Arc::new(store);
        settings.update(|s| s.ui_language = UiLanguage::En).unwrap();
        let heard = Arc::new(Heard::default());
        let backend = Arc::new(backend);
        let jobs = Arc::new(Jobs::default());
        let registry = Registry::new(
            jobs.clone(),
            backend.clone(),
            heard.clone(),
            Arc::new(Notices::default()),
            Box::new(|| 1_000),
        );
        registry.open();
        let trash = Arc::new(FakeTrash::default());
        let meetings = Meetings::new(
            MeetingLibrary::new(library.clone()),
            registry,
            None,
            heard.clone(),
            trash.clone(),
        );
        let fake = Arc::new(fake);
        let surface = Arc::new(FakeSurface::default());
        let notices = Arc::new(Notices::default());
        let tools = Arc::new(Mutex::new(Tools {
            ffmpeg: None,
            yt_dlp: None,
        }));
        let probes = Arc::new(AtomicUsize::new(0));
        let encoder = Arc::new(AtomicBool::new(true));
        let platform = FakePlatform::new(Os::Windows, Arc::new(CallLog::default()));
        let mut deps = Deps {
            meetings: meetings.clone(),
            jobs: jobs.clone(),
            settings: settings.clone(),
            platform: platform.clone(),
            surface: surface.clone(),
            notifier: notices.clone(),
            tools: {
                let tools = tools.clone();
                Arc::new(move || lock(&tools).clone())
            },
            capturer: {
                let fake = fake.clone();
                Arc::new(move |_ffmpeg| -> Arc<dyn Capturer> { fake.clone() })
            },
            sources: Arc::new(|| Ok(Sources::default())),
            encoder_works: {
                let (probes, encoder) = (probes.clone(), encoder.clone());
                Arc::new(move |_ffmpeg| {
                    probes.fetch_add(1, Ordering::SeqCst);
                    encoder.load(Ordering::SeqCst)
                })
            },
        };
        customize(&mut deps);
        Rig {
            recorder: Recorder::new(deps),
            fake,
            backend,
            trash,
            surface,
            notices,
            heard,
            jobs,
            settings,
            meetings,
            tools,
            probes,
            encoder,
            library,
            _dir: dir,
        }
    }

    fn set_ffmpeg(&self, path: Option<&str>) {
        lock(&self.tools).ffmpeg = path.map(PathBuf::from);
    }

    /// The names of the folders in the library.
    fn folders(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.library)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The id of the meeting that the recording is becoming.
    fn meeting_id(&self) -> String {
        self.recorder.view().meeting_id.expect("a meeting id")
    }

    fn meta(&self, id: &str) -> Meta {
        Meta::read(&self.library.join(id)).expect("meta.json")
    }

    fn phase(&self) -> Phase {
        self.recorder.view().phase
    }

    /// Waits until the recording has some sound in it: a third of a second of it, and a buffer of the microphone that was measured.
    fn record_a_moment(&self) {
        wait_until("some sound in the recording", || {
            let view = self.recorder.view();
            view.elapsed_ms >= 300 && view.levels.mic.is_some_and(|level| level > 0.0)
        });
    }

    fn wait_for_idle(&self) {
        wait_until("the recorder to be idle", || self.phase() == Phase::Idle);
    }

    fn notices(&self) -> Vec<Notice> {
        lock(&self.notices.0).clone()
    }
}

fn audio(mic: bool, system_audio: bool) -> RecordOptions {
    RecordOptions {
        mode: Mode::Audio,
        quality: Quality::Standard,
        mic,
        system_audio,
    }
}

fn screen() -> RecordOptions {
    RecordOptions {
        mode: Mode::Screen { display: None },
        quality: Quality::Standard,
        mic: true,
        system_audio: true,
    }
}

// ---- starting

#[test]
fn a_refused_start_leaves_no_folder() {
    let rig = Rig::using(
        FakeCapturer::new().fails(Track::Mic, StartError::Source("no microphone found".into())),
    );

    let error = rig.recorder.start(audio(true, false)).unwrap_err();

    assert_eq!(error.key, "recording.error.source");
    assert_eq!(error.detail.as_deref(), Some("no microphone found"));
    assert!(rig.folders().is_empty(), "{:?}", rig.folders());
    assert_eq!(rig.phase(), Phase::Idle);
    assert!(
        !rig.jobs.is_active(&Job::Recording),
        "the place is free again"
    );
    assert_eq!(
        rig.surface.shown.load(Ordering::SeqCst),
        0,
        "no bar was shown"
    );
    assert!(rig.notices().is_empty());
}

#[test]
fn a_permission_that_is_missing_is_told_with_the_way_to_settings() {
    let rig =
        Rig::using(FakeCapturer::new().fails(Track::System, StartError::NeedsScreenPermission));

    let error = rig.recorder.start(audio(false, true)).unwrap_err();

    assert_eq!(error.key, "recording.error.needs_screen_permission");
    assert_eq!(error.command.as_deref(), Some("open_settings"));
    assert!(rig.folders().is_empty());
}

#[test]
fn sound_alone_with_both_sounds_off_is_refused_before_a_folder_is_made() {
    let rig = Rig::new();

    let error = rig.recorder.start(audio(false, false)).unwrap_err();

    assert_eq!(error.key, "recording.error.nothing_to_record");
    assert!(rig.folders().is_empty());
    assert!(rig.fake.started().is_empty());
}

#[test]
fn no_ffmpeg_means_audio_only() {
    let rig = Rig::new();
    rig.set_ffmpeg(None);

    let sources = rig.recorder.sources();
    assert_eq!(sources.screen, ScreenAvailability::NeedsFfmpeg);
    assert_eq!(
        sources.ffmpeg_command,
        dictate_services::media::tools::install_command(
            dictate_services::media::tools::Tool::Ffmpeg
        )
    );
    let error = rig.recorder.start(screen()).unwrap_err();
    assert_eq!(error.key, "recording.error.needs_ffmpeg");
    assert_eq!(
        error.command.as_deref(),
        Some(sources.ffmpeg_command.as_str())
    );
    assert!(
        rig.folders().is_empty(),
        "the screen was refused before a folder was made"
    );
    assert!(rig.fake.started().is_empty());

    // The sound alone does not need it.
    rig.recorder.start(audio(true, false)).unwrap();
    assert_eq!(rig.phase(), Phase::Recording);
    rig.recorder.discard().unwrap();
}

#[test]
fn a_second_start_is_busy() {
    let rig = Rig::new();
    rig.recorder.start(audio(true, false)).unwrap();

    let error = rig.recorder.start(audio(true, false)).unwrap_err();

    assert_eq!(error.key, "meetings.error.busy");
    assert_eq!(rig.phase(), Phase::Recording, "the first goes on");
    assert_eq!(rig.folders().len(), 1);
    rig.recorder.discard().unwrap();
}

#[test]
fn the_choices_are_remembered_even_when_the_start_is_refused() {
    let rig = Rig::new();
    rig.set_ffmpeg(None);

    rig.recorder
        .start(RecordOptions {
            mode: Mode::Window { id: "7".into() },
            quality: Quality::Small,
            mic: false,
            system_audio: true,
        })
        .unwrap_err();

    let saved = rig.settings.get();
    assert_eq!(saved.record_mode, "window");
    assert_eq!(saved.record_quality, Quality::Small);
    assert!(!saved.record_mic && saved.record_system_audio);
    // The picker has them back, and not the window: it may be gone.
    let last = rig.recorder.sources().last;
    assert_eq!(
        last,
        RecordOptions {
            mode: Mode::Window { id: String::new() },
            quality: Quality::Small,
            mic: false,
            system_audio: true,
        }
    );
}

// ---- what a recording becomes

#[test]
fn a_recording_becomes_a_meeting_with_the_swift_source() {
    let (rig, job) = Rig::gated(FakeCapturer::new().tone(Track::Mic, 440.0));

    rig.recorder.start(audio(true, false)).unwrap();
    let id = rig.meeting_id();
    assert_eq!(rig.phase(), Phase::Recording);
    assert!(rig.jobs.is_active(&Job::Recording));
    assert_eq!(rig.surface.shown.load(Ordering::SeqCst), 1);
    // The meeting is in the library from the start, with its title; it has no job until it ends.
    assert_eq!(rig.folders(), std::slice::from_ref(&id));
    assert_eq!(rig.meta(&id).title.split(' ').next(), Some("Audio"));
    assert!(rig.backend.calls().is_empty());
    // A click on its row cannot start a job on a folder that is being written.
    assert_eq!(
        rig.meetings.rerun(&id).unwrap_err().key,
        "meetings.error.busy"
    );

    rig.record_a_moment();
    rig.recorder.stop().unwrap();

    // The job is at its audio step, held: the meeting is as the recording made it.
    wait_until("the job to start", || !rig.backend.calls().is_empty());
    let meta = rig.meta(&id);
    assert_eq!(meta.source, SCREEN_SOURCE);
    assert_eq!(meta.title.split(' ').next(), Some("Audio"));
    let media = PathBuf::from(&meta.media);
    assert!(media.is_absolute(), "{media:?}");
    let name = media.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name == "audio.m4a" || name == "recording.wav", "{name}");
    assert!(media.is_file());
    assert!(
        meta.duration.is_some_and(|seconds| seconds > 0.0),
        "{:?}",
        meta.duration
    );
    // The recording's own files are gone, and the marker with them.
    assert!(!rig.library.join(&id).join(MARKER).exists());
    // The job starts at the audio, from the file that was made.
    assert_eq!(rig.backend.calls(), ["prepare_audio"]);
    assert_eq!(rig.backend.media_seen(), [media]);
    assert_eq!(rig.phase(), Phase::Idle);
    assert!(!rig.jobs.is_active(&Job::Recording));
    assert_eq!(rig.surface.hidden.load(Ordering::SeqCst), 1);
    assert!(
        rig.notices().is_empty(),
        "stopping is not something to be told of"
    );
    let _ = job.release.send(());
}

#[test]
fn the_state_goes_out_as_the_recording_does() {
    let rig = Rig::new();

    rig.recorder.start(audio(true, false)).unwrap();
    rig.record_a_moment();
    rig.recorder.stop().unwrap();

    let views = lock(&rig.surface.views).clone();
    let phases: Vec<Phase> = views.iter().map(|view| view.phase).collect();
    assert_eq!(phases.first(), Some(&Phase::Starting));
    assert!(phases.contains(&Phase::Recording));
    assert!(phases.contains(&Phase::Finishing));
    assert_eq!(phases.last(), Some(&Phase::Idle));
    // In that order, and Idle only at the end.
    let position = |phase| phases.iter().position(|p| *p == phase).unwrap();
    assert!(position(Phase::Starting) < position(Phase::Recording));
    assert!(position(Phase::Recording) < position(Phase::Finishing));
    assert_eq!(phases.iter().filter(|p| **p == Phase::Idle).count(), 1);
    // The meters of the tracks that are on are there from the first state (quiet until the first buffer), and the track that is
    // off has none, in every state of the recording: the bar does not wait for a buffer to know what to show.
    assert_eq!(
        views[0].levels,
        Levels {
            mic: Some(0.0),
            system: None
        }
    );
    for view in views.iter().filter(|view| view.phase == Phase::Recording) {
        assert_eq!(view.mode, Some(Mode::Audio));
        assert!(view.meeting_id.is_some(), "{view:?}");
        assert!(view.levels.mic.is_some(), "{view:?}");
        assert_eq!(view.levels.system, None, "{view:?}");
    }
    // And once sound arrives it shows on the meter.
    assert!(
        views.iter().any(|view| view.phase == Phase::Recording
            && view.levels.mic.is_some_and(|level| level > 0.0)),
        "{views:?}"
    );
    assert_eq!(views.last().unwrap(), &RecordingView::idle());
}

#[test]
fn what_a_track_has_to_say_is_in_the_state() {
    let rig = Rig::new();
    rig.recorder.start(audio(true, false)).unwrap();

    rig.fake.send_note(Track::Mic, Note::Silent);

    wait_until("the note to reach the state", || {
        !rig.recorder.view().notes.is_empty()
    });
    let notes = rig.recorder.view().notes;
    assert_eq!(
        (notes[0].track, &notes[0].note),
        (Track::Mic, &Note::Silent)
    );
    rig.recorder.discard().unwrap();
}

#[test]
fn a_recording_that_has_nothing_in_it_is_told_and_leaves_no_folder_and_no_job() {
    // The microphone delivers nothing, and the recording is stopped as it begins.
    let rig = Rig::using(FakeCapturer::new());
    rig.recorder.start(audio(true, false)).unwrap();

    let error = rig.recorder.stop().unwrap_err();

    assert_eq!(error.key, "recording.error.nothing_recorded");
    assert!(rig.folders().is_empty(), "{:?}", rig.folders());
    assert!(rig.backend.calls().is_empty());
    assert_eq!(rig.phase(), Phase::Idle);
    assert!(!rig.jobs.is_active(&Job::Recording));
    assert_eq!(rig.surface.hidden.load(Ordering::SeqCst), 1);
}

#[test]
fn pausing_and_resuming_show_in_the_state() {
    let rig = Rig::new();
    rig.recorder.start(audio(true, false)).unwrap();

    rig.recorder.pause();
    wait_until("the pause", || rig.phase() == Phase::Paused);
    let paused_at = rig.recorder.view().elapsed_ms;
    std::thread::sleep(Duration::from_millis(150));
    // The clock stopped where it was.
    assert!(rig.recorder.view().elapsed_ms.abs_diff(paused_at) < 100);

    rig.recorder.resume();
    wait_until("the resume", || rig.phase() == Phase::Recording);
    rig.recorder.discard().unwrap();
}

#[test]
fn a_recording_that_ends_by_itself_is_made_into_a_meeting_and_the_person_is_told() {
    let (rig, job) = Rig::gated(FakeCapturer::new().tone(Track::Mic, 440.0));

    rig.recorder.start(audio(true, false)).unwrap();
    let id = rig.meeting_id();
    // The screen goes away once there is sound in the recording (not after a time, which a busy machine may not give it).
    rig.record_a_moment();
    rig.fake.send_ended(Track::Mic, EndReason::DisplayGone);
    wait_until("the banner", || !rig.notices().is_empty());
    rig.wait_for_idle();

    let notices = rig.notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].id, id);
    assert_eq!(notices[0].title, rig.meta(&id).title);
    // The key says no more than is known: on Linux a stream that breaks ends in this too.
    assert_eq!(
        notices[0].kind,
        NoticeKind::Recording {
            key: "recording.ended.display_gone".into(),
            detail: String::new(),
        }
    );
    let meta = rig.meta(&id);
    assert!(Path::new(&meta.media).is_file());
    assert!(meta.duration.is_some());
    wait_until("the job to start", || !rig.backend.calls().is_empty());
    assert!(!rig.jobs.is_active(&Job::Recording));
    assert_eq!(rig.surface.hidden.load(Ordering::SeqCst), 1);
    let _ = job.release.send(());
}

#[test]
fn a_stop_that_comes_as_the_recording_ends_by_itself_is_not_an_error() {
    let rig = Rig::new();
    rig.recorder.start(audio(true, false)).unwrap();
    rig.record_a_moment();
    rig.fake.send_ended(Track::Mic, EndReason::WindowClosed);

    // Whoever comes first makes the meeting: the other finds it done or on its way.
    let stopped = rig.recorder.stop();

    assert!(
        stopped.is_ok() || stopped.unwrap_err().key == "meetings.error.busy",
        "a second stop is told at most that nothing runs"
    );
    rig.wait_for_idle();
    wait_until("the job to start", || !rig.backend.calls().is_empty());
}

// ---- throwing a recording away

#[test]
fn discard_removes_the_meeting_and_starts_no_job() {
    let rig = Rig::new();
    rig.recorder.start(audio(true, false)).unwrap();
    let id = rig.meeting_id();
    let folder = rig.library.join(&id);
    rig.record_a_moment();
    let changed = rig.heard.changed.load(Ordering::SeqCst);

    rig.recorder.discard().unwrap();

    assert_eq!(*lock(&rig.trash.moved), std::slice::from_ref(&folder));
    assert!(!folder.exists());
    assert!(rig.folders().is_empty());
    assert_eq!(rig.phase(), Phase::Idle);
    assert!(rig.backend.calls().is_empty(), "no job");
    assert_eq!(rig.meetings.registry.status(&id), None);
    assert!(rig.notices().is_empty(), "no notification");
    assert!(!rig.jobs.is_active(&Job::Recording));
    assert!(
        rig.heard.changed.load(Ordering::SeqCst) > changed,
        "the list is told"
    );
    assert_eq!(rig.surface.hidden.load(Ordering::SeqCst), 1);
    let phases: Vec<Phase> = lock(&rig.surface.views).iter().map(|v| v.phase).collect();
    assert!(phases.contains(&Phase::Finishing));
    assert_eq!(phases.last(), Some(&Phase::Idle));
    // The sources were stopped.
    assert_eq!(rig.fake.stopped(), [Track::Mic]);
}

#[test]
fn discard_without_a_recording_is_refused() {
    let rig = Rig::new();

    let error = rig.recorder.discard().unwrap_err();

    assert_eq!(error.key, "meetings.error.busy");
    assert!(lock(&rig.trash.moved).is_empty());
    assert_eq!(rig.phase(), Phase::Idle);
}

#[test]
fn stop_without_a_recording_is_refused() {
    let rig = Rig::new();

    assert_eq!(rig.recorder.stop().unwrap_err().key, "meetings.error.busy");
}

// ---- the shortcut

#[test]
fn the_shortcut_starts_with_the_last_choices_and_stops_what_it_started() {
    let rig = Rig::new();
    rig.settings
        .update(|s| {
            s.record_mode = "audio".into();
            s.record_mic = true;
            s.record_system_audio = false;
        })
        .unwrap();

    rig.recorder.toggle_now().unwrap();
    assert_eq!(rig.phase(), Phase::Recording);
    assert_eq!(rig.fake.started(), [Track::Mic]);
    rig.record_a_moment();
    rig.recorder.toggle_now().unwrap();

    assert_eq!(rig.phase(), Phase::Idle);
    wait_until("the job to start", || !rig.backend.calls().is_empty());
}

/// A capturer whose sources do not stop until they are let go: the recording is "being put together" for as long as the test wants.
struct HoldsStop {
    inner: Arc<FakeCapturer>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}

struct HeldSource {
    inner: Box<dyn RunningSource>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl Capturer for HoldsStop {
    fn start(
        &self,
        track: Track,
        options: &RecordOptions,
        tx: mpsc::Sender<SourceEvent>,
    ) -> Result<Box<dyn RunningSource>, StartError> {
        let inner = self.inner.start(track, options, tx)?;
        Ok(Box::new(HeldSource {
            inner,
            release: self.release.clone(),
        }))
    }
}

impl RunningSource for HeldSource {
    fn stop(self: Box<Self>) {
        // Held until the test lets go (or gives up: the sender is dropped with the test).
        let _ = lock(&self.release).recv_timeout(PATIENCE);
        self.inner.stop();
    }
}

/// A recorder whose recordings are "being put together" for as long as the test does not send on the channel it gets.
fn rig_that_holds_the_stop() -> (Rig, mpsc::Sender<()>) {
    let (release, held) = mpsc::channel::<()>();
    let capturer = Arc::new(HoldsStop {
        inner: Arc::new(FakeCapturer::new().tone(Track::Mic, 440.0)),
        release: Arc::new(Mutex::new(held)),
    });
    let rig = Rig::with(FakeCapturer::new(), |deps| {
        deps.capturer = Arc::new(move |_| -> Arc<dyn Capturer> { capturer.clone() });
    });
    (rig, release)
}

#[test]
fn the_shortcut_pressed_while_a_recording_is_put_together_does_nothing() {
    let (rig, release) = rig_that_holds_the_stop();
    rig.recorder.start(audio(true, false)).unwrap();
    rig.record_a_moment();
    let recorder = rig.recorder.clone();
    let stopping = std::thread::spawn(move || recorder.stop());
    // The recording is being put together, and stays so: its source does not stop.
    wait_until("the recording to be put together", || {
        rig.phase() == Phase::Finishing
    });

    // Neither a second stop nor a new recording.
    let toggled = rig.recorder.toggle_now();

    assert!(toggled.is_ok());
    assert_eq!(rig.phase(), Phase::Finishing);
    assert_eq!(rig.folders().len(), 1, "no new recording was made");
    assert_eq!(
        rig.recorder.stop(),
        Ok(()),
        "a second stop finds it in hand"
    );
    release.send(()).unwrap();
    stopping.join().unwrap().unwrap();
    rig.wait_for_idle();
    assert_eq!(rig.folders().len(), 1);
}

/// A mock app with the meetings window made (a page that is there to hear the picker asked for), and what that window hears of it.
fn app_with_the_meetings_window() -> (tauri::App<MockRuntime>, Arc<Mutex<Vec<serde_json::Value>>>) {
    let app = mock_builder()
        .build(mock_context(noop_assets()))
        .expect("a mock app");
    crate::meetings::window::show(app.handle());
    let window = app
        .get_webview_window(crate::meetings::window::LABEL)
        .expect("the meetings window");
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    window.listen(crate::meetings::window::OPEN_RECORDER_EVENT, move |event| {
        lock(&sink).push(serde_json::from_str(event.payload()).unwrap());
    });
    (app, heard)
}

#[test]
fn the_shortcut_does_not_start_a_recording_of_a_window_that_nobody_chose() {
    // A window is not kept from one recording to the next (it may be gone): the shortcut has none to start with.
    let rig = Rig::new();
    rig.settings
        .update(|s| s.record_mode = "window".into())
        .unwrap();

    let toggled = rig.recorder.toggle_now();

    assert_eq!(toggled, Err(ToggleError::NeedsPicker));
    assert_eq!(rig.phase(), Phase::Idle);
    assert!(
        rig.folders().is_empty(),
        "no folder for a recording that cannot be"
    );
    assert!(rig.fake.started().is_empty());
    assert_eq!(rig.surface.shown.load(Ordering::SeqCst), 0);
    assert!(!rig.jobs.is_active(&Job::Recording));
}

#[test]
fn a_start_from_the_shortcut_that_fails_comes_back_as_a_start_error_for_the_picker() {
    let rig = Rig::new();
    rig.set_ffmpeg(None);
    rig.settings
        .update(|s| s.record_mode = "screen".into())
        .unwrap();

    let toggled = rig.recorder.toggle_now();

    match toggled {
        Err(ToggleError::Start(error)) => assert_eq!(error.key, "recording.error.needs_ffmpeg"),
        other => panic!("{other:?}"),
    }
    assert!(rig.folders().is_empty());
}

#[test]
fn a_stop_from_the_shortcut_that_fails_comes_back_as_a_stop_error_for_a_banner() {
    // The microphone delivers nothing: what is stopped has nothing in it.
    let rig = Rig::using(FakeCapturer::new());
    rig.settings
        .update(|s| {
            s.record_mode = "audio".into();
            s.record_mic = true;
            s.record_system_audio = false;
        })
        .unwrap();
    rig.recorder.toggle_now().unwrap();

    let toggled = rig.recorder.toggle_now();

    match toggled {
        Err(ToggleError::Stop(error)) => {
            assert_eq!(error.key, "recording.error.nothing_recorded");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(rig.phase(), Phase::Idle);
}

#[test]
fn a_start_that_fails_from_the_shortcut_opens_the_picker_with_the_error_and_shows_no_banner() {
    let rig = Rig::new();
    let (app, heard) = app_with_the_meetings_window();
    let error = views::start_error(StartError::NeedsFfmpeg);

    rig.recorder
        .toggle_failed(app.handle(), ToggleError::Start(error.clone()));

    let heard = lock(&heard).clone();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0]["error"]["key"], "recording.error.needs_ffmpeg");
    assert_eq!(heard[0]["error"]["command"], error.command.unwrap());
    assert!(rig.notices().is_empty(), "the picker says it, not a banner");
}

#[test]
fn a_shortcut_with_nothing_to_start_with_opens_the_picker_with_no_error() {
    let rig = Rig::new();
    let (app, heard) = app_with_the_meetings_window();

    rig.recorder
        .toggle_failed(app.handle(), ToggleError::NeedsPicker);

    assert_eq!(*lock(&heard), [serde_json::json!({ "error": null })]);
    assert!(rig.notices().is_empty());
}

#[test]
fn a_stop_that_fails_from_the_shortcut_is_a_banner_and_does_not_open_the_picker() {
    let rig = Rig::new();
    let (app, heard) = app_with_the_meetings_window();
    let error = views::error(
        "Other",
        "recording.error.stop_failed",
        Some("disk full".into()),
        None,
    );

    rig.recorder
        .toggle_failed(app.handle(), ToggleError::Stop(error));

    assert!(lock(&heard).is_empty());
    let notices = rig.notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(
        notices[0].kind,
        NoticeKind::Recording {
            key: "recording.error.stop_failed".into(),
            detail: "disk full".into(),
        }
    );
}

#[test]
fn the_shortcut_of_the_app_starts_and_stops_the_recorder_it_manages() {
    let rig = Rig::new();
    rig.settings
        .update(|s| {
            s.record_mode = "audio".into();
            s.record_mic = true;
            s.record_system_audio = false;
        })
        .unwrap();
    let app = mock_builder()
        .build(mock_context(noop_assets()))
        .expect("a mock app");
    app.manage(rig.recorder.clone());

    toggle(app.handle());
    wait_until("the recording", || rig.phase() == Phase::Recording);
    rig.record_a_moment();
    toggle(app.handle());

    rig.wait_for_idle();
    wait_until("the job to start", || !rig.backend.calls().is_empty());
}

#[test]
fn the_item_of_the_tray_opens_the_picker_when_nothing_is_recorded() {
    let rig = Rig::new();
    let (app, heard) = app_with_the_meetings_window();

    rig.recorder.tray_item(app.handle());

    assert_eq!(*lock(&heard), [serde_json::json!({ "error": null })]);
    assert_eq!(
        rig.phase(),
        Phase::Idle,
        "choosing is the picker's, not the item's"
    );
    assert!(rig.folders().is_empty());
}

#[test]
fn the_item_of_the_tray_stops_the_recording_that_goes_on_and_opens_the_library() {
    let rig = Rig::new();
    rig.recorder.start(audio(true, false)).unwrap();
    rig.record_a_moment();
    let app = mock_builder()
        .build(mock_context(noop_assets()))
        .expect("a mock app");
    assert!(app
        .get_webview_window(crate::meetings::window::LABEL)
        .is_none());

    rig.recorder.tray_item(app.handle());

    assert_eq!(rig.phase(), Phase::Idle);
    wait_until("the job to start", || !rig.backend.calls().is_empty());
    // As the Swift app does, the library opens to show the meeting that the recording became.
    assert!(app
        .get_webview_window(crate::meetings::window::LABEL)
        .is_some());
    assert!(rig.notices().is_empty());
}

#[test]
fn the_item_of_the_tray_tells_by_a_banner_when_the_stop_fails() {
    // The microphone delivers nothing: what is stopped has nothing in it.
    let rig = Rig::using(FakeCapturer::new());
    rig.recorder.start(audio(true, false)).unwrap();
    let (app, heard) = app_with_the_meetings_window();

    rig.recorder.tray_item(app.handle());

    assert_eq!(rig.phase(), Phase::Idle);
    assert!(lock(&heard).is_empty(), "no picker for a stop");
    let notices = rig.notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(
        notices[0].kind,
        NoticeKind::Recording {
            key: "recording.error.nothing_recorded".into(),
            detail: String::new(),
        }
    );
}

// ---- the bar

/// A bar that is slow to come up, as the first one is (its window is made): `show_hud` takes as long as the test lets it. The bar is
/// left as the last call to it made it.
struct SlowBar {
    entered: AtomicBool,
    release: Mutex<mpsc::Receiver<()>>,
    visible: AtomicBool,
}

impl Surface for SlowBar {
    fn state(&self, _: &RecordingView) {}
    fn show_hud(&self) {
        self.entered.store(true, Ordering::SeqCst);
        // Held until the test lets go (or gives up).
        let _ = lock(&self.release).recv_timeout(PATIENCE);
        self.visible.store(true, Ordering::SeqCst);
    }
    fn hide_hud(&self) {
        self.visible.store(false, Ordering::SeqCst);
    }
}

#[test]
fn a_recording_that_ends_as_the_bar_comes_up_does_not_leave_the_bar_over_an_idle_recorder() {
    let (release, held) = mpsc::channel::<()>();
    let bar = Arc::new(SlowBar {
        entered: AtomicBool::new(false),
        release: Mutex::new(held),
        visible: AtomicBool::new(false),
    });
    let rig = Rig::with(FakeCapturer::new().tone(Track::Mic, 440.0), |deps| {
        deps.surface = bar.clone();
    });
    let recorder = rig.recorder.clone();
    let starting = std::thread::spawn(move || recorder.start(audio(true, false)));
    wait_until("the bar to be asked for", || {
        bar.entered.load(Ordering::SeqCst)
    });

    // The source ends while the bar is still coming up.
    rig.fake.send_ended(Track::Mic, EndReason::DisplayGone);
    wait_until("the end to be told", || rig.phase() == Phase::Finishing);
    // Whoever puts the recording together does it now, or once the start is over: either way the bar must not be asked to hide
    // before it was asked to show, which is all the order that matters. (A recording that is put together at once is idle here.)
    let give_up = Instant::now() + Duration::from_millis(500);
    while rig.phase() != Phase::Idle && Instant::now() < give_up {
        std::thread::sleep(Duration::from_millis(10));
    }
    release.send(()).unwrap();
    starting.join().unwrap().unwrap();
    rig.wait_for_idle();
    // The recorder is idle as soon as it says so; the bar is hidden right after that.
    let give_up = Instant::now() + Duration::from_secs(1);
    while bar.visible.load(Ordering::SeqCst) && Instant::now() < give_up {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        !bar.visible.load(Ordering::SeqCst),
        "the bar was shown after the recording was over"
    );
}

// ---- quitting

#[test]
fn quitting_while_recording_finishes_the_recording_first() {
    let rig = Rig::new();
    rig.recorder.start(audio(true, false)).unwrap();
    let id = rig.meeting_id();
    rig.record_a_moment();
    let (told, heard) = mpsc::channel();
    let (library, recorder, meetings) = (
        rig.library.clone(),
        rig.recorder.clone(),
        rig.meetings.clone(),
    );

    let waits = rig.recorder.finish_before_exit(Duration::from_secs(60), {
        let id = id.clone();
        move || {
            // What the app finds when it goes: the recording is a meeting, with its job, and the recorder is free.
            let meta = Meta::read(&library.join(&id)).unwrap();
            let _ = told.send((
                meta.duration.is_some(),
                Path::new(&meta.media).is_file(),
                meetings.registry.status(&id).is_some(),
                recorder.view().phase,
            ));
        }
    });

    assert!(waits, "the exit has to wait");
    let found = heard.recv_timeout(PATIENCE).expect("the exit to go on");
    assert_eq!(found, (true, true, true, Phase::Idle));
}

#[test]
fn closing_the_last_window_leaves_the_recording_going() {
    // The app is a tray app: with no window it goes on, and so does a recording. Only a quit has to wait.
    let rig = Rig::new();
    rig.recorder.start(audio(true, false)).unwrap();

    let waits = before_exit(&rig.recorder, QUIT_WAIT, None, |_| {
        panic!("the app is not exiting")
    });

    assert!(!waits);
    assert_eq!(rig.phase(), Phase::Recording);
    rig.recorder.discard().unwrap();
}

#[test]
fn a_quit_puts_the_recording_together_and_then_exits_with_its_code() {
    let rig = Rig::new();
    rig.recorder.start(audio(true, false)).unwrap();
    let id = rig.meeting_id();
    rig.record_a_moment();
    let (told, heard) = mpsc::channel();
    let (library, recorder) = (rig.library.clone(), rig.recorder.clone());

    let waits = before_exit(&rig.recorder, QUIT_WAIT, Some(3), move |code| {
        let meta = Meta::read(&library.join(&id)).unwrap();
        let _ = told.send((code, meta.duration.is_some(), recorder.view().phase));
    });

    assert!(waits);
    assert_eq!(
        heard.recv_timeout(PATIENCE).unwrap(),
        (3, true, Phase::Idle)
    );
    // The exit that follows finds nothing to wait for.
    assert!(!before_exit(&rig.recorder, QUIT_WAIT, Some(3), |_| ()));
}

#[test]
fn quitting_with_no_recording_does_not_wait() {
    let rig = Rig::new();
    let called = Arc::new(AtomicBool::new(false));
    let seen = called.clone();

    let waits = rig
        .recorder
        .finish_before_exit(Duration::from_secs(60), move || {
            seen.store(true, Ordering::SeqCst)
        });

    assert!(!waits);
    assert!(!called.load(Ordering::SeqCst));
}

/// A capturer whose start takes `how_long` and then fails: the dialog of a desktop that waits for the person.
struct Waits(Duration);

impl Capturer for Waits {
    fn start(
        &self,
        _: Track,
        _: &RecordOptions,
        _: mpsc::Sender<SourceEvent>,
    ) -> Result<Box<dyn RunningSource>, StartError> {
        std::thread::sleep(self.0);
        Err(StartError::PortalCancelled)
    }
}

#[test]
fn quitting_gives_up_waiting_after_the_time_it_was_given() {
    // A start that does not end (the dialog of a desktop, waiting for the person): the app is not held for ever.
    let rig = Rig::with(FakeCapturer::new(), |deps| {
        deps.capturer =
            Arc::new(|_| -> Arc<dyn Capturer> { Arc::new(Waits(Duration::from_secs(3))) });
    });
    let recorder = rig.recorder.clone();
    let starting = std::thread::spawn(move || recorder.start(audio(true, false)));
    wait_until("the start to begin", || rig.phase() == Phase::Starting);
    let (told, heard) = mpsc::channel();
    let asked = Instant::now();

    let waits = rig
        .recorder
        .finish_before_exit(Duration::from_millis(200), move || {
            let _ = told.send(());
        });

    assert!(waits);
    heard
        .recv_timeout(PATIENCE)
        .expect("the exit goes on at the limit");
    assert!(
        asked.elapsed() < Duration::from_millis(2_500),
        "{:?}",
        asked.elapsed()
    );
    assert_eq!(
        starting.join().unwrap().unwrap_err().key,
        "recording.error.portal_cancelled"
    );
}

#[test]
fn a_quit_that_gave_up_waiting_lets_the_next_one_through() {
    // `exit` asks for the exit again, which comes back to `before_exit`: it must not wait another time, or the app could never end.
    let rig = Rig::with(FakeCapturer::new(), |deps| {
        deps.capturer =
            Arc::new(|_| -> Arc<dyn Capturer> { Arc::new(Waits(Duration::from_secs(3))) });
    });
    let recorder = rig.recorder.clone();
    let starting = std::thread::spawn(move || recorder.start(audio(true, false)));
    wait_until("the start to begin", || rig.phase() == Phase::Starting);
    let (told, heard) = mpsc::channel();

    assert!(before_exit(
        &rig.recorder,
        Duration::from_millis(100),
        Some(0),
        move |code| {
            let _ = told.send(code);
        }
    ));
    assert_eq!(heard.recv_timeout(PATIENCE).unwrap(), 0);

    // The start is still going on, and the exit that `exit` asked for is let through.
    assert_eq!(rig.phase(), Phase::Starting);
    assert!(!before_exit(&rig.recorder, QUIT_WAIT, Some(0), |_| {
        panic!("it must not wait again")
    }));
    starting.join().unwrap().unwrap_err();
}

#[test]
fn a_quit_that_gave_up_on_a_recording_being_put_together_lets_the_next_one_through() {
    // A long mix keeps the recording in hand for longer than a quit waits.
    let (rig, release) = rig_that_holds_the_stop();
    rig.recorder.start(audio(true, false)).unwrap();
    rig.record_a_moment();
    let (told, heard) = mpsc::channel();

    assert!(before_exit(
        &rig.recorder,
        Duration::from_millis(100),
        Some(7),
        move |code| {
            let _ = told.send(code);
        }
    ));
    assert_eq!(heard.recv_timeout(PATIENCE).unwrap(), 7);

    assert_eq!(rig.phase(), Phase::Finishing, "it is still in hand");
    assert!(!before_exit(&rig.recorder, QUIT_WAIT, Some(7), |_| {
        panic!("it must not wait again")
    }));
    // Quitting did not end the recording's own work: what is being put together is finished, if the app is still there to.
    release.send(()).unwrap();
    rig.wait_for_idle();
}

// ---- the picker

#[test]
fn what_the_system_offers_is_what_the_picker_lists() {
    let rig = Rig::with(FakeCapturer::new(), |deps| {
        deps.sources = Arc::new(|| {
            Ok(Sources {
                displays: vec![SourceInfo {
                    id: "1".into(),
                    name: "Built-in display".into(),
                    app: None,
                }],
                windows: vec![SourceInfo {
                    id: "42".into(),
                    name: "Notes".into(),
                    app: Some("Notes".into()),
                }],
                picker_is_the_system: false,
            })
        });
    });
    rig.set_ffmpeg(Some("/opt/ffmpeg"));

    let sources = rig.recorder.sources();

    assert_eq!(sources.screen, ScreenAvailability::Ready);
    assert_eq!(sources.displays.len(), 1);
    assert_eq!(sources.windows[0].id, "42");
    assert!(!sources.picker_is_the_system);
    assert_eq!(sources.last, views::options_from(&rig.settings.get()));
}

#[test]
fn an_ffmpeg_without_a_video_encoder_that_works_disables_the_screen_and_is_asked_once() {
    let rig = Rig::new();
    rig.set_ffmpeg(Some("/opt/ffmpeg"));
    rig.encoder.store(false, Ordering::SeqCst);

    assert_eq!(
        rig.recorder.sources().screen,
        ScreenAvailability::NoVideoEncoder
    );
    assert_eq!(
        rig.recorder.sources().screen,
        ScreenAvailability::NoVideoEncoder
    );
    // Looking for an encoder starts programs: it is done once for an ffmpeg, not every time the picker opens.
    assert_eq!(rig.probes.load(Ordering::SeqCst), 1);

    // Another ffmpeg is another answer.
    rig.set_ffmpeg(Some("/usr/local/bin/ffmpeg"));
    rig.encoder.store(true, Ordering::SeqCst);
    assert_eq!(rig.recorder.sources().screen, ScreenAvailability::Ready);
    assert_eq!(rig.probes.load(Ordering::SeqCst), 2);
}

#[test]
fn a_desktop_without_a_portal_and_a_missing_permission_are_told_by_the_list() {
    let portal = Rig::with(FakeCapturer::new(), |deps| {
        deps.sources = Arc::new(|| Err(StartError::PortalUnavailable));
    });
    portal.set_ffmpeg(Some("/opt/ffmpeg"));
    assert_eq!(
        portal.recorder.sources().screen,
        ScreenAvailability::PortalUnavailable
    );

    let denied = Rig::with(FakeCapturer::new(), |deps| {
        deps.sources = Arc::new(|| Err(StartError::NeedsScreenPermission));
    });
    denied.set_ffmpeg(Some("/opt/ffmpeg"));
    let sources = denied.recorder.sources();
    assert_eq!(sources.screen, ScreenAvailability::NeedsPermission);
    assert!(sources.displays.is_empty() && sources.windows.is_empty());

    // Any other reason it could not list is left for the start to say: the picker is not a dead end.
    let other = Rig::with(FakeCapturer::new(), |deps| {
        deps.sources = Arc::new(|| Err(StartError::Source("no display found".into())));
    });
    other.set_ffmpeg(Some("/opt/ffmpeg"));
    assert_eq!(other.recorder.sources().screen, ScreenAvailability::Ready);
}

#[test]
fn the_screen_permission_is_reported_and_requested_on_macos() {
    // The system is asked what it says; it is not asked to show its prompt by a look at the picker.
    let platform = FakePlatform::new(Os::Mac, Arc::new(CallLog::default()));
    *lock(&platform.screen) = Permission::Denied;
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = asked.clone();
    let rig = Rig::with(FakeCapturer::new(), |deps| {
        deps.platform = platform.clone();
        deps.sources = Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Sources::default())
        });
    });
    rig.set_ffmpeg(Some("/opt/ffmpeg"));

    assert_eq!(
        rig.recorder.sources().screen,
        ScreenAvailability::NeedsPermission
    );
    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "listing the windows would show the system's prompt"
    );
    assert_eq!(platform.screen_requests.load(Ordering::SeqCst), 0);

    *lock(&platform.screen) = Permission::Granted;
    assert_eq!(rig.recorder.sources().screen, ScreenAvailability::Ready);
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

// ---- a recording that a crash left

/// A folder of a recording that was cut off: `recording.json`, and the stem of a microphone that heard a second.
fn cut_off_recording(library: &Path, name: &str) -> PathBuf {
    let folder = library.join(name);
    fs::create_dir_all(&folder).unwrap();
    Meta {
        title: "Audio recording 10:00 10/11".into(),
        source: SCREEN_SOURCE.into(),
        media: folder.join("audio.m4a").to_string_lossy().into_owned(),
        created: 1_792_423_258.0,
        duration: None,
    }
    .write(&folder)
    .unwrap();
    Marker {
        version: 1,
        options: audio(true, false),
        codec: None,
        container: None,
        started: 1_792_423_258.0,
    }
    .write(&folder)
    .unwrap();
    let mut stem = StemWriter::create(&folder.join("mic.wav"), 48_000).unwrap();
    let tone = dictate_services::record::fakes::sine(440.0, 48_000, 0, 48_000);
    stem.push(Duration::ZERO, &tone).unwrap();
    stem.finish().unwrap();
    folder
}

#[test]
fn an_interrupted_recording_is_recovered_at_start_up() {
    let (rig, job) = Rig::gated(FakeCapturer::new());
    let folder = cut_off_recording(&rig.library, "2026-11-10 10-00-00");

    let recovered = recover_library(&rig.meetings, None);

    assert_eq!(recovered, 1);
    // Its job is held at the audio, so that what recovery wrote is read before the pipeline writes its own length.
    wait_until("the job to start", || !rig.backend.calls().is_empty());
    let meta = rig.meta("2026-11-10 10-00-00");
    assert!(
        meta.duration.is_some_and(|seconds| seconds >= 1.0),
        "{:?}",
        meta.duration
    );
    assert!(Path::new(&meta.media).is_file());
    assert!(Path::new(&meta.media).starts_with(&folder));
    assert_eq!(meta.source, SCREEN_SOURCE);
    // The recording's own files are put together, and the job is started (it waits for the start-up to end).
    assert!(!folder.join(MARKER).exists());
    assert!(!folder.join("mic.wav").exists());
    assert_eq!(rig.backend.media_seen(), [PathBuf::from(&meta.media)]);
    // And the log says why the recording is shorter than it was meant to be.
    let log = fs::read_to_string(folder.join(files::LOG)).unwrap();
    assert!(
        log.contains("The recording was interrupted; what was recorded is kept."),
        "{log}"
    );
    let _ = job.release.send(());
}

#[test]
fn a_folder_that_is_not_a_recording_is_left_alone_at_start_up() {
    let rig = Rig::new();
    let folder = rig.library.join("2026-11-09 09-00-00");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("transcript.txt"), "[00:00] Xin chào").unwrap();

    assert_eq!(recover_library(&rig.meetings, None), 0);

    assert!(folder.join("transcript.txt").is_file());
    assert!(rig.backend.calls().is_empty());
}

#[test]
fn an_interrupted_recording_with_nothing_in_it_is_removed_at_start_up() {
    let rig = Rig::new();
    let folder = rig.library.join("2026-11-10 10-00-00");
    fs::create_dir_all(&folder).unwrap();
    Meta {
        title: "Audio recording 10:00 10/11".into(),
        source: SCREEN_SOURCE.into(),
        media: String::new(),
        created: 1_792_423_258.0,
        duration: None,
    }
    .write(&folder)
    .unwrap();
    Marker {
        version: 1,
        options: audio(true, false),
        codec: None,
        container: None,
        started: 1_792_423_258.0,
    }
    .write(&folder)
    .unwrap();

    assert_eq!(recover_library(&rig.meetings, None), 0);

    assert!(!folder.exists(), "an empty recording is not a meeting");
    assert!(rig.backend.calls().is_empty());
}

#[test]
fn a_recording_that_is_going_on_is_not_finished_by_the_recovery_of_the_start_up() {
    // The recovery runs on a thread of its own, as long as the biggest mix takes, and a recording may be started meanwhile (a
    // click, the shortcut, `--record` at launch): its marker is in a folder of the library like the one of a recording that was cut off.
    let rig = Rig::new();
    rig.recorder.start(audio(true, false)).unwrap();
    let id = rig.meeting_id();
    let folder = rig.library.join(&id);
    assert!(folder.join(MARKER).is_file());
    rig.record_a_moment();

    assert_eq!(recover_library(&rig.meetings, None), 0);

    // It goes on as it was: its marker and its stem are there, and it is put together by its own stop.
    assert_eq!(rig.phase(), Phase::Recording);
    assert!(folder.join(MARKER).is_file());
    assert!(folder.join("mic.wav").is_file());
    assert!(rig.backend.calls().is_empty(), "no job was started on it");
    rig.recorder.stop().unwrap();
    let meta = rig.meta(&id);
    assert!(meta.duration.is_some_and(|seconds| seconds > 0.0));
    assert!(Path::new(&meta.media).is_file());
    wait_until("the job to start", || !rig.backend.calls().is_empty());
}

#[test]
fn the_recovery_does_not_hold_the_place_of_a_recording_it_has_finished() {
    // The job of the recovered recording needs the place that the recovery held while it worked.
    let rig = Rig::new();
    let folder = cut_off_recording(&rig.library, "2026-11-10 10-00-00");

    assert_eq!(recover_library(&rig.meetings, None), 1);

    assert!(rig
        .meetings
        .registry
        .status("2026-11-10 10-00-00")
        .is_some());
    assert!(folder.join(files::META).is_file());
    wait_until("the job to start", || !rig.backend.calls().is_empty());
}

/// A stem of a second of tone in `folder`.
fn write_stem(folder: &Path, name: &str) {
    let mut stem = StemWriter::create(&folder.join(name), 48_000).unwrap();
    let tone = dictate_services::record::fakes::sine(440.0, 48_000, 0, 48_000);
    stem.push(Duration::ZERO, &tone).unwrap();
    stem.finish().unwrap();
}

#[test]
fn a_stem_that_cannot_be_read_does_not_cost_the_recording_the_stem_that_can() {
    // The stem of a microphone that a crash cut inside its header, next to a system stem that is fine: the good one is not
    // lost with it, at this start or at any later one, and the one that cannot be read is kept as it is.
    let rig = Rig::new();
    let folder = cut_off_recording(&rig.library, "2026-11-10 10-00-00");
    write_stem(&folder, "system.wav");
    fs::write(folder.join("mic.wav"), b"this is not a stem").unwrap();

    assert_eq!(recover_library(&rig.meetings, None), 1);

    assert!(
        !folder.join(MARKER).exists(),
        "nothing is left to try again"
    );
    assert_eq!(
        fs::read(folder.join("mic.wav.unreadable")).unwrap(),
        b"this is not a stem"
    );
    assert!(
        !folder.join("system.wav").exists(),
        "the good one was mixed"
    );
    let meta = rig.meta("2026-11-10 10-00-00");
    assert!(Path::new(&meta.media).is_file());
    assert!(meta.duration.is_some_and(|seconds| seconds >= 1.0));
    let log = fs::read_to_string(folder.join(files::LOG)).unwrap();
    assert!(log.contains("mic.wav could not be read"), "{log}");
    wait_until("the job to start", || !rig.backend.calls().is_empty());
}

#[test]
fn stems_that_cannot_be_read_are_all_there_is_and_the_folder_goes_to_the_trash_not_to_nothing() {
    let rig = Rig::new();
    let folder = cut_off_recording(&rig.library, "2026-11-10 10-00-00");
    fs::write(folder.join("mic.wav"), b"").unwrap();

    assert_eq!(recover_library(&rig.meetings, None), 0);

    // It is nothing that could be put together, and so no meeting; but what is in the folder is moved and not deleted.
    assert_eq!(*lock(&rig.trash.moved), [folder]);
    assert!(rig.backend.calls().is_empty(), "no job without a recording");
}

#[cfg(unix)]
#[test]
fn a_video_that_ffmpeg_cannot_read_is_not_removed_by_the_recovery_of_the_start_up() {
    // A long recording on a slow disk: ffmpeg is not given the time to read its video, and there is no sound to go with it
    // (a screen with both sounds off). It is a recording all the same, and what it has is kept.
    use std::os::unix::fs::PermissionsExt;
    let rig = Rig::new();
    let folder = cut_off_recording(&rig.library, "2026-11-10 10-00-00");
    fs::remove_file(folder.join("mic.wav")).unwrap();
    fs::write(folder.join("video.mp4"), vec![7_u8; 4096]).unwrap();
    let tools = tempfile::tempdir().unwrap();
    let ffmpeg = tools.path().join("ffmpeg");
    fs::write(&ffmpeg, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&ffmpeg, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(recover_library(&rig.meetings, Some(&ffmpeg)), 1);

    assert!(folder.join("video.mp4").is_file(), "the video is kept");
    assert_eq!(
        rig.meta("2026-11-10 10-00-00").media,
        folder.join("video.mp4").to_string_lossy()
    );
    assert!(!folder.join(MARKER).exists());
    assert!(lock(&rig.trash.moved).is_empty());
    wait_until("the job to start", || !rig.backend.calls().is_empty());
}

#[test]
fn only_a_folder_that_lost_its_marker_has_nothing_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "nothing was recorded");
    let other = || io::Error::other("the disk is full");

    // The marker is removed by the one case of "nothing was recorded": any other error leaves it for another try.
    fs::write(dir.path().join(MARKER), b"{}").unwrap();
    assert!(!nothing_was_recorded(&invalid(), dir.path()));
    assert!(!nothing_was_recorded(&other(), dir.path()));
    fs::remove_file(dir.path().join(MARKER)).unwrap();
    assert!(nothing_was_recorded(&invalid(), dir.path()));
    assert!(!nothing_was_recorded(&other(), dir.path()));
}

// ---- the restore token of the portal

#[test]
fn saving_the_token_does_not_wait_for_the_disk() {
    // The portal asks on its own thread, which is shared by every other call to the portal: neither answer may wait for a
    // write of the settings.
    let (written, writes) = mpsc::channel::<String>();
    let (open, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let tokens = token::AsyncTokens::new(Some("display:old".into()), move |token| {
        let _ = gate.lock().unwrap().recv();
        let _ = written.send(token.to_owned());
    });
    use dictate_services::record::capture::TokenStore;

    assert_eq!(tokens.load().as_deref(), Some("display:old"));
    tokens.save("display:new");

    assert_eq!(
        tokens.load().as_deref(),
        Some("display:new"),
        "what was saved is read at once"
    );
    assert!(
        writes.try_recv().is_err(),
        "the write is waiting for the disk"
    );
    open.send(()).unwrap();
    assert_eq!(writes.recv_timeout(PATIENCE).unwrap(), "display:new");
}

#[test]
fn of_tokens_that_come_while_one_is_being_written_the_last_is_what_is_kept() {
    let (began, begun) = mpsc::channel::<String>();
    let (written, writes) = mpsc::channel::<String>();
    let (open, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let tokens = token::AsyncTokens::new(None, move |token| {
        let _ = began.send(token.to_owned());
        let _ = gate.lock().unwrap().recv();
        let _ = written.send(token.to_owned());
    });
    use dictate_services::record::capture::TokenStore;

    tokens.save("display:one");
    // The writer has the first, and is held at the gate.
    assert_eq!(begun.recv_timeout(PATIENCE).unwrap(), "display:one");
    tokens.save("display:two");
    tokens.save("display:three");
    open.send(()).unwrap();
    open.send(()).unwrap();

    assert_eq!(writes.recv_timeout(PATIENCE).unwrap(), "display:one");
    assert_eq!(writes.recv_timeout(PATIENCE).unwrap(), "display:three");
    assert!(writes.recv_timeout(Duration::from_millis(100)).is_err());
}

#[test]
fn the_token_is_written_to_the_settings_and_read_back_from_them_at_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let (store, _) = SettingsStore::open(path.clone());
    let store = Arc::new(store);
    use dictate_services::record::capture::TokenStore;
    let tokens = token::for_settings(store.clone());
    assert_eq!(tokens.load(), None);

    tokens.save("window:abc");

    wait_until("the settings to have the token", || {
        store.get().screencast_restore_token.as_deref() == Some("window:abc")
    });
    let (again, _) = SettingsStore::open(path);
    assert_eq!(
        token::for_settings(Arc::new(again)).load().as_deref(),
        Some("window:abc")
    );
}

// ---- what the windows are told, and when

#[test]
fn a_state_that_only_goes_on_is_told_at_most_every_50_ms_and_a_change_at_once() {
    let started = Instant::now();
    let mut shown = Shown {
        view: RecordingView::idle(),
        last_told: None,
    };
    let recording = |elapsed_ms| RecordingView {
        phase: Phase::Recording,
        elapsed_ms,
        ..RecordingView::idle()
    };
    let at = |ms| started + Duration::from_millis(ms);

    // The first is told (it is a new phase), and the time that goes on is told once in 50 ms.
    assert!(shown.change(recording(0), true, at(0)));
    assert!(!shown.change(recording(20), true, at(20)));
    assert!(!shown.change(recording(49), true, at(49)));
    assert!(shown.change(recording(50), true, at(50)));
    assert!(!shown.change(recording(70), true, at(70)));
    // What is not told is kept: the next one to be told has the time as it is then.
    assert_eq!(shown.view.elapsed_ms, 70);
    assert!(shown.change(recording(100), true, at(100)));

    // A change of the phase, or a note, is told at once however soon it comes.
    let paused = RecordingView {
        phase: Phase::Paused,
        ..recording(101)
    };
    assert!(shown.change(paused, true, at(101)));
    let noted = RecordingView {
        phase: Phase::Paused,
        notes: vec![dictate_services::record::session::TrackNote {
            track: Track::Mic,
            note: Note::Silent,
        }],
        ..recording(102)
    };
    assert!(shown.change(noted, true, at(102)));
    // And what is not throttled (the phases the recorder sets itself) is told.
    assert!(shown.change(recording(103), false, at(103)));
}

#[test]
fn the_state_is_told_to_the_meetings_window_and_to_the_bar_and_to_no_other() {
    use tauri::test::{mock_builder, mock_context, noop_assets};
    use tauri::{Listener, WebviewUrl, WebviewWindowBuilder};

    let app = mock_builder().build(mock_context(noop_assets())).unwrap();
    let heard: Arc<Mutex<Vec<(&'static str, String)>>> = Arc::default();
    for label in ["meetings", "recorder", "settings"] {
        let window = WebviewWindowBuilder::new(&app, label, WebviewUrl::default())
            .build()
            .unwrap();
        let sink = heard.clone();
        window.listen(STATE_EVENT, move |event| {
            lock(&sink).push((label, event.payload().to_owned()));
        });
    }

    AppSurface(app.handle().clone()).state(&RecordingView::idle());

    // The listeners of the mock runtime are called as the event goes out; the wait is for a runtime that would call them later.
    wait_until("both windows to hear it", || lock(&heard).len() >= 2);
    let mut told: Vec<&str> = lock(&heard).iter().map(|(label, _)| *label).collect();
    told.sort_unstable();
    assert_eq!(told, ["meetings", "recorder"]);
    let payload = lock(&heard)[0].1.clone();
    assert!(payload.contains(r#""phase":"idle""#), "{payload}");
    assert!(payload.contains(r#""elapsedMs":0"#), "{payload}");
}

#[test]
fn a_bar_that_is_not_made_yet_is_no_error_for_the_state() {
    // The state goes to the bar's window by its label, whether it is there or not.
    use tauri::test::{mock_builder, mock_context, noop_assets};
    let app = mock_builder().build(mock_context(noop_assets())).unwrap();

    use tauri::Emitter;
    assert!(app
        .handle()
        .emit_to(hud::LABEL, STATE_EVENT, RecordingView::idle())
        .is_ok());
}

#[test]
fn a_recording_that_ends_by_itself_with_nothing_in_it_is_told_so_and_leaves_nothing() {
    let rig = Rig::using(FakeCapturer::new());

    rig.recorder.start(audio(true, false)).unwrap();
    rig.fake.send_ended(Track::Mic, EndReason::WindowClosed);
    wait_until("the banner", || !rig.notices().is_empty());
    rig.wait_for_idle();

    assert_eq!(
        rig.notices()[0].kind,
        NoticeKind::Recording {
            key: "recording.error.nothing_recorded".into(),
            detail: String::new(),
        }
    );
    assert!(rig.folders().is_empty());
    assert!(rig.backend.calls().is_empty());
}
