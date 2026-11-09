//! A long recording through the transcription API, in parts.
//!
//! A provider takes a file of up to 25 MB and a chat model writes the whole transcript as output tokens, so a
//! recording goes in parts, one request at a time. The parts follow each other with no overlap. Each finished part
//! is saved in `<folder>/api-parts`, so that running the job again after a failure sends only the parts still
//! missing (also after switching the model).
//!
//! Neither the slice files nor the cache belong to the library format, but the cache is still written atomically:
//! a crash in the middle of a write must not leave a file that the next run would trust.

use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use dictate_core::meetings::atomic_write;
use dictate_core::text::segments::Segment;
use tokio_util::sync::CancellationToken;

use super::{language, supports_segments, transcribe_file, TranscribeConfig, TranscribeError};
use crate::audio::wav::write_16k_mono;

/// A boxed future, as `futures::future::BoxFuture`, which is the same type. Spelled out so that this crate needs no
/// crate for it.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The sample rate of the audio this module takes.
const SAMPLE_RATE: usize = 16_000;

/// How long a part is, in seconds: this long when the model writes timed segments...
const PART_S: f64 = 480.0;
/// ...shorter for a chat model, which has to write the transcript of the part in one reply...
const CHAT_PART_S: f64 = 300.0;
/// ...and shortest for a model that returns no times (so that the `[mm:ss]` marks of the transcript stay useful),
/// for a recording of more than `UNTIMED_LIMIT_S`.
const UNTIMED_PART_S: f64 = 120.0;
const UNTIMED_LIMIT_S: f64 = 120.0;

/// A reply that is cut off is asked for again as two halves, as long as the range is longer than this, in seconds.
const FLOOR_S: f64 = 60.0;

/// How long a request may take, as in the Swift app.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// A request that fails for a reason that may pass is tried twice more, after these many seconds.
const RETRY_WAITS_S: [u64; 2] = [5, 15];

/// What `transcribe_long` reports while it works.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiProgress {
    /// Part `index` of `count` (from 1) is starting, or, when `reused`, was done by an earlier run and is taken
    /// from the cache.
    Part {
        index: usize,
        count: usize,
        reused: bool,
    },
    /// A request of part `index` failed in a way that may pass: it is sent again in `wait_s` seconds.
    Retry {
        index: usize,
        count: usize,
        wait_s: u64,
        error: String,
    },
    /// The reply for the range that starts `from` seconds into the recording was cut off. With `keeping` the range
    /// is too short to split and what was written in full is kept; otherwise it is asked for again in two halves.
    CutOff { from: f64, keeping: bool },
}

/// Why a long recording did not get a transcript.
#[derive(Debug, Clone, PartialEq)]
pub enum LongApiError {
    /// Part `index` of `count` (from 1) still failed after its retries. `kept` parts before it are done and saved:
    /// running the job again continues with this part.
    PartFailed {
        index: usize,
        count: usize,
        kept: usize,
        error: TranscribeError,
    },
    /// The caller cancelled it.
    Cancelled,
    /// A slice of the audio could not be written.
    Io(String),
}

impl fmt::Display for LongApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LongApiError::PartFailed {
                index,
                count,
                kept,
                error,
            } => {
                write!(f, "Part {index}/{count}: {error}")?;
                match kept {
                    0 => Ok(()),
                    1 => write!(
                        f,
                        "\n1 finished part was kept; running it again continues from part {index}."
                    ),
                    _ => write!(
                        f,
                        "\n{kept} finished parts were kept; running it again continues from part {index}."
                    ),
                }
            }
            LongApiError::Cancelled => f.write_str("Cancelled."),
            LongApiError::Io(m) => write!(f, "Could not prepare the audio for the API: {m}"),
        }
    }
}

impl std::error::Error for LongApiError {}

/// How long the parts of a recording of `total` seconds are.
pub fn part_seconds(c: &TranscribeConfig, total: f64) -> f64 {
    if c.chat {
        CHAT_PART_S
    } else if !supports_segments(&c.model) && total > UNTIMED_LIMIT_S {
        UNTIMED_PART_S
    } else {
        PART_S
    }
}

/// The transcript of a whole recording (`samples_16k`: mono, 16 kHz), sent in parts. `folder` is the recording's
/// folder: the slices and the cache of finished parts are written in it. The times of the segments are from the
/// start of the recording. `sleep` waits between a failure and the next try (a test passes one that is over at
/// once). Cancelling is noticed before each part and after each wait; the request in flight is not stopped, but
/// dropping the future stops it, and the slice file goes with it.
pub async fn transcribe_long(
    c: &TranscribeConfig,
    samples_16k: &[f32],
    folder: &Path,
    cancel: &CancellationToken,
    sleep: &(dyn Fn(Duration) -> BoxFuture<'static, ()> + Sync),
    on_progress: impl FnMut(ApiProgress) + Send,
) -> Result<Vec<Segment>, LongApiError> {
    if samples_16k.is_empty() {
        // Nothing was recorded: there is nothing to send.
        return Ok(Vec::new());
    }
    let total = samples_16k.len() as f64 / SAMPLE_RATE as f64;
    let step = part_seconds(c, total);
    let step_samples = step as usize * SAMPLE_RATE;
    let count = samples_16k.len().div_ceil(step_samples);
    let parts_root = folder.join("api-parts");
    let parts_dir = parts_root.join(cache_dir_name(language(c), step));
    let mut run = Run {
        c,
        samples: samples_16k,
        folder,
        cancel,
        sleep,
        on_progress,
    };

    let mut segments = Vec::new();
    for part in 0..count {
        if cancel.is_cancelled() {
            return Err(LongApiError::Cancelled);
        }
        let index = part + 1;
        let cached = parts_dir.join(format!("{part}.json"));
        if let Some(saved) = read_part(&cached) {
            (run.on_progress)(ApiProgress::Part {
                index,
                count,
                reused: true,
            });
            segments.extend(saved);
            continue;
        }
        (run.on_progress)(ApiProgress::Part {
            index,
            count,
            reused: false,
        });
        let from = part * step_samples;
        let to = (from + step_samples).min(samples_16k.len());
        let range = Range {
            start: from as f64 / SAMPLE_RATE as f64,
            length: (to - from) as f64 / SAMPLE_RATE as f64,
        };
        let done = run.part(index, count, range).await?;
        save_part(&parts_dir, &cached, &done);
        segments.extend(done);
    }
    // Everything is done: the cache is not needed any more (and neither is that of another language or part length).
    let _ = std::fs::remove_dir_all(&parts_root);
    Ok(segments)
}

/// A stretch of the recording, in seconds from its start.
#[derive(Clone, Copy)]
struct Range {
    start: f64,
    length: f64,
}

/// What one request for a range gave.
enum Reply {
    Whole(Vec<Segment>),
    /// The reply was cut off: the segments written in full before the cut.
    Cut(Vec<Segment>),
}

/// The state of one `transcribe_long`.
struct Run<'a, F> {
    c: &'a TranscribeConfig,
    samples: &'a [f32],
    folder: &'a Path,
    cancel: &'a CancellationToken,
    sleep: &'a (dyn Fn(Duration) -> BoxFuture<'static, ()> + Sync),
    on_progress: F,
}

impl<F: FnMut(ApiProgress) + Send> Run<'_, F> {
    /// The segments of one part (number `index` of `count`, from 1), with times from the start of the recording.
    /// A range whose reply is cut off is asked for again as two halves, one after the other, down to `FLOOR_S`.
    async fn part(
        &mut self,
        index: usize,
        count: usize,
        whole: Range,
    ) -> Result<Vec<Segment>, LongApiError> {
        let mut segments = Vec::new();
        // The ranges still to ask for, the next one last.
        let mut pending = vec![whole];
        while let Some(range) = pending.pop() {
            match self.request(index, count, range).await? {
                Reply::Whole(written) => segments.extend(written),
                Reply::Cut(written) => {
                    let keeping = range.length <= FLOOR_S;
                    (self.on_progress)(ApiProgress::CutOff {
                        from: range.start,
                        keeping,
                    });
                    if keeping {
                        segments.extend(written);
                    } else {
                        let half = (range.length / 2.0).round();
                        pending.push(Range {
                            start: range.start + half,
                            length: range.length - half,
                        });
                        pending.push(Range {
                            start: range.start,
                            length: half,
                        });
                    }
                }
            }
        }
        Ok(segments)
    }

    /// One range, sent as a slice file of its own. A failure that may pass is tried again after 5 s and then 15 s.
    async fn request(
        &mut self,
        index: usize,
        count: usize,
        range: Range,
    ) -> Result<Reply, LongApiError> {
        let slice = self.write_slice(range)?;
        let mut tries = 0;
        loop {
            let shifted = |segments: Vec<Segment>| -> Vec<Segment> {
                segments
                    .into_iter()
                    .map(|segment| Segment {
                        start: range.start + segment.start,
                        ..segment
                    })
                    .collect()
            };
            match transcribe_file(self.c, slice.path(), Some(range.length), REQUEST_TIMEOUT).await {
                Ok(segments) => return Ok(Reply::Whole(shifted(segments))),
                Err(TranscribeError::CutOff { segments }) => {
                    return Ok(Reply::Cut(shifted(segments)))
                }
                Err(TranscribeError::Api(e)) if tries < RETRY_WAITS_S.len() && e.is_transient() => {
                    let wait_s = RETRY_WAITS_S[tries];
                    tries += 1;
                    (self.on_progress)(ApiProgress::Retry {
                        index,
                        count,
                        wait_s,
                        error: e.to_string(),
                    });
                    self.wait(wait_s).await?;
                }
                Err(error) => {
                    return Err(LongApiError::PartFailed {
                        index,
                        count,
                        // The parts go one after the other and the first failure ends the run.
                        kept: index - 1,
                        error,
                    });
                }
            }
        }
    }

    /// `<folder>/api-slice-<start>.wav`: the samples of `range`.
    fn write_slice(&self, range: Range) -> Result<SliceFile, LongApiError> {
        let sample_at = |seconds: f64| {
            ((seconds * SAMPLE_RATE as f64).round() as usize).min(self.samples.len())
        };
        let from = sample_at(range.start);
        let to = sample_at(range.start + range.length);
        let slice = SliceFile(
            self.folder
                .join(format!("api-slice-{}.wav", range.start as u64)),
        );
        // The guard exists before the file does: whatever is written of it goes if the write fails.
        write_16k_mono(slice.path(), &self.samples[from..to])
            .map_err(|e| LongApiError::Io(format!("{}: {e}", slice.path().display())))?;
        Ok(slice)
    }

    /// Waits `seconds`, and notices it when the caller cancels in the meantime.
    async fn wait(&self, seconds: u64) -> Result<(), LongApiError> {
        let over = (self.sleep)(Duration::from_secs(seconds));
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => {}
            () = over => {}
        }
        if self.cancel.is_cancelled() {
            return Err(LongApiError::Cancelled);
        }
        Ok(())
    }
}

/// A slice of the audio on disk, deleted when it is dropped: after its request, on an error, or when the caller
/// drops the future.
struct SliceFile(PathBuf);

impl SliceFile {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for SliceFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The name of the folder of the cache of one run, `<language or "auto">-<part length>s`. A run with another language
/// or part length cuts the recording in other places: its parts cannot be reused. Characters other than letters,
/// digits and `-._` become `_`.
fn cache_dir_name(language: Option<&str>, step: f64) -> String {
    format!("{}-{}s", language.unwrap_or("auto"), step as u64)
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || "-._".contains(ch) {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// A part saved by this or an earlier run; `None` when there is none, or it is not readable (a crash can leave
/// anything): that part is then sent again.
fn read_part(file: &Path) -> Option<Vec<Segment>> {
    serde_json::from_slice(&std::fs::read(file).ok()?).ok()
}

/// Saves a finished part. When that fails the run goes on: the part is only not there for the next run.
fn save_part(dir: &Path, file: &Path, segments: &[Segment]) {
    let Ok(json) = serde_json::to_vec(segments) else {
        return;
    };
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = atomic_write(file, &json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::ApiError;
    use serde_json::json;
    use std::sync::Mutex;
    use tempfile::TempDir;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    /// Long enough that hiding it in a message cannot garble an ordinary word.
    const KEY: &str = "sk-test-key-123";
    const RATE: usize = 16_000;

    fn config(base_url: &str, model: &str, chat: bool) -> TranscribeConfig {
        TranscribeConfig {
            base_url: base_url.to_string(),
            model: model.to_string(),
            api_key: KEY.to_string(),
            language: None,
            prompt: String::new(),
            chat,
        }
    }

    fn segment(start: f64, text: &str) -> Segment {
        Segment {
            start,
            speaker: None,
            text: text.to_string(),
        }
    }

    /// `seconds` of silence at 16 kHz.
    fn silence(seconds: usize) -> Vec<f32> {
        vec![0.0; seconds * RATE]
    }

    /// The folder of a recording: a name with spaces and Vietnamese letters, as a library folder may have.
    fn recording_folder() -> (TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Cuộc họp 2026-11-10");
        std::fs::create_dir(&folder).unwrap();
        (dir, folder)
    }

    /// The fake sleep: it notes how long it was asked to wait and is over at once.
    #[derive(Default)]
    struct Waits(Mutex<Vec<u64>>);

    impl Waits {
        fn sleep(&self, wait: Duration) -> BoxFuture<'static, ()> {
            self.0.lock().unwrap().push(wait.as_secs());
            Box::pin(async {})
        }

        fn seen(&self) -> Vec<u64> {
            self.0.lock().unwrap().clone()
        }
    }

    /// The start, in seconds, that the upload's file name gives its slice: `api-slice-<start>.wav`.
    fn slice_start(request: &Request) -> u64 {
        let marker = b"filename=\"api-slice-";
        let at = request
            .body
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("the upload carries an api-slice file name")
            + marker.len();
        let digits: String = request.body[at..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .map(|&byte| byte as char)
            .collect();
        let name_end = &request.body[at + digits.len()..];
        assert!(name_end.starts_with(b".wav\""), "the slice is a WAV file");
        digits.parse().unwrap()
    }

    /// The WAV file inside an upload (RIFF header and 16-bit samples).
    fn wav_of(request: &Request) -> &[u8] {
        let at = request
            .body
            .windows(4)
            .position(|window| window == b"RIFF")
            .expect("the upload holds a WAV file");
        let data_len = u32::from_le_bytes(request.body[at + 40..at + 44].try_into().unwrap());
        &request.body[at..at + 44 + data_len as usize]
    }

    /// How many seconds of audio an upload holds.
    fn wav_seconds(request: &Request) -> f64 {
        (wav_of(request).len() - 44) as f64 / 2.0 / RATE as f64
    }

    fn wav_sample(request: &Request, index: usize) -> i16 {
        let wav = wav_of(request);
        i16::from_le_bytes(wav[44 + 2 * index..46 + 2 * index].try_into().unwrap())
    }

    /// How many seconds of audio a chat request holds, as base64 WAV in the message.
    fn chat_audio_seconds(request: &Request) -> f64 {
        use base64::Engine;
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let data = body["messages"][0]["content"][1]["input_audio"]["data"]
            .as_str()
            .unwrap();
        let wav = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        (wav.len() - 44) as f64 / 2.0 / RATE as f64
    }

    /// The reply of a whisper model: one segment, a second into its slice, naming the slice.
    fn whisper_reply(request: &Request) -> ResponseTemplate {
        let start = slice_start(request);
        ResponseTemplate::new(200).set_body_json(json!({"segments": [
            {"start": 1.0, "text": format!("part at {start}")}
        ]}))
    }

    /// A chat model's reply: `content` and why it stopped.
    fn chat_reply(content: &str, finish_reason: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"choices": [
            {"finish_reason": finish_reason, "message": {"role": "assistant", "content": content}}
        ]}))
    }

    async fn server_with(
        respond: impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static,
    ) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(respond)
            .mount(&server)
            .await;
        server
    }

    fn files_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn part_seconds_rules() {
        let url = "http://localhost";
        assert_eq!(part_seconds(&config(url, "whisper-1", false), 600.0), 480.0);
        assert_eq!(
            part_seconds(&config(url, "google/gemini-3.5-flash", true), 600.0),
            300.0
        );
        assert_eq!(
            part_seconds(&config(url, "gpt-4o-transcribe", false), 600.0),
            120.0
        );
        assert_eq!(
            part_seconds(&config(url, "gpt-4o-transcribe", false), 100.0),
            480.0
        );
    }

    #[tokio::test]
    async fn api_long_offsets_and_merges_parts() {
        let server = server_with(whisper_reply).await;
        let (_dir, folder) = recording_folder();
        // Markers on both sides of the first cut: the last sample of part 1 and the first of part 2.
        let mut samples = silence(20 * 60);
        samples[480 * RATE - 1] = -0.5;
        samples[480 * RATE] = 0.5;
        let waits = Waits::default();
        let mut events = Vec::new();

        let segments = transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &samples,
            &folder,
            &CancellationToken::new(),
            &|wait| waits.sleep(wait),
            |progress| events.push(progress),
        )
        .await
        .unwrap();

        assert_eq!(
            segments,
            [
                segment(1.0, "part at 0"),
                segment(481.0, "part at 480"),
                segment(961.0, "part at 960"),
            ]
        );
        let requests = server.received_requests().await.unwrap();
        let slices: Vec<(u64, f64)> = requests
            .iter()
            .map(|request| (slice_start(request), wav_seconds(request)))
            .collect();
        assert_eq!(slices, [(0, 480.0), (480, 480.0), (960, 240.0)]);
        // The cut falls between the two markers, so no sample is in two parts and none is lost.
        assert_eq!(wav_sample(&requests[0], 480 * RATE - 1), -16_384);
        assert_eq!(wav_sample(&requests[1], 0), 16_384);
        assert_eq!(
            events,
            [
                ApiProgress::Part {
                    index: 1,
                    count: 3,
                    reused: false
                },
                ApiProgress::Part {
                    index: 2,
                    count: 3,
                    reused: false
                },
                ApiProgress::Part {
                    index: 3,
                    count: 3,
                    reused: false
                },
            ]
        );
        assert!(waits.seen().is_empty());
    }

    #[tokio::test]
    async fn api_long_reuses_cached_parts() {
        let server = server_with(whisper_reply).await;
        let (_dir, folder) = recording_folder();
        let cache = folder.join("api-parts").join("auto-480s");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(
            cache.join("0.json"),
            r#"[{"start":3.5,"speaker":"Speaker 1","text":"from the last run"}]"#,
        )
        .unwrap();
        let mut events = Vec::new();

        let segments = transcribe_long(
            // A model other than the one that did part 1: the parts are reused after switching it.
            &config(&server.uri(), "whisper-large-v3", false),
            &silence(20 * 60),
            &folder,
            &CancellationToken::new(),
            &|wait| Waits::default().sleep(wait),
            |progress| events.push(progress),
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        let sent: Vec<u64> = requests.iter().map(slice_start).collect();
        assert_eq!(sent, [480, 960], "only the parts that are missing are sent");
        assert_eq!(
            segments,
            [
                Segment {
                    start: 3.5,
                    speaker: Some("Speaker 1".to_string()),
                    text: "from the last run".to_string()
                },
                segment(481.0, "part at 480"),
                segment(961.0, "part at 960"),
            ]
        );
        assert_eq!(
            events[0],
            ApiProgress::Part {
                index: 1,
                count: 3,
                reused: true
            }
        );
        assert_eq!(
            events[1],
            ApiProgress::Part {
                index: 2,
                count: 3,
                reused: false
            }
        );
    }

    #[tokio::test]
    async fn api_long_retries_transient_then_fails_with_kept_parts() {
        let server = server_with(|request| match slice_start(request) {
            480 => ResponseTemplate::new(503).set_body_string("busy"),
            _ => whisper_reply(request),
        })
        .await;
        let (_dir, folder) = recording_folder();
        let waits = Waits::default();
        let mut events = Vec::new();

        let err = transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &silence(20 * 60),
            &folder,
            &CancellationToken::new(),
            &|wait| waits.sleep(wait),
            |progress| events.push(progress),
        )
        .await
        .unwrap_err();

        let LongApiError::PartFailed {
            index,
            count,
            kept,
            error,
        } = err
        else {
            panic!("expected PartFailed, got {err:?}");
        };
        assert_eq!((index, count, kept), (2, 3, 1));
        assert!(matches!(
            error,
            TranscribeError::Api(ApiError::Status { status: 503, .. })
        ));
        assert_eq!(waits.seen(), [5, 15]);
        let retries: Vec<(usize, u64)> = events
            .iter()
            .filter_map(|event| match event {
                ApiProgress::Retry {
                    index,
                    count: 3,
                    wait_s,
                    ..
                } => Some((*index, *wait_s)),
                _ => None,
            })
            .collect();
        assert_eq!(retries, [(2, 5), (2, 15)]);
        // One request for part 1, three for part 2; part 3 is not tried.
        assert_eq!(server.received_requests().await.unwrap().len(), 4);
        // Part 1 stays for the next run, in the shape the Swift app wrote; the slice does not.
        let kept_part =
            std::fs::read_to_string(folder.join("api-parts").join("auto-480s").join("0.json"))
                .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&kept_part).unwrap(),
            json!([{"start": 1.0, "text": "part at 0"}])
        );
        assert_eq!(files_in(&folder), ["api-parts"]);
        assert_eq!(
            files_in(&folder.join("api-parts").join("auto-480s")),
            ["0.json"]
        );
    }

    #[tokio::test]
    async fn api_long_a_transient_error_that_clears_is_retried() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let calls = std::sync::Arc::new(calls);
        let seen = calls.clone();
        let server = server_with(move |request| {
            match seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => ResponseTemplate::new(429).set_body_string("slow down"),
                1 => ResponseTemplate::new(503).set_body_string("busy"),
                _ => whisper_reply(request),
            }
        })
        .await;
        let (_dir, folder) = recording_folder();
        let waits = Waits::default();

        let segments = transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &silence(100),
            &folder,
            &CancellationToken::new(),
            &|wait| waits.sleep(wait),
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(segments, [segment(1.0, "part at 0")]);
        assert_eq!(waits.seen(), [5, 15]);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn api_long_a_bad_key_is_not_retried() {
        let server = server_with(|_| {
            ResponseTemplate::new(401).set_body_json(json!({"error": {"message": "Bad key"}}))
        })
        .await;
        let (_dir, folder) = recording_folder();
        let waits = Waits::default();

        let err = transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &silence(100),
            &folder,
            &CancellationToken::new(),
            &|wait| waits.sleep(wait),
            |_| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            LongApiError::PartFailed {
                index: 1,
                count: 1,
                kept: 0,
                error: TranscribeError::Api(ApiError::Status { status: 401, .. })
            }
        ));
        assert!(waits.seen().is_empty());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn api_long_halves_cut_off_ranges() {
        let cut_first = r#"{"segments":[{"start":1,"text":"cut"},{"start":5,"te"#;
        let replies = [
            chat_reply(cut_first, "length"),
            chat_reply(r#"{"segments":[{"start":1,"text":"first half"}]}"#, "stop"),
            chat_reply(cut_first, "length"),
            chat_reply(r#"{"segments":[{"start":3,"text":"third"}]}"#, "stop"),
            chat_reply(
                r#"{"segments":[{"start":4,"text":"kept"},{"start":9,"te"#,
                "length",
            ),
        ];
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let server = server_with(move |_| {
            let call = seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            replies[call].clone()
        })
        .await;
        let (_dir, folder) = recording_folder();
        let mut events = Vec::new();

        let segments = transcribe_long(
            &config(&server.uri(), "google/gemini-3.5-flash", true),
            &silence(130),
            &folder,
            &CancellationToken::new(),
            &|wait| Waits::default().sleep(wait),
            |progress| events.push(progress),
        )
        .await
        .unwrap();

        // 130 s is asked again as 65 + 65; the second 65 s is cut again and becomes 33 + 32, and the 32 s
        // that is cut a third time is a stretch under a minute: what was written in full is kept.
        let requests = server.received_requests().await.unwrap();
        let lengths: Vec<f64> = requests.iter().map(chat_audio_seconds).collect();
        assert_eq!(lengths, [130.0, 65.0, 65.0, 33.0, 32.0]);
        assert_eq!(
            segments,
            [
                segment(1.0, "first half"),
                segment(68.0, "third"),
                segment(102.0, "kept"),
            ]
        );
        assert_eq!(
            events,
            [
                ApiProgress::Part {
                    index: 1,
                    count: 1,
                    reused: false
                },
                ApiProgress::CutOff {
                    from: 0.0,
                    keeping: false
                },
                ApiProgress::CutOff {
                    from: 65.0,
                    keeping: false
                },
                ApiProgress::CutOff {
                    from: 98.0,
                    keeping: true
                },
            ]
        );
    }

    #[tokio::test]
    async fn api_long_keeps_what_a_short_part_wrote_before_it_was_cut() {
        let server = server_with(|_| {
            chat_reply(
                r#"{"segments":[{"start":2,"text":"kept"},{"start":9,"te"#,
                "length",
            )
        })
        .await;
        let (_dir, folder) = recording_folder();
        let mut events = Vec::new();

        let segments = transcribe_long(
            &config(&server.uri(), "google/gemini-3.5-flash", true),
            &silence(55),
            &folder,
            &CancellationToken::new(),
            &|wait| Waits::default().sleep(wait),
            |progress| events.push(progress),
        )
        .await
        .unwrap();

        assert_eq!(segments, [segment(2.0, "kept")]);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert_eq!(
            events[1],
            ApiProgress::CutOff {
                from: 0.0,
                keeping: true
            }
        );
    }

    #[tokio::test]
    async fn api_long_removes_slices_and_cache_on_success() {
        let server = server_with(whisper_reply).await;
        let (_dir, folder) = recording_folder();
        // A cache of another run (another language) goes with the rest.
        let other = folder.join("api-parts").join("vi-480s");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("0.json"), "[]").unwrap();

        transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &silence(20 * 60),
            &folder,
            &CancellationToken::new(),
            &|wait| Waits::default().sleep(wait),
            |_| {},
        )
        .await
        .unwrap();

        assert!(files_in(&folder).is_empty(), "left {:?}", files_in(&folder));
    }

    #[tokio::test]
    async fn api_long_cancel_stops_before_next_part() {
        let server = server_with(whisper_reply).await;
        let (_dir, folder) = recording_folder();
        let cancel = CancellationToken::new();

        let err = transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &silence(20 * 60),
            &folder,
            &cancel,
            &|wait| Waits::default().sleep(wait),
            |progress| {
                if let ApiProgress::Part { index: 1, .. } = progress {
                    cancel.cancel();
                }
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err, LongApiError::Cancelled);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        // The part that was done stays, so that the next run continues from part 2.
        assert_eq!(files_in(&folder), ["api-parts"]);
        assert_eq!(
            files_in(&folder.join("api-parts").join("auto-480s")),
            ["0.json"]
        );
    }

    #[tokio::test]
    async fn api_long_cancel_after_a_wait_stops_the_retry() {
        let server = server_with(|_| ResponseTemplate::new(503).set_body_string("busy")).await;
        let (_dir, folder) = recording_folder();
        let cancel = CancellationToken::new();

        let err = transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &silence(100),
            &folder,
            &cancel,
            &|_| {
                // The user cancels while the run waits: the wait is over just as it happens.
                let token = cancel.clone();
                Box::pin(async move { token.cancel() })
            },
            |_| {},
        )
        .await
        .unwrap_err();

        assert_eq!(err, LongApiError::Cancelled);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert!(files_in(&folder).is_empty(), "the slice is gone");
    }

    #[tokio::test]
    async fn api_long_a_cancelled_run_sends_nothing() {
        let server = server_with(whisper_reply).await;
        let (_dir, folder) = recording_folder();
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &silence(100),
            &folder,
            &cancel,
            &|wait| Waits::default().sleep(wait),
            |_| {},
        )
        .await
        .unwrap_err();

        assert_eq!(err, LongApiError::Cancelled);
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(files_in(&folder).is_empty());
    }

    #[tokio::test]
    async fn api_long_a_broken_cache_file_is_sent_again() {
        let server = server_with(whisper_reply).await;
        let (_dir, folder) = recording_folder();
        let cache = folder.join("api-parts").join("auto-480s");
        std::fs::create_dir_all(&cache).unwrap();
        // A crash can leave anything behind.
        std::fs::write(cache.join("0.json"), r#"[{"start":3.5,"te"#).unwrap();

        let segments = transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &silence(100),
            &folder,
            &CancellationToken::new(),
            &|wait| Waits::default().sleep(wait),
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(segments, [segment(1.0, "part at 0")]);
    }

    #[tokio::test]
    async fn api_long_cache_folder_names_the_language_and_part_length() {
        // A model that returns no timed segments gets parts of 120 s: 130 s are two parts, and the second fails.
        let server = server_with(|request| match slice_start(request) {
            0 => ResponseTemplate::new(200).set_body_json(json!({"text": "first"})),
            _ => ResponseTemplate::new(400).set_body_string("no"),
        })
        .await;
        let (_dir, folder) = recording_folder();
        let mut c = config(&server.uri(), "gpt-4o-transcribe", false);
        // Characters that a folder name cannot hold become `_`, and the blanks around the code are not part of it.
        c.language = Some(" vi/VN ".to_string());

        let err = transcribe_long(
            &c,
            &silence(130),
            &folder,
            &CancellationToken::new(),
            &|wait| Waits::default().sleep(wait),
            |_| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            LongApiError::PartFailed {
                index: 2,
                count: 2,
                kept: 1,
                ..
            }
        ));
        assert_eq!(files_in(&folder.join("api-parts")), ["vi_VN-120s"]);
    }

    #[tokio::test]
    async fn api_long_of_no_audio_sends_nothing() {
        let server = server_with(whisper_reply).await;
        let (_dir, folder) = recording_folder();

        let segments = transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &[],
            &folder,
            &CancellationToken::new(),
            &|wait| Waits::default().sleep(wait),
            |_| {},
        )
        .await
        .unwrap();

        assert!(segments.is_empty());
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_long_a_folder_that_is_missing_is_an_io_error() {
        let server = server_with(whisper_reply).await;
        let dir = tempfile::tempdir().unwrap();

        let err = transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &silence(10),
            &dir.path().join("missing"),
            &CancellationToken::new(),
            &|wait| Waits::default().sleep(wait),
            |_| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(err, LongApiError::Io(_)), "{err:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_long_errors_and_progress_never_hold_the_key() {
        // A provider that echoes the key in its message, as a proxy may.
        let server = server_with(|_| {
            ResponseTemplate::new(503)
                .set_body_json(json!({"error": {"message": format!("bad {KEY} here")}}))
        })
        .await;
        let (_dir, folder) = recording_folder();
        let mut events = Vec::new();

        let err = transcribe_long(
            &config(&server.uri(), "whisper-1", false),
            &silence(100),
            &folder,
            &CancellationToken::new(),
            &|wait| Waits::default().sleep(wait),
            |progress| events.push(progress),
        )
        .await
        .unwrap_err();

        assert!(!format!("{err:?} {err} {events:?}").contains(KEY));
        assert!(events
            .iter()
            .any(|event| matches!(event, ApiProgress::Retry { .. })));
    }

    #[test]
    fn errors_read_as_sentences() {
        let failed = |index, kept| LongApiError::PartFailed {
            index,
            count: 3,
            kept,
            error: TranscribeError::Api(ApiError::Status {
                status: 503,
                message: "busy".to_string(),
            }),
        };
        assert_eq!(failed(1, 0).to_string(), "Part 1/3: API error 503: busy");
        assert_eq!(
            failed(3, 2).to_string(),
            "Part 3/3: API error 503: busy\n2 finished parts were kept; running it again continues from part 3."
        );
        assert_eq!(
            failed(2, 1).to_string(),
            "Part 2/3: API error 503: busy\n1 finished part was kept; running it again continues from part 2."
        );
        assert_eq!(LongApiError::Cancelled.to_string(), "Cancelled.");
        assert_eq!(
            LongApiError::Io("disk full".to_string()).to_string(),
            "Could not prepare the audio for the API: disk full"
        );
    }
}
