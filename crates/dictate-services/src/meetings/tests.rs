//! The tests of the pipeline, on a scripted backend (`fakes`) and a library in a temporary folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use dictate_core::meetings::library::SCREEN_SOURCE;
use dictate_core::meetings::{append_log, MeetingRecord, Meta};
use dictate_core::text::segments::Segment;
use dictate_core::text::whisper::Turn;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::error::key;
use super::fakes::{gate, line, FakeBackend, GateControl, SAMPLES};
use super::*;
use crate::http::ApiError;
use crate::media::decode::DecodeError;
use crate::media::tools::{install_command, Tool};
use crate::media::ytdlp::{Downloaded, LinkError};
use crate::meetings::{ErrorKind, Prepared, Step};
use crate::settings::Engine;
use crate::summary::SummaryProgress;
use crate::transcribe_api::{ApiProgress, LongApiError, TranscribeError};
use crate::whisper::local::TimedTranscript;

const CREATED: f64 = 1_731_063_600.5;
const WAITING: &str = "Waiting for another recording to finish transcribing…";
const SUMMARY: &str = "# Weekly sync\n\n## Summary\nWe talked.";

/// A recording's folder in a temporary library, and (for an imported file) the original outside of it, in a
/// folder with spaces and Vietnamese letters, as users have them.
struct Fixture {
    _dir: tempfile::TempDir,
    folder: PathBuf,
    job: JobSpec,
    original: Option<PathBuf>,
}

impl Fixture {
    fn folder_in(dir: &Path) -> PathBuf {
        let folder = dir.join("Dictate Meetings").join("2026-11-10 10-00-00");
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// A file that was imported: the title is its name without the extension.
    fn file(name: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let originals = dir.path().join("Cuộc họp gốc");
        fs::create_dir_all(&originals).unwrap();
        let original = originals.join(name);
        fs::write(&original, b"not really audio").unwrap();
        let folder = Self::folder_in(dir.path());
        let path = original.to_string_lossy().into_owned();
        Meta {
            title: Path::new(name)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            source: path.clone(),
            media: path,
            created: CREATED,
            duration: None,
        }
        .write(&folder)
        .unwrap();
        Fixture {
            job: JobSpec {
                id: "2026-11-10 10-00-00".into(),
                folder: folder.clone(),
                link: None,
            },
            folder,
            original: Some(original),
            _dir: dir,
        }
    }

    /// A link that was pasted: its title and source are the link, and there is no media yet.
    fn link(url: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let folder = Self::folder_in(dir.path());
        Meta {
            title: url.into(),
            source: url.into(),
            media: String::new(),
            created: CREATED,
            duration: None,
        }
        .write(&folder)
        .unwrap();
        Fixture {
            job: JobSpec {
                id: "2026-11-10 10-00-00".into(),
                folder: folder.clone(),
                link: Some(url.into()),
            },
            folder,
            original: None,
            _dir: dir,
        }
    }

    fn meta(&self) -> Meta {
        Meta::read(&self.folder).unwrap()
    }

    fn set_meta(&self, change: impl FnOnce(&mut Meta)) {
        let mut meta = self.meta();
        change(&mut meta);
        meta.write(&self.folder).unwrap();
    }

    fn raw_meta(&self) -> serde_json::Value {
        serde_json::from_slice(&fs::read(self.folder.join("meta.json")).unwrap()).unwrap()
    }

    fn read(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.folder.join(file)).ok()
    }

    fn write(&self, file: &str, text: &str) {
        fs::write(self.folder.join(file), text).unwrap();
    }

    fn exists(&self, file: &str) -> bool {
        self.folder.join(file).exists()
    }

    fn log(&self) -> Vec<String> {
        self.read("log.txt")
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

struct Ran {
    outcome: Outcome,
    events: Vec<JobEvent>,
}

impl Ran {
    fn error(&self) -> &MeetingError {
        match &self.outcome {
            Outcome::Failed(error) => error,
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// The steps that started, in order (a step that reports progress is one step).
    fn steps(&self) -> Vec<Step> {
        let mut steps = Vec::new();
        for event in &self.events {
            if let JobEvent::Step { step, .. } = event {
                if steps.last() != Some(step) {
                    steps.push(*step);
                }
            }
        }
        steps
    }

    fn progress(&self, of: Step) -> Vec<Option<f32>> {
        self.events
            .iter()
            .filter_map(|event| match event {
                JobEvent::Step { step, progress } if *step == of => Some(*progress),
                _ => None,
            })
            .collect()
    }

    fn logs(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|event| match event {
                JobEvent::Log(line) => Some(line.clone()),
                _ => None,
            })
            .collect()
    }
}

/// A safety net, not a way to wait: a test whose controller waits for a call that never comes fails after this
/// long, instead of hanging.
async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(30), future)
        .await
        .expect("the run did not finish: a call the test waits for never came")
}

/// Hands the thread over a few times, so that every other future of a `join!` that can go on does. Not a wait for
/// time: it is the turns that are counted, which is what a test of "this must not happen" needs.
async fn let_the_others_run() {
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
}

fn assert_progress(actual: Vec<Option<f32>>, expected: &[Option<f32>]) {
    let same = actual.len() == expected.len()
        && actual.iter().zip(expected).all(|pair| match pair {
            (Some(a), Some(b)) => (a - b).abs() < 1e-6,
            (a, b) => a == b,
        });
    assert!(same, "progress {actual:?}, expected {expected:?}");
}

async fn run_with(
    backend: &FakeBackend,
    lock: &LocalLock,
    fx: &Fixture,
    from: StartAt,
    cancel: &CancellationToken,
) -> Ran {
    let events = Mutex::new(Vec::new());
    let emit = |event: JobEvent| events.lock().unwrap().push(event);
    let outcome = run(backend, lock, &fx.job, from, cancel, &emit).await;
    Ran {
        outcome,
        events: events.into_inner().unwrap(),
    }
}

async fn run_job(backend: &FakeBackend, fx: &Fixture, from: StartAt) -> Ran {
    run_with(
        backend,
        &LocalLock::new(),
        fx,
        from,
        &CancellationToken::new(),
    )
    .await
}

fn api_part_failed(
    index: usize,
    count: usize,
    kept: usize,
    error: TranscribeError,
) -> LongApiError {
    LongApiError::PartFailed {
        index,
        count,
        kept,
        error,
    }
}

fn server_error(message: &str) -> TranscribeError {
    TranscribeError::Api(ApiError::Status {
        status: 500,
        message: message.into(),
    })
}

/// A backend for the API engine: chosen, set up, and with Whisper behind it.
fn api_backend() -> FakeBackend {
    let mut backend = FakeBackend::new();
    backend.engine = Engine::Api;
    backend.api_ready = true;
    backend
}

// ---- the whole run ----

#[tokio::test]
async fn pipeline_link_runs_all_steps_in_order() {
    let fx = Fixture::link("https://youtu.be/abc");
    let source = fx.folder.join("source.m4a");
    let mut backend = FakeBackend::new();
    backend.download = Ok(Downloaded {
        title: Some("Weekly sync recording".into()),
        file: source.clone(),
    });
    backend.download_writes = vec!["source.m4a".into()];
    backend.download_progress = vec![0.5];

    let ran = run_job(&backend, &fx, StartAt::Download).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert_eq!(
        ran.steps(),
        [Step::Download, Step::Audio, Step::Transcript, Step::Summary]
    );
    assert_eq!(
        ran.events.last(),
        Some(&JobEvent::Finished(Outcome::Done { summary: true }))
    );
    assert!(ran.events.contains(&JobEvent::Step {
        step: Step::Download,
        progress: Some(0.5)
    }));
    assert_eq!(
        backend.calls(),
        [
            "download",
            "prepare_audio",
            "transcribe_local words=false",
            "summarize"
        ]
    );
    let meta = fx.meta();
    assert_eq!(meta.title, "Weekly sync recording");
    assert_eq!(meta.source, "https://youtu.be/abc");
    assert_eq!(meta.media, source.to_string_lossy());
    assert_eq!(meta.duration, Some(10.0));
    assert_eq!(meta.created, CREATED);
    assert_eq!(backend.media_seen(), [source]);
    assert_eq!(fx.read("transcript.txt").unwrap(), "[00:00] Hello there");
    assert_eq!(fx.read("summary.md").unwrap(), SUMMARY);
    // The title of a link is the video's: the summary's own title does not replace it.
    assert_eq!(
        backend.summarize_seen(),
        [(
            "Weekly sync recording".to_owned(),
            false,
            "[00:00] Hello there".to_owned()
        )]
    );
}

#[tokio::test]
async fn pipeline_summary_step_starts_with_no_progress_figure() {
    let fx = Fixture::file("Standup.mp3");
    let backend = FakeBackend::new();

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_progress(ran.progress(Step::Summary), &[None]);
}

#[tokio::test]
async fn pipeline_long_summary_reports_its_parts() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.summary_progress = vec![
        SummaryProgress::Part { index: 1, count: 2 },
        SummaryProgress::Part { index: 2, count: 2 },
        SummaryProgress::Merging,
    ];

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    let logs = ran.logs();
    assert!(
        logs.contains(&"Summarizing part 1/2…".to_owned()),
        "{logs:?}"
    );
    assert!(
        logs.contains(&"Summarizing part 2/2…".to_owned()),
        "{logs:?}"
    );
    assert!(logs.contains(&"Merging the parts…".to_owned()), "{logs:?}");
    assert_progress(
        ran.progress(Step::Summary),
        &[None, Some(0.0), Some(0.45), Some(0.9)],
    );
}

#[tokio::test]
async fn pipeline_file_untitled_takes_title_from_summary() {
    let fx = Fixture::file("Standup notes.mp4");
    let mut backend = FakeBackend::new();
    backend.summary = Ok("# Sprint planning\n\n## Summary\nPlans.".into());

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert_eq!(fx.meta().title, "Sprint planning");
    assert!(ran
        .events
        .contains(&JobEvent::TitleChanged("Sprint planning".into())));
    assert_eq!(backend.summarize_seen()[0].0, "Standup notes");
    assert!(
        backend.summarize_seen()[0].1,
        "the model is asked for a title"
    );
    // Still the same recording, with its own file.
    assert_eq!(fx.meta().source, fx.original.unwrap().to_string_lossy());
}

#[tokio::test]
async fn pipeline_keeps_a_title_that_is_not_the_file_name() {
    let fx = Fixture::file("Standup notes.mp4");
    fx.set_meta(|meta| meta.title = "Our own title".into());
    let mut backend = FakeBackend::new();
    backend.summary = Ok("# Something else\n\n## Summary\nPlans.".into());

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(fx.meta().title, "Our own title");
    assert!(!backend.summarize_seen()[0].1);
    assert!(!ran
        .events
        .iter()
        .any(|event| matches!(event, JobEvent::TitleChanged(_))));
}

#[tokio::test]
async fn pipeline_screen_recording_is_untitled() {
    let fx = Fixture::file("recording.mov");
    fx.set_meta(|meta| {
        meta.source = SCREEN_SOURCE.into();
        meta.title = "Screen recording 10:00 10/11".into();
    });
    let backend = FakeBackend::new();

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert!(backend.summarize_seen()[0].1);
    assert_eq!(fx.meta().title, "Weekly sync");
    assert!(ran
        .events
        .contains(&JobEvent::TitleChanged("Weekly sync".into())));
}

#[tokio::test]
async fn pipeline_summary_without_a_title_line_keeps_the_title() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.summary = Ok("## Summary\nNo title line here.".into());

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert_eq!(fx.meta().title, "Standup");
}

// ---- the audio ----

#[tokio::test]
async fn pipeline_missing_media_fails_with_no_media() {
    let fx = Fixture::file("Standup.mp3");
    fs::remove_file(fx.original.as_ref().unwrap()).unwrap();
    // What an earlier run wrote stays.
    fx.write("transcript.txt", "[00:00] Earlier");
    fx.write("summary.md", "# Earlier");
    let backend = FakeBackend::new();

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(
        ran.error(),
        &MeetingError {
            kind: ErrorKind::NoMedia,
            key: key::NO_MEDIA,
            detail: None
        }
    );
    assert!(backend.calls().is_empty(), "{:?}", backend.calls());
    assert_eq!(fx.read("transcript.txt").unwrap(), "[00:00] Earlier");
    assert_eq!(fx.read("summary.md").unwrap(), "# Earlier");
    assert_eq!(fx.log().last().unwrap(), "ERROR: No media file.");
}

#[tokio::test]
async fn pipeline_without_a_media_path_fails_with_no_media() {
    let fx = Fixture::link("https://youtu.be/abc");
    let backend = FakeBackend::new();

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.error().kind, ErrorKind::NoMedia);
    assert!(backend.calls().is_empty());
}

#[tokio::test]
async fn pipeline_media_that_vanishes_while_decoding_is_no_media() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.prepare = Err(DecodeError::Missing);

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.error().key, key::NO_MEDIA);
}

#[tokio::test]
async fn pipeline_no_audio_track_fails_clearly() {
    let fx = Fixture::file("Silent film.mp4");
    let mut backend = FakeBackend::new();
    backend.prepare = Err(DecodeError::NoAudio);

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(
        ran.error(),
        &MeetingError {
            kind: ErrorKind::NoAudio,
            key: key::NO_AUDIO,
            detail: None
        }
    );
    assert_eq!(fx.log().last().unwrap(), "ERROR: The file has no audio.");
    assert_eq!(
        ran.events.last(),
        Some(&JobEvent::Finished(ran.outcome.clone()))
    );
    assert!(!fx.exists("transcript.txt"));
}

#[tokio::test]
async fn pipeline_unsupported_format_names_ffmpeg() {
    let fx = Fixture::file("Call.wma");
    let command = install_command(Tool::Ffmpeg);
    let mut backend = FakeBackend::new();
    backend.prepare = Err(DecodeError::Unsupported {
        what: "no decoder for wma".into(),
        ffmpeg_hint: command,
    });

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(
        ran.error(),
        &MeetingError {
            kind: ErrorKind::Unsupported {
                command: command.into()
            },
            key: key::NEEDS_FFMPEG,
            detail: Some("no decoder for wma".into())
        }
    );
    assert_eq!(
        fx.log().last().unwrap(),
        &format!("ERROR: This format needs ffmpeg: {command}")
    );
}

#[tokio::test]
async fn pipeline_other_decode_errors_are_other() {
    let fx = Fixture::file("Call.mp3");
    let mut backend = FakeBackend::new();
    backend.prepare = Err(DecodeError::Ffmpeg {
        code: Some(1),
        tail: "Invalid data".into(),
    });

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.error().kind, ErrorKind::Other);
    assert_eq!(ran.error().key, key::OTHER);
    assert_eq!(
        ran.error().detail.as_deref(),
        Some("ffmpeg failed (1): Invalid data")
    );
    assert_eq!(
        fx.log().last().unwrap(),
        "ERROR: ffmpeg failed (1): Invalid data"
    );
}

#[tokio::test]
async fn pipeline_audio_writes_the_duration_and_logs_it() {
    let fx = Fixture::file("Long call.mp3");
    let mut backend = FakeBackend::new();
    backend.prepare = Ok(Prepared {
        samples_16k: Arc::new(vec![0.0; SAMPLES]),
        duration: 3725.9,
        m4a: Ok(()),
    });
    backend.prepare_progress = vec![0.25];

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    // The container's figure is the duration; this one is far longer than the samples, so it is a damaged file.
    assert_eq!(fx.meta().duration, Some(3725.9));
    assert!(
        fx.log().contains(&"Audio: 1:02:05".to_owned()),
        "{:?}",
        fx.log()
    );
    assert_progress(ran.progress(Step::Audio), &[None, Some(0.25)]);
}

#[tokio::test]
async fn pipeline_uses_the_decoded_length_when_the_container_has_none() {
    let fx = Fixture::file("Voice.mp3");
    let mut backend = FakeBackend::new();
    backend.prepare = Ok(Prepared {
        samples_16k: Arc::new(vec![0.0; SAMPLES]),
        duration: 0.0,
        m4a: Ok(()),
    });

    run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(fx.meta().duration, Some(10.0));
    assert!(fx.log().contains(&"Audio: 00:10".to_owned()));
}

#[tokio::test]
async fn pipeline_goes_on_without_audio_m4a() {
    let fx = Fixture::file("Voice.mp3");
    let mut backend = FakeBackend::new();
    backend.prepare = Ok(Prepared {
        samples_16k: Arc::new(vec![0.0; SAMPLES]),
        duration: 10.0,
        m4a: Err("no encoder".into()),
    });

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert!(fx
        .log()
        .contains(&"Couldn't write audio.m4a (no encoder) — continuing without it.".to_owned()));
}

#[tokio::test]
async fn pipeline_warns_when_the_file_ends_early() {
    let fx = Fixture::file("Cut short.mp3");
    let mut backend = FakeBackend::new();
    backend.prepare = Ok(Prepared {
        samples_16k: Arc::new(vec![0.0; SAMPLES]),
        duration: 60.0,
        m4a: Ok(()),
    });

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert!(
        fx.log().contains(
            &"The file seems damaged: it should last 01:00 but the audio ends at 00:10. Continuing with what could be read."
                .to_owned()
        ),
        "{:?}",
        fx.log()
    );
}

#[tokio::test]
async fn pipeline_does_not_warn_for_the_encoders_delay() {
    let fx = Fixture::file("Fine.mp3");
    let mut backend = FakeBackend::new();
    backend.prepare = Ok(Prepared {
        samples_16k: Arc::new(vec![0.0; SAMPLES]),
        duration: 10.05,
        m4a: Ok(()),
    });

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert!(
        fx.log().contains(&"Audio: 00:10".to_owned()),
        "{:?}",
        fx.log()
    );
    assert!(
        !fx.log().iter().any(|l| l.contains("damaged")),
        "{:?}",
        fx.log()
    );
}

// ---- the engine ----

#[tokio::test]
async fn pipeline_api_error_falls_back_to_whisper_and_logs() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = api_backend();
    backend.api = Err(api_part_failed(1, 2, 0, server_error("boom")));
    backend.api_writes = vec!["api-parts/en-480s/0.json".into()];

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert_eq!(
        backend.calls(),
        [
            "prepare_audio",
            "transcribe_api",
            "transcribe_local words=false",
            "summarize"
        ]
    );
    let log = fx.log();
    assert!(
        log.contains(&"API transcript: whisper-1…".to_owned()),
        "{log:?}"
    );
    assert!(
        log.contains(
            &"Transcription API failed (Part 1/2: API error 500: boom) → using Whisper on this computer."
                .to_owned()
        ),
        "{log:?}"
    );
    assert_eq!(fx.read("transcript.txt").unwrap(), "[00:00] Hello there");
    // Whisper did the work: what the API had cached is of no use now.
    assert!(!fx.exists("api-parts"));
}

#[tokio::test]
async fn pipeline_api_without_key_uses_whisper() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.engine = Engine::Api;
    backend.api_ready = false;

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert!(!backend.calls().contains(&"transcribe_api".to_owned()));
    assert!(fx
        .log()
        .contains(&"No transcription API key → using Whisper on this computer.".to_owned()));
}

#[tokio::test]
async fn pipeline_whisper_engine_ignores_a_ready_api() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.api_ready = true;

    run_job(&backend, &fx, StartAt::Audio).await;

    assert!(backend
        .calls()
        .contains(&"transcribe_local words=false".to_owned()));
    assert!(!backend.calls().contains(&"transcribe_api".to_owned()));
    assert!(
        !fx.log().iter().any(|l| l.contains("API")),
        "{:?}",
        fx.log()
    );
}

#[tokio::test]
async fn pipeline_no_model_and_no_api_is_no_model() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.whisper_ready = false;
    backend.api_ready = false;

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(
        ran.error(),
        &MeetingError {
            kind: ErrorKind::NoModel,
            key: key::NO_MODEL,
            detail: None
        }
    );
    assert_eq!(
        fx.log().last().unwrap(),
        "ERROR: Download a Whisper model in Settings → Speech, or set up a transcription API."
    );
    assert_eq!(backend.calls(), ["prepare_audio"]);
    assert!(!fx.exists("transcript.txt"));
}

#[tokio::test]
async fn pipeline_api_engine_with_no_api_and_no_model_is_no_model() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.engine = Engine::Api;
    backend.whisper_ready = false;

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.error().kind, ErrorKind::NoModel);
    assert!(!fx
        .log()
        .iter()
        .any(|l| l.contains("No transcription API key")));
}

#[tokio::test]
async fn pipeline_api_failure_without_whisper_names_the_part_and_keeps_the_cache() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = api_backend();
    backend.whisper_ready = false;
    backend.api = Err(api_part_failed(3, 5, 2, server_error("boom")));
    backend.api_writes = vec!["api-parts/en-480s/0.json".into()];

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    let detail = "Part 3/5: API error 500: boom\n2 finished parts were kept; running it again continues from part 3.";
    assert_eq!(
        ran.error(),
        &MeetingError {
            kind: ErrorKind::Api,
            key: key::PART_FAILED,
            detail: Some(detail.into())
        }
    );
    assert_eq!(
        fx.log().last().unwrap(),
        "2 finished parts were kept; running it again continues from part 3."
    );
    assert!(fx
        .read("log.txt")
        .unwrap()
        .contains(&format!("ERROR: {detail}")));
    // The finished parts are what a second run continues from.
    assert!(fx.exists("api-parts/en-480s/0.json"));
    assert!(!fx.exists("transcript.txt"));
    assert!(!backend
        .calls()
        .iter()
        .any(|c| c.starts_with("transcribe_local")));
}

#[tokio::test]
async fn pipeline_api_blocked_reply_is_told_apart() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = api_backend();
    backend.whisper_ready = false;
    backend.api = Err(api_part_failed(
        1,
        1,
        0,
        TranscribeError::Blocked {
            model: "google/gemini-3.5-flash".into(),
        },
    ));

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.error().kind, ErrorKind::Api);
    assert_eq!(ran.error().key, key::TRANSCRIPT_API);
    let detail = ran.error().detail.as_deref().unwrap();
    assert!(
        detail.starts_with("Part 1/1: Google's safety filter blocked"),
        "{detail}"
    );
}

#[tokio::test]
async fn pipeline_api_io_error_is_a_transcript_api_error() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = api_backend();
    backend.whisper_ready = false;
    backend.api = Err(LongApiError::Io("disk full".into()));

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.error().kind, ErrorKind::Api);
    assert_eq!(ran.error().key, key::TRANSCRIPT_API);
    assert_eq!(
        ran.error().detail.as_deref(),
        Some("Could not prepare the audio for the API: disk full")
    );
}

#[tokio::test]
async fn pipeline_api_logs_each_part_and_reaches_100() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = api_backend();
    backend.api_progress = vec![
        ApiProgress::Part {
            index: 1,
            count: 2,
            reused: false,
        },
        ApiProgress::Part {
            index: 2,
            count: 2,
            reused: true,
        },
        ApiProgress::Retry {
            index: 2,
            count: 2,
            wait_s: 5,
            error: "API error 500: x".into(),
        },
        ApiProgress::CutOff {
            from: 65.0,
            keeping: false,
        },
        ApiProgress::CutOff {
            from: 65.0,
            keeping: true,
        },
    ];

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    let log = fx.log();
    let start = log
        .iter()
        .position(|l| l == "API transcript: whisper-1…")
        .unwrap();
    assert_eq!(
        log[start..start + 8],
        [
            "API transcript: whisper-1…",
            "API whisper-1: part 1/2 — 0%",
            "API whisper-1: part 2/2 — reusing the earlier result",
            "Part 2/2 failed (API error 500: x) → retrying in 5 seconds…",
            "The part from 01:05 was cut off → splitting it in two and redoing it.",
            "The part from 01:05 was still cut off → keeping what was written in full.",
            "API whisper-1: 100%",
            "Saved the transcript (26 characters).",
        ]
    );
    assert_progress(
        ran.progress(Step::Transcript),
        &[None, Some(0.0), Some(0.5), Some(1.0)],
    );
    assert_eq!(
        fx.read("transcript.txt").unwrap(),
        "[00:00] Hello from the API"
    );
}

// ---- local transcripts ----

#[tokio::test]
async fn pipeline_local_progress_without_speakers_spans_zero_to_one() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.local_progress = vec![0.25, 0.5];

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_progress(
        ran.progress(Step::Transcript),
        &[None, Some(0.25), Some(0.5), Some(1.0)],
    );
}

#[tokio::test]
async fn pipeline_diarize_progress_starts_preparing_and_ends_at_100() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.diarize_enabled = true;
    backend.local_progress = vec![0.5, 1.0];
    backend.diarize_progress = vec![0.5];

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(
        backend.calls(),
        [
            "prepare_audio",
            "transcribe_local words=true",
            "diarize",
            "summarize"
        ]
    );
    // Transcription takes the first 85%; the speaker separation says nothing until its second half, which is
    // "preparing" and then the last 15%; and it is at 100% when it returns, heard or not.
    assert_progress(
        ran.progress(Step::Transcript),
        &[None, Some(0.425), Some(0.85), None, Some(0.925), Some(1.0)],
    );
    assert!(ran.logs().contains(&"Separating speakers…".to_owned()));
}

#[tokio::test]
async fn pipeline_diarize_failure_keeps_transcript_without_speakers() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.diarize_enabled = true;
    backend.diarize = Err("could not download the model".into());

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert_eq!(fx.read("transcript.txt").unwrap(), "[00:00] Hello there");
    let log = fx.log();
    assert!(log.contains(&"Separating speakers…".to_owned()), "{log:?}");
    assert!(
        log.contains(
            &"Couldn't separate speakers (could not download the model); the transcript has no speaker names."
                .to_owned()
        ),
        "{log:?}"
    );
    assert_eq!(
        ran.progress(Step::Transcript).last().copied(),
        Some(Some(1.0))
    );
}

#[tokio::test]
async fn pipeline_diarize_turns_name_the_speakers() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.diarize_enabled = true;
    backend.local = Ok(TimedTranscript {
        lines: vec![line("Hello there", 0.0, 2.0), line("Hi back", 3.0, 5.0)],
        language: None,
    });
    backend.diarize = Ok(vec![
        Turn {
            start: 0.0,
            end: 2.5,
            speaker: 7,
        },
        Turn {
            start: 2.5,
            end: 6.0,
            speaker: 3,
        },
    ]);

    run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(
        fx.read("transcript.txt").unwrap(),
        "[00:00] Speaker 1: Hello there\n\n[00:03] Speaker 2: Hi back"
    );
}

#[tokio::test]
async fn pipeline_local_jobs_wait_for_each_other() {
    let lock = LocalLock::new();
    let order = Arc::new(Mutex::new(Vec::new()));
    let (hold_local, local) = gate();
    let (hold_diarize, diarize) = gate();
    let mut a = FakeBackend::new();
    a.gates.local = Some(hold_local);
    a.gates.diarize = Some(hold_diarize);
    a.diarize_enabled = true;
    a.trace = Some(("a".into(), order.clone()));
    let mut b = FakeBackend::new();
    b.trace = Some(("b".into(), order.clone()));
    let (fx_a, fx_b) = (Fixture::file("a.mp3"), Fixture::file("b.mp3"));
    let cancel = CancellationToken::new();
    let (waiting_tx, mut waiting_rx) = mpsc::unbounded_channel::<String>();

    let run_a = run_with(&a, &lock, &fx_a, StartAt::Audio, &cancel);
    let run_b = async {
        // B starts when A is in the middle of its transcription, holding the lock.
        local.started.await.unwrap();
        let emit = |event: JobEvent| {
            if let JobEvent::Log(line) = event {
                let _ = waiting_tx.send(line);
            }
        };
        run(&b, &lock, &fx_b.job, StartAt::Audio, &cancel, &emit).await
    };
    let controller = async {
        loop {
            if waiting_rx.recv().await.unwrap() == WAITING {
                break;
            }
        }
        // B has said it waits; it has not started.
        assert!(!b
            .calls()
            .iter()
            .any(|call| call.starts_with("transcribe_local")));
        local.release.send(()).unwrap();
        // A is in its speaker separation now, and B must still be waiting: the lock is A's until that is over.
        diarize.started.await.unwrap();
        let_the_others_run().await;
        assert!(!b
            .calls()
            .iter()
            .any(|call| call.starts_with("transcribe_local")));
        diarize.release.send(()).unwrap();
    };
    let (ran_a, outcome_b, ()) = bounded(async { tokio::join!(run_a, run_b, controller) }).await;

    assert_eq!(ran_a.outcome, Outcome::Done { summary: true });
    assert_eq!(outcome_b, Outcome::Done { summary: true });
    // B starts after A has let go, which is after A's speaker separation too.
    assert_eq!(
        *order.lock().unwrap(),
        [
            "a:local_start",
            "a:local_end",
            "a:diarize_end",
            "b:local_start",
            "b:local_end"
        ]
    );
    assert!(fx_b.log().contains(&WAITING.to_owned()), "{:?}", fx_b.log());
    assert!(!fx_a.log().contains(&WAITING.to_owned()));
}

#[tokio::test]
async fn local_lock_serves_jobs_in_the_order_they_asked() {
    let lock = LocalLock::new();
    let order = Arc::new(Mutex::new(Vec::new()));
    let (hold, control) = gate();
    let GateControl {
        started, release, ..
    } = control;
    let backend = |label: &str| {
        let mut backend = FakeBackend::new();
        backend.trace = Some((label.into(), order.clone()));
        backend
    };
    let mut a = backend("a");
    a.gates.local = Some(hold);
    let (b, c) = (backend("b"), backend("c"));
    let (fx_a, fx_b, fx_c) = (
        Fixture::file("a.mp3"),
        Fixture::file("b.mp3"),
        Fixture::file("c.mp3"),
    );
    let cancel = CancellationToken::new();
    let (b_tx, mut b_rx) = mpsc::unbounded_channel::<String>();
    let (c_tx, mut c_rx) = mpsc::unbounded_channel::<String>();
    let (go_c_tx, go_c_rx) = tokio::sync::oneshot::channel::<()>();

    let run_a = run_with(&a, &lock, &fx_a, StartAt::Audio, &cancel);
    let run_b = async {
        started.await.unwrap();
        let emit = |event: JobEvent| {
            if let JobEvent::Log(line) = event {
                let _ = b_tx.send(line);
            }
        };
        run(&b, &lock, &fx_b.job, StartAt::Audio, &cancel, &emit).await
    };
    let run_c = async {
        go_c_rx.await.unwrap();
        let emit = |event: JobEvent| {
            if let JobEvent::Log(line) = event {
                let _ = c_tx.send(line);
            }
        };
        run(&c, &lock, &fx_c.job, StartAt::Audio, &cancel, &emit).await
    };
    let controller = async {
        // B asks first, then C; only then does A let go.
        while b_rx.recv().await.unwrap() != WAITING {}
        go_c_tx.send(()).unwrap();
        while c_rx.recv().await.unwrap() != WAITING {}
        release.send(()).unwrap();
    };
    let (ran_a, outcome_b, outcome_c, ()) =
        bounded(async { tokio::join!(run_a, run_b, run_c, controller) }).await;

    assert_eq!(ran_a.outcome, Outcome::Done { summary: true });
    assert_eq!(outcome_b, Outcome::Done { summary: true });
    assert_eq!(outcome_c, Outcome::Done { summary: true });
    assert_eq!(
        *order.lock().unwrap(),
        [
            "a:local_start",
            "a:local_end",
            "b:local_start",
            "b:local_end",
            "c:local_start",
            "c:local_end"
        ]
    );
}

#[tokio::test]
async fn pipeline_local_failure_is_other_and_keeps_the_lock_free() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.local = Err("Could not load the Whisper model: the file is damaged".into());
    let lock = LocalLock::new();

    let ran = run_with(
        &backend,
        &lock,
        &fx,
        StartAt::Audio,
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(ran.error().kind, ErrorKind::Other);
    assert_eq!(ran.error().key, key::OTHER);
    assert_eq!(
        fx.log().last().unwrap(),
        "ERROR: Could not load the Whisper model: the file is damaged"
    );
    assert!(lock.try_acquire().is_some());
}

#[tokio::test]
async fn pipeline_empty_transcript_fails() {
    let fx = Fixture::file("Silence.mp3");
    let mut backend = FakeBackend::new();
    backend.local = Ok(TimedTranscript {
        lines: vec![line("   ", 0.0, 2.0)],
        language: None,
    });
    // An earlier transcript is not replaced by nothing.
    fx.write("transcript.txt", "[00:00] Earlier");

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(
        ran.error(),
        &MeetingError {
            kind: ErrorKind::Empty,
            key: key::EMPTY_TRANSCRIPT,
            detail: None
        }
    );
    assert_eq!(
        fx.log().last().unwrap(),
        "ERROR: Empty transcript — no speech was heard."
    );
    assert_eq!(fx.read("transcript.txt").unwrap(), "[00:00] Earlier");
    assert!(!backend.calls().contains(&"summarize".to_owned()));
}

#[tokio::test]
async fn pipeline_empty_api_transcript_fails() {
    let fx = Fixture::file("Silence.mp3");
    let mut backend = api_backend();
    backend.api = Ok(vec![Segment {
        start: 0.0,
        speaker: None,
        text: "  ".into(),
    }]);

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.error().kind, ErrorKind::Empty);
    assert!(!fx.exists("transcript.txt"));
}

// ---- the summary ----

#[tokio::test]
async fn pipeline_summary_failure_keeps_transcript() {
    let fx = Fixture::file("Standup.mp3");
    fx.write("summary.md", "# Earlier summary");
    fx.write("tasks.json", "[0]");
    let mut backend = FakeBackend::new();
    backend.summary = Err("API error 401: bad key".into());

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(
        ran.error(),
        &MeetingError {
            kind: ErrorKind::Api,
            key: key::SUMMARY_FAILED,
            detail: Some("API error 401: bad key".into())
        }
    );
    assert_eq!(
        fx.log().last().unwrap(),
        "ERROR: Summary failed: API error 401: bad key"
    );
    assert_eq!(fx.read("transcript.txt").unwrap(), "[00:00] Hello there");
    // The old summary and its ticks are as they were.
    assert_eq!(fx.read("summary.md").unwrap(), "# Earlier summary");
    assert_eq!(fx.read("tasks.json").unwrap(), "[0]");
}

#[tokio::test]
async fn pipeline_blank_summary_is_a_failure() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.summary = Ok("  \n".into());

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.error().key, key::SUMMARY_FAILED);
    assert!(!fx.exists("summary.md"));
}

#[tokio::test]
async fn pipeline_without_ai_is_transcript_only() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.ai_ready = false;

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: false });
    assert_eq!(ran.steps(), [Step::Audio, Step::Transcript]);
    assert!(!backend.calls().contains(&"summarize".to_owned()));
    assert!(fx.exists("transcript.txt"));
    assert!(!fx.exists("summary.md"));
    assert_eq!(
        fx.log().last().unwrap(),
        "API not set up in Settings — transcript only."
    );
    assert_eq!(
        ran.events.last(),
        Some(&JobEvent::Finished(Outcome::Done { summary: false }))
    );
}

#[tokio::test]
async fn pipeline_resummarize_clears_tasks() {
    let fx = Fixture::file("Standup.mp3");
    fx.write("transcript.txt", "[00:00] Already transcribed");
    fx.write("summary.md", "# Old");
    fx.write("tasks.json", "[0,2]");
    let backend = FakeBackend::new();

    let ran = run_job(&backend, &fx, StartAt::Summary).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert_eq!(backend.calls(), ["summarize"]);
    assert_eq!(ran.steps(), [Step::Summary]);
    assert_eq!(fx.read("summary.md").unwrap(), SUMMARY);
    assert!(!fx.exists("tasks.json"));
    assert_eq!(backend.summarize_seen()[0].2, "[00:00] Already transcribed");
    assert_eq!(
        fx.read("transcript.txt").unwrap(),
        "[00:00] Already transcribed"
    );
    // Whatever the title, a run that starts at the summary leaves the log it found.
    assert_eq!(fx.log().last().unwrap(), "Done: summary.md");
}

#[tokio::test]
async fn pipeline_summarize_again_without_a_transcript_starts_at_the_audio() {
    let fx = Fixture::file("Standup.mp3");
    fx.write("log.txt", "ERROR: an earlier failure\n");
    let backend = FakeBackend::new();

    let ran = run_job(&backend, &fx, StartAt::Summary).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert_eq!(
        backend.calls(),
        ["prepare_audio", "transcribe_local words=false", "summarize"]
    );
    assert_eq!(fx.log()[0], "ERROR: an earlier failure");
}

#[tokio::test]
async fn pipeline_start_at_the_audio_clears_the_log() {
    let fx = Fixture::file("Standup.mp3");
    fx.write("log.txt", "ERROR: an earlier failure\n");
    let backend = FakeBackend::new();

    run_job(&backend, &fx, StartAt::Audio).await;

    let log = fx.log();
    assert!(
        !log.iter().any(|l| l.contains("earlier failure")),
        "{log:?}"
    );
    assert_eq!(log[0], "Audio: 00:10");
}

#[tokio::test]
async fn pipeline_start_at_the_download_without_a_link_goes_on_with_the_audio() {
    let fx = Fixture::file("Standup.mp3");
    let backend = FakeBackend::new();

    let ran = run_job(&backend, &fx, StartAt::Download).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert!(!backend.calls().contains(&"download".to_owned()));
}

// ---- links ----

#[tokio::test]
async fn pipeline_retry_after_failed_download_downloads_again() {
    let fx = Fixture::link("https://youtu.be/abc");
    let mut failing = FakeBackend::new();
    failing.download = Err(LinkError::Failed {
        code: Some(1),
        tail: "ERROR: Video unavailable".into(),
    });
    failing.download_writes = vec!["source.m4a.part".into()];

    let first = run_job(&failing, &fx, StartAt::Download).await;

    assert_eq!(
        first.error(),
        &MeetingError {
            kind: ErrorKind::Other,
            key: key::YTDLP_FAILED,
            detail: Some("yt-dlp failed (1): ERROR: Video unavailable".into())
        }
    );
    assert_eq!(
        fx.log().last().unwrap(),
        "ERROR: yt-dlp failed (1): ERROR: Video unavailable"
    );
    // The failed download left nothing behind, and "Try again" from the audio has nothing to work on.
    assert!(!fx.exists("source.m4a.part"));
    assert_eq!(fx.meta().media, "");
    let stuck = run_job(&failing, &fx, StartAt::Audio).await;
    assert_eq!(stuck.error().kind, ErrorKind::NoMedia);

    let mut working = FakeBackend::new();
    working.download = Ok(Downloaded {
        title: Some("A video".into()),
        file: fx.folder.join("source.m4a"),
    });
    working.download_writes = vec!["source.m4a".into()];
    let second = run_job(&working, &fx, StartAt::Download).await;

    assert_eq!(second.outcome, Outcome::Done { summary: true });
    assert_eq!(working.calls()[0], "download");
    assert_eq!(fx.meta().title, "A video");
    assert!(fx.exists("transcript.txt"));
}

#[tokio::test]
async fn pipeline_rejects_a_link_yt_dlp_must_not_get() {
    for link in [
        "-x",
        "--exec=rm",
        "ftp://example.com/a",
        "javascript:alert(1)",
        "https://a b",
    ] {
        let fx = Fixture::link(link);
        let backend = FakeBackend::new();

        let ran = run_job(&backend, &fx, StartAt::Download).await;

        assert_eq!(
            ran.error(),
            &MeetingError {
                kind: ErrorKind::Other,
                key: key::INVALID_LINK,
                detail: None
            },
            "{link}"
        );
        assert!(backend.calls().is_empty(), "{link}");
    }
}

#[tokio::test]
async fn pipeline_missing_ytdlp_names_the_install_command() {
    let fx = Fixture::link("https://youtu.be/abc");
    let mut backend = FakeBackend::new();
    backend.download = Err(LinkError::Spawn("No such file or directory".into()));
    let command = install_command(Tool::YtDlp);

    let ran = run_job(&backend, &fx, StartAt::Download).await;

    assert_eq!(
        ran.error(),
        &MeetingError {
            kind: ErrorKind::MissingTool {
                command: command.into()
            },
            key: key::YTDLP_MISSING,
            detail: Some("No such file or directory".into())
        }
    );
    assert_eq!(
        fx.log().last().unwrap(),
        &format!("ERROR: yt-dlp isn't installed. Run: {command}")
    );
}

#[tokio::test]
async fn pipeline_download_falls_back_to_the_only_source_file() {
    let fx = Fixture::link("https://youtu.be/abc");
    let mut backend = FakeBackend::new();
    // yt-dlp named a file that is not there (it printed the name before a move); the one it left is used.
    backend.download = Ok(Downloaded {
        title: None,
        file: fx.folder.join("source.mp4"),
    });
    backend.download_writes = vec!["source.webm".into(), "source.webm.part".into()];

    let ran = run_job(&backend, &fx, StartAt::Download).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    assert_eq!(
        fx.meta().media,
        fx.folder.join("source.webm").to_string_lossy()
    );
    // No title came with it: the link stays the title.
    assert_eq!(fx.meta().title, "https://youtu.be/abc");
}

#[tokio::test]
async fn pipeline_download_with_no_clear_file_is_no_media() {
    for files in [vec![], vec!["source.webm", "source.m4a"]] {
        let fx = Fixture::link("https://youtu.be/abc");
        let mut backend = FakeBackend::new();
        backend.download = Ok(Downloaded {
            title: Some("A video".into()),
            file: fx.folder.join("source.mp4"),
        });
        backend.download_writes = files.iter().map(|f| (*f).to_owned()).collect();

        let ran = run_job(&backend, &fx, StartAt::Download).await;

        assert_eq!(ran.error().kind, ErrorKind::NoMedia, "{files:?}");
        assert_eq!(fx.meta().media, "");
        assert!(!backend.calls().contains(&"prepare_audio".to_owned()));
    }
}

// ---- cancelling ----

/// Cancels the run once `step` has started, then lets that call go: a result that arrives after the cancel.
async fn cancel_at(step: &str) -> (Fixture, FakeBackend, Ran, GateFlag, LocalLock) {
    let fx = if step == "download" {
        Fixture::link("https://youtu.be/abc")
    } else {
        Fixture::file("Standup.mp3")
    };
    let mut backend = match step {
        "api" => api_backend(),
        _ => FakeBackend::new(),
    };
    backend.diarize_enabled = step == "diarize";
    backend.download_writes = vec!["source.m4a.part".into()];
    backend.api_writes = vec!["api-parts/en-480s/0.json".into()];
    let (hold, control) = gate();
    match step {
        "download" => backend.gates.download = Some(hold),
        "prepare" => backend.gates.prepare = Some(hold),
        "local" => backend.gates.local = Some(hold),
        "diarize" => backend.gates.diarize = Some(hold),
        "api" => backend.gates.api = Some(hold),
        "summarize" => backend.gates.summarize = Some(hold),
        other => panic!("no step {other}"),
    }
    let GateControl {
        started,
        release,
        finished,
    } = control;
    let lock = LocalLock::new();
    let cancel = CancellationToken::new();
    let from = if step == "download" {
        StartAt::Download
    } else {
        StartAt::Audio
    };
    let controller = async {
        started.await.unwrap();
        cancel.cancel();
        // The answer comes after the cancel. It may be sent, but it must go nowhere.
        let _ = release.send(());
    };
    let (ran, ()) =
        bounded(async { tokio::join!(run_with(&backend, &lock, &fx, from, &cancel), controller) })
            .await;
    (fx, backend, ran, GateFlag(finished), lock)
}

struct GateFlag(Arc<std::sync::atomic::AtomicBool>);

#[tokio::test]
async fn pipeline_cancel_discards_late_results() {
    let (fx, _backend, ran, finished, _lock) = cancel_at("summarize").await;

    assert_eq!(
        ran.error(),
        &MeetingError {
            kind: ErrorKind::Cancelled,
            key: key::CANCELLED,
            detail: None
        }
    );
    assert!(
        !fx.exists("summary.md"),
        "a summary that came after the cancel was written"
    );
    // The transcript was finished before the summary, and stays.
    assert!(fx.exists("transcript.txt"));
    assert_eq!(fx.log().last().unwrap(), "ERROR: Cancelled");
    assert!(
        !finished.0.load(Ordering::SeqCst),
        "the call was left to finish"
    );
    assert_eq!(
        ran.events.last(),
        Some(&JobEvent::Finished(ran.outcome.clone()))
    );
}

#[tokio::test]
async fn pipeline_cancel_stops_each_step_without_writing_anything_more() {
    for (step, absent) in [
        ("download", "meta.media"),
        ("prepare", "meta.duration"),
        ("local", "transcript.txt"),
        ("diarize", "transcript.txt"),
        ("api", "transcript.txt"),
        ("summarize", "summary.md"),
    ] {
        let (fx, _backend, ran, finished, lock) = cancel_at(step).await;

        assert_eq!(ran.error().kind, ErrorKind::Cancelled, "{step}");
        assert_eq!(fx.log().last().unwrap(), "ERROR: Cancelled", "{step}");
        assert!(!finished.0.load(Ordering::SeqCst), "{step}");
        match absent {
            "meta.media" => assert_eq!(fx.meta().media, "", "{step}"),
            "meta.duration" => assert_eq!(fx.meta().duration, None, "{step}"),
            file => assert!(!fx.exists(file), "{step}: {file}"),
        }
        // Whatever the step, the lock of the local transcription is free again.
        assert!(lock.try_acquire().is_some(), "{step}");
    }
}

#[tokio::test]
async fn pipeline_cancel_removes_the_partial_download_but_keeps_api_parts() {
    let (fx, _backend, _ran, _finished, _lock) = cancel_at("download").await;
    assert!(!fx.exists("source.m4a.part"));

    // The finished parts of an API run are what the next run continues from.
    let (fx, _backend, _ran, _finished, _lock) = cancel_at("api").await;
    assert!(fx.exists("api-parts/en-480s/0.json"));
}

#[tokio::test]
async fn pipeline_result_that_arrives_with_the_cancel_is_discarded() {
    for (call, absent) in [
        ("summarize", "summary.md"),
        ("transcribe_local", "transcript.txt"),
        ("prepare_audio", "meta.duration"),
    ] {
        let fx = Fixture::file("Standup.mp3");
        let cancel = CancellationToken::new();
        let mut backend = FakeBackend::new();
        backend.cancel_in = Some((call, cancel.clone()));

        let ran = run_with(&backend, &LocalLock::new(), &fx, StartAt::Audio, &cancel).await;

        assert_eq!(ran.error().kind, ErrorKind::Cancelled, "{call}");
        match absent {
            "meta.duration" => assert_eq!(fx.meta().duration, None),
            file => assert!(!fx.exists(file), "{call}: {file}"),
        }
    }
}

#[tokio::test]
async fn pipeline_cancelled_before_it_starts_does_nothing() {
    let fx = Fixture::file("Standup.mp3");
    // A run from scratch clears the log, but only a run that starts: the history of one that never did stays.
    fx.write("log.txt", "ERROR: an earlier failure\n");
    let backend = FakeBackend::new();
    let cancel = CancellationToken::new();
    cancel.cancel();

    let ran = run_with(&backend, &LocalLock::new(), &fx, StartAt::Audio, &cancel).await;

    assert_eq!(ran.error().kind, ErrorKind::Cancelled);
    assert!(backend.calls().is_empty());
    assert_eq!(fx.log(), ["ERROR: an earlier failure", "ERROR: Cancelled"]);
}

#[tokio::test]
async fn pipeline_cancel_while_waiting_for_the_lock() {
    let fx = Fixture::file("Standup.mp3");
    let backend = FakeBackend::new();
    let lock = LocalLock::new();
    let held = lock.try_acquire().unwrap();
    let cancel = CancellationToken::new();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let run_it = async {
        let emit = |event: JobEvent| {
            if let JobEvent::Log(line) = event {
                let _ = tx.send(line);
            }
        };
        run(&backend, &lock, &fx.job, StartAt::Audio, &cancel, &emit).await
    };
    let controller = async {
        while rx.recv().await.unwrap() != WAITING {}
        cancel.cancel();
    };
    let (outcome, ()) = bounded(async { tokio::join!(run_it, controller) }).await;

    assert_eq!(
        outcome,
        Outcome::Failed(MeetingError {
            kind: ErrorKind::Cancelled,
            key: key::CANCELLED,
            detail: None
        })
    );
    assert!(!backend
        .calls()
        .iter()
        .any(|call| call.starts_with("transcribe_local")));
    // The one who waited is out of the queue: the lock is the holder's to give up, and then it is free.
    drop(held);
    assert!(lock.try_acquire().is_some());
}

// ---- the log and the files ----

#[tokio::test]
async fn pipeline_log_is_english() {
    let fx = Fixture::link("https://youtu.be/abc");
    let lock = LocalLock::new();
    let held = lock.try_acquire().unwrap();
    let mut backend = FakeBackend::new();
    backend.download = Ok(Downloaded {
        title: Some("A video".into()),
        file: fx.folder.join("source.m4a"),
    });
    backend.download_writes = vec!["source.m4a".into()];
    let cancel = CancellationToken::new();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let run_it = async {
        let emit = |event: JobEvent| {
            if let JobEvent::Log(line) = event {
                let _ = tx.send(line);
            }
        };
        run(&backend, &lock, &fx.job, StartAt::Download, &cancel, &emit).await
    };
    let controller = async {
        while rx.recv().await.unwrap() != WAITING {}
        drop(held);
    };
    let (outcome, ()) = bounded(async { tokio::join!(run_it, controller) }).await;

    assert_eq!(outcome, Outcome::Done { summary: true });
    let transcript = fx.read("transcript.txt").unwrap();
    assert_eq!(
        fx.log(),
        [
            "Downloading audio from https://youtu.be/abc…".to_owned(),
            "Audio: 00:10".to_owned(),
            WAITING.to_owned(),
            "Language: en".to_owned(),
            format!(
                "Saved the transcript ({} characters).",
                transcript.chars().count()
            ),
            "Done: summary.md".to_owned(),
        ]
    );
}

#[tokio::test]
async fn pipeline_backend_log_lines_reach_log_txt() {
    let fx = Fixture::link("https://youtu.be/abc");
    let mut backend = FakeBackend::new();
    backend.download = Ok(Downloaded {
        title: Some("A video".into()),
        file: fx.folder.join("source.m4a"),
    });
    backend.download_writes = vec!["source.m4a".into()];
    // Blank lines are not lines.
    backend.download_logs = vec![
        "[youtube] abc: Downloading webpage".into(),
        String::new(),
        "   ".into(),
        "[info] abc: Downloading 1 format(s): 140".into(),
    ];
    backend.local_logs = vec![
        "Whisper on this computer · small…".into(),
        "Loading the Whisper model…".into(),
    ];
    backend.diarize_enabled = true;
    backend.diarize_logs = vec![
        "Downloading the speaker separation model… 10%".into(),
        "Downloading the speaker separation model… 20%".into(),
    ];

    let ran = run_job(&backend, &fx, StartAt::Download).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    let transcript = fx.read("transcript.txt").unwrap();
    assert_eq!(
        fx.log(),
        [
            "Downloading audio from https://youtu.be/abc…".to_owned(),
            "[youtube] abc: Downloading webpage".to_owned(),
            "[info] abc: Downloading 1 format(s): 140".to_owned(),
            "Audio: 00:10".to_owned(),
            "Whisper on this computer · small…".to_owned(),
            "Loading the Whisper model…".to_owned(),
            "Language: en".to_owned(),
            "Separating speakers…".to_owned(),
            "Downloading the speaker separation model… 10%".to_owned(),
            "Downloading the speaker separation model… 20%".to_owned(),
            format!(
                "Saved the transcript ({} characters).",
                transcript.chars().count()
            ),
            "Done: summary.md".to_owned(),
        ]
    );
    // The app is told the same lines.
    assert_eq!(ran.logs(), fx.log());
}

#[tokio::test]
async fn pipeline_caps_a_long_line_from_the_backend() {
    let fx = Fixture::file("Standup.mp3");
    for n in 0..700 {
        append_log(
            &fx.folder,
            &format!("earlier line {n:04} {}", "x".repeat(100)),
        )
        .unwrap();
    }
    let mut backend = FakeBackend::new();
    // Two bytes to a character, and more than `append_log` keeps in one line.
    backend.local_logs = vec!["é".repeat(100_000)];

    let ran = run_job(&backend, &fx, StartAt::Summary).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    let log = fx.read("log.txt").unwrap();
    assert!(log.contains("earlier line 0699"), "the history was wiped");
    let long = log.lines().find(|line| line.starts_with("éé")).unwrap();
    assert!(long.chars().count() <= 2_001, "{}", long.chars().count());
    assert!(long.ends_with('…'));
}

#[tokio::test]
async fn pipeline_logs_the_language_whisper_heard() {
    for (heard, expected) in [
        (Some("vi"), Some("Language: vi")),
        (Some(" en "), Some("Language: en")),
        (Some(""), None),
        (Some("  "), None),
        (None, None),
    ] {
        let fx = Fixture::file("Standup.mp3");
        let mut backend = FakeBackend::new();
        backend.local = Ok(TimedTranscript {
            lines: vec![line("Hello there", 0.0, 2.0)],
            language: heard.map(str::to_owned),
        });

        run_job(&backend, &fx, StartAt::Audio).await;

        let log = fx.log();
        let logged = log.iter().find(|l| l.starts_with("Language:")).cloned();
        assert_eq!(logged.as_deref(), expected, "{heard:?}");
        if expected.is_some() {
            // After the transcription, before the transcript is saved.
            let at = log.iter().position(|l| l.starts_with("Language:")).unwrap();
            assert!(log[at + 1].starts_with("Saved the transcript"), "{log:?}");
        }
    }
}

#[tokio::test]
async fn pipeline_api_does_not_log_a_language() {
    let fx = Fixture::file("Standup.mp3");
    let backend = api_backend();

    run_job(&backend, &fx, StartAt::Audio).await;

    assert!(!fx.log().iter().any(|l| l.starts_with("Language:")));
}

#[tokio::test]
async fn pipeline_api_fallback_restarts_the_transcript_progress() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = api_backend();
    backend.api_progress = vec![
        ApiProgress::Part {
            index: 1,
            count: 2,
            reused: false,
        },
        ApiProgress::Part {
            index: 2,
            count: 2,
            reused: false,
        },
    ];
    backend.api = Err(api_part_failed(2, 2, 1, server_error("boom")));
    backend.local_progress = vec![0.25];

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    // The API got to half; Whisper starts again from nothing, which says "preparing" first, so that the bar is
    // not made to go back.
    assert_progress(
        ran.progress(Step::Transcript),
        &[None, Some(0.0), Some(0.5), None, Some(0.25), Some(1.0)],
    );
}

#[tokio::test]
async fn pipeline_every_log_event_is_in_log_txt() {
    let fx = Fixture::file("Standup.mp3");
    let mut backend = FakeBackend::new();
    backend.diarize_enabled = true;
    backend.diarize = Err("no network".into());

    let ran = run_job(&backend, &fx, StartAt::Audio).await;

    assert!(ran.logs().len() >= 4, "{:?}", ran.logs());
    assert_eq!(ran.logs(), fx.log());
}

#[tokio::test]
async fn pipeline_keeps_created_as_found() {
    // A real date stays, and so does a key the app does not know.
    let fx = Fixture::file("Standup.mp3");
    let mut raw = fx.raw_meta();
    raw["created"] = serde_json::json!(1_731_063_600);
    raw["custom"] = serde_json::json!("keep me");
    fs::write(
        fx.folder.join("meta.json"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();

    run_job(&FakeBackend::new(), &fx, StartAt::Audio).await;

    let raw = fx.raw_meta();
    assert_eq!(raw["created"], serde_json::json!(1_731_063_600));
    assert_eq!(raw["custom"], "keep me");
    assert_eq!(raw["duration"], 10.0);
    assert_eq!(raw["title"], "Weekly sync");

    // No date at all: the library goes by the folder's name, and the run must not turn that into 1970.
    let fx = Fixture::file("Standup.mp3");
    let mut raw = fx.raw_meta();
    raw.as_object_mut().unwrap().remove("created");
    fs::write(
        fx.folder.join("meta.json"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();

    run_job(&FakeBackend::new(), &fx, StartAt::Audio).await;

    assert!(fx.raw_meta().get("created").is_none(), "{}", fx.raw_meta());
    assert_eq!(fx.raw_meta()["duration"], 10.0);
}

#[tokio::test]
async fn pipeline_makes_a_meta_when_there_is_none_without_inventing_a_date() {
    let fx = Fixture::link("https://youtu.be/abc");
    fs::remove_file(fx.folder.join("meta.json")).unwrap();
    let mut backend = FakeBackend::new();
    backend.download = Ok(Downloaded {
        title: Some("A video".into()),
        file: fx.folder.join("source.m4a"),
    });
    backend.download_writes = vec!["source.m4a".into()];

    let ran = run_job(&backend, &fx, StartAt::Download).await;

    assert_eq!(ran.outcome, Outcome::Done { summary: true });
    let raw = fx.raw_meta();
    assert_eq!(raw["title"], "A video");
    assert_eq!(raw["source"], "https://youtu.be/abc");
    assert!(raw.get("created").is_none(), "{raw}");
}

#[tokio::test]
async fn pipeline_caps_a_huge_error_and_keeps_the_log() {
    let fx = Fixture::file("Standup.mp3");
    // More than the log's limit, in lines: a wiped log would lose the last of them.
    for n in 0..700 {
        append_log(
            &fx.folder,
            &format!("earlier line {n:04} {}", "x".repeat(100)),
        )
        .unwrap();
    }
    let mut backend = FakeBackend::new();
    // Two bytes to a character: a cut by bytes would land inside one.
    backend.summary = Err("é".repeat(100_000));

    let ran = run_job(&backend, &fx, StartAt::Summary).await;
    let transcript_missing_so_it_ran_from_the_audio = ran.error();

    let detail = transcript_missing_so_it_ran_from_the_audio
        .detail
        .as_deref()
        .unwrap();
    assert!(detail.chars().count() <= 501, "{}", detail.chars().count());
    assert!(detail.starts_with("éé") && detail.ends_with('…'));
    let log = fx.read("log.txt").unwrap();
    assert!(log.contains("earlier line 0699"), "the history was wiped");
    let error = log.lines().last().unwrap();
    assert!(error.starts_with("ERROR: Summary failed: éé"), "{error}");
    assert!(error.chars().count() < 2_100, "{}", error.chars().count());
}

#[tokio::test]
async fn pipeline_caps_a_very_long_link_in_the_log() {
    // Valid, and long enough for one line to be more than `append_log` keeps: it would drop the history.
    let link = format!("https://example.com/{}", "a".repeat(100_000));
    let fx = Fixture::link(&link);
    for n in 0..700 {
        append_log(
            &fx.folder,
            &format!("earlier line {n:04} {}", "x".repeat(100)),
        )
        .unwrap();
    }
    let mut backend = FakeBackend::new();
    backend.download = Err(LinkError::Failed {
        code: Some(1),
        tail: "ERROR: Unsupported URL".into(),
    });

    let ran = run_job(&backend, &fx, StartAt::Download).await;

    assert_eq!(ran.error().key, key::YTDLP_FAILED);
    let log = fx.read("log.txt").unwrap();
    assert!(log.contains("earlier line 0699"), "the history was wiped");
    let downloading = log
        .lines()
        .find(|line| line.starts_with("Downloading audio from https://example.com/aaa"))
        .unwrap();
    assert!(
        downloading.chars().count() <= 2_001,
        "{}",
        downloading.chars().count()
    );
    assert!(downloading.ends_with('…'));
    // The same line went to the app.
    assert!(ran.logs().contains(&downloading.to_owned()));
}

#[tokio::test]
async fn pipeline_second_job_on_the_same_folder_is_refused_and_writes_nothing() {
    let fx = Fixture::file("Standup.mp3");
    let (hold, control) = gate();
    let GateControl {
        started, release, ..
    } = control;
    let mut first = FakeBackend::new();
    first.gates.prepare = Some(hold);
    let second = FakeBackend::new();
    let lock = LocalLock::new();
    let cancel = CancellationToken::new();

    let run_first = run_with(&first, &lock, &fx, StartAt::Audio, &cancel);
    let controller = async {
        started.await.unwrap();
        let refused = run_with(&second, &lock, &fx, StartAt::Audio, &cancel).await;
        release.send(()).unwrap();
        refused
    };
    let (first_ran, refused) = bounded(async { tokio::join!(run_first, controller) }).await;

    assert_eq!(refused.error().kind, ErrorKind::Other);
    assert_eq!(refused.error().key, key::OTHER);
    assert!(refused.events.is_empty(), "{:?}", refused.events);
    assert!(second.calls().is_empty());
    // The first job went on as if nothing had happened, and its log has none of the second's.
    assert_eq!(first_ran.outcome, Outcome::Done { summary: true });
    assert!(
        !fx.log().iter().any(|l| l.contains("ERROR")),
        "{:?}",
        fx.log()
    );

    // Once it is over the folder is free again.
    let again = run_job(&FakeBackend::new(), &fx, StartAt::Summary).await;
    assert_eq!(again.outcome, Outcome::Done { summary: true });
}

// ---- Send ----

fn assert_send<T: Send>(_: &T) {}

#[tokio::test]
async fn run_future_is_send_and_can_be_spawned() {
    let fx = Fixture::file("Standup.mp3");
    let backend = FakeBackend::new();
    let lock = LocalLock::new();
    let cancel = CancellationToken::new();
    let emit = |_: JobEvent| {};
    let future = run(&backend, &lock, &fx.job, StartAt::Audio, &cancel, &emit);
    assert_send(&future);
    drop(future);

    let backend = Arc::new(FakeBackend::new());
    let lock = Arc::new(LocalLock::new());
    let events = Arc::new(Mutex::new(Vec::new()));
    let job = fx.job.clone();
    let task = tokio::spawn({
        let (backend, lock, events) = (backend.clone(), lock.clone(), events.clone());
        async move {
            let cancel = CancellationToken::new();
            let emit = move |event: JobEvent| events.lock().unwrap().push(event);
            run(&*backend, &lock, &job, StartAt::Audio, &cancel, &emit).await
        }
    });

    assert_eq!(task.await.unwrap(), Outcome::Done { summary: true });
    assert!(!events.lock().unwrap().is_empty());
}

// ---- at startup ----

#[test]
fn startup_marks_unfinished_folders() {
    let dir = tempfile::tempdir().unwrap();
    let folder = |name: &str, files: &[(&str, &str)]| {
        let folder = dir.path().join(name);
        fs::create_dir_all(&folder).unwrap();
        for (file, text) in files {
            fs::write(folder.join(file), text).unwrap();
        }
        MeetingRecord::from_folder(&folder).unwrap()
    };

    let unfinished = MeetingError {
        kind: ErrorKind::Unfinished,
        key: key::UNFINISHED,
        detail: None,
    };
    assert_eq!(
        startup_status(&folder("2026-11-10 09-00-00", &[("meta.json", "{}")])),
        Some(unfinished.clone())
    );
    assert_eq!(
        startup_status(&folder("2026-11-10 09-00-01", &[])),
        Some(unfinished.clone())
    );
    // Nothing in it but the audio, the log and the rougher live captions of a screen recording.
    assert_eq!(
        startup_status(&folder(
            "2026-11-10 09-00-02",
            &[
                ("audio.m4a", "x"),
                ("log.txt", "ERROR: Cancelled\n"),
                ("live_transcript.txt", "[00:00] You: hi")
            ]
        )),
        Some(unfinished.clone())
    );
    assert_eq!(
        startup_status(&folder("2026-11-10 09-00-03", &[("transcript.txt", "")])),
        Some(unfinished)
    );

    assert_eq!(
        startup_status(&folder(
            "2026-11-10 09-00-04",
            &[("transcript.txt", "[00:00] hi")]
        )),
        None
    );
    assert_eq!(
        startup_status(&folder("2026-11-10 09-00-05", &[("summary.md", "# Hi")])),
        None
    );
    assert_eq!(
        startup_status(&folder(
            "2026-11-10 09-00-06",
            &[("transcript.txt", "[00:00] hi"), ("summary.md", "# Hi")]
        )),
        None
    );
}

#[test]
fn startup_sweep_removes_leftovers_directly_in_a_folder_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Cuộc họp 2026-11-10 09-00-00");
    fs::create_dir_all(folder.join("api-parts").join("en-480s")).unwrap();
    fs::create_dir_all(folder.join("nested")).unwrap();
    fs::create_dir_all(folder.join("a directory.tmp")).unwrap();
    let stale = [
        "meta.json.1234-0.tmp",
        "transcript.txt.1234-1.tmp",
        "source.m4a.part",
        "source.webm.part",
        "audio.m4a.part",
        // The `.part` file of an encoder has a nonce in its name.
        "audio.m4a.1a2b-0.part",
        "audio.m4a.1a2b-1.part",
        "api-slice-0.wav",
        "api-slice-480.wav",
        "UPPER.TMP",
    ];
    let kept = [
        "meta.json",
        "transcript.txt",
        "summary.md",
        "tasks.json",
        "log.txt",
        "audio.m4a",
        "source.m4a",
        "live_transcript.txt",
        "api-slice.wav",
        "slice.wav",
        "partial.txt",
        "api-parts/en-480s/0.json",
        "nested/old.tmp",
        "nested/source.m4a.part",
        "a directory.tmp/inside.txt",
    ];
    for file in stale.iter().chain(&kept) {
        fs::write(folder.join(file), b"x").unwrap();
    }

    let removed = sweep_stale_files(&folder);

    assert_eq!(removed, stale.len());
    for file in stale {
        assert!(!folder.join(file).exists(), "{file} is still there");
    }
    for file in kept {
        assert!(folder.join(file).exists(), "{file} was removed");
    }
    // A folder that is not there, or a file, is nothing to sweep.
    assert_eq!(sweep_stale_files(&dir.path().join("none")), 0);
    assert_eq!(sweep_stale_files(&folder.join("meta.json")), 0);
}
