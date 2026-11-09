//! Speaker separation: who speaks when, worked out offline with sherpa-onnx (pyannote segmentation, then a speaker
//! embedding per segment, then clustering).
//!
//! It takes the 16 kHz mono samples the rest of the pipeline already holds and gives `Turn`s, which the pipeline
//! lays over the word times of Whisper. The models are two files that are downloaded on first use
//! (`DIARIZE_MODELS`, by `whisper::models::download_file`).
//!
//! The raw bindings (`sherpa-onnx-sys`) are used, not the `sherpa-onnx` wrapper: the wrapper does not give out the
//! pointer that the one call with progress needs.

use std::any::Any;
use std::ffi::{c_void, CString};
use std::fmt;
use std::fs;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use dictate_core::text::whisper::Turn;
use sherpa_onnx_sys as sys;

use crate::whisper::models::RemoteFile;

/// The two models, segmentation first: pyannote segmentation 3.0 (MIT) as re-hosted by the sherpa-onnx author, which
/// needs no login, and NVIDIA NeMo's TitaNet-small speaker embedding (CC-BY-4.0). Both hosts answer with a redirect
/// to a CDN. The tag of the second really is spelled "recongition".
pub const DIARIZE_MODELS: [RemoteFile; 2] = [
    RemoteFile {
        url: "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.onnx",
        file: "pyannote-segmentation-3.0.onnx",
        size: 5_992_913,
    },
    RemoteFile {
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx",
        file: "nemo_en_titanet_small.onnx",
        size: 40_257_283,
    },
];

/// Why speaker separation did not give turns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiarizeError {
    /// One of the models is not in the folder (or is not whole): download them first.
    MissingModels,
    /// The library could not load the models, or could not process the audio.
    Failed(String),
}

impl fmt::Display for DiarizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiarizeError::MissingModels => {
                f.write_str("The speaker separation models are not downloaded.")
            }
            DiarizeError::Failed(message) => write!(f, "Speaker separation failed: {message}"),
        }
    }
}

impl std::error::Error for DiarizeError {}

/// The models of `DIARIZE_MODELS` that are not in `dir` as whole files, in their order. A file counts only at exactly
/// its size: a `.part` of it does not, nor does a file of another size (an older release of the model, a broken
/// copy), and downloading it again puts the right one in its place.
pub fn missing_models(dir: &Path) -> Vec<&'static RemoteFile> {
    DIARIZE_MODELS
        .iter()
        .filter(|model| {
            !fs::metadata(dir.join(model.file)).is_ok_and(|m| m.is_file() && m.len() == model.size)
        })
        .collect()
}

/// The threads the models run on: the cores of the machine up to four, which is as many as they can use.
fn threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4)
}

/// The library's configuration, with the C strings that its pointers point into.
struct ModelConfig {
    raw: sys::OfflineSpeakerDiarizationConfig,
    /// The paths of the two models: `raw` holds pointers into these, so they live as long as it does.
    _paths: [CString; 2],
}

/// The configuration for the models in `dir`: no speaker count given (it is found out from the audio, where the
/// embeddings of two speakers are further than 0.5 apart), the other values the library's own defaults.
fn model_config(dir: &Path) -> Result<ModelConfig, DiarizeError> {
    let path = |model: &RemoteFile| {
        dir.join(model.file)
            .to_str()
            .and_then(|p| CString::new(p).ok())
            .ok_or_else(|| {
                DiarizeError::Failed(
                    "The path of the model folder cannot be handed to the speaker separation library."
                        .into(),
                )
            })
    };
    let paths = [path(&DIARIZE_MODELS[0])?, path(&DIARIZE_MODELS[1])?];
    let threads = i32::try_from(threads()).unwrap_or(1);
    let cpu = c"cpu".as_ptr();
    let raw = sys::OfflineSpeakerDiarizationConfig {
        segmentation: sys::OfflineSpeakerSegmentationModelConfig {
            pyannote: sys::OfflineSpeakerSegmentationPyannoteModelConfig {
                model: paths[0].as_ptr(),
                window_shift_ratio: 0.1,
            },
            num_threads: threads,
            debug: 0,
            provider: cpu,
        },
        embedding: sys::SpeakerEmbeddingExtractorConfig {
            model: paths[1].as_ptr(),
            num_threads: threads,
            debug: 0,
            provider: cpu,
        },
        clustering: sys::FastClusteringConfig {
            num_clusters: -1,
            threshold: 0.5,
            compute_confidence: 0,
        },
        min_duration_on: 0.3,
        min_duration_off: 0.5,
    };
    Ok(ModelConfig { raw, _paths: paths })
}

/// The library's diarizer, which is freed with it.
struct Handle(*const sys::OfflineSpeakerDiarization);

// SAFETY: the library keeps no thread-local state for an object, so one may be used from another thread than the
// one that made it, as long as it is used from one at a time: `Diarizer` takes care of that with a lock.
unsafe impl Send for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the pointer came from the library's `Create` and is freed only here.
        unsafe { sys::SherpaOnnxDestroyOfflineSpeakerDiarization(self.0) }
    }
}

/// Speaker separation with the models loaded: loading takes a moment and the models take memory, so one is kept for
/// as long as it is needed. It can be shared between threads; a second `run` waits for the first to end.
pub struct Diarizer {
    handle: Mutex<Handle>,
}

impl Diarizer {
    /// Loads the models from `dir`. `MissingModels` when they are not all there: the library itself answers a path
    /// that is wrong with no reason at all, so the files are looked at first. (A file of the right size that is not
    /// the model is not found out here: the library stops the program on it.)
    pub fn load(dir: &Path) -> Result<Diarizer, DiarizeError> {
        if !missing_models(dir).is_empty() {
            return Err(DiarizeError::MissingModels);
        }
        let config = model_config(dir)?;
        // SAFETY: `config` is a whole configuration whose strings stay alive for the call.
        let handle = unsafe { sys::SherpaOnnxCreateOfflineSpeakerDiarization(&config.raw) };
        if handle.is_null() {
            return Err(DiarizeError::Failed(
                "The speaker separation models could not be loaded.".into(),
            ));
        }
        Ok(Diarizer {
            handle: Mutex::new(Handle(handle)),
        })
    }

    /// Finds the speaker turns in 16 kHz mono samples (in -1..1), ordered by start. `on_progress` hears a fraction
    /// from 0 to 1 as the library finishes its chunks, ending with 1.
    ///
    /// The library reports only the second part of its work (the speaker embeddings): for the first part, the
    /// segmentation, nothing is heard. Measured on 10 minutes of audio on Apple silicon with 4 threads, that is the
    /// first 15 of 54 seconds, so a progress bar needs to treat "no call yet" as "preparing".
    ///
    /// It cannot be stopped: the library has no way to cancel. The caller that is cancelled lets it end and throws
    /// the answer away. It takes about 0.06 to 0.14 of the audio's length on Apple silicon, and its memory grows with
    /// the length too, since all of the audio is processed at once.
    pub fn run(
        &self,
        samples_16k: &[f32],
        mut on_progress: impl FnMut(f32),
    ) -> Result<Vec<Turn>, DiarizeError> {
        if samples_16k.is_empty() {
            return Ok(Vec::new());
        }
        let n = i32::try_from(samples_16k.len()).map_err(|_| {
            DiarizeError::Failed("The recording is too long to separate its speakers.".into())
        })?;
        let mut progress = Progress {
            callback: &mut on_progress,
            panic: None,
        };
        let answer = {
            let handle = self.handle.lock().unwrap_or_else(PoisonError::into_inner);
            // SAFETY: the handle is a live diarizer and nothing else uses it while the lock is held; `samples_16k`
            // holds `n` floats; `progress` outlives the call and is what `on_chunk` takes as its argument.
            let answer = unsafe {
                SherpaOnnxOfflineSpeakerDiarizationProcessWithCallback(
                    handle.0,
                    samples_16k.as_ptr(),
                    n,
                    Some(on_chunk),
                    (&mut progress as *mut Progress<'_>).cast(),
                )
            };
            Answer(answer)
        };
        if let Some(payload) = progress.panic.take() {
            // The callback panicked: the library was let finish (the unwinding must not go through its frames), and
            // now the panic goes on, after `answer` is freed.
            drop(answer);
            resume_unwind(payload);
        }
        if answer.0.is_null() {
            return Err(DiarizeError::Failed(
                "The library could not process the audio.".into(),
            ));
        }
        Ok(turns_from(&answer.segments()))
    }
}

/// The result object of a call, which is freed with it.
struct Answer(*const sys::OfflineSpeakerDiarizationResult);

impl Answer {
    /// Its segments, ordered by start time.
    fn segments(&self) -> Vec<sys::OfflineSpeakerDiarizationSegment> {
        // SAFETY: the pointer is a live result. `SortByStartTime` gives an array of `NumSegments` segments that
        // `DestroySegment` frees, and nothing of it is used after that.
        unsafe {
            let count = sys::SherpaOnnxOfflineSpeakerDiarizationResultGetNumSegments(self.0);
            let count = usize::try_from(count).unwrap_or(0);
            let array = sys::SherpaOnnxOfflineSpeakerDiarizationResultSortByStartTime(self.0);
            if array.is_null() || count == 0 {
                return Vec::new();
            }
            let segments = std::slice::from_raw_parts(array, count).to_vec();
            sys::SherpaOnnxOfflineSpeakerDiarizationDestroySegment(array);
            segments
        }
    }
}

impl Drop for Answer {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from the library's `Process` and is freed only here.
            unsafe { sys::SherpaOnnxOfflineSpeakerDiarizationDestroyResult(self.0) }
        }
    }
}

/// The turns of a list of segments, in its order. The library numbers the speakers from 0.
fn turns_from(segments: &[sys::OfflineSpeakerDiarizationSegment]) -> Vec<Turn> {
    segments
        .iter()
        .filter_map(|segment| {
            Some(Turn {
                start: f64::from(segment.start),
                end: f64::from(segment.end),
                speaker: u32::try_from(segment.speaker).ok()?,
            })
        })
        .collect()
}

/// What the library's progress call carries back: the caller's callback, and a panic that came out of it, which
/// must not unwind through the library's frames.
struct Progress<'a> {
    callback: &'a mut dyn FnMut(f32),
    panic: Option<Box<dyn Any + Send>>,
}

/// The library's progress call: `done` of `total` chunks are finished. Its answer is not looked at by the library,
/// so there is no way to stop it from here; 0 means "go on" all the same.
extern "C" fn on_chunk(done: i32, total: i32, arg: *mut c_void) -> i32 {
    // SAFETY: `arg` is the `Progress` that `run` passed with this function, and it lives until the call has ended.
    let progress = unsafe { &mut *arg.cast::<Progress<'_>>() };
    if progress.panic.is_none() && total > 0 {
        let fraction = (done as f32 / total as f32).clamp(0.0, 1.0);
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| (progress.callback)(fraction))) {
            progress.panic = Some(payload);
        }
    }
    0
}

/// The call of the library that reports progress. It is in the library (`c-api.h`), but neither crate declares it.
type ProgressCallback = extern "C" fn(i32, i32, *mut c_void) -> i32;

extern "C" {
    fn SherpaOnnxOfflineSpeakerDiarizationProcessWithCallback(
        sd: *const sys::OfflineSpeakerDiarization,
        samples: *const f32,
        n: i32,
        callback: Option<ProgressCallback>,
        arg: *mut c_void,
    ) -> *const sys::OfflineSpeakerDiarizationResult;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::ffi::CStr;
    use std::fs;
    use std::path::PathBuf;

    const SEGMENTATION: &str = "pyannote-segmentation-3.0.onnx";
    const EMBEDDING: &str = "nemo_en_titanet_small.onnx";

    #[test]
    fn the_linked_sherpa_onnx_is_the_pinned_version() {
        // Reads the version of the native library that was linked, not of the crate: a library dir that holds
        // another release (SHERPA_ONNX_LIB_DIR in CI) would be caught here.
        let version = unsafe { CStr::from_ptr(sherpa_onnx_sys::SherpaOnnxGetVersionStr()) };
        assert_eq!(version.to_str().unwrap(), "1.13.8");
    }

    #[test]
    fn diarize_models_are_https_with_sizes() {
        assert_eq!(
            DIARIZE_MODELS.map(|m| (m.url, m.file, m.size)),
            [
                (
                    "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.onnx",
                    SEGMENTATION,
                    5_992_913
                ),
                (
                    "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx",
                    EMBEDDING,
                    40_257_283
                ),
            ]
        );
        for model in &DIARIZE_MODELS {
            assert!(model.url.starts_with("https://"), "{}", model.url);
            assert!(!model.file.contains(['/', '\\']), "{}", model.file);
        }
    }

    /// A file of `size` bytes, made without writing them out.
    fn file_of(dir: &Path, name: &str, size: u64) {
        fs::File::create(dir.join(name))
            .unwrap()
            .set_len(size)
            .unwrap();
    }

    #[test]
    fn a_model_is_missing_unless_its_file_has_the_exact_size() {
        let tmp = tempfile::tempdir().unwrap();
        let files = |dir: &Path| {
            missing_models(dir)
                .iter()
                .map(|m| m.file)
                .collect::<Vec<_>>()
        };
        assert_eq!(files(tmp.path()), [SEGMENTATION, EMBEDDING]);

        file_of(tmp.path(), SEGMENTATION, 5_992_913);
        assert_eq!(files(tmp.path()), [EMBEDDING]);

        // What an interrupted download leaves, and a file that is not the one that was downloaded.
        file_of(tmp.path(), "nemo_en_titanet_small.onnx.part", 40_257_283);
        assert_eq!(files(tmp.path()), [EMBEDDING], "a .part is not a model");
        file_of(tmp.path(), EMBEDDING, 40_257_282);
        assert_eq!(files(tmp.path()), [EMBEDDING], "one byte short");
        file_of(tmp.path(), EMBEDDING, 40_257_284);
        assert_eq!(files(tmp.path()), [EMBEDDING], "one byte too long");

        file_of(tmp.path(), EMBEDDING, 40_257_283);
        assert!(files(tmp.path()).is_empty());

        // A folder that is not there holds nothing.
        assert_eq!(files(&tmp.path().join("nowhere")).len(), 2);
    }

    #[test]
    fn diarizer_without_models_is_missing_models() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            Diarizer::load(tmp.path()).err(),
            Some(DiarizeError::MissingModels)
        );

        // One of the two is not enough either.
        file_of(tmp.path(), SEGMENTATION, 5_992_913);
        assert_eq!(
            Diarizer::load(tmp.path()).err(),
            Some(DiarizeError::MissingModels)
        );
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            DiarizeError::MissingModels.to_string(),
            "The speaker separation models are not downloaded."
        );
        assert_eq!(
            DiarizeError::Failed("out of memory".into()).to_string(),
            "Speaker separation failed: out of memory"
        );
    }

    #[test]
    fn the_diarizer_can_be_shared_between_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Diarizer>();
    }

    #[test]
    fn the_models_are_set_up_for_an_unknown_number_of_speakers_on_up_to_four_threads() {
        let config = model_config(Path::new("models")).unwrap();
        let raw = &config.raw;
        let text = |p: *const std::os::raw::c_char| {
            unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned()
        };
        let model = |name: &str| Path::new("models").join(name).to_str().unwrap().to_owned();

        assert_eq!(text(raw.segmentation.pyannote.model), model(SEGMENTATION));
        assert_eq!(text(raw.embedding.model), model(EMBEDDING));
        assert_eq!(text(raw.segmentation.provider), "cpu");
        assert_eq!(text(raw.embedding.provider), "cpu");
        // As many speakers as the audio holds, told apart at a cosine distance of 0.5.
        assert_eq!(raw.clustering.num_clusters, -1);
        assert_eq!(raw.clustering.threshold, 0.5);
        let threads = i32::try_from(threads()).unwrap();
        assert_eq!(raw.segmentation.num_threads, threads);
        assert_eq!(raw.embedding.num_threads, threads);
        assert!((1..=4).contains(&threads), "{threads}");
    }

    #[test]
    fn a_folder_that_cannot_be_passed_to_c_is_an_error_and_not_a_panic() {
        let result = model_config(Path::new("with\0nul"));
        assert!(
            matches!(result, Err(DiarizeError::Failed(_))),
            "{:?}",
            result.err()
        );
    }

    fn segment(
        start: f32,
        end: f32,
        speaker: i32,
    ) -> sherpa_onnx_sys::OfflineSpeakerDiarizationSegment {
        sherpa_onnx_sys::OfflineSpeakerDiarizationSegment {
            start,
            end,
            speaker,
            confidence: 0.0,
        }
    }

    #[test]
    fn segments_become_turns_in_the_same_order() {
        let turns = turns_from(&[
            segment(0.03, 6.43, 1),
            segment(6.75, 13.5, 0),
            segment(13.5, 19.0, 1),
        ]);

        assert_eq!(
            turns,
            [
                Turn {
                    start: f64::from(0.03f32),
                    end: f64::from(6.43f32),
                    speaker: 1
                },
                Turn {
                    start: f64::from(6.75f32),
                    end: 13.5,
                    speaker: 0
                },
                Turn {
                    start: 13.5,
                    end: 19.0,
                    speaker: 1
                },
            ]
        );
    }

    #[test]
    fn a_segment_without_a_speaker_is_left_out() {
        let turns = turns_from(&[segment(0.0, 1.0, -1), segment(1.0, 2.0, 0)]);
        assert_eq!(
            turns,
            [Turn {
                start: 1.0,
                end: 2.0,
                speaker: 0
            }]
        );
    }

    /// Calls `on_chunk` the way the C library does, with `progress` as its argument.
    fn chunk(progress: &mut Progress<'_>, done: i32, total: i32) -> i32 {
        on_chunk(done, total, (progress as *mut Progress<'_>).cast())
    }

    #[test]
    fn progress_is_the_share_of_chunks_done() {
        let mut seen = Vec::new();
        let mut record = |p: f32| seen.push(p);
        let mut progress = Progress {
            callback: &mut record,
            panic: None,
        };

        assert_eq!(chunk(&mut progress, 1, 4), 0, "0 is: go on");
        chunk(&mut progress, 4, 4);
        chunk(&mut progress, 9, 4); // more done than there are: never above 1
        chunk(&mut progress, 1, 0); // nothing to divide by: not reported

        assert_eq!(seen, [0.25, 1.0, 1.0]);
    }

    #[test]
    fn a_panic_in_the_progress_callback_is_kept_and_does_not_unwind_into_c() {
        let mut calls = 0;
        let mut panicking = |_: f32| {
            calls += 1;
            panic!("the callback broke");
        };
        let mut progress = Progress {
            callback: &mut panicking,
            panic: None,
        };

        assert_eq!(chunk(&mut progress, 1, 4), 0);
        assert_eq!(chunk(&mut progress, 2, 4), 0);

        let payload = progress.panic.take().expect("the panic is kept for run");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"the callback broke"));
        drop(progress);
        assert_eq!(
            calls, 1,
            "the callback is not called again after it panicked"
        );
    }

    /// The folder with the two models that the tests of the real thing use.
    fn models_dir() -> PathBuf {
        std::env::var_os("DICTATE_TEST_DIARIZE_DIR")
            .map(PathBuf::from)
            .expect("set DICTATE_TEST_DIARIZE_DIR to a folder that holds the two models")
    }

    /// The fixture as the 16 kHz mono samples the diarizer takes.
    fn two_voices() -> Vec<f32> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/media/two-voices.wav"
        );
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        assert_eq!((spec.sample_rate, spec.channels), (16_000, 1));
        reader
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.0)
            .collect()
    }

    #[test]
    #[ignore = "needs the two models: set DICTATE_TEST_DIARIZE_DIR to the folder that holds them"]
    fn diarizer_finds_two_speakers() {
        // Samantha, Daniel, Samantha, Daniel: the speakers change at about 5.9, 13.5 and 19.1 s.
        let diarizer = Diarizer::load(&models_dir()).unwrap();
        let mut progress = Vec::new();

        let turns = diarizer.run(&two_voices(), |p| progress.push(p)).unwrap();

        let speakers: BTreeSet<u32> = turns.iter().map(|t| t.speaker).collect();
        assert_eq!(speakers.len(), 2, "{turns:?}");
        assert!(turns.len() >= 4, "{turns:?}");
        assert!(
            turns.windows(2).all(|w| w[0].start <= w[1].start),
            "{turns:?}"
        );
        for boundary in [5.9, 13.5, 19.1] {
            let near = turns.windows(2).any(|w| {
                w[0].speaker != w[1].speaker
                    && ((w[0].end - boundary).abs() <= 0.6 || (w[1].start - boundary).abs() <= 0.6)
            });
            assert!(near, "no change of speaker near {boundary} s: {turns:?}");
        }

        assert!(!progress.is_empty(), "the library reports progress");
        assert!(
            progress.iter().all(|p| (0.0..=1.0).contains(p)),
            "{progress:?}"
        );
        assert!(progress.windows(2).all(|w| w[0] <= w[1]), "{progress:?}");
        assert_eq!(progress.last(), Some(&1.0));
    }

    #[test]
    #[ignore = "needs the two models: set DICTATE_TEST_DIARIZE_DIR to the folder that holds them"]
    fn no_audio_and_silence_have_no_turns() {
        // The library logs an error for no samples at all, and answers silence with an empty result.
        let diarizer = Diarizer::load(&models_dir()).unwrap();
        assert_eq!(diarizer.run(&[], |_| {}), Ok(Vec::new()));
        assert_eq!(
            diarizer.run(&vec![0.0; 16_000 * 12], |_| {}),
            Ok(Vec::new())
        );
    }

    #[test]
    #[ignore = "downloads about 46 MB from Hugging Face and GitHub"]
    fn the_models_download_from_their_real_hosts_at_the_listed_size() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for model in &DIARIZE_MODELS {
            let path = runtime
                .block_on(crate::whisper::models::download_file(
                    tmp.path(),
                    model,
                    |_, _| {},
                    tokio_util::sync::CancellationToken::new(),
                ))
                .unwrap();
            assert_eq!(
                fs::metadata(path).unwrap().len(),
                model.size,
                "{}",
                model.file
            );
        }
        assert!(missing_models(tmp.path()).is_empty());
    }
}
