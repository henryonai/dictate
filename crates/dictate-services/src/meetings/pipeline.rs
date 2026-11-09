//! Runs the steps of a recording, in this order: download (for a link), audio, transcript, summary.
//!
//! What each step leaves on disk is whole (every write is atomic) and stays when a later step fails, so that "Try
//! again" can start at the step that failed. `run` is the only writer of a recording's folder while it runs: all of
//! its writes go through `Job`, and a second `run` on the same folder is refused.
//!
//! Cancelling: every call into the backend is raced against the token and dropped when it is cancelled, whether or not
//! the backend itself notices the token, and what a call answers after the cancel is thrown away: nothing is
//! written once the token is cancelled.

use std::collections::BTreeSet;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use dictate_core::meetings::library::{files, MeetingRecord, Meta, SCREEN_SOURCE};
use dictate_core::meetings::{append_log, atomic_write, transcript_text};
use dictate_core::text::segments::Segment;
use dictate_core::text::timestamp;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::error::{cap, DETAIL_LIMIT};
use super::{
    ErrorKind, JobEvent, JobSpec, LocalLock, MeetingBackend, MeetingError, Outcome, StartAt, Step,
};
use crate::media::ytdlp::link_is_valid;
use crate::settings::Engine;
use crate::summary::{title_from_summary, SummaryProgress};
use crate::transcribe_api::ApiProgress;
use crate::whisper::long::timed_to_segments;

/// The rate of the samples the backend gives.
const SAMPLE_RATE: f64 = 16_000.0;

/// With speaker separation, the share of the transcript step that transcription takes: the rest is the separation.
const TRANSCRIPTION_SHARE: f32 = 0.85;

/// A container that says it lasts this much longer than the audio that could be read (in seconds, and as a share of
/// what it says) is damaged or cut short. The container's figure is a little more than what decodes even for a good
/// file, because it does not always take the encoder's delay off.
const DAMAGED_SECONDS: f64 = 3.0;
const DAMAGED_SHARE: f64 = 0.03;

/// The longest line of `log.txt`, in characters. `append_log` drops the whole history when the newest line is longer
/// than about 40 KB, and a line carries what a program printed (a link, an error page): no line is allowed to be that
/// long, whatever is in it.
const LOG_LINE_LIMIT: usize = 2_000;

/// The folder where the transcription API keeps the parts it has done.
const API_PARTS: &str = "api-parts";

/// The folders a `run` is working in now, so that two never work in the same one.
static ACTIVE: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// The right to write in a folder; gives it up when dropped, however the run ends (the future of a run that is
/// dropped included).
struct FolderClaim(PathBuf);

impl FolderClaim {
    fn take(folder: &Path) -> Option<FolderClaim> {
        let mut active = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
        active
            .insert(folder.to_path_buf())
            .then(|| FolderClaim(folder.to_path_buf()))
    }
}

impl Drop for FolderClaim {
    fn drop(&mut self) {
        ACTIVE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// Runs the steps of the recording `job`, from `from` on, and tells `emit` what happens. Whatever `emit` is told is in
/// `log.txt` too, when it is a `Log`. The last event is `Finished`, with what this returns, however the run ends.
///
/// Only one run may work on a folder at a time (the app keeps one job per recording): a second one is refused at once,
/// with an `Other` failure, without a word to `emit` (not even `Finished`: it is not a run of that job) or a write to
/// the folder.
///
/// The future is `Send`: it may be spawned.
pub async fn run(
    backend: &dyn MeetingBackend,
    lock: &LocalLock,
    job: &JobSpec,
    from: StartAt,
    cancel: &CancellationToken,
    emit: &(dyn Fn(JobEvent) + Sync),
) -> Outcome {
    let Some(_claim) = FolderClaim::take(&job.folder) else {
        return Outcome::Failed(MeetingError::other(
            "This recording is already being processed.",
        ));
    };
    let job = Job {
        backend,
        lock,
        spec: job,
        cancel,
        emit,
    };
    let outcome = match job.steps(from).await {
        Ok(summary) => Outcome::Done { summary },
        Err(error) => {
            job.log(format!("ERROR: {}", error.message()));
            Outcome::Failed(error)
        }
    };
    job.emit(JobEvent::Finished(outcome.clone()));
    outcome
}

/// Why a recording that is in the library is not done, when it is not: the app was closed (or crashed) before the
/// transcript or the summary was written. Both missing (or empty) is "Not finished processing"; a transcript alone
/// is a recording that has no summary, which is not a failure. Only `transcript.txt` and `summary.md` count: the
/// rougher live captions of a screen recording do not.
pub fn startup_status(record: &MeetingRecord) -> Option<MeetingError> {
    let has = |file: &str| {
        fs::metadata(record.folder.join(file)).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
    };
    (!has(files::TRANSCRIPT) && !has(files::SUMMARY)).then(MeetingError::unfinished)
}

/// Removes what a job that was cut short (a quit, a crash) left in a recording's folder: the temporary files of
/// atomic writes (`*.tmp`), the unfinished downloads and audio (`*.part`, such as `source.m4a.part` and
/// `audio.m4a.part`) and the slices of audio the API was sent (`api-slice-*.wav`). Only files directly in `folder`:
/// the cache of API parts, which a second run continues from, is left alone, and so is anything in another folder.
/// Returns how many were removed.
///
/// For the start of the app, before any job runs: a job that is running has such files in its folder.
pub fn sweep_stale_files(folder: &Path) -> usize {
    files_in(folder, is_stale)
        .into_iter()
        .filter(|path| fs::remove_file(path).is_ok())
        .count()
}

/// The files (not folders, not links) directly in `folder` whose name `wanted` accepts.
fn files_in(folder: &Path, wanted: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| wanted(&entry.file_name().to_string_lossy().to_lowercase()))
        .map(|entry| entry.path())
        .collect()
}

/// A name that is left over from a job that did not end (lower case).
fn is_stale(name: &str) -> bool {
    name.ends_with(".tmp")
        || name.ends_with(".part")
        || (name.starts_with("api-slice-") && name.ends_with(".wav"))
}

/// What a download leaves when it does not end (lower case).
fn is_partial_download(name: &str) -> bool {
    name.starts_with("source.") && name.ends_with(".part")
}

/// `fraction` as a share of a step: 0 to 1, whatever the backend says.
fn share(fraction: f32) -> f32 {
    if fraction.is_nan() {
        0.0
    } else {
        fraction.clamp(0.0, 1.0)
    }
}

/// The name of the media file without its extension: what a recording is called until its summary names it.
fn file_stem(source: &str) -> Option<String> {
    Path::new(source)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}

/// Whether the recording has no title of its own yet: a screen recording, or one named after its file.
fn is_untitled(meta: &Meta) -> bool {
    meta.source == SCREEN_SOURCE || file_stem(&meta.source).is_some_and(|stem| stem == meta.title)
}

/// The first line of a message (a failed part says on the second what was kept).
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

/// One run: the backend and the recording it works on.
struct Job<'a> {
    backend: &'a dyn MeetingBackend,
    lock: &'a LocalLock,
    spec: &'a JobSpec,
    cancel: &'a CancellationToken,
    emit: &'a (dyn Fn(JobEvent) + Sync),
}

impl Job<'_> {
    fn folder(&self) -> &Path {
        &self.spec.folder
    }

    fn emit(&self, event: JobEvent) {
        (self.emit)(event);
    }

    fn step(&self, step: Step, progress: Option<f32>) {
        self.emit(JobEvent::Step { step, progress });
    }

    /// Writes a line to `log.txt` and tells `emit`. A log that cannot be written is no reason to fail the job.
    fn log(&self, line: impl AsRef<str>) {
        let line = cap(line.as_ref(), LOG_LINE_LIMIT);
        let _ = append_log(self.folder(), &line);
        self.emit(JobEvent::Log(line));
    }

    /// A line the backend wants in the log: as one of the pipeline's own, unless it is blank.
    fn backend_log(&self, line: &str) {
        if !line.trim().is_empty() {
            self.log(line);
        }
    }

    /// Fails with `Cancelled` when the job is cancelled: before every step and every write.
    fn checkpoint(&self) -> Result<(), MeetingError> {
        if self.cancel.is_cancelled() {
            Err(MeetingError::cancelled())
        } else {
            Ok(())
        }
    }

    /// `call`, unless the job is cancelled first: it is dropped then. An answer that comes after the cancel (at the
    /// same moment, or by the time this looks) is dropped too.
    async fn guarded<T>(&self, call: impl Future<Output = T>) -> Result<T, MeetingError> {
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(MeetingError::cancelled()),
            answer = call => {
                self.checkpoint()?;
                Ok(answer)
            }
        }
    }

    /// Writes a file of the recording, whole or not at all, unless the job is cancelled.
    fn write(&self, file: &str, bytes: &[u8]) -> Result<(), MeetingError> {
        self.checkpoint()?;
        atomic_write(&self.folder().join(file), bytes)
            .map_err(|e| MeetingError::other(format!("Couldn't write {file}: {e}")))
    }

    /// `meta.json` as the library reads it; a recording without one has its folder's name for a title.
    fn meta(&self) -> Meta {
        Meta::read(self.folder()).unwrap_or_else(|| Meta {
            title: self.spec.id.clone(),
            source: self.spec.link.clone().unwrap_or_default(),
            ..Meta::default()
        })
    }

    /// Changes some fields of `meta.json` and leaves all else in it as it was: a `created` that is not there stays
    /// missing (the library then goes by the name of the folder), and a key this version does not know stays too.
    /// Without a readable `meta.json` there is a new one with the title, the source and what is changed.
    fn patch_meta(&self, changes: &[(&str, Value)]) -> Result<(), MeetingError> {
        let path = self.folder().join(files::META);
        let found = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let mut fields = match found {
            Some(Value::Object(fields)) => fields,
            _ => {
                let meta = self.meta();
                let mut fields = Map::new();
                fields.insert("title".into(), meta.title.into());
                fields.insert("source".into(), meta.source.into());
                fields.insert("media".into(), "".into());
                fields
            }
        };
        for (name, value) in changes {
            fields.insert((*name).to_owned(), value.clone());
        }
        let json = serde_json::to_vec_pretty(&Value::Object(fields))
            .map_err(|e| MeetingError::other(format!("Couldn't write meta.json: {e}")))?;
        self.write(files::META, &json)
    }

    /// Whether the folder has `file`, and something in it.
    fn has(&self, file: &str) -> bool {
        fs::metadata(self.folder().join(file)).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
    }

    /// The steps from `from` on. Whether a summary was written.
    async fn steps(&self, from: StartAt) -> Result<bool, MeetingError> {
        // A run that is cancelled before it starts leaves the folder as it was, the log included.
        self.checkpoint()?;
        let mut start = from;
        // "Summarize again" with nothing to summarize is "Transcribe + summarize again"; and a recording that is not
        // a link has nothing to download.
        if start == StartAt::Summary && !self.has(files::TRANSCRIPT) {
            start = StartAt::Audio;
        }
        if start == StartAt::Download && self.spec.link.is_none() {
            start = StartAt::Audio;
        }
        // The log of a run from scratch is the log of that run. (Not when it only fell back to this.)
        if from == StartAt::Audio {
            let _ = atomic_write(&self.folder().join(files::LOG), b"");
        }

        if start == StartAt::Download {
            if let Some(link) = &self.spec.link {
                self.download(link).await?;
            }
        }
        let transcript = if start == StartAt::Summary {
            fs::read_to_string(self.folder().join(files::TRANSCRIPT))
                .map_err(|e| MeetingError::other(format!("Couldn't read transcript.txt: {e}")))?
        } else {
            let samples = self.audio().await?;
            self.transcript(samples).await?
        };
        self.summary(&transcript).await
    }

    // ---- 1. download ----

    async fn download(&self, link: &str) -> Result<(), MeetingError> {
        self.checkpoint()?;
        let url = link.trim();
        if !link_is_valid(url) {
            return Err(MeetingError::invalid_link());
        }
        self.step(Step::Download, None);
        self.log(format!("Downloading audio from {url}…"));
        let progress = |fraction: f32| self.step(Step::Download, Some(share(fraction)));
        let log = |line: &str| self.backend_log(line);
        let answer = self
            .guarded(
                self.backend
                    .download(url, self.folder(), self.cancel, &progress, &log),
            )
            .await;
        let downloaded = match answer {
            Ok(Ok(downloaded)) => downloaded,
            Ok(Err(error)) => {
                self.discard_partial_download();
                return Err(MeetingError::from_link(error));
            }
            Err(cancelled) => {
                self.discard_partial_download();
                return Err(cancelled);
            }
        };

        // The file yt-dlp names is the one; if it is not there, the one `source.*` that is.
        let file = if downloaded.file.is_file() {
            downloaded.file
        } else {
            let mut sources = files_in(self.folder(), |name| {
                name.starts_with("source.") && !name.ends_with(".part")
            });
            match sources.len() {
                1 => sources.remove(0),
                _ => return Err(MeetingError::no_media()),
            }
        };
        let mut changes = vec![("media", Value::from(file.to_string_lossy().into_owned()))];
        if let Some(title) = downloaded.title.filter(|title| !title.trim().is_empty()) {
            changes.push(("title", Value::from(title)));
        }
        self.patch_meta(&changes)
    }

    /// A download that does not end (it failed, or was cancelled) leaves the part of the file it had.
    fn discard_partial_download(&self) {
        for part in files_in(self.folder(), is_partial_download) {
            let _ = fs::remove_file(part);
        }
    }

    // ---- 2. audio ----

    /// The recording's audio at 16 kHz, from its media file; and `audio.m4a` and the duration, on the way.
    async fn audio(&self) -> Result<Arc<Vec<f32>>, MeetingError> {
        self.checkpoint()?;
        self.step(Step::Audio, None);
        let media = self.meta().media;
        if media.is_empty() || !Path::new(&media).is_file() {
            return Err(MeetingError::no_media());
        }
        let progress = |fraction: f32| self.step(Step::Audio, Some(share(fraction)));
        let prepared = self
            .guarded(self.backend.prepare_audio(
                Path::new(&media),
                self.folder(),
                self.cancel,
                &progress,
            ))
            .await?
            .map_err(MeetingError::from_decode)?;

        let decoded = prepared.samples_16k.len() as f64 / SAMPLE_RATE;
        let claimed = prepared.duration;
        let duration = if claimed.is_finite() && claimed > 0.0 {
            claimed
        } else {
            decoded
        };
        if duration > 0.0 {
            self.patch_meta(&[("duration", Value::from(duration))])?;
            self.log(format!("Audio: {}", timestamp::format(duration)));
        }
        if duration - decoded > DAMAGED_SECONDS.max(duration * DAMAGED_SHARE) {
            self.log(format!(
                "The file seems damaged: it should last {} but the audio ends at {}. Continuing with what could be read.",
                timestamp::format(duration),
                timestamp::format(decoded)
            ));
        }
        if let Err(error) = &prepared.m4a {
            self.log(format!(
                "Couldn't write audio.m4a ({}) — continuing without it.",
                cap(error, DETAIL_LIMIT)
            ));
        }
        Ok(prepared.samples_16k)
    }

    // ---- 3. transcript ----

    /// Writes `transcript.txt` from the audio and gives its text. The audio is let go when this is done.
    async fn transcript(&self, samples: Arc<Vec<f32>>) -> Result<String, MeetingError> {
        self.checkpoint()?;
        self.step(Step::Transcript, None);
        let engine = self.backend.file_engine();
        let mut fell_back = false;
        let mut segments = None;

        if engine == Engine::Api && self.backend.api_ready() {
            match self.api_transcript(&samples).await {
                Ok(done) => segments = Some(done),
                Err(error) if error.kind == ErrorKind::Cancelled => return Err(error),
                Err(error) if self.backend.whisper_ready() => {
                    let reason = error.detail.as_deref().unwrap_or("unknown error");
                    self.log(format!(
                        "Transcription API failed ({}) → using Whisper on this computer.",
                        first_line(reason)
                    ));
                    fell_back = true;
                }
                Err(error) => return Err(error),
            }
        } else if self.backend.whisper_ready() {
            if engine == Engine::Api {
                self.log("No transcription API key → using Whisper on this computer.");
            }
        } else {
            return Err(MeetingError::no_model());
        }

        let segments = match segments {
            Some(segments) => segments,
            None => {
                if fell_back {
                    // The API got part of the way; this starts from nothing, and the bar is not to go back.
                    self.step(Step::Transcript, None);
                }
                self.local_transcript(&samples).await?
            }
        };
        if fell_back {
            // Whisper did it: the parts the API had done are of no use to anyone.
            let _ = fs::remove_dir_all(self.folder().join(API_PARTS));
        }

        let text = transcript_text(&segments);
        if text.is_empty() {
            return Err(MeetingError::empty_transcript());
        }
        self.write(files::TRANSCRIPT, text.as_bytes())?;
        self.log(format!(
            "Saved the transcript ({} characters).",
            text.chars().count()
        ));
        Ok(text)
    }

    async fn api_transcript(&self, samples: &Arc<Vec<f32>>) -> Result<Vec<Segment>, MeetingError> {
        let model = self.backend.transcribe_model();
        self.log(format!("API transcript: {model}…"));
        let progress = |event: ApiProgress| match event {
            ApiProgress::Part {
                index,
                count,
                reused,
            } => {
                let done = index.saturating_sub(1) as f32 / count.max(1) as f32;
                if reused {
                    self.log(format!(
                        "API {model}: part {index}/{count} — reusing the earlier result"
                    ));
                } else {
                    self.log(format!(
                        "API {model}: part {index}/{count} — {}%",
                        (done * 100.0).round()
                    ));
                }
                self.step(Step::Transcript, Some(share(done)));
            }
            ApiProgress::Retry {
                index,
                count,
                wait_s,
                error,
            } => self.log(format!(
                "Part {index}/{count} failed ({}) → retrying in {wait_s} seconds…",
                cap(first_line(&error), DETAIL_LIMIT)
            )),
            ApiProgress::CutOff { from, keeping } => {
                let at = timestamp::format(from);
                if keeping {
                    self.log(format!(
                        "The part from {at} was still cut off → keeping what was written in full."
                    ));
                } else {
                    self.log(format!(
                        "The part from {at} was cut off → splitting it in two and redoing it."
                    ));
                }
            }
        };
        let segments = self
            .guarded(self.backend.transcribe_api(
                samples.clone(),
                self.folder(),
                self.cancel,
                &progress,
            ))
            .await?
            .map_err(MeetingError::from_api)?;
        // The last part has no event of its own to say it is done.
        self.log(format!("API {model}: 100%"));
        self.step(Step::Transcript, Some(1.0));
        Ok(segments)
    }

    /// Whisper on this computer, one recording at a time, and the speaker separation if it is on.
    async fn local_transcript(
        &self,
        samples: &Arc<Vec<f32>>,
    ) -> Result<Vec<Segment>, MeetingError> {
        let turn = match self.lock.try_acquire() {
            Some(turn) => turn,
            None => {
                self.log("Waiting for another recording to finish transcribing…");
                self.guarded(self.lock.acquire()).await?
            }
        };
        let diarize = self.backend.diarize_enabled();
        let whisper_share = if diarize { TRANSCRIPTION_SHARE } else { 1.0 };
        let progress =
            |fraction: f32| self.step(Step::Transcript, Some(whisper_share * share(fraction)));
        let log = |line: &str| self.backend_log(line);
        let heard = self
            .guarded(self.backend.transcribe_local(
                samples.clone(),
                diarize,
                self.cancel,
                &progress,
                &log,
            ))
            .await?
            .map_err(MeetingError::other)?;
        if let Some(language) = heard.language.as_deref().map(str::trim) {
            if !language.is_empty() {
                self.log(format!("Language: {language}"));
            }
        }

        let mut turns = None;
        if diarize {
            self.log("Separating speakers…");
            // The separation says nothing until its second half: that is "preparing".
            self.step(Step::Transcript, None);
            let progress = |fraction: f32| {
                self.step(
                    Step::Transcript,
                    Some(TRANSCRIPTION_SHARE + (1.0 - TRANSCRIPTION_SHARE) * share(fraction)),
                );
            };
            let log = |line: &str| self.backend_log(line);
            let separated = self
                .guarded(
                    self.backend
                        .diarize(samples.clone(), self.cancel, &progress, &log),
                )
                .await?;
            match separated {
                Ok(found) => turns = Some(found),
                Err(error) => self.log(format!(
                    "Couldn't separate speakers ({}); the transcript has no speaker names.",
                    cap(&error, DETAIL_LIMIT)
                )),
            }
        }
        drop(turn);
        // Not every run says it is done: audio with nothing to tell apart has no calls at all.
        self.step(Step::Transcript, Some(1.0));
        let seconds = samples.len() as f64 / SAMPLE_RATE;
        Ok(timed_to_segments(heard.lines, turns.as_deref(), seconds))
    }

    // ---- 4. summary ----

    async fn summary(&self, transcript: &str) -> Result<bool, MeetingError> {
        self.checkpoint()?;
        if !self.backend.ai_ready() {
            self.log("API not set up in Settings — transcript only.");
            return Ok(false);
        }
        // The short summary is one request, and says nothing before it is over.
        self.step(Step::Summary, None);
        let meta = self.meta();
        let untitled = is_untitled(&meta);
        let progress = |event: SummaryProgress| match event {
            SummaryProgress::Part { index, count } => {
                self.log(format!("Summarizing part {index}/{count}…"));
                let done = index.saturating_sub(1) as f32 / count.max(1) as f32;
                self.step(Step::Summary, Some(share(done * 0.9)));
            }
            SummaryProgress::Merging => {
                self.log("Merging the parts…");
                self.step(Step::Summary, Some(0.9));
            }
        };
        let summary = self
            .guarded(
                self.backend
                    .summarize(&meta.title, untitled, transcript, &progress),
            )
            .await?
            .map_err(|error| MeetingError::summary_failed(&error))?;
        if summary.trim().is_empty() {
            return Err(MeetingError::summary_failed("The API returned no content."));
        }

        self.write(files::SUMMARY, summary.as_bytes())?;
        // The ticks belong to the summary that was just replaced.
        if let Some(record) = MeetingRecord::from_folder(self.folder()) {
            if let Err(error) = record.clear_tasks() {
                self.log(format!(
                    "Couldn't clear the ticked action items ({}).",
                    cap(&error.to_string(), DETAIL_LIMIT)
                ));
            }
        }
        if untitled {
            if let Some(title) = title_from_summary(&summary) {
                self.patch_meta(&[("title", Value::from(title.clone()))])?;
                self.emit(JobEvent::TitleChanged(title));
            }
        }
        self.log("Done: summary.md");
        Ok(true)
    }
}
