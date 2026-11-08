//! Local Whisper against a real model.
//!
//! The tests that need a model are ignored. Point `DICTATE_WHISPER_TEST_MODEL` at a ggml model file
//! (`ggml-tiny.bin` is enough) and run them with
//!
//! ```text
//! DICTATE_WHISPER_TEST_MODEL=/path/to/ggml-tiny.bin cargo test -p dictate-services --test whisper_local -- --ignored
//! ```

use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use dictate_core::text::segments::Segment;
use dictate_services::whisper::local::{read_wav_16k_mono, LocalWhisper, WhisperError};
use tokio_util::sync::CancellationToken;

const MODEL_VARIABLE: &str = "DICTATE_WHISPER_TEST_MODEL";

fn model() -> LocalWhisper {
    let path = std::env::var_os(MODEL_VARIABLE)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!("set {MODEL_VARIABLE} to a ggml model file (ggml-tiny.bin will do)")
        });
    LocalWhisper::load(&path).unwrap()
}

/// "Hello from Dictate, this is a test.", spoken by a text-to-speech voice: 16 kHz, mono, a bit over 2 s.
fn hello() -> Vec<f32> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello.wav");
    read_wav_16k_mono(&path).unwrap()
}

fn lowercase_text(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[test]
fn the_fixture_is_a_short_16k_clip_of_speech() {
    let audio = hello();

    assert!(
        (16_000..=48_000).contains(&audio.len()),
        "{} samples",
        audio.len()
    );
    let peak = audio.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
    assert!(peak > 0.1, "the clip is nearly silent: peak {peak}");
}

#[test]
#[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
fn transcribes_hello() {
    let segments = model()
        .transcribe(&hello(), Some("en"), "", &CancellationToken::new(), |_| {})
        .unwrap();

    let text = lowercase_text(&segments);
    assert!(text.contains("hello"), "{text:?}");
    assert!(text.contains("test"), "{text:?}");
    assert!(segments[0].start < 1.0, "{:?}", segments[0]);
    assert!(segments.iter().all(|s| s.speaker.is_none()));
}

#[test]
#[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
fn detects_the_language_when_none_is_given() {
    let segments = model()
        .transcribe(&hello(), None, "", &CancellationToken::new(), |_| {})
        .unwrap();

    let text = lowercase_text(&segments);
    assert!(text.contains("hello"), "{text:?}");
}

#[test]
#[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
fn a_long_prompt_is_taken_in() {
    let prompt = "Dictate, Whisper, Tauri. ".repeat(100);

    let segments = model()
        .transcribe(
            &hello(),
            Some("en"),
            &prompt,
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap();

    assert!(lowercase_text(&segments).contains("hello"));
}

#[test]
#[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
fn progress_rises_to_100() {
    let mut seen = Vec::new();

    model()
        .transcribe(&hello(), Some("en"), "", &CancellationToken::new(), |p| {
            seen.push(p)
        })
        .unwrap();

    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert_eq!(seen.last(), Some(&100), "{seen:?}");
}

#[test]
#[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
fn one_model_serves_one_transcription_after_another() {
    let model = model();
    let audio = hello();
    let cancel = CancellationToken::new();

    let first = model
        .transcribe(&audio, Some("en"), "", &cancel, |_| {})
        .unwrap();
    let second = model
        .transcribe(&audio, Some("en"), "", &cancel, |_| {})
        .unwrap();

    assert_eq!(first, second);
}

#[test]
#[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
fn empty_audio_has_no_segments() {
    let segments = model()
        .transcribe(&[], Some("en"), "", &CancellationToken::new(), |_| {})
        .unwrap();

    assert!(segments.is_empty());
}

#[test]
#[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
fn an_unknown_language_fails_before_any_work() {
    let result = model().transcribe(
        &hello(),
        Some("klingon"),
        "",
        &CancellationToken::new(),
        |_| {},
    );

    assert!(matches!(result, Err(WhisperError::Failed(_))), "{result:?}");
}

#[test]
#[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
fn a_token_cancelled_beforehand_returns_cancelled() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut progress_calls = 0;

    let result = model().transcribe(&hello(), Some("en"), "", &cancel, |_| progress_calls += 1);

    assert_eq!(result, Err(WhisperError::Cancelled));
    assert_eq!(progress_calls, 0, "it should not have started");
}

#[test]
#[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
fn cancel_returns_cancelled() {
    // 36 s: past one 30 s window, so Whisper comes back for a second one, and a third progress report at the
    // very end, unless the abort callback stops it.
    let audio: Vec<f32> = hello().repeat(15);
    let cancel = CancellationToken::new();
    let mut seen = Vec::new();

    let result = model().transcribe(&audio, Some("en"), "", &cancel, |p| {
        seen.push(p);
        if p >= 50 {
            cancel.cancel();
        }
    });

    assert_eq!(result, Err(WhisperError::Cancelled));
    assert!(
        seen.iter().any(|p| *p >= 50),
        "never got to the second window: {seen:?}"
    );
    assert!(
        seen.iter().all(|p| *p < 100),
        "kept going after the cancel: {seen:?}"
    );
}

#[test]
#[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
fn a_panic_in_the_progress_callback_comes_out_of_transcribe() {
    let model = model();
    let audio = hello();
    let cancel = CancellationToken::new();
    let calls = Cell::new(0);

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        model.transcribe(&audio, Some("en"), "", &cancel, |_| {
            calls.set(calls.get() + 1);
            panic!("progress callback blew up");
        })
    }));

    let payload = outcome.expect_err("the panic should have come out");
    assert_eq!(
        payload.downcast_ref::<&str>(),
        Some(&"progress callback blew up")
    );
    assert_eq!(
        calls.get(),
        1,
        "the callback must not be called again after it panicked"
    );
}
