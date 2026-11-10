//! Local speech recognition: Whisper (whisper.cpp, through whisper-rs) on the user's machine.
//!
//! `LocalWhisper::load` reads a model of up to 1.6 GB, and `transcribe` and `transcribe_timed` (for recordings of
//! any length) run as long as the audio takes: all block. From async code call them on a blocking thread
//! (`tokio::task::spawn_blocking`).
//!
//! whisper.cpp and ggml log to stderr by default. `load` hands their logging to whisper-rs, which (with the
//! `log_backend` feature) passes it on to the `log` crate: nothing shows until the app installs a logger.

use std::any::Any;
use std::ffi::{c_int, c_void};
use std::fmt;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::Path;

use dictate_core::text::segments::Segment;
use dictate_core::text::whisper::{clean, Timed};
use tokio_util::sync::CancellationToken;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
    WhisperError as EngineError, WhisperState, WhisperSysContext, WhisperSysState,
};

use super::long::{
    chunk_bounds, chunk_prompt, seconds, words_from_tokens, TokenSpan, CONTEXT_CHARS,
};

/// The only sample rate Whisper takes.
const SAMPLE_RATE: u32 = 16_000;

/// The longest prompt, in characters. A longer one is cut: only its last tokens would fit the model anyway.
const MAX_PROMPT_CHARS: usize = 800;

/// More tokens than any prompt of `MAX_PROMPT_CHARS` characters can make: a character is at most 4 bytes of
/// UTF-8 and a token stands for at least one byte.
const MAX_PROMPT_TOKENS: usize = 4 * MAX_PROMPT_CHARS;

/// The same for the prompt of a chunk of a long recording: the vocabulary, a space, and the end of the text before.
const MAX_CHUNK_PROMPT_TOKENS: usize = 4 * (MAX_PROMPT_CHARS + 1 + CONTEXT_CHARS);

/// How long a chunk of a long recording is, in seconds, and how far either side of that a quiet place to cut it is
/// looked for. Whisper takes the whole of a recording at once, which for hours of audio means a lot of memory, no
/// progress to show, and no way to stop it between steps.
const CHUNK_S: f64 = 600.0;
const CUT_SEARCH_S: f64 = 30.0;

/// Why local transcription did not give a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhisperError {
    /// The model file is missing, damaged or not a Whisper model.
    Load(String),
    /// The audio file could not be read.
    Audio(String),
    /// The caller cancelled it.
    Cancelled,
    /// Whisper itself failed, or was asked for something it cannot do (a language it does not know).
    Failed(String),
}

impl fmt::Display for WhisperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WhisperError::Load(m) => write!(f, "Could not load the Whisper model: {m}"),
            WhisperError::Audio(m) => write!(f, "Could not read the audio: {m}"),
            WhisperError::Cancelled => f.write_str("The transcription was cancelled."),
            WhisperError::Failed(m) => write!(f, "Whisper failed: {m}"),
        }
    }
}

impl std::error::Error for WhisperError {}

/// What `LocalWhisper::transcribe_timed` heard.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedTranscript {
    /// The lines Whisper wrote, in time order, with the words of each when they were asked for (else none). Times are
    /// seconds from the start of the recording. The text is as Whisper wrote it, with its leading space: what
    /// `timed_to_segments` takes.
    pub lines: Vec<(Timed, Vec<Timed>)>,
    /// The language the transcript is in: the one asked for, or the one Whisper detected.
    pub language: Option<String>,
}

/// A Whisper model in memory, ready to transcribe. It is `Send + Sync`: share it (in an `Arc`) rather than
/// loading it again for each recording.
pub struct LocalWhisper {
    context: WhisperContext,
}

impl fmt::Debug for LocalWhisper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalWhisper").finish_non_exhaustive()
    }
}

impl LocalWhisper {
    /// Loads the model file. On a Mac it runs on the GPU (Metal), elsewhere on the CPU.
    pub fn load(model_path: &Path) -> Result<LocalWhisper, WhisperError> {
        keep_whisper_cpp_quiet();
        if !model_path.is_file() {
            return Err(WhisperError::Load("the model file is not there".into()));
        }
        let path = model_path
            .to_str()
            .ok_or_else(|| WhisperError::Load("the path of the model file is not text".into()))?;
        let context = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| {
                WhisperError::Load(match e {
                    EngineError::InitError => {
                        "the file is damaged or is not a Whisper model".into()
                    }
                    other => other.to_string(),
                })
            })?;
        Ok(LocalWhisper { context })
    }

    /// The segments of 16 kHz mono audio, in time order: `start` in seconds, the text cleaned of Whisper's tags,
    /// segments without text left out.
    ///
    /// `language` is a Whisper language code ("en", "vi"); `None` (or "auto") has Whisper detect it from the
    /// first 30 seconds. `prompt` (cut to 800 characters) is shown to the model as what came before: names and
    /// terms in it are spelled that way. `on_progress` hears the percent done, rising to 100.
    ///
    /// Cancelling `cancel` stops it within a moment and gives `WhisperError::Cancelled` (also when it was
    /// already over by then). If `on_progress` panics, the audio is still transcribed and then the panic comes
    /// out of this call.
    pub fn transcribe(
        &self,
        samples_16k_mono: &[f32],
        language: Option<&str>,
        prompt: &str,
        cancel: &CancellationToken,
        mut on_progress: impl FnMut(u8),
    ) -> Result<Vec<Segment>, WhisperError> {
        if cancel.is_cancelled() {
            return Err(WhisperError::Cancelled);
        }
        let language = language_choice(language)?;
        if samples_16k_mono.is_empty() {
            return Ok(Vec::new());
        }
        // The prompt goes in as tokens: whisper-rs's `set_initial_prompt` panics on a NUL and leaks the text
        // on every call.
        let prompt = cut_prompt(prompt);
        let prompt_tokens = if prompt.trim().is_empty() {
            Vec::new()
        } else {
            self.context
                .tokenize(&prompt, MAX_PROMPT_TOKENS)
                .map_err(|e| {
                    WhisperError::Failed(format!("the prompt is no text Whisper can read: {e}"))
                })?
        };
        let mut state = self
            .context
            .create_state()
            .map_err(|e| WhisperError::Failed(format!("could not start: {e}")))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 5 });
        params.set_language(language);
        if !prompt_tokens.is_empty() {
            params.set_tokens(&prompt_tokens);
        }
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);

        let mut sink = ProgressSink {
            report: &mut on_progress,
            last: None,
            panic: None,
        };
        // SAFETY: both pointers outlive `state.full` below, the only call that uses them (`full` takes
        // `params` by value). The sink is only ever touched by `report_progress`, which whisper.cpp calls on
        // this thread; the token is only read, through a shared reference, by `poll_cancel`.
        unsafe {
            params.set_progress_callback(Some(report_progress));
            params.set_progress_callback_user_data(std::ptr::from_mut(&mut sink).cast());
            params.set_abort_callback(Some(poll_cancel));
            params.set_abort_callback_user_data(std::ptr::from_ref(cancel).cast_mut().cast());
        }
        let outcome = state.full(params, samples_16k_mono);

        if let Some(payload) = sink.panic.take() {
            resume_unwind(payload);
        }
        if cancel.is_cancelled() {
            return Err(WhisperError::Cancelled);
        }
        outcome.map_err(|e| WhisperError::Failed(e.to_string()))?;
        if sink.last != Some(100) {
            (sink.report)(100);
        }

        let mut segments = Vec::new();
        for segment in state.as_iter() {
            let text = segment
                .to_str_lossy()
                .map_err(|e| WhisperError::Failed(e.to_string()))?;
            let text = clean(&text);
            if text.is_empty() {
                continue;
            }
            // Whisper counts in hundredths of a second.
            segments.push(Segment {
                start: segment.start_timestamp() as f64 / 100.0,
                speaker: None,
                text,
            });
        }
        Ok(segments)
    }

    /// The lines of 16 kHz mono audio of any length, with the times of their words when `words` is set (for
    /// speaker separation, which needs to know who said what), in the form `timed_to_segments` takes.
    ///
    /// The audio goes to Whisper in chunks of about 10 minutes, cut where it is quiet (see `chunk_bounds`), each
    /// shown the vocabulary in `prompt` and the end of the text before it. Times are from the start of the audio.
    /// `language` is as in `transcribe`; when none is given, the one Whisper hears in the first chunk with speech
    /// in it is used for the others too, so that it cannot change half way through a meeting.
    ///
    /// `on_progress` hears how far it is, from 0 to 1. Cancelling `cancel` stops it between chunks and, within a
    /// chunk, within a moment: `WhisperError::Cancelled`. If `on_progress` panics, the chunk is still finished and
    /// then the panic comes out of this call.
    pub fn transcribe_timed(
        &self,
        samples_16k: &[f32],
        language: Option<&str>,
        prompt: &str,
        words: bool,
        cancel: &CancellationToken,
        mut on_progress: impl FnMut(f32),
    ) -> Result<TimedTranscript, WhisperError> {
        if cancel.is_cancelled() {
            return Err(WhisperError::Cancelled);
        }
        let language = language_choice(language)?;
        let mut transcript = TimedTranscript {
            lines: Vec::new(),
            language: language.map(str::to_owned),
        };
        if samples_16k.is_empty() {
            return Ok(transcript);
        }
        let vocabulary = cut_prompt(prompt);
        let mut state = self
            .context
            .create_state()
            .map_err(|e| WhisperError::Failed(format!("could not start: {e}")))?;

        let chunks = chunk_bounds(samples_16k, SAMPLE_RATE, CHUNK_S, CUT_SEARCH_S);
        let count = chunks.len();
        // What the chunk before said.
        let mut before = String::new();
        for (i, chunk) in chunks.into_iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(WhisperError::Cancelled);
            }
            let prompt_tokens = self.chunk_prompt_tokens(&chunk_prompt(&vocabulary, &before))?;
            let job = ChunkJob {
                language: transcript.language.as_deref(),
                prompt_tokens: &prompt_tokens,
                words,
                offset: chunk.start as f64 / f64::from(SAMPLE_RATE),
            };
            let mut report =
                |percent: u8| on_progress((i as f32 + f32::from(percent) / 100.0) / count as f32);
            let lines =
                self.transcribe_chunk(&mut state, &samples_16k[chunk], &job, cancel, &mut report)?;

            if transcript.language.is_none() && !lines.is_empty() {
                transcript.language =
                    whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_owned);
            }
            before = lines
                .iter()
                .map(|(line, _)| clean(&line.text))
                .collect::<Vec<_>>()
                .join(" ");
            transcript.lines.extend(lines);
        }
        Ok(transcript)
    }

    /// The prompt of a chunk as Whisper gets it. A prompt goes in as tokens (see `transcribe`).
    fn chunk_prompt_tokens(&self, prompt: &str) -> Result<Vec<c_int>, WhisperError> {
        if prompt.trim().is_empty() {
            return Ok(Vec::new());
        }
        self.context
            .tokenize(prompt, MAX_CHUNK_PROMPT_TOKENS)
            .map_err(|e| {
                WhisperError::Failed(format!("the prompt is no text Whisper can read: {e}"))
            })
    }

    /// One chunk of `transcribe_timed`: its lines, in seconds from the start of the whole recording.
    fn transcribe_chunk(
        &self,
        state: &mut WhisperState,
        audio: &[f32],
        job: &ChunkJob<'_>,
        cancel: &CancellationToken,
        report: &mut dyn FnMut(u8),
    ) -> Result<Vec<(Timed, Vec<Timed>)>, WhisperError> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 5 });
        params.set_language(job.language);
        if !job.prompt_tokens.is_empty() {
            params.set_tokens(job.prompt_tokens);
        }
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        params.set_token_timestamps(job.words);

        let mut sink = ProgressSink {
            report,
            last: None,
            panic: None,
        };
        // SAFETY: as in `transcribe`: the pointers outlive `state.full`, the only call that uses them, and the
        // sink and the token are only touched the way `report_progress` and `poll_cancel` do.
        unsafe {
            params.set_progress_callback(Some(report_progress));
            params.set_progress_callback_user_data(std::ptr::from_mut(&mut sink).cast());
            params.set_abort_callback(Some(poll_cancel));
            params.set_abort_callback_user_data(std::ptr::from_ref(cancel).cast_mut().cast());
        }
        let outcome = state.full(params, audio);

        if let Some(payload) = sink.panic.take() {
            resume_unwind(payload);
        }
        if cancel.is_cancelled() {
            return Err(WhisperError::Cancelled);
        }
        outcome.map_err(|e| WhisperError::Failed(e.to_string()))?;
        if sink.last != Some(100) {
            (sink.report)(100);
        }

        // Tokens from this one on are Whisper's own marks (the language, the times) and not text.
        let first_special = self.context.token_eot();
        let mut lines = Vec::new();
        for segment in state.as_iter() {
            let text = segment
                .to_str_lossy()
                .map_err(|e| WhisperError::Failed(e.to_string()))?
                .into_owned();
            if clean(&text).is_empty() {
                continue;
            }
            let start = job.offset + seconds(segment.start_timestamp());
            let line = Timed {
                start,
                end: (job.offset + seconds(segment.end_timestamp())).max(start),
                text,
                speaker: None,
            };
            let words = if job.words {
                let tokens: Vec<TokenSpan> = (0..segment.n_tokens())
                    .filter_map(|n| segment.get_token(n))
                    .filter(|token| token.token_id() < first_special)
                    .filter_map(|token| {
                        let times = token.token_data();
                        Some(TokenSpan {
                            bytes: token.to_bytes().ok()?.to_vec(),
                            start: times.t0,
                            end: times.t1,
                        })
                    })
                    .collect();
                words_from_tokens(&tokens, job.offset)
            } else {
                Vec::new()
            };
            lines.push((line, words));
        }
        Ok(lines)
    }
}

/// What one chunk of `transcribe_timed` is transcribed with.
struct ChunkJob<'a> {
    language: Option<&'a str>,
    prompt_tokens: &'a [c_int],
    /// Whether the words of each line are timed.
    words: bool,
    /// Where the chunk starts in the recording, in seconds.
    offset: f64,
}

/// What `report_progress` reaches while whisper.cpp runs.
struct ProgressSink<'a> {
    report: &'a mut dyn FnMut(u8),
    /// The last percent passed on: a value that comes twice is passed on once.
    last: Option<u8>,
    /// What `report` panicked with. A panic must not unwind into C++, so it waits here for `transcribe`.
    panic: Option<Box<dyn Any + Send>>,
}

/// whisper.cpp's progress callback: at the start of every 30 seconds of audio, on the calling thread.
unsafe extern "C" fn report_progress(
    _context: *mut WhisperSysContext,
    _state: *mut WhisperSysState,
    progress: c_int,
    user_data: *mut c_void,
) {
    // SAFETY: `user_data` is the `ProgressSink` of the `transcribe` call that is running (see there).
    let sink = unsafe { &mut *user_data.cast::<ProgressSink<'_>>() };
    let percent = progress.clamp(0, 100) as u8;
    if sink.panic.is_some() || sink.last == Some(percent) {
        return;
    }
    sink.last = Some(percent);
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| (sink.report)(percent))) {
        sink.panic = Some(payload);
    }
}

/// ggml's abort callback: asked between and during the heavy steps, from any of its threads.
unsafe extern "C" fn poll_cancel(user_data: *mut c_void) -> bool {
    // SAFETY: `user_data` is the `&CancellationToken` of the `transcribe` call that is running, which is `Sync`
    // and only read here.
    let cancel = unsafe { &*user_data.cast::<CancellationToken>() };
    cancel.is_cancelled()
}

/// Hands whisper.cpp's and ggml's logging to whisper-rs (once; later calls do nothing).
fn keep_whisper_cpp_quiet() {
    whisper_rs::install_logging_hooks();
}

/// The prompt as Whisper gets it: without NULs, at most `MAX_PROMPT_CHARS` characters.
fn cut_prompt(prompt: &str) -> String {
    prompt
        .chars()
        .filter(|c| *c != '\0')
        .take(MAX_PROMPT_CHARS)
        .collect()
}

/// The language to give Whisper: `None` for "detect it", else a code Whisper knows.
fn language_choice(language: Option<&str>) -> Result<Option<&str>, WhisperError> {
    let Some(language) = language
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.eq_ignore_ascii_case("auto"))
    else {
        return Ok(None);
    };
    // `get_lang_id` logs the language it does not know.
    keep_whisper_cpp_quiet();
    if !language.contains('\0') && whisper_rs::get_lang_id(language).is_some() {
        Ok(Some(language))
    } else {
        Err(WhisperError::Failed(format!(
            "Whisper does not know the language {language:?}"
        )))
    }
}

/// The samples of a WAV file as Whisper wants them: 16 kHz, mono, `f32` between -1 and 1.
///
/// Reads 16-bit integer and 32-bit float WAV files with any number of channels (averaged to one) and any
/// sample rate (resampled linearly, which is plain: better resampling comes with ffmpeg in a later plan).
/// Other formats are an `Audio` error.
pub fn read_wav_16k_mono(path: &Path) -> Result<Vec<f32>, WhisperError> {
    let reader = hound::WavReader::open(path).map_err(|e| WhisperError::Audio(e.to_string()))?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 {
        return Err(WhisperError::Audio(
            "the file has no channels or a sample rate of 0".into(),
        ));
    }
    let channels = usize::from(spec.channels);
    match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => to_16k_mono(
            reader.into_samples::<i16>(),
            channels,
            spec.sample_rate,
            |s| f32::from(s) / 32_768.0,
        ),
        (hound::SampleFormat::Float, 32) => to_16k_mono(
            reader.into_samples::<f32>(),
            channels,
            spec.sample_rate,
            |s| s,
        ),
        (format, bits) => Err(WhisperError::Audio(format!(
            "{bits}-bit {} samples are not supported: use a 16-bit or 32-bit float WAV file",
            match format {
                hound::SampleFormat::Int => "integer",
                hound::SampleFormat::Float => "float",
            }
        ))),
    }
}

/// Averages each frame of `channels` interleaved samples to one and resamples to 16 kHz as they come, so no
/// copy of the whole recording at its own rate is ever held.
fn to_16k_mono<S>(
    samples: impl Iterator<Item = hound::Result<S>>,
    channels: usize,
    sample_rate: u32,
    to_f32: impl Fn(S) -> f32,
) -> Result<Vec<f32>, WhisperError> {
    let mut resampler = Resampler::new(sample_rate);
    let mut audio = Vec::new();
    let mut frame_sum = 0.0_f32;
    let mut in_frame = 0;
    for sample in samples {
        let sample = sample.map_err(|e| WhisperError::Audio(e.to_string()))?;
        frame_sum += to_f32(sample);
        in_frame += 1;
        if in_frame == channels {
            resampler.push(frame_sum / channels as f32, &mut audio);
            frame_sum = 0.0;
            in_frame = 0;
        }
    }
    resampler.finish(&mut audio);
    Ok(audio)
}

/// Linear interpolation from `sample_rate` to 16 kHz, one input sample at a time.
struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// Input samples taken so far.
    seen: u64,
    /// Output samples made so far: the next one sits at `produced * step` input samples.
    produced: u64,
    /// The last input sample.
    previous: f32,
}

impl Resampler {
    fn new(sample_rate: u32) -> Self {
        Resampler {
            step: f64::from(sample_rate) / f64::from(SAMPLE_RATE),
            seen: 0,
            produced: 0,
            previous: 0.0,
        }
    }

    fn push(&mut self, sample: f32, out: &mut Vec<f32>) {
        if self.seen > 0 {
            // Input sample `seen` sits at that position, `previous` one before it: every output sample that
            // falls in between is now known.
            let end = self.seen as f64;
            loop {
                let at = self.produced as f64 * self.step;
                if at > end {
                    break;
                }
                let toward_sample = (at - (end - 1.0)) as f32;
                out.push(self.previous * (1.0 - toward_sample) + sample * toward_sample);
                self.produced += 1;
            }
        }
        self.previous = sample;
        self.seen += 1;
    }

    /// A recording of one sample has no interval to interpolate in: the sample is the whole of it.
    fn finish(&self, out: &mut Vec<f32>) {
        if self.seen == 1 {
            out.push(self.previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;
    use std::path::PathBuf;

    fn wav_spec(
        channels: u16,
        sample_rate: u32,
        bits_per_sample: u16,
        sample_format: hound::SampleFormat,
    ) -> hound::WavSpec {
        hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample,
            sample_format,
        }
    }

    /// A 16-bit WAV of `samples` (interleaved when there are several channels).
    fn write_i16(dir: &Path, channels: u16, sample_rate: u32, samples: &[i16]) -> PathBuf {
        let path = dir.join("audio.wav");
        let spec = wav_spec(channels, sample_rate, 16, hound::SampleFormat::Int);
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for sample in samples {
            writer.write_sample(*sample).unwrap();
        }
        writer.finalize().unwrap();
        path
    }

    fn write_f32(dir: &Path, channels: u16, sample_rate: u32, samples: &[f32]) -> PathBuf {
        let path = dir.join("audio.wav");
        let spec = wav_spec(channels, sample_rate, 32, hound::SampleFormat::Float);
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for sample in samples {
            writer.write_sample(*sample).unwrap();
        }
        writer.finalize().unwrap();
        path
    }

    fn assert_close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-4, "{actual:?} vs {expected:?}");
        }
    }

    #[test]
    fn read_wav_handles_stereo_44k() {
        let tmp = tempfile::tempdir().unwrap();
        // One second of 440 Hz, the same tone in both ears.
        let samples: Vec<i16> = (0..44_100)
            .flat_map(|n| {
                let tone = (TAU * 440.0 * n as f32 / 44_100.0).sin();
                let sample = (tone * 16_384.0) as i16;
                [sample, sample]
            })
            .collect();
        let path = write_i16(tmp.path(), 2, 44_100, &samples);

        let audio = read_wav_16k_mono(&path).unwrap();

        assert!(
            (15_999..=16_001).contains(&audio.len()),
            "got {} samples",
            audio.len()
        );
        // The pitch survives: a 440 Hz tone crosses zero about 880 times a second.
        let signs: Vec<bool> = audio
            .iter()
            .filter(|s| **s != 0.0)
            .map(|s| *s > 0.0)
            .collect();
        let crossings = signs.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(
            (878..=882).contains(&crossings),
            "got {crossings} crossings"
        );
        let peak = audio.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        assert!((0.45..=0.55).contains(&peak), "peak {peak}");
    }

    #[test]
    fn read_wav_rejects_non_wav() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("notes.wav");
        std::fs::write(&path, "this is not a WAV file").unwrap();

        assert!(matches!(
            read_wav_16k_mono(&path),
            Err(WhisperError::Audio(_))
        ));
    }

    #[test]
    fn read_wav_of_a_missing_file_is_an_audio_error() {
        let tmp = tempfile::tempdir().unwrap();

        assert!(matches!(
            read_wav_16k_mono(&tmp.path().join("missing.wav")),
            Err(WhisperError::Audio(_))
        ));
    }

    #[test]
    fn read_wav_keeps_16k_mono_samples_as_they_are() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_i16(
            tmp.path(),
            1,
            16_000,
            &[0, 16_384, -16_384, 32_767, -32_768],
        );

        let audio = read_wav_16k_mono(&path).unwrap();

        assert_eq!(audio, [0.0, 0.5, -0.5, 32_767.0 / 32_768.0, -1.0]);
    }

    #[test]
    fn read_wav_averages_the_channels() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_i16(
            tmp.path(),
            2,
            16_000,
            &[16_384, 0, 0, -16_384, 8_192, 8_192],
        );

        let audio = read_wav_16k_mono(&path).unwrap();

        assert_close(&audio, &[0.25, -0.25, 0.25]);
    }

    #[test]
    fn read_wav_reads_32_bit_float() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_f32(tmp.path(), 1, 16_000, &[0.25, -0.5, 1.0]);

        let audio = read_wav_16k_mono(&path).unwrap();

        assert_eq!(audio, [0.25, -0.5, 1.0]);
    }

    #[test]
    fn read_wav_upsamples_8k_linearly() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_i16(tmp.path(), 1, 8_000, &[0, 16_384, 0]);

        let audio = read_wav_16k_mono(&path).unwrap();

        // Twice the samples, the new ones halfway between their neighbours.
        assert_close(&audio, &[0.0, 0.25, 0.5, 0.25, 0.0]);
    }

    #[test]
    fn read_wav_rejects_a_sample_format_it_cannot_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("audio.wav");
        let spec = wav_spec(1, 16_000, 24, hound::SampleFormat::Int);
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        writer.write_sample(1_000_i32).unwrap();
        writer.finalize().unwrap();

        assert!(matches!(
            read_wav_16k_mono(&path),
            Err(WhisperError::Audio(_))
        ));
    }

    #[test]
    fn read_wav_of_no_samples_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_i16(tmp.path(), 1, 44_100, &[]);

        assert_eq!(read_wav_16k_mono(&path), Ok(Vec::new()));
    }

    #[test]
    fn a_loaded_model_can_be_shared_between_threads() {
        fn assert_send_and_sync<T: Send + Sync>() {}
        assert_send_and_sync::<LocalWhisper>();
    }

    #[test]
    fn load_missing_model_is_load_error() {
        let tmp = tempfile::tempdir().unwrap();

        let result = LocalWhisper::load(&tmp.path().join("ggml-tiny.bin"));

        assert!(matches!(result, Err(WhisperError::Load(_))), "{result:?}");
    }

    #[test]
    fn load_file_that_is_no_model_is_load_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ggml-tiny.bin");
        std::fs::write(&path, vec![7_u8; 4096]).unwrap();

        let result = LocalWhisper::load(&path);

        assert!(matches!(result, Err(WhisperError::Load(_))), "{result:?}");
    }

    #[test]
    fn prompt_is_cut_to_800_characters() {
        let long = "é".repeat(1000);

        let cut = cut_prompt(&long);

        assert_eq!(cut.chars().count(), 800);
        assert!(long.starts_with(&cut));
        assert_eq!(cut_prompt("Dictate, Whisper"), "Dictate, Whisper");
    }

    #[test]
    fn prompt_loses_nul_characters() {
        assert_eq!(cut_prompt("ab\0cd"), "abcd");
    }

    #[test]
    fn no_language_empty_or_auto_means_detect() {
        for language in [None, Some(""), Some("  "), Some("auto"), Some("AUTO")] {
            assert_eq!(language_choice(language), Ok(None), "{language:?}");
        }
    }

    #[test]
    fn a_known_language_code_is_kept() {
        assert_eq!(language_choice(Some("vi")), Ok(Some("vi")));
        assert_eq!(language_choice(Some(" en ")), Ok(Some("en")));
    }

    #[test]
    fn an_unknown_language_is_refused() {
        for language in ["klingon", "e\0n"] {
            assert!(
                matches!(
                    language_choice(Some(language)),
                    Err(WhisperError::Failed(_))
                ),
                "{language:?}"
            );
        }
    }
}
