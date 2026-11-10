//! The session against scripted sources (`record::fakes`) and the real ffmpeg: what it records, what it keeps when
//! a source ends or the program stops, and what it leaves behind.

use super::*;
use crate::media::decode::MonoDecoder;
#[cfg(unix)]
use crate::media::test_support::script;
use crate::media::test_support::{read_all, real_ffmpeg};
use crate::record::fakes::{sine, FakeCapturer};
use crate::record::marker::{Marker, MARKER};
use crate::record::quality::Quality;
use crate::record::stem::StemWriter;
use crate::record::test_support::{cut_off_video, video_seconds};
use crate::record::video::Frame;
use crate::record::{EndReason, Mode, RecordOptions, StartError, Track};
use std::sync::Mutex;
use std::time::Instant;

fn screen() -> Mode {
    Mode::Screen { display: None }
}

fn options(mode: Mode, mic: bool, system_audio: bool) -> RecordOptions {
    RecordOptions {
        mode,
        quality: Quality::Standard,
        mic,
        system_audio,
    }
}

/// What a session reported, and when.
type States = Arc<Mutex<Vec<(Instant, SessionState)>>>;

fn start_with(
    options: RecordOptions,
    folder: &Path,
    fake: &Arc<FakeCapturer>,
    ffmpeg: Option<&Path>,
    timing: Timing,
) -> Result<(Session, States), StartError> {
    let states: States = Arc::default();
    let sink = states.clone();
    let session = Session::start_with(
        options,
        folder,
        fake.clone(),
        ffmpeg.map(Path::to_path_buf),
        Box::new(move |state| sink.lock().unwrap().push((Instant::now(), state))),
        timing,
    )?;
    Ok((session, states))
}

fn start(
    options: RecordOptions,
    folder: &Path,
    fake: &Arc<FakeCapturer>,
    ffmpeg: Option<&Path>,
) -> Result<(Session, States), StartError> {
    start_with(options, folder, fake, ffmpeg, Timing::default())
}

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn name_of(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

fn sleep(milliseconds: u64) {
    std::thread::sleep(Duration::from_millis(milliseconds));
}

/// Waits until `done` is true, at most 20 s.
fn wait_until(what: &str, done: impl Fn() -> bool) {
    let give_up = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < give_up, "waited for {what}");
        sleep(10);
    }
}

fn has_ended(states: &States) -> bool {
    states
        .lock()
        .unwrap()
        .iter()
        .any(|(_, state)| matches!(state, SessionState::Ended(_)))
}

/// How far this machine's scheduler is from the clock right now: how much later than asked the sleeps of a test have
/// ended, the worst of them. A test cannot tell the moment it asked for something from the moment it was done (it may be
/// put aside between two statements), so what it asserts about time is bounded by this and not by a figure that suits
/// an idle machine: on a busy one the figure goes up with the load, which is what makes a loaded runner no reason for
/// a test to fail.
struct Slack(std::cell::Cell<Duration>);

impl Slack {
    /// The slack as it is now: a few short sleeps.
    fn measure() -> Slack {
        let slack = Slack(std::cell::Cell::new(Duration::ZERO));
        for _ in 0..10 {
            slack.sleep(5);
        }
        slack
    }

    /// Sleeps `milliseconds` and notes how late it ended.
    fn sleep(&self, milliseconds: u64) {
        let asked = Duration::from_millis(milliseconds);
        let begun = Instant::now();
        std::thread::sleep(asked);
        let late = begun.elapsed().saturating_sub(asked);
        self.0.set(self.0.get().max(late));
    }

    /// What a statement of the test may be off by, in seconds: 30 ms, and three times the worst sleep that was late.
    fn tolerance(&self) -> f64 {
        0.03 + 3.0 * self.0.get().as_secs_f64()
    }
}

/// How the test's `Instant`s and the session's time go together: the first report of the session says how much of
/// the recording had gone by when the test saw it, and from then on the recording runs as the test's clock does
/// (less the pauses, which the test counts).
struct Reference {
    seen: Instant,
    active: f64,
}

impl Reference {
    /// Waits for the first report.
    fn of(states: &States) -> Reference {
        wait_until("the first report", || !states.lock().unwrap().is_empty());
        let reports = states.lock().unwrap();
        let (seen, SessionState::Recording { active, .. }) = &reports[0] else {
            panic!("the first report is not of a recording: {:?}", reports[0].1)
        };
        Reference {
            seen: *seen,
            active: active.as_secs_f64(),
        }
    }

    /// How much of the recording would have gone by at `at`, in seconds, had it not been paused.
    fn active_at(&self, at: Instant) -> f64 {
        if at >= self.seen {
            self.active + (at - self.seen).as_secs_f64()
        } else {
            self.active - (self.seen - at).as_secs_f64()
        }
    }
}

/// How long one buffer of a live tone is: a stem can be this much longer than the recording, by the buffer that was
/// being taken as the recording ended.
const CHUNK_SECONDS: f64 = 0.02;

/// How long a frame of the Standard quality is. The video of a recording is a whole number of frames, and the sound is made
/// as long as the video: a recording with video can be this much longer than the clock says.
const FRAME_SECONDS: f64 = 1.0 / 15.0;

/// The instants just before and just after a call: what it did was done between them.
fn bracket<R>(call: impl FnOnce() -> R) -> (Instant, Instant) {
    let before = Instant::now();
    call();
    (before, Instant::now())
}

/// That a recording lasted as long as it ran: `lo` to `hi` seconds by the clock (the bounds of when it was stopped, from the
/// test's own instants), and no more than `longer_by` over (the buffer that was being taken as it ended, or the frame that
/// the video is rounded up to). What the machine did to the time between the test's statements is `slack`.
fn assert_lasted(duration: f64, lo: f64, hi: f64, longer_by: f64, slack: &Slack) {
    let tolerance = slack.tolerance();
    assert!(
        duration >= lo - tolerance && duration <= hi + tolerance + longer_by,
        "the recording was {duration} s, and ran {lo} s to {hi} s (to within {tolerance} s, and {longer_by} s over)"
    );
}

/// That the sound of `media` is as long as the recording said it was. The AAC of an m4a has priming at the start (2 112
/// samples with the encoder of macOS, 1 024 with ffmpeg's) and the last frame is filled to its end (up to 1 024 samples
/// more): 66 ms at the most at 48 kHz, so that is no more than 0.1 s off.
fn assert_decodes_as_long(media: &Path, ffmpeg: &Path, duration: f64) {
    let decoded = audio_seconds(media, ffmpeg);
    assert!(
        (decoded - duration).abs() < 0.1,
        "{decoded} s of sound for a recording of {duration} s"
    );
}

/// That the video of `media` is as long as the recording said it was. The ticker gives every frame that is due when the
/// clock stops, so the video is up to a frame (1/15 s) longer, and the last time of the decoder is a frame one way or the
/// other.
fn assert_video_as_long(media: &Path, ffmpeg: &Path, duration: f64) {
    let video = video_seconds(media, ffmpeg);
    assert!(
        (video - duration).abs() < 0.2,
        "{video} s of video for a recording of {duration} s"
    );
}

/// The levels that were reported of `track` so far, in order.
fn levels_of(states: &States, track: Track) -> Vec<Option<f32>> {
    states
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, state)| match state {
            SessionState::Recording { levels, .. } | SessionState::Paused { levels, .. } => {
                Some(if track == Track::Mic {
                    levels.mic
                } else {
                    levels.system
                })
            }
            SessionState::Ended(_) => None,
        })
        .collect()
}

/// Waits until the session has taken the buffers that were sent to it before this call, which it says by the meter: a
/// buffer of a level of its own (constant 0.01: a third of the meter) is sent as `track` and waited for. What is taken
/// is taken in order, so the ones before it are on their way to the stem by then.
fn settle(fake: &FakeCapturer, states: &States, track: Track, at: Instant) {
    fake.send_audio(track, at, 48_000, vec![0.01; 480]);
    wait_until("the session to take what was sent", || {
        levels_of(states, track)
            .iter()
            .any(|level| level.is_some_and(|level| (0.30..0.37).contains(&level)))
    });
}

fn audio_of(path: &Path, ffmpeg: &Path) -> (Vec<f32>, u32) {
    let mut decoder = MonoDecoder::open(path, Some(ffmpeg)).unwrap();
    let samples = read_all(&mut decoder);
    (samples, decoder.rate())
}

fn audio_seconds(path: &Path, ffmpeg: &Path) -> f64 {
    let (samples, rate) = audio_of(path, ffmpeg);
    samples.len() as f64 / f64::from(rate)
}

/// The RMS of the sound from `from` to `to` seconds.
fn rms(samples: &[f32], rate: u32, from: f64, to: f64) -> f32 {
    let part = &samples[(from * f64::from(rate)) as usize..(to * f64::from(rate)) as usize];
    (part.iter().map(|s| s * s).sum::<f32>() / part.len() as f32).sqrt()
}

fn is_sound_file(name: &str) -> bool {
    name == "audio.m4a" || name == "recording.wav"
}

fn is_recording(name: &str) -> bool {
    name == "recording.mp4" || name == "recording.webm"
}

// --- Refusals ---

#[test]
fn audio_only_with_both_toggles_off_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new());

    let result = start(options(Mode::Audio, false, false), dir.path(), &fake, None);

    assert_eq!(result.err(), Some(StartError::NothingToRecord));
    assert!(fake.started().is_empty());
    assert!(names(dir.path()).is_empty());
}

#[test]
fn a_screen_mode_without_ffmpeg_is_refused() {
    for mode in [screen(), Mode::Window { id: "7".into() }] {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeCapturer::new());

        let result = start(options(mode, true, true), dir.path(), &fake, None);

        assert_eq!(result.err(), Some(StartError::NeedsFfmpeg));
        assert!(fake.started().is_empty());
        assert!(names(dir.path()).is_empty());
    }
}

#[cfg(unix)]
#[test]
fn a_screen_mode_with_an_ffmpeg_that_has_no_encoder_is_refused() {
    let tools = tempfile::tempdir().unwrap();
    // An ffmpeg that lists no encoders at all.
    let ffmpeg = script(tools.path(), "fake-ffmpeg", "exit 0", "");
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new());

    let result = start(
        options(screen(), true, true),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    );

    assert_eq!(result.err(), Some(StartError::NoVideoEncoder));
    assert!(fake.started().is_empty());
    assert!(names(dir.path()).is_empty());
}

#[test]
fn a_track_that_fails_to_start_stops_the_others_and_leaves_nothing() {
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(
        FakeCapturer::new()
            .frames(320, 180, [1, 2, 3, 255], 15)
            .tone(Track::System, 440.0)
            .fails(Track::Mic, StartError::Source("no microphone".into())),
    );

    let result = start(
        options(screen(), true, true),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    );

    assert_eq!(
        result.err(),
        Some(StartError::Source("no microphone".into()))
    );
    assert_eq!(fake.started(), [Track::Screen, Track::System]);
    let mut stopped = fake.stopped();
    stopped.sort_by_key(|track| format!("{track:?}"));
    assert_eq!(stopped, [Track::Screen, Track::System]);
    // The caller removes the folder; what the session made in it is gone.
    assert_eq!(names(dir.path()), Vec::<String>::new());
}

#[test]
fn the_tracks_start_screen_then_system_then_mic_and_start_returns_when_all_have() {
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new());
    let wanted = options(screen(), true, true);

    let (session, _states) = start(wanted.clone(), dir.path(), &fake, Some(&ffmpeg)).unwrap();

    assert_eq!(fake.started(), [Track::Screen, Track::System, Track::Mic]);
    // The marker was written before any of them, and says what was asked for and what encodes it.
    let marker = Marker::read(dir.path()).unwrap().expect("a marker");
    assert_eq!(marker.version, 1);
    assert_eq!(marker.options, wanted);
    assert!(marker.codec.is_some());
    assert!(matches!(marker.container.as_deref(), Some("mp4" | "webm")));
    assert!(marker.started > 1_700_000_000.0);
    session.discard();
}

#[test]
fn only_the_tracks_that_are_asked_for_are_started() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new());

    let (session, _states) =
        start(options(Mode::Audio, false, true), dir.path(), &fake, None).unwrap();

    assert_eq!(fake.started(), [Track::System]);
    session.discard();
}

// --- Audio ---

#[test]
fn audio_only_makes_an_m4a_of_the_active_time() {
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(
        FakeCapturer::new()
            .tone(Track::Mic, 440.0)
            .tone(Track::System, 880.0),
    );
    let (session, states) = start(
        options(Mode::Audio, true, true),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();
    let reference = Reference::of(&states);
    let slack = Slack::measure();

    slack.sleep(2000);
    let asked = Instant::now();
    let finished = session.stop().unwrap();

    let name = name_of(&finished.media);
    assert!(is_sound_file(&name), "{name}");
    assert_eq!(finished.media.parent(), Some(dir.path()));
    // It is as long as it ran, which is no less than the 2 s that were slept (a sleep is never short), and as much
    // more as the machine took in getting back to the test.
    let ran = reference.active_at(asked);
    assert!(ran >= 2.0 - slack.tolerance(), "{ran} s ran");
    assert_lasted(finished.duration, ran, ran, CHUNK_SECONDS, &slack);
    assert_decodes_as_long(&finished.media, &ffmpeg, finished.duration);
    assert!(finished.notes.is_empty(), "{:?}", finished.notes);
    // Nothing is left but the sound: no stems, no marker.
    assert_eq!(names(dir.path()), [name]);
    assert_eq!(fake.stopped().len(), 2);
}

#[test]
fn the_recording_ends_when_stop_is_called_and_not_when_the_session_gets_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new().tone(Track::Mic, 440.0));
    let states: States = Arc::default();
    let sink = states.clone();
    // The session's thread is held in the report of its own state (a slow interface does that) until the test lets it go.
    let (held, is_held) = std::sync::mpsc::channel();
    let (go, held_until) = std::sync::mpsc::channel::<()>();
    let held_until = Mutex::new(held_until);
    let on_state = move |state| {
        let mut reports = sink.lock().unwrap();
        reports.push((Instant::now(), state));
        // About 1.5 s into the recording: past the second that a recording is made to be at least.
        if reports.len() == 30 {
            drop(reports);
            let _ = held.send(());
            let _ = held_until.lock().unwrap().recv();
        }
    };
    let session = Session::start_with(
        options(Mode::Audio, true, false),
        dir.path(),
        fake.clone(),
        None,
        Box::new(on_state),
        Timing::default(),
    )
    .map_err(|e| format!("{e:?}"))
    .unwrap();
    let reference = Reference::of(&states);
    is_held.recv_timeout(Duration::from_secs(20)).expect("held");
    let slack = Slack::measure();

    let stopping = std::thread::spawn(move || {
        let asked = Instant::now();
        (asked, session.stop())
    });
    // The recording is stopped, and the session's thread does not hear of it for most of a second.
    slack.sleep(700);
    go.send(()).unwrap();
    let (asked, finished) = stopping.join().unwrap();
    let finished = finished.unwrap();

    // What was recorded is what there was when stop was called, and not the 0.7 s more of the sources that went on
    // delivering while the session's thread was not looking.
    let active = reference.active_at(asked);
    assert!(
        finished.duration >= active - slack.tolerance(),
        "{} s for {active} s",
        finished.duration
    );
    assert!(
        finished.duration <= active + slack.tolerance() + CHUNK_SECONDS,
        "{} s for {active} s: the time it took the session to hear of the stop was recorded",
        finished.duration
    );
}

#[test]
fn system_audio_that_stops_and_starts_stays_in_place() {
    let Some(ffmpeg) = real_ffmpeg() else { return };

    // The system plays a second, nothing for two, and a second again; the mic goes on for the four. It is all sent at
    // once, stamped with the times it would have come at, and the session is let take it before it is stopped: what
    // is recorded is then what was sent, whatever the machine is doing.
    let record = |with_system: bool| -> (Vec<f32>, u32, f64) {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeCapturer::new());
        let (session, states) = start(
            options(Mode::Audio, true, true),
            dir.path(),
            &fake,
            Some(&ffmpeg),
        )
        .unwrap();
        let base = Instant::now();
        for k in 0..200u32 {
            let at = base + Duration::from_millis(20) * k;
            let first = k as usize * 960;
            fake.send_audio(Track::Mic, at, 48_000, sine(440.0, 48_000, first, 960));
            if with_system && !(50..150).contains(&k) {
                fake.send_audio(Track::System, at, 48_000, sine(880.0, 48_000, first, 960));
            }
        }
        // Ten milliseconds more of the mic, to know when it is all taken: 4.01 s in all.
        settle(&fake, &states, Track::Mic, base + Duration::from_secs(4));
        let finished = session.stop().unwrap();
        let (samples, rate) = audio_of(&finished.media, &ffmpeg);
        (samples, rate, finished.duration)
    };

    let (with, rate, duration) = record(true);
    let (without, _, _) = record(false);

    // The stems are laid out by the stamps of what was sent, and the clock is not in it: the length is that of the longest.
    assert!((duration - 4.01).abs() < 0.001, "{duration} s");
    assert!((with.len() as f64 / f64::from(rate) - duration).abs() < 0.1);
    // The system's tone is there at 3.5 s (the mic alone is 0.35 RMS, the two together 0.5), and not at 2 s.
    let (late_with, late_without) = (rms(&with, rate, 3.2, 3.8), rms(&without, rate, 3.2, 3.8));
    assert!(
        late_with > late_without * 1.25,
        "{late_with} vs {late_without}"
    );
    let (mid_with, mid_without) = (rms(&with, rate, 1.5, 2.5), rms(&without, rate, 1.5, 2.5));
    assert!(mid_with < mid_without * 1.1, "{mid_with} vs {mid_without}");
}

#[test]
fn levels_are_reported_for_enabled_tracks_only() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new().tone(Track::System, 440.0));
    let (session, states) =
        start(options(Mode::Audio, false, true), dir.path(), &fake, None).unwrap();

    // A tone at half of full scale is loud on the meter.
    wait_until("the meter to show the tone", || {
        levels_of(&states, Track::System)
            .iter()
            .any(|l| l.is_some_and(|s| s > 0.9))
    });
    session.discard();

    // Whatever was reported, the microphone, which is off, has no level in any of it.
    let mic = levels_of(&states, Track::Mic);
    assert!(mic.len() >= 2, "{} reports", mic.len());
    assert!(mic.iter().all(Option::is_none), "{mic:?}");
    // And the system, which is on, always has one.
    assert!(levels_of(&states, Track::System)
        .iter()
        .all(Option::is_some));
}

#[test]
fn a_meter_whose_source_went_quiet_falls_back_to_zero() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new());
    let (session, states) =
        start(options(Mode::Audio, true, false), dir.path(), &fake, None).unwrap();
    fake.send_audio(
        Track::Mic,
        Instant::now(),
        48_000,
        sine(440.0, 48_000, 0, 960),
    );

    // A source that delivers nothing for a while (a system that plays no sound) is not still loud: the meter shows
    // the buffer, and some time later (half a second) it shows nothing.
    wait_until("the meter to fall back", || {
        let mic = levels_of(&states, Track::Mic);
        let loud = mic.iter().position(|l| l.is_some_and(|l| l > 0.9));
        loud.is_some_and(|at| mic[at..].contains(&Some(0.0)))
    });
    session.discard();
}

#[test]
fn the_state_is_reported_at_most_every_50_ms() {
    let dir = tempfile::tempdir().unwrap();
    // A chunk of sound every 20 ms: a report for each would be 50 a second.
    let fake = Arc::new(FakeCapturer::new().tone(Track::Mic, 440.0));
    let (session, states) =
        start(options(Mode::Audio, true, false), dir.path(), &fake, None).unwrap();

    // It is reported, and often (a second of 20 chunks of 50 ms is what it comes to on an idle machine, eight of
    // them is as much as is asked)...
    wait_until("eight reports", || states.lock().unwrap().len() >= 8);
    session.discard();

    // ...but no two of them are less than 50 ms apart (a sleep is never short, so 45 ms is only for the clock).
    let times: Vec<Instant> = states.lock().unwrap().iter().map(|(at, _)| *at).collect();
    for pair in times.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap >= Duration::from_millis(45),
            "{gap:?} between two reports"
        );
    }
}

#[test]
fn a_recording_stopped_at_once_is_a_second_of_silence() {
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new());
    let (session, states) = start(
        options(Mode::Audio, true, false),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();
    let reference = Reference::of(&states);
    let slack = Slack::measure();
    // The one buffer that was captured before the stop: 10 ms, which the session is let take.
    fake.send_audio(
        Track::Mic,
        Instant::now(),
        48_000,
        sine(440.0, 48_000, 0, 480),
    );
    wait_until("the buffer to be taken", || {
        levels_of(&states, Track::Mic)
            .iter()
            .any(|l| l.is_some_and(|l| l > 0.9))
    });

    let asked = Instant::now();
    let finished = session.stop().unwrap();

    // A second, unless the machine took longer than that to get to the stop (then it is as long as it ran).
    let ran = reference.active_at(asked);
    assert_lasted(
        finished.duration,
        ran.max(1.0),
        ran.max(1.0),
        CHUNK_SECONDS,
        &slack,
    );
    assert_decodes_as_long(&finished.media, &ffmpeg, finished.duration);
    if ran + slack.tolerance() < 1.0 {
        assert!(
            finished.notes.iter().any(|note| note.contains("silence")),
            "{:?}",
            finished.notes
        );
    }
}

#[test]
fn a_source_that_went_quiet_does_not_make_the_recording_shorter() {
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new());
    let (session, states) = start(
        options(Mode::Audio, false, true),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();
    let reference = Reference::of(&states);
    let slack = Slack::measure();
    // The system plays a tenth of a second of sound and then nothing (a system that plays no sound sends no buffers).
    fake.send_audio(
        Track::System,
        Instant::now(),
        48_000,
        sine(880.0, 48_000, 0, 4_800),
    );
    wait_until("the sound to be taken", || {
        levels_of(&states, Track::System)
            .iter()
            .any(|l| l.is_some_and(|l| l > 0.9))
    });

    slack.sleep(2000);
    let asked = Instant::now();
    let finished = session.stop().unwrap();

    // The recording was as long as it ran, and so is what it made, though its only stem was 0.1 s long.
    let ran = reference.active_at(asked);
    assert!(ran >= 2.0 - slack.tolerance(), "{ran} s ran");
    assert_lasted(finished.duration, ran, ran, CHUNK_SECONDS, &slack);
    assert_decodes_as_long(&finished.media, &ffmpeg, finished.duration);
}

#[test]
fn a_recording_that_caught_nothing_is_an_error_and_leaves_the_folder_empty() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new());
    let (session, _states) =
        start(options(Mode::Audio, true, true), dir.path(), &fake, None).unwrap();

    let error = session.stop().unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error.to_string().contains("nothing was recorded"),
        "{error}"
    );
    assert_eq!(fake.stopped().len(), 2);
    assert!(names(dir.path()).is_empty(), "{:?}", names(dir.path()));
}

// --- Video ---

fn frames_of_screen() -> FakeCapturer {
    FakeCapturer::new().frames(320, 180, [10, 200, 30, 255], 15)
}

#[test]
fn pausing_removes_the_paused_time_from_every_track() {
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    // Every source goes on sending while the recording is paused.
    let fake = Arc::new(
        frames_of_screen()
            .tone(Track::Mic, 440.0)
            .tone(Track::System, 880.0),
    );
    let (session, states) = start(
        options(screen(), true, true),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();
    let reference = Reference::of(&states);
    let slack = Slack::measure();

    slack.sleep(2000);
    let (paused_from, paused_after) = bracket(|| session.pause());
    slack.sleep(2000);
    let (resumed_from, resumed_after) = bracket(|| session.resume());
    slack.sleep(2000);
    let asked = Instant::now();
    let finished = session.stop().unwrap();

    // It ran from the start to the stop, less the time it was paused: which is at least the 2 s that were slept, and
    // is between the instants of the two calls (each was made somewhere between the instants around it).
    let paused_least = (resumed_from - paused_after).as_secs_f64();
    let paused_most = (resumed_after - paused_from).as_secs_f64();
    assert!(
        paused_least >= 2.0 - slack.tolerance(),
        "{paused_least} s paused"
    );
    let ran = reference.active_at(asked);
    assert_lasted(
        finished.duration,
        ran - paused_most,
        ran - paused_least,
        FRAME_SECONDS,
        &slack,
    );
    // And none of it is what was delivered while it was paused: the recording is shorter than the time it took by
    // what was slept in the pause.
    assert!(
        finished.duration < ran - 2.0 + slack.tolerance() + FRAME_SECONDS,
        "{} s of {ran} s",
        finished.duration
    );
    let name = name_of(&finished.media);
    assert!(is_recording(&name), "{name}");
    assert_video_as_long(&finished.media, &ffmpeg, finished.duration);
    assert_decodes_as_long(&finished.media, &ffmpeg, finished.duration);
    // While it was paused the time stood still, and the state said so: all the reports of it have the same time, and
    // it is the time of the instant of the pause.
    let paused: Vec<Duration> = states
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, state)| match state {
            SessionState::Paused { active, .. } => Some(*active),
            _ => None,
        })
        .collect();
    assert!(paused.len() >= 10, "{} reports while paused", paused.len());
    assert!(
        paused.windows(2).all(|pair| pair[0] == pair[1]),
        "{paused:?}"
    );
    let at_pause = paused[0].as_secs_f64();
    assert!(
        at_pause >= reference.active_at(paused_from) - slack.tolerance()
            && at_pause <= reference.active_at(paused_after) + slack.tolerance(),
        "{at_pause} s at the pause"
    );
}

#[test]
fn an_ended_track_stops_and_keeps_what_was_recorded() {
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(frames_of_screen().tone(Track::Mic, 440.0).ends_after(
        Track::Screen,
        Duration::from_secs(2),
        EndReason::WindowClosed,
    ));
    let (session, states) = start(
        options(screen(), true, false),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();
    let reference = Reference::of(&states);
    let slack = Slack::measure();

    wait_until("the end", || has_ended(&states));
    let told = states
        .lock()
        .unwrap()
        .iter()
        .find_map(|(at, state)| {
            matches!(state, SessionState::Ended(EndReason::WindowClosed)).then_some(*at)
        })
        .expect("the state of the end");

    assert_eq!(session.wait_end(), Some(EndReason::WindowClosed));
    // Every source is stopped, not only the one that ended.
    wait_until("the sources to stop", || fake.stopped().len() == 2);
    // The recording does not go on after the end, whatever is asked of it.
    slack.sleep(700);
    session.resume();
    slack.sleep(300);
    let finished = session.stop().unwrap();

    // It stopped between the instant the end was sent and the instant the session told of it.
    let sent = fake.ended_at(Track::Screen).expect("when it ended");
    assert_lasted(
        finished.duration,
        reference.active_at(sent),
        reference.active_at(told),
        FRAME_SECONDS,
        &slack,
    );
    let name = name_of(&finished.media);
    assert!(is_recording(&name), "{name}");
    assert_video_as_long(&finished.media, &ffmpeg, finished.duration);
    let left = names(dir.path());
    assert!(
        !left.contains(&MARKER.to_owned()) && !left.contains(&"mic.wav".to_owned()),
        "{left:?}"
    );
    assert!(left.contains(&name), "{left:?}");
}

#[test]
fn a_screen_recording_that_never_got_a_frame_has_only_the_sound() {
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    // A window that is minimised gives no frame: the screen starts and says nothing.
    let fake = Arc::new(FakeCapturer::new().tone(Track::Mic, 440.0));
    let (session, states) = start(
        options(screen(), true, false),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();
    let reference = Reference::of(&states);
    let slack = Slack::measure();

    slack.sleep(1500);
    let asked = Instant::now();
    let finished = session.stop().unwrap();

    let name = name_of(&finished.media);
    assert!(is_sound_file(&name), "{name}");
    let ran = reference.active_at(asked);
    assert_lasted(finished.duration, ran, ran, CHUNK_SECONDS, &slack);
    assert!(
        finished.notes.iter().any(|note| note.contains("no video")),
        "{:?}",
        finished.notes
    );
    assert_eq!(names(dir.path()), [name]);
}

#[test]
fn a_frame_of_another_size_is_fitted_and_the_recording_goes_on() {
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new().tone(Track::Mic, 440.0));
    let (session, _states) = start(
        options(screen(), true, false),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();

    // The window is resized: the first frame fixes the size of the video, and the later ones are fitted into it.
    for n in 0..18u32 {
        let (width, height) = if n < 6 { (320, 180) } else { (200, 240) };
        fake.send_frame(Frame {
            width,
            height,
            stride: width * 4,
            bgra: [n as u8 * 10, 90, 200, 255].repeat((width * height) as usize),
        });
        sleep(66);
    }
    assert_eq!(session.wait_end(), None);
    let finished = session.stop().unwrap();

    assert!(is_recording(&name_of(&finished.media)));
    let video = video_seconds(&finished.media, &ffmpeg);
    assert!(
        (video - finished.duration).abs() < 0.3,
        "{video} s of video, {} s",
        finished.duration
    );
    assert!(finished.notes.is_empty(), "{:?}", finished.notes);
}

// --- A video that goes wrong (stand-ins for ffmpeg, which the shell runs) ---

/// An ffmpeg that is the real one, except for what it does as the video encoder (`on_encode` is shell: when it ends
/// the real one runs), and that writes its process id to `pids` each time it is run.
#[cfg(unix)]
fn ffmpeg_that(dir: &Path, real: &Path, pids: &Path, on_encode: &str) -> PathBuf {
    script(
        dir,
        "fake-ffmpeg",
        &format!(
            "echo $$ >> '{}'\ncase \"$*\" in *'-f null'*) ;; *rawvideo*) {on_encode};; esac\nexec '{}' \"$@\"",
            pids.display(),
            real.display()
        ),
        "",
    )
}

/// Whether the process `pid` is there (a process that has ended but was not waited for still is).
#[cfg(unix)]
fn running(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[cfg(unix)]
fn pids_in(file: &Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[cfg(unix)]
#[test]
fn the_video_is_as_long_as_the_recording_when_the_encoder_is_late() {
    let Some(real) = real_ffmpeg() else { return };
    let tools = tempfile::tempdir().unwrap();
    // The encoder does not take the first frame for a second: the frames that were due meanwhile are made up.
    let ffmpeg = ffmpeg_that(tools.path(), &real, &tools.path().join("pids"), "sleep 1");
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(frames_of_screen().tone(Track::Mic, 440.0));
    let (session, states) = start(
        options(screen(), true, false),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();
    let reference = Reference::of(&states);
    let slack = Slack::measure();

    slack.sleep(3000);
    let asked = Instant::now();
    let finished = session.stop().unwrap();

    // What was recorded is as long as it ran, and the video is as long as that, with the second that the encoder was
    // late made up (without it, it is a second short, whatever the machine does).
    let ran = reference.active_at(asked);
    assert!(ran >= 3.0 - slack.tolerance(), "{ran} s ran");
    assert_lasted(finished.duration, ran, ran, FRAME_SECONDS, &slack);
    assert_video_as_long(&finished.media, &real, finished.duration);
}

#[cfg(unix)]
#[test]
fn discard_leaves_nothing_behind() {
    let Some(real) = real_ffmpeg() else { return };
    let tools = tempfile::tempdir().unwrap();
    let pids = tools.path().join("pids");
    let ffmpeg = ffmpeg_that(tools.path(), &real, &pids, "");
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(
        frames_of_screen()
            .tone(Track::Mic, 440.0)
            .tone(Track::System, 880.0),
    );
    let (session, _states) = start(
        options(screen(), true, true),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();
    // The recording is under way: stems, a part of the video and the marker are in the folder.
    wait_until("the stems and the part of the video", || {
        let during = names(dir.path());
        during.contains(&MARKER.to_owned())
            && during.contains(&"mic.wav".to_owned())
            && during.contains(&"system.wav".to_owned())
            && during.iter().any(|n| n.ends_with(".part"))
    });

    session.discard();

    assert_eq!(fake.stopped().len(), 3);
    assert_eq!(names(dir.path()), Vec::<String>::new());
    // Every ffmpeg that was started is gone: the encoder was killed, not left to go on.
    let pids = pids_in(&pids);
    assert!(!pids.is_empty());
    for pid in pids {
        assert!(!running(&pid), "ffmpeg {pid} is still running");
    }
}

#[cfg(unix)]
#[test]
fn stop_gives_up_on_an_encoder_that_is_stuck_and_keeps_the_sound() {
    let Some(real) = real_ffmpeg() else { return };
    let tools = tempfile::tempdir().unwrap();
    let pids = tools.path().join("pids");
    // An encoder that takes nothing: the pipe fills up and the push that is waiting on it never ends.
    let ffmpeg = ffmpeg_that(tools.path(), &real, &pids, "exec sleep 600");
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(frames_of_screen().tone(Track::Mic, 440.0));
    let timing = Timing {
        video_finish_wait: Duration::from_millis(800),
    };
    let (session, states) = start_with(
        options(screen(), true, false),
        dir.path(),
        &fake,
        Some(&ffmpeg),
        timing,
    )
    .unwrap();
    let reference = Reference::of(&states);
    let slack = Slack::measure();
    slack.sleep(1500);
    let started = Instant::now();

    let finished = session.stop().unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
    let name = name_of(&finished.media);
    assert!(is_sound_file(&name), "{name}");
    // The recording is as long as it ran, as long as the stop was called (and not as long as the encoder was waited for).
    let ran = reference.active_at(started);
    assert!(ran >= 1.5 - slack.tolerance(), "{ran} s ran");
    assert_lasted(finished.duration, ran, ran, CHUNK_SECONDS, &slack);
    // The note says what happened with the video, and not in the words of the audio encoder.
    assert!(
        finished.notes.iter().any(|note| note.contains("video")),
        "{:?}",
        finished.notes
    );
    assert!(
        finished
            .notes
            .iter()
            .all(|note| !note.contains("audio encoder")),
        "{:?}",
        finished.notes
    );
    assert_eq!(names(dir.path()), [name]);
    for pid in pids_in(&pids) {
        assert!(!running(&pid), "ffmpeg {pid} is still running");
    }
}

#[cfg(unix)]
#[test]
fn discard_does_not_wait_for_an_encoder_that_is_stuck() {
    let Some(real) = real_ffmpeg() else { return };
    let tools = tempfile::tempdir().unwrap();
    let pids = tools.path().join("pids");
    let ffmpeg = ffmpeg_that(tools.path(), &real, &pids, "exec sleep 600");
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(frames_of_screen().tone(Track::Mic, 440.0));
    let (session, _states) = start(
        options(screen(), true, false),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();
    sleep(1500);
    let started = Instant::now();

    session.discard();

    // A stop waits 40 s for the video to end before it kills ffmpeg: a discard does not wait at all.
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "{:?}",
        started.elapsed()
    );
    assert!(names(dir.path()).is_empty(), "{:?}", names(dir.path()));
    for pid in pids_in(&pids) {
        assert!(!running(&pid), "ffmpeg {pid} is still running");
    }
}

#[cfg(unix)]
#[test]
fn a_video_that_fails_ends_the_recording_and_keeps_the_sound() {
    let Some(real) = real_ffmpeg() else { return };
    let tools = tempfile::tempdir().unwrap();
    // An encoder that takes a little, says what is wrong, and is gone.
    let ffmpeg = ffmpeg_that(
        tools.path(),
        &real,
        &tools.path().join("pids"),
        "head -c 1000 > /dev/null; echo 'Error while encoding' >&2; exit 1",
    );
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(frames_of_screen().tone(Track::Mic, 440.0));
    let (session, states) = start(
        options(screen(), true, false),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();

    wait_until("the end", || has_ended(&states));
    let reason = session.wait_end();
    let Some(EndReason::Failed(reason)) = reason else {
        panic!("{reason:?}")
    };
    // The reason is what ffmpeg said, not the words of the audio encoder.
    assert!(reason.contains("Error while encoding"), "{reason}");
    assert!(!reason.contains("audio encoder"), "{reason}");
    let finished = session.stop().unwrap();

    let name = name_of(&finished.media);
    assert!(is_sound_file(&name), "{name}");
    assert!(
        finished
            .notes
            .iter()
            .any(|note| note.contains("Error while encoding")),
        "{:?}",
        finished.notes
    );
}

// --- A recording that the program stopped ---

fn write_marker(folder: &Path, mode: Mode) {
    Marker {
        version: 1,
        options: options(mode, true, false),
        codec: None,
        container: None,
        started: 1_700_000_000.0,
    }
    .write(folder)
    .unwrap();
}

#[test]
fn recover_finishes_a_recording_left_by_a_crash() {
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    write_marker(dir.path(), screen());
    // A mic.wav as a crash leaves it: pushed 2.4 s of, never finished, so its header says what the last flush saw.
    let live = dir.path().join("live.wav");
    let mut stem = StemWriter::create(&live, 48_000).unwrap();
    let tone = sine(440.0, 48_000, 0, 115_200);
    for (i, block) in tone.chunks(4_800).enumerate() {
        stem.push(Duration::from_millis(i as u64 * 100), block)
            .unwrap();
    }
    std::fs::copy(&live, dir.path().join("mic.wav")).unwrap();
    drop(stem);
    std::fs::remove_file(&live).unwrap();
    // What the crash left of the stem is what its header says: it is that long, and no more than the 2.4 s.
    let stem_seconds = f64::from(
        hound::WavReader::open(dir.path().join("mic.wav"))
            .unwrap()
            .duration(),
    ) / 48_000.0;
    assert!((2.0..=2.4).contains(&stem_seconds), "{stem_seconds} s");
    // A video part as it is left: the first 2 s of the 6 that were pushed, or more where the encoder is quick.
    let (part, readable) = cut_off_video(dir.path(), &ffmpeg);
    assert!(part.exists());
    assert!(readable >= 1.9, "{readable} s of the cut off video");

    let finished = recover(dir.path(), Some(&ffmpeg))
        .expect("a marker")
        .unwrap();

    let name = name_of(&finished.media);
    assert!(is_recording(&name), "{name}");
    // The recording is as long as the longer of what the stem and the video kept, which is what there is to recover.
    let expected = stem_seconds.max(readable);
    assert!(
        (finished.duration - expected).abs() < 0.3,
        "{} s for {stem_seconds} s of sound and {readable} s of video",
        finished.duration
    );
    let video = video_seconds(&finished.media, &ffmpeg);
    assert!(
        (video - readable).abs() < 0.3,
        "{video} s of video, {readable} s were readable"
    );
    let sound = audio_seconds(&finished.media, &ffmpeg);
    assert!((sound - expected).abs() < 0.3, "{sound} s of sound");
    // The marker, the stem and the part are gone; the sound and the recording are what is left.
    let left = names(dir.path());
    assert!(!left.contains(&MARKER.to_owned()), "{left:?}");
    assert!(
        !left.iter().any(|n| n == "mic.wav" || n.ends_with(".part")),
        "{left:?}"
    );
    assert!(left.contains(&name), "{left:?}");
}

#[test]
fn recover_ignores_a_folder_without_a_marker() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mic.wav"), b"not looked at").unwrap();

    assert!(recover(dir.path(), None).is_none());
    assert_eq!(names(dir.path()), ["mic.wav"]);
}

#[test]
fn dropping_a_session_leaves_the_recording_to_be_recovered() {
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let dir = tempfile::tempdir().unwrap();
    // The microphone says 0.1 s and nothing more: what is recovered is as long as the recording ran, and not as long
    // as what the stem holds.
    let fake = Arc::new(FakeCapturer::new());
    let (session, states) = start(
        options(Mode::Audio, true, false),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();
    let reference = Reference::of(&states);
    let slack = Slack::measure();
    fake.send_audio(
        Track::Mic,
        Instant::now(),
        48_000,
        sine(440.0, 48_000, 0, 4_800),
    );
    wait_until("the sound to be taken", || {
        levels_of(&states, Track::Mic)
            .iter()
            .any(|l| l.is_some_and(|l| l > 0.9))
    });
    slack.sleep(1500);

    let asked = Instant::now();
    drop(session);

    assert_eq!(fake.stopped(), [Track::Mic]);
    let left = names(dir.path());
    assert!(
        left.contains(&MARKER.to_owned()) && left.contains(&"mic.wav".to_owned()),
        "{left:?}"
    );
    let finished = recover(dir.path(), Some(&ffmpeg))
        .expect("a marker")
        .unwrap();
    let ran = reference.active_at(asked);
    assert!(ran >= 1.5 - slack.tolerance(), "{ran} s ran");
    assert_lasted(finished.duration, ran, ran, CHUNK_SECONDS, &slack);
}

// --- A long recording ---

/// The memory the program holds, in bytes.
fn memory() -> u64 {
    let pid = sysinfo::get_current_pid().unwrap();
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).unwrap().memory()
}

/// Two hours of sound and of frames pushed as fast as the session takes them (`DICTATE_TEST_LONG_SECONDS` makes it
/// shorter): the memory of the program must not grow with the length of the recording.
#[test]
#[ignore = "takes minutes: DICTATE_TEST_LONG_RECORDING=1 cargo test -p dictate-services a_long_recording -- --ignored"]
fn a_long_recording_keeps_memory_flat() {
    if std::env::var_os("DICTATE_TEST_LONG_RECORDING").is_none() {
        eprintln!("skipped: DICTATE_TEST_LONG_RECORDING is not set");
        return;
    }
    let Some(ffmpeg) = real_ffmpeg() else { return };
    let seconds: u64 = std::env::var("DICTATE_TEST_LONG_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(7200);
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCapturer::new());
    let (session, _states) = start(
        options(screen(), true, true),
        dir.path(),
        &fake,
        Some(&ffmpeg),
    )
    .unwrap();
    let base = Instant::now();
    let chunk = sine(440.0, 48_000, 0, 960);
    let mut at_five_minutes = 0;

    for second in 0..seconds {
        for k in 0..50u32 {
            let at = base + Duration::from_secs(second) + Duration::from_millis(20) * k;
            fake.send_audio(Track::Mic, at, 48_000, chunk.clone());
            fake.send_audio(Track::System, at, 48_000, chunk.clone());
        }
        for _ in 0..10 {
            fake.send_frame(Frame {
                width: 320,
                height: 180,
                stride: 1280,
                bgra: vec![40; 320 * 180 * 4],
            });
        }
        // As fast as the session takes it: the sources deliver in real time, and a channel that nothing stops
        // would let a test that does not wait run ahead of what it is testing.
        sleep(5);
        if second == 300 {
            at_five_minutes = memory();
        }
    }
    let at_the_end = memory();
    let finished = session.stop().unwrap();
    let after = memory();

    let grew = |now: u64| now.saturating_sub(at_five_minutes) as f64 / 1e6;
    eprintln!(
        "memory: {:.0} MB at minute 5, +{:.0} MB at the end, +{:.0} MB after the mix ({} s, {})",
        at_five_minutes as f64 / 1e6,
        grew(at_the_end),
        grew(after),
        finished.duration,
        finished.media.display()
    );
    assert!(grew(at_the_end) < 100.0, "+{} MB", grew(at_the_end));
    assert!(grew(after) < 100.0, "+{} MB after the mix", grew(after));
    assert!((finished.duration - seconds as f64).abs() < 5.0);
}
