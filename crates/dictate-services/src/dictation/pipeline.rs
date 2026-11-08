//! What happens after the user stops talking: an optional second transcription by a speech API, then the AI
//! cleanup, with a notice for whatever could not be done.
//!
//! The local transcript is the starting point. When the user chose the API engine and there is audio, the API
//! hears it again, unless the recording was a short tap with nothing heard. An audio-capable chat model (the
//! OpenRouter kind) transcribes and applies the default edit in a single call when the AI edit is on, and then no
//! other model touches the text. Otherwise the transcript is cleaned up by the AI model in a second step, when
//! the edit is on and a key is set. A step that fails never loses the text: the earlier text goes out as it is,
//! with a notice that says what failed. The network is behind [`Backend`], so everything here runs on fakes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::presets::EditPreset;

/// The longest the plain API transcription of a recording takes.
pub const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest the one-call transcription and edit takes.
pub const TRANSCRIBE_AND_EDIT_TIMEOUT: Duration = Duration::from_secs(60);
/// A recording this long (in seconds) is sent to the API even if the local engine heard nothing in it; a shorter
/// one is only sent when there is local text, because a tap of the hotkey is not worth a call.
pub const MIN_API_SECONDS: f64 = 1.0;

/// The network calls of the pipeline. The app implements it with the speech API and AI clients. The error
/// strings are shown to the user as they are, so they must already be redacted.
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    /// The transcript of the recording in `wav`.
    async fn transcribe(&self, wav: &Path, timeout: Duration) -> Result<String, String>;

    /// The transcript of the recording in `wav` with `instruction` applied to it, in one call.
    async fn transcribe_and_edit(
        &self,
        wav: &Path,
        instruction: &str,
        timeout: Duration,
    ) -> Result<String, String>;

    /// `text` with `instruction` applied to it. An empty answer is an error, not a result.
    async fn refine(&self, text: &str, instruction: &str) -> Result<String, String>;
}

/// What the recording gave: the local transcript, how long it was, and the audio file if there is one.
#[derive(Debug, Clone, PartialEq)]
pub struct StopInput {
    pub local_text: String,
    pub duration_s: f64,
    pub wav: Option<PathBuf>,
}

/// The speech API to hear the recording again with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiPlan {
    pub model: String,
    /// Whether the audio goes to a chat model, which can edit the text in the same call.
    pub chat: bool,
}

/// What the pipeline needs to know about the settings.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Set when the API engine is chosen and the transcription API is configured.
    pub api: Option<ApiPlan>,
    /// Whether the AI model has what it needs to run (its key).
    pub ai_configured: bool,
    /// Whether the transcript is edited automatically.
    pub auto_refine: bool,
    /// The edit to apply automatically.
    pub style: &'static EditPreset,
}

/// What the pipeline is doing now, for the title of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// The API is transcribing again, and editing in the same call when `editing`.
    Retranscribing { model: String, editing: bool },
    /// The AI model is editing the transcript.
    Editing { style_id: &'static str },
}

/// What a notice is about. It is sent to the UI by name and shown with the text of the locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKey {
    /// The API could not transcribe the recording: the local text is kept.
    ApiFailed,
    /// The AI edit is on but no key is set: the text is not edited.
    NotEditedNoKey,
    /// The AI model could not edit the text: the text is not edited.
    EditFailed,
    /// There is no text at all.
    NothingHeard,
}

/// A notice about the result, with the error behind it when there is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notice {
    pub key: NoticeKey,
    pub detail: Option<String>,
}

impl Notice {
    fn new(key: NoticeKey) -> Self {
        Self { key, detail: None }
    }

    fn with_detail(key: NoticeKey, detail: String) -> Self {
        Self {
            key,
            detail: Some(detail),
        }
    }
}

/// The end of the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// What to write. Empty when there is nothing to write.
    pub text: String,
    /// The text before the AI edit, when there was one, so that the edit can be undone.
    pub original: Option<String>,
    /// The edit that was applied.
    pub style: Option<&'static str>,
    pub notice: Option<Notice>,
    /// What to keep with the recording: the transcript before the AI edit, except that the result of the one-call
    /// transcription and edit is kept as it is.
    pub saved_text: String,
}

/// Runs the pipeline for a recording that was just stopped. `skip_ai` is cancelled when the user asks to write
/// the text as it is: an AI edit under way is dropped and the text before it goes out. `on_progress` is told
/// when each step starts.
pub async fn run(
    input: StopInput,
    cfg: &PipelineConfig,
    backend: &dyn Backend,
    skip_ai: &CancellationToken,
    on_progress: impl Fn(Progress),
) -> Outcome {
    let StopInput {
        local_text: mut text,
        duration_s,
        wav,
    } = input;
    let mut api_notice = None;
    // Set when the API was asked to edit as well, and so no other model may.
    let mut edited_by_api = false;

    if let (Some(plan), Some(wav)) = (&cfg.api, &wav) {
        if !is_blank(&text) || duration_s >= MIN_API_SECONDS {
            edited_by_api = plan.chat && cfg.auto_refine;
            on_progress(Progress::Retranscribing {
                model: plan.model.clone(),
                editing: edited_by_api,
            });
            let heard = if edited_by_api {
                backend
                    .transcribe_and_edit(wav, cfg.style.instruction, TRANSCRIBE_AND_EDIT_TIMEOUT)
                    .await
            } else {
                backend.transcribe(wav, TRANSCRIBE_TIMEOUT).await
            };
            match heard {
                Ok(api_text) if !is_blank(&api_text) && edited_by_api => {
                    return Outcome {
                        original: (!is_blank(&text)).then_some(text),
                        style: Some(cfg.style.id),
                        notice: None,
                        saved_text: api_text.clone(),
                        text: api_text,
                    };
                }
                Ok(api_text) if !is_blank(&api_text) => text = api_text,
                // The API heard nothing: the local text, if any, still stands.
                Ok(_) => {}
                Err(detail) => {
                    api_notice = Some(Notice::with_detail(NoticeKey::ApiFailed, detail));
                }
            }
        }
    }

    if is_blank(&text) {
        // An API error says more than "heard nothing".
        let notice = api_notice.unwrap_or_else(|| Notice::new(NoticeKey::NothingHeard));
        return raw(String::new(), Some(notice));
    }

    // A call that was meant to edit and did not leaves the local text as it is: no second model.
    if edited_by_api || !cfg.auto_refine {
        return raw(text, api_notice);
    }
    if !cfg.ai_configured {
        let notice = api_notice.unwrap_or_else(|| Notice::new(NoticeKey::NotEditedNoKey));
        return raw(text, Some(notice));
    }
    if skip_ai.is_cancelled() {
        return raw(text, api_notice);
    }

    on_progress(Progress::Editing {
        style_id: cfg.style.id,
    });
    let refined = tokio::select! {
        biased;
        _ = skip_ai.cancelled() => None,
        refined = backend.refine(&text, cfg.style.instruction) => Some(refined),
    };
    match refined {
        None => raw(text, api_notice),
        Some(Ok(edited)) => Outcome {
            text: edited,
            original: Some(text.clone()),
            style: Some(cfg.style.id),
            // The API failure still stands, whatever the edit did.
            notice: api_notice,
            saved_text: text,
        },
        Some(Err(detail)) => raw(
            text,
            Some(Notice::with_detail(NoticeKey::EditFailed, detail)),
        ),
    }
}

fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// The text goes out as it is, and is also what is kept.
fn raw(text: String, notice: Option<Notice>) -> Outcome {
    Outcome {
        saved_text: text.clone(),
        text,
        original: None,
        style: None,
        notice,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::find_edit_preset;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Transcribe {
            wav: PathBuf,
            timeout: Duration,
        },
        TranscribeAndEdit {
            wav: PathBuf,
            instruction: String,
            timeout: Duration,
        },
        Refine {
            text: String,
            instruction: String,
        },
    }

    /// A backend that answers with what the test gave it and records what it was asked.
    struct Fake {
        transcribe: Result<String, String>,
        transcribe_and_edit: Result<String, String>,
        refine: Result<String, String>,
        /// Cancels this token when the edit starts and then never answers, like a slow model.
        cancel_in_refine: Option<CancellationToken>,
        calls: Mutex<Vec<Call>>,
    }

    impl Fake {
        fn new() -> Self {
            let unexpected = || Err("unexpected call".to_owned());
            Self {
                transcribe: unexpected(),
                transcribe_and_edit: unexpected(),
                refine: unexpected(),
                cancel_in_refine: None,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn transcribing(mut self, result: Result<&str, &str>) -> Self {
            self.transcribe = own(result);
            self
        }

        fn editing_in_one_call(mut self, result: Result<&str, &str>) -> Self {
            self.transcribe_and_edit = own(result);
            self
        }

        fn refining(mut self, result: Result<&str, &str>) -> Self {
            self.refine = own(result);
            self
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn own(result: Result<&str, &str>) -> Result<String, String> {
        result.map(str::to_owned).map_err(str::to_owned)
    }

    #[async_trait::async_trait]
    impl Backend for Fake {
        async fn transcribe(&self, wav: &Path, timeout: Duration) -> Result<String, String> {
            self.calls.lock().unwrap().push(Call::Transcribe {
                wav: wav.to_owned(),
                timeout,
            });
            self.transcribe.clone()
        }

        async fn transcribe_and_edit(
            &self,
            wav: &Path,
            instruction: &str,
            timeout: Duration,
        ) -> Result<String, String> {
            self.calls.lock().unwrap().push(Call::TranscribeAndEdit {
                wav: wav.to_owned(),
                instruction: instruction.to_owned(),
                timeout,
            });
            self.transcribe_and_edit.clone()
        }

        async fn refine(&self, text: &str, instruction: &str) -> Result<String, String> {
            self.calls.lock().unwrap().push(Call::Refine {
                text: text.to_owned(),
                instruction: instruction.to_owned(),
            });
            if let Some(token) = &self.cancel_in_refine {
                token.cancel();
                return std::future::pending().await;
            }
            self.refine.clone()
        }
    }

    fn clean() -> &'static EditPreset {
        find_edit_preset("clean")
    }

    /// Local engine only, the AI model set up and the edit on.
    fn local_config() -> PipelineConfig {
        PipelineConfig {
            api: None,
            ai_configured: true,
            auto_refine: true,
            style: clean(),
        }
    }

    fn api_config(chat: bool) -> PipelineConfig {
        PipelineConfig {
            api: Some(ApiPlan {
                model: "whisper-1".to_owned(),
                chat,
            }),
            ..local_config()
        }
    }

    fn input(local_text: &str, duration_s: f64) -> StopInput {
        StopInput {
            local_text: local_text.to_owned(),
            duration_s,
            wav: Some(PathBuf::from("take.wav")),
        }
    }

    fn api_failed(detail: &str) -> Option<Notice> {
        Some(Notice::with_detail(NoticeKey::ApiFailed, detail.to_owned()))
    }

    /// Fails a test whose pipeline hangs, such as one that waits for a model it should have dropped.
    async fn finishes<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), future)
            .await
            .expect("the pipeline did not finish")
    }

    async fn run_with(input: StopInput, cfg: &PipelineConfig, backend: &Fake) -> Outcome {
        run(input, cfg, backend, &CancellationToken::new(), |_| {}).await
    }

    #[tokio::test]
    async fn local_only_refines_with_default_style() {
        let backend = Fake::new().refining(Ok("Hello, world."));

        let out = run_with(input("hello world", 3.0), &local_config(), &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: "Hello, world.".to_owned(),
                original: Some("hello world".to_owned()),
                style: Some("clean"),
                notice: None,
                saved_text: "hello world".to_owned(),
            }
        );
        assert_eq!(
            backend.calls(),
            [Call::Refine {
                text: "hello world".to_owned(),
                instruction: clean().instruction.to_owned(),
            }]
        );
    }

    #[tokio::test]
    async fn no_ai_key_writes_raw_with_notice() {
        let cfg = PipelineConfig {
            ai_configured: false,
            ..local_config()
        };
        let backend = Fake::new();

        let out = run_with(input("hello world", 3.0), &cfg, &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: "hello world".to_owned(),
                original: None,
                style: None,
                notice: Some(Notice::new(NoticeKey::NotEditedNoKey)),
                saved_text: "hello world".to_owned(),
            }
        );
        assert_eq!(backend.calls(), []);
    }

    #[tokio::test]
    async fn auto_refine_off_writes_raw_silently() {
        let cfg = PipelineConfig {
            auto_refine: false,
            ..local_config()
        };
        let backend = Fake::new();

        let out = run_with(input("hello world", 3.0), &cfg, &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: "hello world".to_owned(),
                original: None,
                style: None,
                notice: None,
                saved_text: "hello world".to_owned(),
            }
        );
        assert_eq!(backend.calls(), []);
    }

    #[tokio::test]
    async fn chat_api_does_one_call_and_no_refine() {
        let backend = Fake::new().editing_in_one_call(Ok("Hello, world."));

        let out = run_with(input("hello world", 3.0), &api_config(true), &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: "Hello, world.".to_owned(),
                original: Some("hello world".to_owned()),
                style: Some("clean"),
                notice: None,
                saved_text: "Hello, world.".to_owned(),
            }
        );
        assert_eq!(
            backend.calls(),
            [Call::TranscribeAndEdit {
                wav: PathBuf::from("take.wav"),
                instruction: clean().instruction.to_owned(),
                timeout: Duration::from_secs(60),
            }]
        );
    }

    #[tokio::test]
    async fn chat_api_uses_the_configured_style() {
        let cfg = PipelineConfig {
            style: find_edit_preset("concise"),
            ..api_config(true)
        };
        let backend = Fake::new().editing_in_one_call(Ok("Short."));

        let out = run_with(input("a long winded sentence", 3.0), &cfg, &backend).await;

        assert_eq!(out.style, Some("concise"));
        assert_eq!(
            backend.calls(),
            [Call::TranscribeAndEdit {
                wav: PathBuf::from("take.wav"),
                instruction: find_edit_preset("concise").instruction.to_owned(),
                timeout: Duration::from_secs(60),
            }]
        );
    }

    #[tokio::test]
    async fn chat_api_edits_without_an_ai_key() {
        let cfg = PipelineConfig {
            ai_configured: false,
            ..api_config(true)
        };
        let backend = Fake::new().editing_in_one_call(Ok("Hello, world."));

        let out = run_with(input("hello world", 3.0), &cfg, &backend).await;

        assert_eq!(out.text, "Hello, world.");
        assert_eq!(out.notice, None);
    }

    #[tokio::test]
    async fn chat_api_with_no_local_text_has_no_original() {
        let backend = Fake::new().editing_in_one_call(Ok("Hello, world."));

        let out = run_with(input("", 2.0), &api_config(true), &backend).await;

        assert_eq!(out.text, "Hello, world.");
        assert_eq!(out.original, None);
        assert_eq!(out.style, Some("clean"));
        assert_eq!(out.saved_text, "Hello, world.");
    }

    #[tokio::test]
    async fn chat_api_failure_writes_local_raw_without_second_pass() {
        let backend = Fake::new()
            .editing_in_one_call(Err("HTTP 500"))
            .refining(Ok("must not be used"));

        let out = run_with(input("hello world", 3.0), &api_config(true), &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: "hello world".to_owned(),
                original: None,
                style: None,
                notice: api_failed("HTTP 500"),
                saved_text: "hello world".to_owned(),
            }
        );
        assert_eq!(backend.calls().len(), 1, "{:?}", backend.calls());
    }

    #[tokio::test]
    async fn chat_api_empty_answer_writes_local_raw_without_second_pass() {
        let backend = Fake::new()
            .editing_in_one_call(Ok(""))
            .refining(Ok("must not be used"));

        let out = run_with(input("hello world", 3.0), &api_config(true), &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: "hello world".to_owned(),
                original: None,
                style: None,
                notice: None,
                saved_text: "hello world".to_owned(),
            }
        );
        assert_eq!(backend.calls().len(), 1, "{:?}", backend.calls());
    }

    #[tokio::test]
    async fn chat_api_without_auto_refine_only_transcribes() {
        let cfg = PipelineConfig {
            auto_refine: false,
            ..api_config(true)
        };
        let backend = Fake::new().transcribing(Ok("api text"));

        let out = run_with(input("local text", 3.0), &cfg, &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: "api text".to_owned(),
                original: None,
                style: None,
                notice: None,
                saved_text: "api text".to_owned(),
            }
        );
        assert_eq!(
            backend.calls(),
            [Call::Transcribe {
                wav: PathBuf::from("take.wav"),
                timeout: Duration::from_secs(120),
            }]
        );
    }

    #[tokio::test]
    async fn plain_api_then_refine() {
        let backend = Fake::new()
            .transcribing(Ok("api text"))
            .refining(Ok("Api text."));

        let out = run_with(input("local text", 3.0), &api_config(false), &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: "Api text.".to_owned(),
                original: Some("api text".to_owned()),
                style: Some("clean"),
                notice: None,
                saved_text: "api text".to_owned(),
            }
        );
        assert_eq!(
            backend.calls(),
            [
                Call::Transcribe {
                    wav: PathBuf::from("take.wav"),
                    timeout: Duration::from_secs(120),
                },
                Call::Refine {
                    text: "api text".to_owned(),
                    instruction: clean().instruction.to_owned(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn plain_api_failure_refines_local_and_keeps_notice() {
        let backend = Fake::new()
            .transcribing(Err("HTTP 401"))
            .refining(Ok("Local text."));

        let out = run_with(input("local text", 3.0), &api_config(false), &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: "Local text.".to_owned(),
                original: Some("local text".to_owned()),
                style: Some("clean"),
                notice: api_failed("HTTP 401"),
                saved_text: "local text".to_owned(),
            }
        );
        assert_eq!(
            backend.calls().last(),
            Some(&Call::Refine {
                text: "local text".to_owned(),
                instruction: clean().instruction.to_owned(),
            })
        );
    }

    #[tokio::test]
    async fn plain_api_empty_answer_keeps_the_local_text() {
        let backend = Fake::new()
            .transcribing(Ok("  "))
            .refining(Ok("Local text."));

        let out = run_with(input("local text", 3.0), &api_config(false), &backend).await;

        assert_eq!(out.text, "Local text.");
        assert_eq!(out.original, Some("local text".to_owned()));
        assert_eq!(out.notice, None);
        assert_eq!(out.saved_text, "local text");
    }

    #[tokio::test]
    async fn api_failure_with_no_ai_key_keeps_the_api_notice() {
        let cfg = PipelineConfig {
            ai_configured: false,
            ..api_config(false)
        };
        let backend = Fake::new().transcribing(Err("HTTP 401"));

        let out = run_with(input("local text", 3.0), &cfg, &backend).await;

        assert_eq!(out.text, "local text");
        assert_eq!(out.notice, api_failed("HTTP 401"));
    }

    #[tokio::test]
    async fn short_silent_tap_skips_api() {
        let backend = Fake::new();

        let out = run_with(input("", 0.6), &api_config(false), &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: String::new(),
                original: None,
                style: None,
                notice: Some(Notice::new(NoticeKey::NothingHeard)),
                saved_text: String::new(),
            }
        );
        assert_eq!(backend.calls(), []);
    }

    #[tokio::test]
    async fn short_tap_with_local_text_still_calls_api() {
        let backend = Fake::new().transcribing(Ok("yes")).refining(Ok("Yes."));

        let out = run_with(input("yes", 0.6), &api_config(false), &backend).await;

        assert_eq!(out.text, "Yes.");
        assert_eq!(backend.calls().len(), 2, "{:?}", backend.calls());
    }

    #[tokio::test]
    async fn a_second_of_silence_still_calls_api() {
        let backend = Fake::new()
            .transcribing(Ok("faint words"))
            .refining(Ok("Faint words."));

        let out = run_with(input("", 1.0), &api_config(false), &backend).await;

        assert_eq!(out.text, "Faint words.");
        assert_eq!(out.original, Some("faint words".to_owned()));
    }

    #[tokio::test]
    async fn api_is_skipped_without_audio() {
        let backend = Fake::new().refining(Ok("Hello."));
        let input = StopInput {
            wav: None,
            ..input("hello", 3.0)
        };

        let out = run_with(input, &api_config(false), &backend).await;

        assert_eq!(out.text, "Hello.");
        assert!(matches!(backend.calls()[..], [Call::Refine { .. }]));
    }

    #[tokio::test]
    async fn empty_after_api_is_nothing_heard() {
        let backend = Fake::new().transcribing(Ok("")).refining(Ok("never"));

        let out = run_with(input("", 2.0), &api_config(false), &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: String::new(),
                original: None,
                style: None,
                notice: Some(Notice::new(NoticeKey::NothingHeard)),
                saved_text: String::new(),
            }
        );
        assert_eq!(backend.calls().len(), 1, "{:?}", backend.calls());
    }

    #[tokio::test]
    async fn whitespace_only_text_is_nothing_heard() {
        let backend = Fake::new().refining(Ok("never"));

        let out = run_with(input(" \n", 2.0), &local_config(), &backend).await;

        assert_eq!(out.text, "");
        assert_eq!(out.notice, Some(Notice::new(NoticeKey::NothingHeard)));
        assert_eq!(backend.calls(), []);
    }

    #[tokio::test]
    async fn api_error_beats_nothing_heard() {
        let backend = Fake::new().transcribing(Err("timed out"));

        let out = run_with(input("", 2.0), &api_config(false), &backend).await;

        assert_eq!(out.text, "");
        assert_eq!(out.notice, api_failed("timed out"));
        assert_eq!(backend.calls().len(), 1, "{:?}", backend.calls());
    }

    #[tokio::test]
    async fn refine_failure_writes_raw_with_edit_failed() {
        let backend = Fake::new().refining(Err("HTTP 429"));

        let out = run_with(input("hello world", 3.0), &local_config(), &backend).await;

        assert_eq!(
            out,
            Outcome {
                text: "hello world".to_owned(),
                original: None,
                style: None,
                notice: Some(Notice::with_detail(
                    NoticeKey::EditFailed,
                    "HTTP 429".to_owned()
                )),
                saved_text: "hello world".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn refine_failure_after_api_failure_reports_the_edit() {
        let backend = Fake::new()
            .transcribing(Err("HTTP 401"))
            .refining(Err("HTTP 429"));

        let out = run_with(input("hello world", 3.0), &api_config(false), &backend).await;

        assert_eq!(out.text, "hello world");
        assert_eq!(
            out.notice,
            Some(Notice::with_detail(
                NoticeKey::EditFailed,
                "HTTP 429".to_owned()
            ))
        );
    }

    #[tokio::test]
    async fn skip_ai_during_refine_writes_raw() {
        let skip_ai = CancellationToken::new();
        let mut backend = Fake::new().refining(Ok("never seen"));
        backend.cancel_in_refine = Some(skip_ai.clone());

        let out = finishes(run(
            input("hello world", 3.0),
            &local_config(),
            &backend,
            &skip_ai,
            |_| {},
        ))
        .await;

        assert_eq!(
            out,
            Outcome {
                text: "hello world".to_owned(),
                original: None,
                style: None,
                notice: None,
                saved_text: "hello world".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn skip_ai_before_refine_never_calls_the_model() {
        let skip_ai = CancellationToken::new();
        skip_ai.cancel();
        let backend = Fake::new().refining(Ok("never seen"));
        let progress = Mutex::new(Vec::new());

        let out = run(
            input("hello world", 3.0),
            &local_config(),
            &backend,
            &skip_ai,
            |p| progress.lock().unwrap().push(p),
        )
        .await;

        assert_eq!(out.text, "hello world");
        assert_eq!(out.style, None);
        assert_eq!(out.notice, None);
        assert_eq!(backend.calls(), []);
        assert_eq!(*progress.lock().unwrap(), []);
    }

    #[tokio::test]
    async fn skip_ai_keeps_the_api_notice() {
        let skip_ai = CancellationToken::new();
        let mut backend = Fake::new().transcribing(Err("HTTP 401"));
        backend.cancel_in_refine = Some(skip_ai.clone());

        let out = finishes(run(
            input("hello world", 3.0),
            &api_config(false),
            &backend,
            &skip_ai,
            |_| {},
        ))
        .await;

        assert_eq!(out.text, "hello world");
        assert_eq!(out.notice, api_failed("HTTP 401"));
    }

    #[tokio::test]
    async fn progress_reports_retranscribing_then_editing() {
        let backend = Fake::new()
            .transcribing(Ok("api text"))
            .refining(Ok("Api text."));
        let progress = Mutex::new(Vec::new());

        run(
            input("local text", 3.0),
            &api_config(false),
            &backend,
            &CancellationToken::new(),
            |p| progress.lock().unwrap().push(p),
        )
        .await;

        assert_eq!(
            *progress.lock().unwrap(),
            [
                Progress::Retranscribing {
                    model: "whisper-1".to_owned(),
                    editing: false,
                },
                Progress::Editing { style_id: "clean" },
            ]
        );
    }

    #[tokio::test]
    async fn progress_for_the_one_call_is_retranscribing_and_editing() {
        let backend = Fake::new().editing_in_one_call(Ok("Hello."));
        let progress = Mutex::new(Vec::new());

        run(
            input("hello", 3.0),
            &api_config(true),
            &backend,
            &CancellationToken::new(),
            |p| progress.lock().unwrap().push(p),
        )
        .await;

        assert_eq!(
            *progress.lock().unwrap(),
            [Progress::Retranscribing {
                model: "whisper-1".to_owned(),
                editing: true,
            }]
        );
    }

    #[tokio::test]
    async fn local_only_reports_just_the_edit() {
        let backend = Fake::new().refining(Ok("Hello."));
        let progress = Mutex::new(Vec::new());

        run(
            input("hello", 3.0),
            &local_config(),
            &backend,
            &CancellationToken::new(),
            |p| progress.lock().unwrap().push(p),
        )
        .await;

        assert_eq!(
            *progress.lock().unwrap(),
            [Progress::Editing { style_id: "clean" }]
        );
    }

    #[test]
    fn the_pipeline_can_run_on_a_spawned_task() {
        fn assert_send<T: Send>(_: &T) {}
        let backend = Fake::new();
        let cfg = local_config();
        let skip_ai = CancellationToken::new();

        let future = run(input("hello", 3.0), &cfg, &backend, &skip_ai, |_| {});

        assert_send(&future);
    }

    #[test]
    fn notices_serialise_with_a_snake_case_key() {
        let notice = Notice::with_detail(NoticeKey::ApiFailed, "HTTP 401".to_owned());
        assert_eq!(
            serde_json::to_value(&notice).unwrap(),
            serde_json::json!({ "key": "api_failed", "detail": "HTTP 401" })
        );

        for (key, name) in [
            (NoticeKey::NotEditedNoKey, "not_edited_no_key"),
            (NoticeKey::EditFailed, "edit_failed"),
            (NoticeKey::NothingHeard, "nothing_heard"),
        ] {
            assert_eq!(
                serde_json::to_value(Notice::new(key)).unwrap(),
                serde_json::json!({ "key": name, "detail": null })
            );
        }
    }
}
