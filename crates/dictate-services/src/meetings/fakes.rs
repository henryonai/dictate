//! A scripted [`MeetingBackend`] for the tests of the pipeline: what each call gives is a field to set, every call
//! is noted, and a call can be held at a [`gate`] until the test lets it go, so that the tests need no sleeping.
//!
//! Built for this crate's own tests, and for any other crate that turns on the `test-support` feature (the app's tests,
//! as a dev-dependency); no other build has it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use dictate_core::text::segments::Segment;
use dictate_core::text::whisper::{Timed, Turn};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::{MeetingBackend, Prepared};
use crate::media::decode::DecodeError;
use crate::media::ytdlp::{Downloaded, LinkError};
use crate::settings::Engine;
use crate::summary::SummaryProgress;
use crate::transcribe_api::{ApiProgress, LongApiError};
use crate::whisper::local::TimedTranscript;

/// The backend's side of a gate: a call that goes through it says it has started, then waits to be let go.
pub struct Gate {
    started: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    finished: Arc<AtomicBool>,
}

/// The test's side of a gate.
pub struct GateControl {
    /// Resolves when the call has started (and is held).
    pub started: oneshot::Receiver<()>,
    /// Lets the call go. Sending fails when the call is gone already: it was dropped.
    pub release: oneshot::Sender<()>,
    /// Whether the call got past the gate, which a call that was dropped while held never does.
    pub finished: Arc<AtomicBool>,
}

pub fn gate() -> (Gate, GateControl) {
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let finished = Arc::new(AtomicBool::new(false));
    (
        Gate {
            started: Mutex::new(Some(started_tx)),
            release: Mutex::new(Some(release_rx)),
            finished: finished.clone(),
        },
        GateControl {
            started: started_rx,
            release: release_tx,
            finished,
        },
    )
}

impl Gate {
    async fn pass(&self) {
        let started = lock(&self.started).take();
        if let Some(started) = started {
            let _ = started.send(());
        }
        let release = lock(&self.release).take();
        if let Some(release) = release {
            // A sender that is dropped lets the call go too.
            let _ = release.await;
        }
        self.finished.store(true, Ordering::SeqCst);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The gates of the calls that can be held.
#[derive(Default)]
pub struct Gates {
    pub download: Option<Gate>,
    pub prepare: Option<Gate>,
    pub local: Option<Gate>,
    pub diarize: Option<Gate>,
    pub api: Option<Gate>,
    pub summarize: Option<Gate>,
}

pub struct FakeBackend {
    pub engine: Engine,
    pub whisper_ready: bool,
    pub api_ready: bool,
    pub ai_ready: bool,
    pub diarize_enabled: bool,
    pub model: String,

    pub download: Result<Downloaded, LinkError>,
    /// Files the download leaves in the folder before it answers.
    pub download_writes: Vec<String>,
    pub download_progress: Vec<f32>,
    /// The lines the backend sends to `log` during the call, in order.
    pub download_logs: Vec<String>,
    pub prepare: Result<Prepared, DecodeError>,
    pub prepare_progress: Vec<f32>,
    pub local: Result<TimedTranscript, String>,
    pub local_progress: Vec<f32>,
    pub local_logs: Vec<String>,
    pub diarize: Result<Vec<Turn>, String>,
    pub diarize_progress: Vec<f32>,
    pub diarize_logs: Vec<String>,
    pub api: Result<Vec<Segment>, LongApiError>,
    pub api_progress: Vec<ApiProgress>,
    /// Files the API leaves in the folder before it answers, as its cache of parts does.
    pub api_writes: Vec<String>,
    pub summary: Result<String, String>,
    pub summary_progress: Vec<SummaryProgress>,

    pub gates: Gates,
    /// A call by this name cancels the token inside itself, then answers as scripted: a result that arrives after
    /// the cancel.
    pub cancel_in: Option<(&'static str, CancellationToken)>,
    /// A label and a list shared with other backends, which every start and end of a local transcription is noted
    /// in, to tell the order the jobs went in.
    pub trace: Option<(String, Arc<Mutex<Vec<String>>>)>,

    calls: Mutex<Vec<String>>,
    media_seen: Mutex<Vec<PathBuf>>,
    summarize_seen: Mutex<Vec<(String, bool, String)>>,
}

/// A line of Whisper's, without words.
pub fn line(text: &str, start: f64, end: f64) -> (Timed, Vec<Timed>) {
    (
        Timed {
            start,
            end,
            text: format!(" {text}"),
            speaker: None,
        },
        Vec::new(),
    )
}

/// What `FakeBackend::new` hears in its ten seconds of audio.
pub const SAMPLES: usize = 16_000 * 10;

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeBackend {
    /// A backend where everything works: Whisper is ready, the AI is set up, there is no API and no speaker
    /// separation. The download is for a file that is not there: set `download` for a test that needs the file.
    pub fn new() -> Self {
        FakeBackend {
            engine: Engine::Whisper,
            whisper_ready: true,
            api_ready: false,
            ai_ready: true,
            diarize_enabled: false,
            model: "whisper-1".into(),
            download: Ok(Downloaded {
                title: None,
                file: PathBuf::from("source.m4a"),
            }),
            download_writes: Vec::new(),
            download_progress: Vec::new(),
            download_logs: Vec::new(),
            prepare: Ok(Prepared {
                samples_16k: Arc::new(vec![0.0; SAMPLES]),
                duration: 10.0,
                m4a: Ok(()),
            }),
            prepare_progress: Vec::new(),
            local: Ok(TimedTranscript {
                lines: vec![line("Hello there", 0.0, 2.0)],
                language: Some("en".into()),
            }),
            local_progress: Vec::new(),
            local_logs: Vec::new(),
            diarize: Ok(Vec::new()),
            diarize_progress: Vec::new(),
            diarize_logs: Vec::new(),
            api: Ok(vec![Segment {
                start: 0.0,
                speaker: None,
                text: "Hello from the API".into(),
            }]),
            api_progress: Vec::new(),
            api_writes: Vec::new(),
            summary: Ok("# Weekly sync\n\n## Summary\nWe talked.".into()),
            summary_progress: Vec::new(),
            gates: Gates::default(),
            cancel_in: None,
            trace: None,
            calls: Mutex::new(Vec::new()),
            media_seen: Mutex::new(Vec::new()),
            summarize_seen: Mutex::new(Vec::new()),
        }
    }

    /// The calls made so far, by name, in order.
    pub fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }

    /// The media file of each `prepare_audio` call.
    pub fn media_seen(&self) -> Vec<PathBuf> {
        lock(&self.media_seen).clone()
    }

    /// The title, `untitled` and transcript of each `summarize` call.
    pub fn summarize_seen(&self) -> Vec<(String, bool, String)> {
        lock(&self.summarize_seen).clone()
    }

    fn call(&self, name: &str) {
        lock(&self.calls).push(name.to_owned());
        if let Some((label, cancel)) = &self.cancel_in {
            if name.split(' ').next() == Some(label) {
                cancel.cancel();
            }
        }
    }

    fn note(&self, event: &str) {
        if let Some((label, order)) = &self.trace {
            lock(order).push(format!("{label}:{event}"));
        }
    }

    fn write_files(folder: &Path, names: &[String]) {
        for name in names {
            let path = folder.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, b"x").unwrap();
        }
    }
}

#[async_trait]
impl MeetingBackend for FakeBackend {
    async fn download(
        &self,
        _url: &str,
        folder: &Path,
        _cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
        log: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<Downloaded, LinkError> {
        self.call("download");
        Self::write_files(folder, &self.download_writes);
        for p in &self.download_progress {
            on_progress(*p);
        }
        for line in &self.download_logs {
            log(line);
        }
        if let Some(gate) = &self.gates.download {
            gate.pass().await;
        }
        self.download.clone()
    }

    async fn prepare_audio(
        &self,
        media: &Path,
        _folder: &Path,
        _cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
    ) -> Result<Prepared, DecodeError> {
        self.call("prepare_audio");
        lock(&self.media_seen).push(media.to_path_buf());
        for p in &self.prepare_progress {
            on_progress(*p);
        }
        if let Some(gate) = &self.gates.prepare {
            gate.pass().await;
        }
        self.prepare.clone()
    }

    fn file_engine(&self) -> Engine {
        self.engine
    }

    fn whisper_ready(&self) -> bool {
        self.whisper_ready
    }

    fn api_ready(&self) -> bool {
        self.api_ready
    }

    fn ai_ready(&self) -> bool {
        self.ai_ready
    }

    fn diarize_enabled(&self) -> bool {
        self.diarize_enabled
    }

    fn transcribe_model(&self) -> String {
        self.model.clone()
    }

    async fn transcribe_local(
        &self,
        _samples: Arc<Vec<f32>>,
        words: bool,
        _cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
        log: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<TimedTranscript, String> {
        self.call(&format!("transcribe_local words={words}"));
        self.note("local_start");
        for line in &self.local_logs {
            log(line);
        }
        for p in &self.local_progress {
            on_progress(*p);
        }
        if let Some(gate) = &self.gates.local {
            gate.pass().await;
        }
        self.note("local_end");
        self.local.clone()
    }

    async fn diarize(
        &self,
        _samples: Arc<Vec<f32>>,
        _cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
        log: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<Vec<Turn>, String> {
        self.call("diarize");
        for line in &self.diarize_logs {
            log(line);
        }
        for p in &self.diarize_progress {
            on_progress(*p);
        }
        if let Some(gate) = &self.gates.diarize {
            gate.pass().await;
        }
        self.note("diarize_end");
        self.diarize.clone()
    }

    async fn transcribe_api(
        &self,
        _samples: Arc<Vec<f32>>,
        folder: &Path,
        _cancel: &CancellationToken,
        on_progress: &(dyn Fn(ApiProgress) + Send + Sync),
    ) -> Result<Vec<Segment>, LongApiError> {
        self.call("transcribe_api");
        Self::write_files(folder, &self.api_writes);
        for event in &self.api_progress {
            on_progress(event.clone());
        }
        if let Some(gate) = &self.gates.api {
            gate.pass().await;
        }
        self.api.clone()
    }

    async fn summarize(
        &self,
        title: &str,
        untitled: bool,
        transcript: &str,
        on_progress: &(dyn Fn(SummaryProgress) + Send + Sync),
    ) -> Result<String, String> {
        self.call("summarize");
        lock(&self.summarize_seen).push((title.to_owned(), untitled, transcript.to_owned()));
        for event in &self.summary_progress {
            on_progress(*event);
        }
        if let Some(gate) = &self.gates.summarize {
            gate.pass().await;
        }
        self.summary.clone()
    }
}
