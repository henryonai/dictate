//! The meeting pipeline: a link or a file in, `transcript.txt` and `summary.md` out. The steps are download (links),
//! audio, transcript and summary, in this order, and [`pipeline::run`] runs them for one recording.
//!
//! Everything that touches the outside world (yt-dlp, the decoder and the audio encoder, Whisper, the speaker
//! separation, the transcription API, the AI model, the settings) is behind [`MeetingBackend`], so the pipeline runs on
//! fakes. The app implements the trait with the real clients.
//!
//! The callbacks the trait methods take are `&(dyn Fn(T) + Send + Sync)`: they are shared, not owned, so a backend
//! that works on another thread hands them on to a scoped thread or reports through a channel. A `log` callback takes a
//! line for `log.txt`, in English: the pipeline writes it there, capped and without the empty ones, as it does its own.
//! (It is spelled `for<'a> Fn(&'a str)`: written as `Fn(&str)`, `#[async_trait]` ties the `&str` to the lifetimes of
//! the call, and a backend could not hand it a line of its own.) The cancel token is
//! `&CancellationToken`; a backend clones it where it needs an owned one. The pipeline does not rely on a backend to
//! notice the token: it races every call against it and drops the call on cancel (see `pipeline`).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use dictate_core::text::segments::Segment;
use dictate_core::text::whisper::Turn;
use tokio::sync::MutexGuard;
use tokio_util::sync::CancellationToken;

use crate::media::decode::DecodeError;
use crate::media::ytdlp::{Downloaded, LinkError};
use crate::settings::Engine;
use crate::summary::SummaryProgress;
use crate::transcribe_api::{ApiProgress, LongApiError};
use crate::whisper::local::TimedTranscript;

pub mod error;
pub mod pipeline;

/// A scripted [`MeetingBackend`], for the tests of the pipeline and of whatever runs it: with the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub mod fakes;
#[cfg(test)]
mod tests;

pub use error::{ErrorKind, MeetingError};
pub use pipeline::{run, startup_status, sweep_stale_files};

/// The steps of a job, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Download,
    Audio,
    Transcript,
    Summary,
}

/// Where a run begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartAt {
    /// The link is downloaded first. For a recording that has no link, or whose media is already on disk, use
    /// `Audio`.
    Download,
    /// Everything after the download, from scratch ("Transcribe + summarize again"): the log is cleared first.
    Audio,
    /// The summary only ("Summarize again"). A recording with no transcript yet starts at `Audio` instead.
    Summary,
}

/// The recording a run works on. It is the only writer of the folder while it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSpec {
    /// The folder name.
    pub id: String,
    pub folder: std::path::PathBuf,
    /// The link to download, for a recording that came from one.
    pub link: Option<String>,
}

/// What a run tells the app while it works.
#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    /// A step has started (`progress` is `None`, "working, no figure yet") or has got further (0 to 1).
    Step { step: Step, progress: Option<f32> },
    /// A line of `log.txt`, in English: it was written there before this was sent.
    Log(String),
    /// The recording got a title from its summary.
    TitleChanged(String),
    /// The last event of a run: what `run` returns. Every run that works says it, however it ends; the one that is
    /// refused because another run is working in the same folder does not, since it is not a run of that job.
    Finished(Outcome),
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The transcript is written, and the summary is too when `summary`: without an AI set up there is none.
    Done {
        summary: bool,
    },
    Failed(MeetingError),
}

/// What `MeetingBackend::prepare_audio` made of the media file.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    /// The audio as Whisper wants it: mono, 16 kHz.
    pub samples_16k: Arc<Vec<f32>>,
    /// How long the file is, in seconds, as the container says. When this is clearly more than the samples hold,
    /// the file was damaged or cut short.
    pub duration: f64,
    /// Whether `audio.m4a` was written; the text is why not. The job goes on without the file.
    pub m4a: Result<(), String>,
}

/// Everything the pipeline needs from the outside. The error strings are shown to the user (and written to the
/// log) as they are, so they must already be redacted: no key in them.
///
/// The real implementation decodes in a blocking task and streams the audio once: 16 kHz samples for Whisper and
/// the API, and 48 kHz blocks straight into `audio.m4a`, so that no 48 kHz copy of the recording is ever held.
#[async_trait]
pub trait MeetingBackend: Send + Sync {
    /// Downloads the audio of `url` into `folder` (as `source.<ext>`). `on_progress` hears 0 to 1.
    ///
    /// `log` is for the lines yt-dlp prints (`ytdlp::download`'s `on_line`), as the Swift app copied them. Not all of
    /// them: yt-dlp prints a progress line with every update (the `DL <bytes> <total> <estimate>` lines of the progress
    /// template, which `ytdlp::progress` reads), and hundreds of those would bury the log. The backend sends those to
    /// `on_progress` only, and everything else to `log`.
    async fn download(
        &self,
        url: &str,
        folder: &Path,
        cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
        log: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<Downloaded, LinkError>;

    /// Decodes `media` for the transcript and writes `<folder>/audio.m4a`. `on_progress` hears 0 to 1.
    async fn prepare_audio(
        &self,
        media: &Path,
        folder: &Path,
        cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
    ) -> Result<Prepared, DecodeError>;

    /// The engine chosen for files in the settings. The API is only used when it is chosen and `api_ready`.
    fn file_engine(&self) -> Engine;
    /// Whether the chosen Whisper model is downloaded.
    fn whisper_ready(&self) -> bool;
    /// Whether the transcription API has what it needs to run (its key and its model).
    fn api_ready(&self) -> bool;
    /// Whether the AI model that writes summaries has what it needs to run.
    fn ai_ready(&self) -> bool;
    /// Whether speakers are told apart (local transcripts only).
    fn diarize_enabled(&self) -> bool;
    /// The model of the transcription API, for the log.
    fn transcribe_model(&self) -> String;

    /// The lines Whisper hears in `samples` (16 kHz mono), with the times of their words when `words` is set.
    /// `on_progress` hears 0 to 1. `log` is for what the backend knows and the pipeline does not: which model runs
    /// (`Whisper on this computer · <model>…`) and that it is being loaded (`Loading the Whisper model…`). The pipeline
    /// logs the language itself, from `TimedTranscript::language`.
    async fn transcribe_local(
        &self,
        samples: Arc<Vec<f32>>,
        words: bool,
        cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
        log: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<TimedTranscript, String>;

    /// Who speaks when in `samples`. Downloading the models the first time is part of it. `on_progress` hears 0
    /// to 1, but only in the second half of the work, and not at all for audio with nothing to tell apart. `log` is
    /// for the download of the models, which says nothing in `on_progress` (for example every 10%).
    async fn diarize(
        &self,
        samples: Arc<Vec<f32>>,
        cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
        log: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<Vec<Turn>, String>;

    /// The transcript of `samples` through the transcription API, in parts cached in `folder`.
    async fn transcribe_api(
        &self,
        samples: Arc<Vec<f32>>,
        folder: &Path,
        cancel: &CancellationToken,
        on_progress: &(dyn Fn(ApiProgress) + Send + Sync),
    ) -> Result<Vec<Segment>, LongApiError>;

    /// The summary of `transcript` as Markdown. `untitled`: ask for a title too. `on_progress` is only told on the
    /// long path (a transcript of more than 100,000 characters, summarized in parts).
    async fn summarize(
        &self,
        title: &str,
        untitled: bool,
        transcript: &str,
        on_progress: &(dyn Fn(SummaryProgress) + Send + Sync),
    ) -> Result<String, String>;
}

/// One local transcription at a time, app-wide: Whisper and the speaker separation take the machine's whole
/// processor. Shared between the jobs; whoever asks first is served first (tokio's mutex queues its waiters).
#[derive(Debug, Default)]
pub struct LocalLock(tokio::sync::Mutex<()>);

impl LocalLock {
    pub fn new() -> Self {
        Self::default()
    }

    /// The lock, when nobody has it.
    pub fn try_acquire(&self) -> Option<MutexGuard<'_, ()>> {
        self.0.try_lock().ok()
    }

    /// Waits for the lock, behind everyone who asked before.
    pub async fn acquire(&self) -> MutexGuard<'_, ()> {
        self.0.lock().await
    }
}
