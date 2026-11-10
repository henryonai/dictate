//! The real [`MeetingBackend`]: what the pipeline asks of the outside world, answered with this machine's tools and the
//! app's settings.
//!
//! The blocking work (decoding, the encoder, Whisper, the speaker separation, the key store) runs on blocking threads,
//! and the pipeline's callbacks, which are borrowed and not `'static`, are called from the async side: what a blocking
//! thread has to say goes over a channel. The pipeline drops a call's future when the job is cancelled; the blocking
//! thread may go on (the speaker separation can't be stopped), and from then on what it says goes nowhere.
//!
//! Whisper and the separation take the machine's whole processor. The pipeline's `LocalLock` is released when a future
//! is dropped, so a job that was cancelled while its thread still runs would let the next one start next to it; every
//! such thread takes `LOCAL_GATE` first, and the next job's thread waits there for the orphan to end.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use dictate_core::meetings::library::files;
use dictate_core::text::segments::Segment;
use dictate_core::text::whisper::Turn;
use dictate_services::audio::resample::StreamResampler;
use dictate_services::diarize::{self, Diarizer};
use dictate_services::media::decode::{DecodeError, MonoDecoder};
use dictate_services::media::encode::{self, EncodeError, M4aSink, M4A_RATE};
use dictate_services::media::tools::Tools;
use dictate_services::media::ytdlp::{self, Downloaded, LinkError};
use dictate_services::meetings::{MeetingBackend, Prepared};
use dictate_services::secrets::{Account, SecretStore};
use dictate_services::settings::{self, Engine, Settings, SettingsStore};
use dictate_services::summary::{self, SummaryProgress};
use dictate_services::system;
use dictate_services::transcribe_api::{self, ApiProgress, BoxFuture, LongApiError};
use dictate_services::whisper::local::{LocalWhisper, TimedTranscript};
use dictate_services::whisper::models::{self, ModelDir};
use tokio_util::sync::CancellationToken;

use crate::commands::api_error_text;
use crate::config::{local_model, summary_ai_config, transcribe_config};
use crate::state::{AppState, KeyFlags, ModelCache, WhisperCache};

/// The speaker separation models in memory, until nobody has used them for a while.
pub type DiarizerCache = ModelCache<Diarizer>;

/// The way an `audio.m4a` is started: where it goes and the ffmpeg that may write it. `encode::m4a_writer`, or a stand-in in
/// the tests.
type StartSink<'a> = &'a dyn Fn(&Path, Option<&Path>) -> Result<Box<dyn M4aSink>, EncodeError>;

/// The id the one speaker separation model is kept under in its cache.
const DIARIZER: &str = "speaker-separation";

/// The rate Whisper and the API listen at, and the transcript's timing is counted in.
const RATE_16K: u32 = 16_000;

/// How much of a recording's length is made room for in the 16 kHz copy before it is decoded, at most (4 hours). A
/// copy that grows by doubling would hold half as much again while it is moved.
const RESERVE_LIMIT_S: f64 = 4.0 * 3600.0;

/// The share of the speaker separation that downloading its models takes, when they are missing: the separation itself
/// is the rest.
const DOWNLOAD_SHARE: f32 = 0.5;

/// One local run at a time, whatever the pipeline believes (see the top of the file). A thread that ends in a panic
/// leaves nothing half done: the gate is the same for the next one.
static LOCAL_GATE: Mutex<()> = Mutex::new(());

/// The right to use the processor for Whisper or the speaker separation. Call it on a blocking thread: it waits.
fn take_turn() -> MutexGuard<'static, ()> {
    LOCAL_GATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a blocking thread tells the async side while it works.
#[derive(Debug, Clone, PartialEq)]
enum Report {
    Progress(f32),
    /// A line for `log.txt`.
    Log(String),
}

/// Runs `work` on a blocking thread and hands every [`Report`] it makes to `on_report`, here on the async side, in order.
/// Resolves with what `work` returns. Dropping the future (the job was cancelled) stops the reports; the thread is not
/// stopped, it goes on to its end, and what it reports then is thrown away.
async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce(&dyn Fn(Report)) -> T + Send + 'static,
    mut on_report: impl FnMut(Report),
) -> Result<T, String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut thread = tauri::async_runtime::spawn_blocking(move || {
        work(&|report| {
            // The receiver is gone when the future was dropped: nobody is listening.
            let _ = tx.send(report);
        })
    });
    loop {
        tokio::select! {
            biased;
            Some(report) = rx.recv() => on_report(report),
            ended = &mut thread => {
                // What was said just before the end is still in the channel.
                while let Ok(report) = rx.try_recv() {
                    on_report(report);
                }
                return ended.map_err(|e| format!("The work stopped unexpectedly: {e}"));
            }
        }
    }
}

/// `run_blocking` for work that needs the processor to itself (Whisper, the speaker separation): the thread takes its turn
/// first, and a job that is cancelled while the thread before it still runs waits there for it.
async fn run_local<T: Send + 'static>(
    work: impl FnOnce(&dyn Fn(Report)) -> T + Send + 'static,
    on_report: impl FnMut(Report),
) -> Result<T, String> {
    run_blocking(
        move |report| {
            let _turn = take_turn();
            work(report)
        },
        on_report,
    )
    .await
}

/// Lets a progress figure through when it has gone up by a whole percent since the last one that went through: a
/// decoder reports with every packet, and the window needs not hear a thousand figures a second.
struct Throttle(AtomicI32);

impl Throttle {
    fn new() -> Self {
        Throttle(AtomicI32::new(-1))
    }

    /// `fraction` (0 to 1) when it is worth telling, else `None`.
    fn passes(&self, fraction: f32) -> Option<f32> {
        if fraction.is_nan() {
            return None;
        }
        let fraction = fraction.clamp(0.0, 1.0);
        let percent = (fraction * 100.0).floor() as i32;
        (self.0.fetch_max(percent, Ordering::SeqCst) < percent).then_some(fraction)
    }
}

/// Says "Downloading the speaker separation model… n%" at every tenth of the download (and at the start).
struct DownloadLog(AtomicI32);

impl DownloadLog {
    fn new() -> Self {
        DownloadLog(AtomicI32::new(-1))
    }

    /// The line to log for a download that has come `fraction` of the way, when it has passed a tenth since the last.
    fn line(&self, fraction: f32) -> Option<String> {
        let tenth = (fraction.clamp(0.0, 1.0) * 10.0).floor() as i32;
        (self.0.fetch_max(tenth, Ordering::SeqCst) < tenth)
            .then(|| format!("Downloading the speaker separation model… {}%", tenth * 10))
    }
}

/// Whether the settings and the saved keys give the AI model what it needs to write summaries.
pub fn ai_ready(settings: &Settings, keys: &KeyFlags) -> bool {
    settings::is_ai_configured(settings, keys.get(Account::Ai))
}

/// Whether the chosen (or recommended) Whisper model is downloaded.
pub fn whisper_ready(settings: &Settings, models: &ModelDir, ram_gb: u64) -> bool {
    local_model(settings, models, ram_gb).is_ok()
}

/// Whether the transcription API has what it needs: a model, an address and a key (its own, or the AI key when both
/// services are OpenRouter). Like the dictation's, this goes by the flags and never asks the key store.
pub fn api_ready(settings: &Settings, keys: &KeyFlags) -> bool {
    let (own, ai) = (keys.get(Account::Transcribe), keys.get(Account::Ai));
    (own || settings::shares_ai_key(settings, ai, own))
        && !settings.transcribe_model.trim().is_empty()
        && !settings.transcribe_base_url.trim().is_empty()
}

pub struct AppBackend {
    settings: Arc<SettingsStore>,
    secrets: Arc<dyn SecretStore>,
    keys: Arc<KeyFlags>,
    models: ModelDir,
    whisper: Arc<WhisperCache>,
    diarizer: Arc<DiarizerCache>,
    /// Where the speaker separation models are (and are downloaded to).
    diarize_dir: PathBuf,
    ram_gb: u64,
    /// Where ffmpeg and yt-dlp are: looked for each time, so that one installed while the app runs is found.
    tools: Arc<dyn Fn() -> Tools + Send + Sync>,
}

impl AppBackend {
    /// The backend of this app. `keys` is read through the flags only: `state.loaded_keys()` has to have run (see
    /// `AppState::loaded_keys`) before the first job, which the registry's start-up sees to.
    pub fn new(state: &AppState, diarizer: Arc<DiarizerCache>, diarize_dir: PathBuf) -> Self {
        Self {
            settings: state.settings.clone(),
            secrets: state.secrets.clone(),
            keys: state.keys.clone(),
            models: state.models.clone(),
            whisper: state.whisper.clone(),
            diarizer,
            diarize_dir,
            ram_gb: system::total_ram_gb(),
            tools: Arc::new(Tools::find),
        }
    }

    /// The same, finding the tools by `tools` (the tests give it stand-ins).
    #[cfg(test)]
    fn with_tools(mut self, tools: impl Fn() -> Tools + Send + Sync + 'static) -> Self {
        self.tools = Arc::new(tools);
        self
    }

    /// The tools, looked for on a blocking thread.
    async fn find_tools(&self) -> Result<Tools, String> {
        let tools = self.tools.clone();
        tauri::async_runtime::spawn_blocking(move || tools())
            .await
            .map_err(|e| format!("The work stopped unexpectedly: {e}"))
    }
}

#[async_trait]
impl MeetingBackend for AppBackend {
    async fn download(
        &self,
        url: &str,
        folder: &Path,
        cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
        log: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<Downloaded, LinkError> {
        // Only a yt-dlp that is not there is "not installed": the pipeline takes every `Spawn` for that.
        let tools = self.find_tools().await.map_err(LinkError::Spawn)?;
        let Some(yt_dlp) = tools.yt_dlp else {
            return Err(LinkError::Spawn("yt-dlp was not found.".into()));
        };
        download_with(&yt_dlp, url, folder, cancel, on_progress, log).await
    }

    async fn prepare_audio(
        &self,
        media: &Path,
        folder: &Path,
        cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
    ) -> Result<Prepared, DecodeError> {
        let (media, out) = (media.to_path_buf(), folder.join(files::AUDIO));
        let (cancel, find) = (cancel.clone(), self.tools.clone());
        run_blocking(
            move |report| {
                let ffmpeg = find().ffmpeg;
                let throttle = Throttle::new();
                prepare_blocking(
                    &media,
                    &out,
                    ffmpeg.as_deref(),
                    &cancel,
                    &|fraction| {
                        if let Some(fraction) = throttle.passes(fraction) {
                            report(Report::Progress(fraction));
                        }
                    },
                    &encode::m4a_writer,
                )
            },
            |report| {
                if let Report::Progress(fraction) = report {
                    on_progress(fraction);
                }
            },
        )
        .await
        .map_err(DecodeError::Io)?
    }

    fn file_engine(&self) -> Engine {
        self.settings.get().file_engine
    }

    fn whisper_ready(&self) -> bool {
        whisper_ready(&self.settings.get(), &self.models, self.ram_gb)
    }

    fn api_ready(&self) -> bool {
        api_ready(&self.settings.get(), &self.keys)
    }

    fn ai_ready(&self) -> bool {
        ai_ready(&self.settings.get(), &self.keys)
    }

    fn diarize_enabled(&self) -> bool {
        self.settings.get().diarize
    }

    fn transcribe_model(&self) -> String {
        self.settings.get().transcribe_model
    }

    async fn transcribe_local(
        &self,
        samples: Arc<Vec<f32>>,
        words: bool,
        cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
        log: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<TimedTranscript, String> {
        let settings = self.settings.get();
        let (id, path) = local_model(&settings, &self.models, self.ram_gb)?;
        log(&format!("Whisper on this computer · {id}…"));

        let cache = self.whisper.clone();
        let cancel = cancel.clone();
        let language = settings.transcribe_language.trim().to_owned();
        let vocabulary = settings.vocabulary;
        let throttle = Throttle::new();
        run_local(
            move |report| {
                // The turn can come a long time after the job was cancelled: a model is not loaded for nobody.
                if cancel.is_cancelled() {
                    return Err("The transcription was cancelled.".to_owned());
                }
                let model = cache.get(&id, || {
                    report(Report::Log("Loading the Whisper model…".into()));
                    LocalWhisper::load(&path).map_err(|e| e.to_string())
                })?;
                model
                    .transcribe_timed(
                        &samples,
                        (!language.is_empty()).then_some(language.as_str()),
                        &vocabulary,
                        words,
                        &cancel,
                        |fraction| {
                            if let Some(fraction) = throttle.passes(fraction) {
                                report(Report::Progress(fraction));
                            }
                        },
                    )
                    .map_err(|e| e.to_string())
            },
            |report| match report {
                Report::Progress(fraction) => on_progress(fraction),
                Report::Log(line) => log(&line),
            },
        )
        .await?
    }

    async fn diarize(
        &self,
        samples: Arc<Vec<f32>>,
        cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
        log: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<Vec<Turn>, String> {
        // Once for the job, and on a blocking thread: it reads both models (46 MB) to check them.
        let dir = self.diarize_dir.clone();
        let missing = tauri::async_runtime::spawn_blocking(move || diarize::missing_models(&dir))
            .await
            .map_err(|e| format!("The work stopped unexpectedly: {e}"))?;
        let downloaded = !missing.is_empty();
        if downloaded {
            self.download_models(&missing, cancel, on_progress, log)
                .await?;
        }

        let (cache, dir, cancel) = (
            self.diarizer.clone(),
            self.diarize_dir.clone(),
            cancel.clone(),
        );
        let throttle = Throttle::new();
        run_local(
            move |report| {
                if cancel.is_cancelled() {
                    return Err("The speaker separation was cancelled.".to_owned());
                }
                let diarizer =
                    cache.get(DIARIZER, || Diarizer::load(&dir).map_err(|e| e.to_string()))?;
                diarizer
                    .run(&samples, |fraction| {
                        if let Some(fraction) = throttle.passes(run_share(downloaded, fraction)) {
                            report(Report::Progress(fraction));
                        }
                    })
                    .map_err(|e| e.to_string())
            },
            |report| {
                if let Report::Progress(fraction) = report {
                    on_progress(fraction);
                }
            },
        )
        .await?
    }

    async fn transcribe_api(
        &self,
        samples: Arc<Vec<f32>>,
        folder: &Path,
        cancel: &CancellationToken,
        on_progress: &(dyn Fn(ApiProgress) + Send + Sync),
    ) -> Result<Vec<Segment>, LongApiError> {
        // The key store is read on a blocking thread: it can wait for the person to answer a prompt.
        let (settings, secrets) = (self.settings.clone(), self.secrets.clone());
        let config =
            tauri::async_runtime::spawn_blocking(move || transcribe_config(&settings, &*secrets))
                .await
                .map_err(|e| LongApiError::Io(format!("The work stopped unexpectedly: {e}")))?;
        let sleep =
            |wait: Duration| -> BoxFuture<'static, ()> { Box::pin(tokio::time::sleep(wait)) };
        transcribe_api::transcribe_long(&config, &samples, folder, cancel, &sleep, |event| {
            on_progress(event)
        })
        .await
    }

    async fn summarize(
        &self,
        title: &str,
        untitled: bool,
        transcript: &str,
        on_progress: &(dyn Fn(SummaryProgress) + Send + Sync),
    ) -> Result<String, String> {
        let (settings, secrets) = (self.settings.clone(), self.secrets.clone());
        let config =
            tauri::async_runtime::spawn_blocking(move || summary_ai_config(&settings, &*secrets))
                .await
                .map_err(|e| format!("The work stopped unexpectedly: {e}"))??;
        summary::summarize(&config, title, untitled, transcript, |event| {
            on_progress(event)
        })
        .await
        .map_err(|e| api_error_text(&e))
    }
}

impl AppBackend {
    /// Downloads the speaker separation models that are missing. The share of the work they take is
    /// `DOWNLOAD_SHARE`, and the log says how far it is at every tenth.
    async fn download_models(
        &self,
        missing: &[&'static models::RemoteFile],
        cancel: &CancellationToken,
        on_progress: &(dyn Fn(f32) + Send + Sync),
        log: &(dyn for<'a> Fn(&'a str) + Send + Sync),
    ) -> Result<(), String> {
        let progress = ModelProgress::new(missing);
        let mut before = 0;
        for model in missing {
            let report = |done: u64, _announced: Option<u64>| {
                let (shown, line) = progress.update(before, done);
                if let Some(line) = line {
                    log(&line);
                }
                if let Some(fraction) = shown {
                    on_progress(fraction);
                }
            };
            models::download_file(&self.diarize_dir, model, report, cancel.clone())
                .await
                .map_err(|e| format!("Could not download the speaker separation model: {e}"))?;
            before += model.size;
        }
        Ok(())
    }
}

/// How far the models have come, over all of the files that are being downloaded.
struct ModelProgress {
    total: u64,
    says: DownloadLog,
    throttle: Throttle,
}

impl ModelProgress {
    fn new(models: &[&models::RemoteFile]) -> Self {
        Self {
            total: models.iter().map(|model| model.size).sum(),
            says: DownloadLog::new(),
            throttle: Throttle::new(),
        }
    }

    /// What a download of `done` bytes of a model, after `before` bytes of the ones before it, tells: the progress of the
    /// speaker separation (the download is the first `DOWNLOAD_SHARE` of it) and the line for the log, when there is one.
    fn update(&self, before: u64, done: u64) -> (Option<f32>, Option<String>) {
        let fraction = (before + done) as f32 / self.total.max(1) as f32;
        (
            self.throttle.passes(DOWNLOAD_SHARE * fraction),
            self.says.line(fraction),
        )
    }
}

/// The progress of the speaker separation itself (0 to 1) as part of the whole: the whole of it, or what is left after
/// the models were downloaded.
fn run_share(downloaded: bool, fraction: f32) -> f32 {
    if downloaded {
        DOWNLOAD_SHARE + (1.0 - DOWNLOAD_SHARE) * fraction
    } else {
        fraction
    }
}

/// Downloads with the yt-dlp at `yt_dlp`. yt-dlp's `DL <bytes> <total> <estimate>` lines (the progress template) tell the
/// progress and only that: there would be hundreds of them in the log. Every other line it prints goes to the log.
async fn download_with(
    yt_dlp: &Path,
    url: &str,
    folder: &Path,
    cancel: &CancellationToken,
    on_progress: &(dyn Fn(f32) + Send + Sync),
    log: &(dyn for<'a> Fn(&'a str) + Send + Sync),
) -> Result<Downloaded, LinkError> {
    let throttle = Throttle::new();
    ytdlp::download(
        yt_dlp,
        url,
        folder,
        cancel,
        |fraction| {
            if let Some(fraction) = throttle.passes(fraction) {
                on_progress(fraction);
            }
        },
        |line| {
            if !is_progress_line(line) {
                log(line);
            }
        },
    )
    .await
}

/// A line of yt-dlp's progress template, whether or not it has figures (`DL NA NA NA` when it knows none).
fn is_progress_line(line: &str) -> bool {
    line.split_whitespace().next() == Some("DL")
}

/// How the audio of a recording is made ready: decodes `media` once, block by block, to the 16 kHz samples the
/// transcript is made from and, on the way, writes `out` (`audio.m4a`) with the encoder `start_sink` gives. No 48 kHz copy
/// is ever held: each block is resampled twice and handed on.
///
/// All of it runs on the thread it is called on, the sink's creation, writes, end and drop too: Media Foundation's COM
/// belongs to the thread it started on. The sink is started with the first audio, and a recording that gave none ends
/// with no `audio.m4a`: an encoder that is finished with no audio publishes a file that is only a header.
///
/// `progress` hears 0 to 1 when the file says how long it is. A cancelled `cancel` ends it at the next block, with the
/// sink dropped (which removes what it wrote).
fn prepare_blocking(
    media: &Path,
    out: &Path,
    ffmpeg: Option<&Path>,
    cancel: &CancellationToken,
    progress: &dyn Fn(f32),
    start_sink: StartSink,
) -> Result<Prepared, DecodeError> {
    let mut decoder = MonoDecoder::open(media, ffmpeg)?;
    let rate = decoder.rate();
    let duration = decoder.duration();
    let mut to_16k = StreamResampler::new(rate, RATE_16K);
    let mut to_48k = StreamResampler::new(rate, M4A_RATE);
    let reserve = duration.map_or(0, |seconds| {
        (seconds.clamp(0.0, RESERVE_LIMIT_S) * f64::from(RATE_16K)) as usize
    });
    let mut samples = Vec::with_capacity(reserve);
    let mut m4a = M4a::new(out, ffmpeg, start_sink);

    let mut frames: u64 = 0;
    while let Some(block) = decoder.next_block()? {
        if cancel.is_cancelled() {
            // `m4a` is dropped with the function: no half-written file is left.
            return Err(DecodeError::Io("The audio step was cancelled.".into()));
        }
        frames += block.len() as u64;
        samples.extend(to_16k.push(&block));
        m4a.write(&to_48k.push(&block));
        if let Some(seconds) = duration.filter(|seconds| *seconds > 0.0) {
            progress((frames as f64 / f64::from(rate) / seconds) as f32);
        }
    }
    samples.extend(to_16k.finish());
    m4a.write(&to_48k.finish());

    Ok(Prepared {
        samples_16k: Arc::new(samples),
        duration: duration.unwrap_or(0.0),
        m4a: m4a.finish(),
    })
}

/// The `audio.m4a` being written: started with the first audio, and given up (with a reason) at the first thing that
/// goes wrong, since the job goes on without the file.
struct M4a<'a> {
    out: &'a Path,
    ffmpeg: Option<&'a Path>,
    start: StartSink<'a>,
    state: M4aState,
}

enum M4aState {
    /// No audio has come yet.
    Waiting,
    Writing(Box<dyn M4aSink>),
    /// It could not be written; why.
    Failed(String),
}

impl<'a> M4a<'a> {
    fn new(out: &'a Path, ffmpeg: Option<&'a Path>, start: StartSink<'a>) -> Self {
        Self {
            out,
            ffmpeg,
            start,
            state: M4aState::Waiting,
        }
    }

    fn write(&mut self, block: &[f32]) {
        if block.is_empty() {
            return;
        }
        if matches!(self.state, M4aState::Waiting) {
            self.state = match (self.start)(self.out, self.ffmpeg) {
                Ok(sink) => M4aState::Writing(sink),
                Err(e) => M4aState::Failed(e.to_string()),
            };
        }
        if let M4aState::Writing(sink) = &mut self.state {
            if let Err(e) = sink.write(block) {
                // Dropping the sink takes its unfinished file away.
                self.state = M4aState::Failed(format!("Could not write the audio: {e}"));
            }
        }
    }

    /// Ends the file. `Err` with the reason when there is none.
    fn finish(self) -> Result<(), String> {
        match self.state {
            M4aState::Waiting => Err("There was no audio to write.".to_owned()),
            M4aState::Failed(reason) => Err(reason),
            M4aState::Writing(sink) => sink
                .finish()
                .map_err(|e| format!("Could not finish the audio file: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::mpsc;

    use dictate_services::media::tools::Tools;
    use dictate_services::meetings::MeetingError;
    use dictate_services::presets::TranscribePreset;
    use dictate_services::whisper::models::RemoteFile;

    use super::*;
    use crate::test_support::{script, CountingStore};

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ---- what is told while a blocking thread works

    #[test]
    fn a_progress_figure_passes_once_for_each_whole_percent_and_never_goes_back() {
        let throttle = Throttle::new();
        assert_eq!(throttle.passes(0.0), Some(0.0));
        assert_eq!(throttle.passes(0.004), None, "the same percent");
        assert_eq!(throttle.passes(0.011), Some(0.011));
        assert_eq!(throttle.passes(0.5), Some(0.5));
        assert_eq!(throttle.passes(0.2), None, "never back");
        assert_eq!(throttle.passes(f32::NAN), None);
        assert_eq!(
            throttle.passes(7.0),
            Some(1.0),
            "a figure past the end is the end"
        );
        assert_eq!(throttle.passes(1.0), None, "and then it is over");
    }

    #[test]
    fn the_download_of_the_models_is_logged_at_every_tenth() {
        let log = DownloadLog::new();
        let says = |percent: u32| {
            Some(format!(
                "Downloading the speaker separation model… {percent}%"
            ))
        };
        assert_eq!(log.line(0.0), says(0));
        assert_eq!(log.line(0.05), None);
        assert_eq!(log.line(0.1), says(10));
        assert_eq!(log.line(0.19), None);
        // A jump over several tenths says the one it landed in, once.
        assert_eq!(log.line(0.55), says(50));
        assert_eq!(log.line(0.55), None);
        assert_eq!(log.line(1.0), says(100));
        assert_eq!(log.line(1.0), None);
    }

    fn remote(size: u64) -> RemoteFile {
        RemoteFile {
            url: "https://example.test/model.onnx",
            file: "model.onnx",
            size,
            sha256: "00",
        }
    }

    #[test]
    fn the_download_is_the_first_half_of_the_separation_over_all_the_files() {
        let (small, big) = (remote(100), remote(300));
        let progress = ModelProgress::new(&[&small, &big]);

        // The first file, half way: 50 of 400 bytes, an eighth of the download, a sixteenth of the whole.
        let (shown, line) = progress.update(0, 50);
        assert_eq!(shown, Some(DOWNLOAD_SHARE * 0.125));
        assert_eq!(
            line.as_deref(),
            Some("Downloading the speaker separation model… 10%")
        );
        // The second file after the first: 100 + 150 of 400.
        let (shown, line) = progress.update(100, 150);
        assert_eq!(shown, Some(DOWNLOAD_SHARE * 0.625));
        assert_eq!(
            line.as_deref(),
            Some("Downloading the speaker separation model… 60%")
        );
        // All of it is half of the separation, and nothing is said twice.
        let (shown, _) = progress.update(100, 300);
        assert_eq!(shown, Some(DOWNLOAD_SHARE));
        assert_eq!(progress.update(100, 300), (None, None));
    }

    #[test]
    fn the_separation_takes_what_the_download_left() {
        assert_eq!(run_share(false, 0.0), 0.0);
        assert_eq!(run_share(false, 0.4), 0.4);
        assert_eq!(run_share(false, 1.0), 1.0);
        assert_eq!(run_share(true, 0.0), DOWNLOAD_SHARE);
        assert_eq!(run_share(true, 1.0), 1.0);
        assert_eq!(run_share(true, 0.5), 0.75);
    }

    #[tokio::test]
    async fn a_blocking_thread_reports_in_order_and_what_it_returns_comes_after() {
        let heard = Mutex::new(Vec::new());
        let result = run_blocking(
            |report| {
                report(Report::Progress(0.25));
                report(Report::Log("Loading".into()));
                report(Report::Progress(1.0));
                "done"
            },
            |report| lock(&heard).push(report),
        )
        .await;

        assert_eq!(result, Ok("done"));
        assert_eq!(
            *lock(&heard),
            [
                Report::Progress(0.25),
                Report::Log("Loading".into()),
                Report::Progress(1.0)
            ],
            "everything the thread said was heard, whatever came first"
        );
    }

    #[tokio::test]
    async fn a_thread_whose_future_was_dropped_goes_on_without_anyone_listening() {
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let (end_tx, end_rx) = mpsc::channel::<i32>();
        let heard = Arc::new(Mutex::new(Vec::new()));

        {
            let writer = heard.clone();
            let mut run = Box::pin(run_blocking(
                move |report| {
                    report(Report::Progress(0.1));
                    // The future is dropped while the thread waits here.
                    go_rx.recv().unwrap();
                    report(Report::Progress(0.9));
                    end_tx.send(7).unwrap();
                    7
                },
                move |report| lock(&writer).push(report),
            ));
            // Polled until the first report is in: the thread has been started and has spoken.
            let listening = heard.clone();
            let first_report = async move {
                while lock(&listening).is_empty() {
                    tokio::task::yield_now().await;
                }
            };
            tokio::select! {
                _ = &mut run => panic!("the thread has not been let go"),
                () = first_report => {}
            }
        }

        go_tx.send(()).unwrap();
        // The thread came to its end, with nobody to report to and no trouble.
        assert_eq!(end_rx.recv_timeout(Duration::from_secs(30)), Ok(7));
        assert_eq!(*lock(&heard), [Report::Progress(0.1)]);
    }

    #[tokio::test]
    async fn the_next_local_run_waits_for_one_whose_job_was_cancelled_and_is_still_running() {
        let (held_tx, held_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let mut orphan = Box::pin(run_local(
            move |_| {
                held_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            },
            |_| {},
        ));
        let thread_has_its_turn = tauri::async_runtime::spawn_blocking(move || held_rx.recv());
        tokio::select! {
            _ = &mut orphan => panic!("the run ended before it was let go"),
            held = thread_has_its_turn => assert_eq!(held.unwrap(), Ok(())),
        }
        // The job is cancelled: the pipeline drops the future, and with it its own lock. The thread is still here.
        drop(orphan);

        assert!(
            LOCAL_GATE.try_lock().is_err(),
            "the next run would start next to one that is still running"
        );
        release_tx.send(()).unwrap();
        // Once the orphan ends, the turn is free again.
        drop(take_turn());
    }

    // ---- what the settings and the saved keys allow

    struct Parts {
        _dir: tempfile::TempDir,
        secrets: Arc<CountingStore>,
        state: AppState,
        backend: AppBackend,
    }

    fn parts() -> Parts {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = SettingsStore::open(dir.path().join("settings.json"));
        let models = ModelDir::new(dir.path().join("models"));
        fs::create_dir_all(models.root()).unwrap();
        let secrets = Arc::new(CountingStore::default());
        let state = AppState::new(store, None, secrets.clone(), models);
        let backend = AppBackend::new(
            &state,
            Arc::new(DiarizerCache::default()),
            dir.path().join("diarization"),
        )
        .with_tools(|| Tools {
            ffmpeg: None,
            yt_dlp: None,
        });
        Parts {
            _dir: dir,
            secrets,
            state,
            backend,
        }
    }

    #[test]
    fn the_backend_is_a_backend_the_registry_can_hold() {
        fn takes(_: Arc<dyn MeetingBackend>) {}
        takes(Arc::new(parts().backend));
    }

    #[test]
    fn readiness_goes_by_the_settings_and_the_key_flags_and_never_asks_the_key_store() {
        let p = parts();
        let b = &p.backend;
        assert_eq!(b.file_engine(), Engine::Whisper);
        assert!(b.diarize_enabled(), "separating speakers is on by default");
        assert_eq!(
            b.transcribe_model(),
            p.state.settings.get().transcribe_model
        );
        assert!(!b.transcribe_model().is_empty());
        assert!(!b.ai_ready() && !b.api_ready() && !b.whisper_ready());

        p.state.keys.set(Account::Ai, true);
        assert!(b.ai_ready(), "a saved AI key is what Anthropic needs");
        p.state.keys.set(Account::Transcribe, true);
        assert!(b.api_ready());

        // The model that the settings choose (or the recommended one) is a file in the model folder.
        p.state
            .settings
            .update(|s| {
                s.file_engine = Engine::Api;
                s.diarize = false;
            })
            .unwrap();
        assert_eq!(b.file_engine(), Engine::Api);
        assert!(!b.diarize_enabled());
        let id = models::recommended(system::total_ram_gb());
        fs::write(p.state.models.path(id), b"a model").unwrap();
        assert!(b.whisper_ready());

        assert_eq!(p.secrets.gets(), 0, "the key store is not asked on the way");
    }

    #[test]
    fn the_api_needs_a_model_an_address_and_a_key_of_its_own_or_one_that_is_shared() {
        let keys = KeyFlags::default();
        let mut s = Settings {
            transcribe_model: "whisper-1".into(),
            transcribe_base_url: "https://api.openai.com/v1".into(),
            ..Settings::default()
        };
        assert!(!api_ready(&s, &keys), "no key");

        keys.set(Account::Transcribe, true);
        assert!(api_ready(&s, &keys));
        let mut no_model = s.clone();
        no_model.transcribe_model = "  ".into();
        assert!(!api_ready(&no_model, &keys));
        let mut no_address = s.clone();
        no_address.transcribe_base_url = String::new();
        assert!(!api_ready(&no_address, &keys));

        // With OpenRouter for both, the AI key is enough.
        keys.set(Account::Transcribe, false);
        keys.set(Account::Ai, true);
        assert!(
            !api_ready(&s, &keys),
            "the AI key is not sent to another host"
        );
        s.ai_base_url = "https://openrouter.ai/api/v1".into();
        s.set_transcribe_preset(TranscribePreset::OpenRouter);
        assert!(api_ready(&s, &keys));
    }

    #[tokio::test]
    async fn local_transcription_without_a_downloaded_model_says_so_and_loads_nothing() {
        let p = parts();
        let logged = Mutex::new(Vec::new());
        let error = p
            .backend
            .transcribe_local(
                Arc::new(vec![0.0; 16_000]),
                false,
                &CancellationToken::new(),
                &|_| {},
                &|line| lock(&logged).push(line.to_owned()),
            )
            .await
            .unwrap_err();
        assert!(error.contains("isn't downloaded"), "{error}");
        assert!(lock(&logged).is_empty(), "no model: nothing to say it runs");
    }

    #[tokio::test]
    async fn local_transcription_says_which_model_runs_and_that_it_is_loading_it() {
        let p = parts();
        // A file that is no model: it is "downloaded", so Whisper is asked to run, and it can't load it.
        let id = models::recommended(system::total_ram_gb());
        fs::write(p.state.models.path(id), b"not a real model").unwrap();
        let logged = Mutex::new(Vec::new());

        let error = p
            .backend
            .transcribe_local(
                Arc::new(vec![0.0; 16_000]),
                false,
                &CancellationToken::new(),
                &|_| {},
                &|line| lock(&logged).push(line.to_owned()),
            )
            .await
            .unwrap_err();

        assert!(
            error.contains("Could not load the Whisper model"),
            "{error}"
        );
        assert_eq!(
            *lock(&logged),
            [
                format!("Whisper on this computer · {id}…"),
                "Loading the Whisper model…".to_owned()
            ]
        );
    }

    // ---- downloading a link

    /// A stand-in for yt-dlp that prints what it prints: a title, progress lines (one that has no figures), a plain line
    /// and the file.
    fn yt_dlp(dir: &Path) -> PathBuf {
        script(
            dir,
            "yt-dlp",
            "echo 'TITLE:A talk'\necho 'DL 10 100 NA'\necho 'DL NA NA NA'\necho '[download] Destination: x'\necho 'DL 100 100 NA'\necho 'FILE:source.m4a'",
            "echo TITLE:A talk\r\necho DL 10 100 NA\r\necho DL NA NA NA\r\necho [download] Destination: x\r\necho DL 100 100 NA\r\necho FILE:source.m4a",
        )
    }

    #[tokio::test]
    async fn progress_lines_of_yt_dlp_go_to_progress_only_and_every_other_line_to_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let tool = yt_dlp(dir.path());
        let (progress, logged) = (Mutex::new(Vec::new()), Mutex::new(Vec::new()));

        let got = download_with(
            &tool,
            "https://youtu.be/x",
            dir.path(),
            &CancellationToken::new(),
            &|fraction| lock(&progress).push(fraction),
            &|line| lock(&logged).push(line.to_owned()),
        )
        .await
        .unwrap();

        assert_eq!(got.title.as_deref(), Some("A talk"));
        assert_eq!(got.file, PathBuf::from("source.m4a"));
        assert_eq!(*lock(&progress), [0.1, 1.0]);
        let logged = lock(&logged);
        assert_eq!(
            *logged,
            [
                "TITLE:A talk",
                "[download] Destination: x",
                "FILE:source.m4a"
            ]
        );
        // Not even the one that said nothing.
        assert!(
            logged.iter().all(|line| !line.starts_with("DL")),
            "{logged:?}"
        );
    }

    #[test]
    fn a_progress_line_is_one_that_starts_with_dl() {
        assert!(is_progress_line("DL 10 100 NA"));
        assert!(is_progress_line("DL NA NA NA"));
        assert!(is_progress_line("DL"));
        assert!(!is_progress_line("[download] 42%"));
        assert!(!is_progress_line("TITLE:DL 1 2 3"));
        assert!(!is_progress_line("DLX 1"));
        assert!(!is_progress_line(""));
    }

    #[tokio::test]
    async fn the_backend_downloads_with_the_yt_dlp_it_finds() {
        let dir = tempfile::tempdir().unwrap();
        let tool = yt_dlp(dir.path());
        let mut p = parts();
        p.backend = p.backend.with_tools(move || Tools {
            ffmpeg: None,
            yt_dlp: Some(tool.clone()),
        });
        let logged = Mutex::new(Vec::new());

        let got = p
            .backend
            .download(
                "https://youtu.be/x",
                dir.path(),
                &CancellationToken::new(),
                &|_| {},
                &|line| lock(&logged).push(line.to_owned()),
            )
            .await
            .unwrap();
        assert_eq!(got.title.as_deref(), Some("A talk"));
        assert_eq!(lock(&logged).len(), 3);
    }

    #[tokio::test]
    async fn only_a_yt_dlp_that_is_missing_is_a_spawn_error() {
        let p = parts();
        let dir = tempfile::tempdir().unwrap();
        let error = p
            .backend
            .download(
                "https://youtu.be/x",
                dir.path(),
                &CancellationToken::new(),
                &|_| {},
                &|_| {},
            )
            .await
            .unwrap_err();
        assert!(matches!(error, LinkError::Spawn(_)), "{error:?}");
        // The pipeline says "yt-dlp isn't installed" for it, with the command that installs it.
        assert_eq!(
            MeetingError::from_link(error).key,
            "meetings.error.ytdlp_missing"
        );

        // A yt-dlp that ran and failed is a failure of the download, not a missing tool.
        let failing = script(
            dir.path(),
            "failing",
            "echo boom\nexit 1",
            "echo boom\r\nexit /b 1",
        );
        let error = download_with(
            &failing,
            "https://youtu.be/x",
            dir.path(),
            &CancellationToken::new(),
            &|_| {},
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(error, LinkError::Failed { .. }), "{error:?}");
    }

    // ---- the audio

    /// A WAV file of a tone: mono, 16-bit, at `rate` Hz.
    fn tone_wav(path: &Path, rate: u32, seconds: f64) {
        let samples: Vec<i16> = (0..(f64::from(rate) * seconds) as usize)
            .map(|i| {
                let phase = std::f64::consts::TAU * 440.0 * i as f64 / f64::from(rate);
                (0.5 * phase.sin() * 32_767.0) as i16
            })
            .collect();
        let data = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + data).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * 2).to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(data.to_le_bytes());
        for sample in samples {
            bytes.extend(sample.to_le_bytes());
        }
        fs::write(path, bytes).unwrap();
    }

    /// What a fake `audio.m4a` encoder was asked.
    #[derive(Default)]
    struct SinkLog {
        started: AtomicUsize,
        written: AtomicUsize,
        finished: AtomicBool,
        dropped: AtomicBool,
    }

    struct FakeSink {
        log: Arc<SinkLog>,
        /// Fails from this write on (counted from 1).
        fail_from: Option<usize>,
        /// Cancels this when it is written to.
        cancel: Option<CancellationToken>,
        writes: usize,
    }

    impl M4aSink for FakeSink {
        fn write(&mut self, block: &[f32]) -> io::Result<()> {
            self.writes += 1;
            if self.fail_from.is_some_and(|from| self.writes >= from) {
                return Err(io::Error::other("the disk is full"));
            }
            self.log.written.fetch_add(block.len(), Ordering::SeqCst);
            if let Some(cancel) = &self.cancel {
                cancel.cancel();
            }
            Ok(())
        }

        fn finish(self: Box<Self>) -> io::Result<()> {
            self.log.finished.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    impl Drop for FakeSink {
        fn drop(&mut self) {
            self.log.dropped.store(true, Ordering::SeqCst);
        }
    }

    /// The way to start a fake sink (or fail to), the same each time.
    fn fake_sink(
        log: &Arc<SinkLog>,
        fail_from: Option<usize>,
        cancel: Option<CancellationToken>,
    ) -> impl Fn(&Path, Option<&Path>) -> Result<Box<dyn M4aSink>, EncodeError> {
        let log = log.clone();
        move |_, _| {
            log.started.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeSink {
                log: log.clone(),
                fail_from,
                cancel: cancel.clone(),
                writes: 0,
            }))
        }
    }

    fn prepare(
        wav: &Path,
        cancel: &CancellationToken,
        start: StartSink,
    ) -> (Result<Prepared, DecodeError>, Vec<f32>) {
        let progress = Mutex::new(Vec::new());
        let out = wav.with_file_name("audio.m4a");
        let prepared = prepare_blocking(
            wav,
            &out,
            None,
            cancel,
            &|fraction| lock(&progress).push(fraction),
            start,
        );
        let progress = lock(&progress).clone();
        (prepared, progress)
    }

    #[test]
    fn the_audio_is_decoded_once_for_whisper_and_for_the_encoder_and_progress_reaches_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("talk.wav");
        tone_wav(&wav, 44_100, 2.0);
        let log = Arc::new(SinkLog::default());

        let (prepared, progress) = prepare(
            &wav,
            &CancellationToken::new(),
            &fake_sink(&log, None, None),
        );
        let prepared = prepared.unwrap();

        // 2 seconds: 32,000 samples at 16 kHz for Whisper, and 96,000 at 48 kHz written to the encoder, give or take the
        // delay of the resampler. Nothing else is held.
        let len = prepared.samples_16k.len() as i64;
        assert!((len - 32_000).abs() < 400, "{len}");
        let written = log.written.load(Ordering::SeqCst) as i64;
        assert!((written - 96_000).abs() < 1_200, "{written}");
        assert!(
            (prepared.duration - 2.0).abs() < 0.05,
            "{}",
            prepared.duration
        );
        assert_eq!(prepared.m4a, Ok(()));
        assert_eq!(log.started.load(Ordering::SeqCst), 1);
        assert!(log.finished.load(Ordering::SeqCst));

        // The progress only goes forward, in whole percents, and ends where the file does.
        assert!(!progress.is_empty());
        assert!(
            progress.windows(2).all(|pair| pair[0] < pair[1]),
            "{progress:?}"
        );
        assert!(progress
            .iter()
            .all(|fraction| (0.0..=1.0).contains(fraction)));
        assert!(
            progress.last().copied().unwrap_or(0.0) > 0.9,
            "{progress:?}"
        );
    }

    #[test]
    fn audio_that_is_already_at_16_khz_is_not_changed_for_whisper() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("talk.wav");
        tone_wav(&wav, 16_000, 1.0);
        let log = Arc::new(SinkLog::default());

        let (prepared, _) = prepare(
            &wav,
            &CancellationToken::new(),
            &fake_sink(&log, None, None),
        );
        assert_eq!(prepared.unwrap().samples_16k.len(), 16_000);
    }

    #[test]
    fn a_recording_with_no_audio_never_has_an_encoder_started_or_finished() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("silent.wav");
        tone_wav(&wav, 16_000, 0.0);
        let log = Arc::new(SinkLog::default());

        let (prepared, _) = prepare(
            &wav,
            &CancellationToken::new(),
            &fake_sink(&log, None, None),
        );
        // The decoder refuses an empty file ("no audio could be decoded"); had it given no block, the file would not be
        // published either (see the next test). Either way no header-only audio.m4a is left.
        if let Ok(prepared) = prepared {
            assert!(prepared.samples_16k.is_empty());
            assert!(prepared.m4a.is_err());
        }
        assert_eq!(log.started.load(Ordering::SeqCst), 0);
        assert!(!log.finished.load(Ordering::SeqCst));
        assert!(!dir.path().join("audio.m4a").exists());
    }

    #[test]
    fn the_encoder_is_started_by_the_first_audio_and_is_not_finished_without_any() {
        let log = Arc::new(SinkLog::default());
        let start = fake_sink(&log, None, None);
        let out = Path::new("audio.m4a");

        // A decoder that gave blocks that came to nothing (a resampler that has not put anything out yet) gives no file.
        let mut m4a = M4a::new(out, None, &start);
        m4a.write(&[]);
        m4a.write(&[]);
        assert_eq!(log.started.load(Ordering::SeqCst), 0);
        assert_eq!(m4a.finish(), Err("There was no audio to write.".to_owned()));
        assert!(
            !log.finished.load(Ordering::SeqCst),
            "a header-only audio.m4a"
        );

        // The first audio starts it, once, and the end finishes it.
        let mut m4a = M4a::new(out, None, &start);
        m4a.write(&[0.1; 480]);
        m4a.write(&[]);
        m4a.write(&[0.1; 480]);
        assert_eq!(log.started.load(Ordering::SeqCst), 1);
        assert_eq!(log.written.load(Ordering::SeqCst), 960);
        assert_eq!(m4a.finish(), Ok(()));
        assert!(log.finished.load(Ordering::SeqCst));
    }

    #[test]
    fn a_job_goes_on_without_audio_m4a_when_the_encoder_cannot_start() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("talk.wav");
        tone_wav(&wav, 16_000, 1.0);

        let none = |_: &Path, _: Option<&Path>| Err(EncodeError::Unavailable);
        let (prepared, _) = prepare(&wav, &CancellationToken::new(), &none);
        let prepared = prepared.unwrap();

        assert_eq!(
            prepared.samples_16k.len(),
            16_000,
            "the transcript has all its audio"
        );
        assert_eq!(
            prepared.m4a,
            Err("There is no AAC encoder to save the audio with.".to_owned())
        );
    }

    #[test]
    fn an_encoder_that_fails_half_way_is_dropped_unfinished_and_the_job_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("talk.wav");
        tone_wav(&wav, 16_000, 3.0);
        let log = Arc::new(SinkLog::default());

        let (prepared, _) = prepare(
            &wav,
            &CancellationToken::new(),
            &fake_sink(&log, Some(2), None),
        );
        let prepared = prepared.unwrap();

        assert_eq!(prepared.samples_16k.len(), 48_000);
        let reason = prepared.m4a.unwrap_err();
        assert!(reason.contains("the disk is full"), "{reason}");
        assert!(log.dropped.load(Ordering::SeqCst), "its file goes with it");
        assert!(!log.finished.load(Ordering::SeqCst));
        assert_eq!(log.started.load(Ordering::SeqCst), 1, "no second try");
    }

    #[test]
    fn a_cancelled_audio_step_stops_at_the_next_block_and_drops_the_encoder_unfinished() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("talk.wav");
        tone_wav(&wav, 16_000, 5.0);
        let cancel = CancellationToken::new();
        let log = Arc::new(SinkLog::default());

        // The job is cancelled while the first audio is being written.
        let (prepared, _) = prepare(&wav, &cancel, &fake_sink(&log, None, Some(cancel.clone())));

        assert!(prepared.is_err());
        assert!(log.dropped.load(Ordering::SeqCst));
        assert!(
            !log.finished.load(Ordering::SeqCst),
            "a cancelled file is not published"
        );

        // And one cancelled before it began never starts an encoder.
        let log = Arc::new(SinkLog::default());
        let (prepared, _) = prepare(&wav, &cancel, &fake_sink(&log, None, None));
        assert!(prepared.is_err());
        assert_eq!(log.started.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn audio_m4a_is_written_with_the_real_encoder_into_a_vietnamese_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Cuộc họp 1");
        fs::create_dir_all(&folder).unwrap();
        let wav = folder.join("âm thanh.wav");
        tone_wav(&wav, 44_100, 1.0);
        let out = folder.join("audio.m4a");
        let ffmpeg = Tools::find().ffmpeg;

        let prepared = prepare_blocking(
            &wav,
            &out,
            ffmpeg.as_deref(),
            &CancellationToken::new(),
            &|_| {},
            &encode::m4a_writer,
        )
        .unwrap();

        // On CI every system has an encoder (ffmpeg is installed there); elsewhere the file is there when one was found.
        let on_ci = std::env::var_os("CI").is_some_and(|value| !value.is_empty());
        if on_ci {
            assert_eq!(prepared.m4a, Ok(()));
        }
        assert_eq!(out.is_file(), prepared.m4a.is_ok(), "{:?}", prepared.m4a);
        let leftovers: Vec<_> = fs::read_dir(&folder)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty(), "a .part file is left: {leftovers:?}");
        if prepared.m4a.is_ok() {
            assert!(fs::metadata(&out).unwrap().len() > 1_000);
        }
        assert!((prepared.samples_16k.len() as i64 - 16_000).abs() < 300);
    }
}
