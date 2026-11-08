//! The AI provider client: edits of a dictated transcript, one-shot completions and the connection check.
//! The request shapes follow the Swift app (Refiner.swift).

use std::fmt;
use std::time::Duration;

use serde_json::{json, Value};

use crate::http::{self, ApiError};
use crate::presets::AiProvider;

/// Everything needed to talk to the configured AI provider.
#[derive(Clone)]
pub struct AiConfig {
    pub provider: AiProvider,
    pub base_url: String,
    pub model: String,
    /// `None` or empty: no key is sent (local servers).
    pub api_key: Option<String>,
    /// Custom vocabulary / context the user gave, added to the system prompt.
    pub vocabulary: String,
}

impl fmt::Debug for AiConfig {
    // Written by hand so that `{:?}` in a log line cannot print the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiConfig")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("vocabulary", &self.vocabulary)
            .finish()
    }
}

/// The system prompt of the edit calls: the editor's role, plus the user's vocabulary when there is one.
pub fn system_prompt(vocabulary: &str) -> String {
    let mut s = String::from(
        "You are an editor for raw speech-to-text dictation. The user message contains a transcript produced by automatic speech recognition: it may contain misrecognized words (often homophones), missing punctuation, filler words and repetitions, and may mix languages (e.g. Vietnamese with English technical terms).\n\
         Apply the requested edit. Unless asked to translate, keep the original language(s) — do not translate English technical terms that the speaker used.\n\
         Output ONLY the final text. No preamble, no explanations, no surrounding quotes, no markdown code fences.",
    );
    let vocabulary = vocabulary.trim();
    if !vocabulary.is_empty() {
        s.push_str("\n\nCustom vocabulary / context from the user (prefer these spellings when the transcript sounds similar):\n");
        s.push_str(vocabulary);
    }
    s
}

/// One-shot chat completion against the configured provider. `quick`: someone is waiting on a short edit, so
/// models on OpenRouter that think first (Gemini Flash cannot turn it off) keep that short.
pub async fn complete(
    c: &AiConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
    timeout: Duration,
    quick: bool,
) -> Result<String, ApiError> {
    let key = c.api_key.as_deref().filter(|k| !k.is_empty());
    send(c, key, system, user, max_tokens, timeout, quick)
        .await
        // A proxy or provider may echo the key in its message: it must not reach the screen or a log.
        .map_err(|e| e.redact(key))
}

async fn send(
    c: &AiConfig,
    key: Option<&str>,
    system: &str,
    user: &str,
    max_tokens: u32,
    timeout: Duration,
    quick: bool,
) -> Result<String, ApiError> {
    let base = c.base_url.trim_matches(|ch| ch == '/' || ch == ' ');
    let request = match c.provider {
        AiProvider::Anthropic => {
            let url = if base.ends_with("/v1") {
                format!("{base}/messages")
            } else {
                format!("{base}/v1/messages")
            };
            http::client()
                .post(parse_url(&url)?)
                // Swift sets this header even without a key; the API then answers 401 itself.
                .header("x-api-key", key.unwrap_or_default())
                .header("anthropic-version", "2023-06-01")
                .json(&json!({
                    "model": c.model,
                    "max_tokens": max_tokens,
                    "system": system,
                    "messages": [{"role": "user", "content": user}],
                }))
        }
        AiProvider::OpenAi => {
            let mut body = json!({
                "model": c.model,
                "temperature": 0.3,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
            });
            // Other OpenAI-compatible servers may reject the unknown parameter.
            if quick && base.contains("openrouter.ai") {
                body["reasoning"] = json!({"effort": "low"});
            }
            let mut request = http::client()
                .post(parse_url(&format!("{base}/chat/completions"))?)
                .json(&body);
            if let Some(key) = key {
                request = request.bearer_auth(key);
            }
            request
        }
    };

    let response = request.timeout(timeout).send().await?;
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        return Err(http::status_error(status.as_u16(), &body));
    }
    let Some(reply) = serde_json::from_slice::<Value>(&body)
        .ok()
        .filter(Value::is_object)
    else {
        return Err(ApiError::Invalid(http::snippet(&body)));
    };

    let text = match c.provider {
        AiProvider::Anthropic => reply["content"].as_array().map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<String>()
        }),
        AiProvider::OpenAi => reply["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_owned),
    };
    match text {
        Some(text) if !text.is_empty() => Ok(text.trim().to_string()),
        _ => Err(ApiError::Empty),
    }
}

fn parse_url(url: &str) -> Result<reqwest::Url, ApiError> {
    reqwest::Url::parse(url)
        .map_err(|_| ApiError::Invalid("The base URL is not valid.".to_string()))
}

/// Applies `instruction` to the transcript `text`; the edited text comes back trimmed.
pub async fn refine(c: &AiConfig, text: &str, instruction: &str) -> Result<String, ApiError> {
    let user = format!("Edit instruction: {instruction}\n\n<transcript>\n{text}\n</transcript>");
    complete(
        c,
        &system_prompt(&c.vocabulary),
        &user,
        4096,
        Duration::from_secs(90),
        true,
    )
    .await
}

/// Checks the provider settings with a tiny call: any non-empty reply is a success.
pub async fn check(c: &AiConfig) -> Result<(), ApiError> {
    complete(
        c,
        "Reply with OK.",
        "OK?",
        16,
        Duration::from_secs(20),
        false,
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::net::TcpListener;
    use std::time::Instant;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    const TIMEOUT: Duration = Duration::from_secs(5);
    /// Long enough that hiding it in a message cannot garble an ordinary word.
    const KEY: &str = "sk-test-key-123";

    fn config(provider: AiProvider, base_url: &str, api_key: Option<&str>) -> AiConfig {
        AiConfig {
            provider,
            base_url: base_url.to_string(),
            model: "test-model".to_string(),
            api_key: api_key.map(str::to_string),
            vocabulary: String::new(),
        }
    }

    /// A server that answers every request with `reply`.
    async fn server_replying(reply: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(any()).respond_with(reply).mount(&server).await;
        server
    }

    fn anthropic_reply(text: &str) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_json(json!({"content": [{"type": "text", "text": text}]}))
    }

    fn openai_reply(text: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(
            json!({"choices": [{"message": {"role": "assistant", "content": text}}]}),
        )
    }

    /// The one request the server got, with its JSON body.
    async fn only_request(server: &MockServer) -> (Request, Value) {
        let mut requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "expected exactly one request");
        let request = requests.remove(0);
        let body = serde_json::from_slice(&request.body).unwrap();
        (request, body)
    }

    fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
        request.headers.get(name).map(|v| v.to_str().unwrap())
    }

    async fn ask(c: &AiConfig, quick: bool) -> Result<String, ApiError> {
        complete(c, "SYSTEM", "USER", 123, TIMEOUT, quick).await
    }

    #[tokio::test]
    async fn anthropic_request_shape() {
        let server = server_replying(anthropic_reply(" hi ")).await;
        let c = config(AiProvider::Anthropic, &server.uri(), Some("sk-ant-key"));

        let out = ask(&c, false).await.unwrap();

        assert_eq!(out, "hi");
        let (request, body) = only_request(&server).await;
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url.path(), "/v1/messages");
        assert_eq!(header(&request, "x-api-key"), Some("sk-ant-key"));
        assert_eq!(header(&request, "anthropic-version"), Some("2023-06-01"));
        assert_eq!(header(&request, "content-type"), Some("application/json"));
        assert!(header(&request, "authorization").is_none());
        assert_eq!(
            body,
            json!({
                "model": "test-model",
                "max_tokens": 123,
                "system": "SYSTEM",
                "messages": [{"role": "user", "content": "USER"}],
            })
        );
    }

    #[tokio::test]
    async fn anthropic_base_ending_in_v1() {
        let server = server_replying(anthropic_reply("ok")).await;
        let c = config(
            AiProvider::Anthropic,
            &format!("{}/v1/", server.uri()),
            Some(KEY),
        );

        ask(&c, false).await.unwrap();

        let (request, _) = only_request(&server).await;
        assert_eq!(request.url.path(), "/v1/messages");
    }

    #[tokio::test]
    async fn base_url_is_trimmed_of_slashes_and_spaces() {
        let server = server_replying(openai_reply("ok")).await;
        let c = config(
            AiProvider::OpenAi,
            &format!("  {}/v1//  ", server.uri()),
            None,
        );

        ask(&c, false).await.unwrap();

        let (request, _) = only_request(&server).await;
        assert_eq!(request.url.path(), "/v1/chat/completions");
    }

    #[tokio::test]
    async fn anthropic_joins_the_text_blocks() {
        let server = server_replying(ResponseTemplate::new(200).set_body_json(json!({"content": [
            {"type": "thinking", "thinking": "hmm"},
            {"type": "text", "text": "Hello, "},
            {"type": "text", "text": "world"},
        ]})))
        .await;
        let c = config(AiProvider::Anthropic, &server.uri(), Some(KEY));

        assert_eq!(ask(&c, false).await.unwrap(), "Hello, world");
    }

    #[tokio::test]
    async fn openai_request_shape() {
        let server = server_replying(openai_reply("\n done\n")).await;
        let c = config(AiProvider::OpenAi, &format!("{}/v1", server.uri()), None);

        let out = ask(&c, false).await.unwrap();

        assert_eq!(out, "done");
        let (request, body) = only_request(&server).await;
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url.path(), "/v1/chat/completions");
        assert_eq!(header(&request, "content-type"), Some("application/json"));
        assert!(
            header(&request, "authorization").is_none(),
            "no key, no Authorization"
        );
        assert!(header(&request, "x-api-key").is_none());
        assert_eq!(
            body,
            json!({
                "model": "test-model",
                "temperature": 0.3,
                "messages": [
                    {"role": "system", "content": "SYSTEM"},
                    {"role": "user", "content": "USER"},
                ],
            })
        );
    }

    #[tokio::test]
    async fn openai_sends_the_key_as_a_bearer_token() {
        let server = server_replying(openai_reply("ok")).await;
        let c = config(AiProvider::OpenAi, &server.uri(), Some("sk-openai"));

        ask(&c, false).await.unwrap();

        let (request, _) = only_request(&server).await;
        assert_eq!(header(&request, "authorization"), Some("Bearer sk-openai"));
    }

    #[tokio::test]
    async fn openai_with_an_empty_key_sends_no_authorization() {
        let server = server_replying(openai_reply("ok")).await;
        let c = config(AiProvider::OpenAi, &server.uri(), Some(""));

        ask(&c, false).await.unwrap();

        let (request, _) = only_request(&server).await;
        assert!(header(&request, "authorization").is_none());
    }

    #[tokio::test]
    async fn openrouter_quick_adds_low_reasoning() {
        // The test server is local, so the host name goes in the path: the check is on the base URL's text.
        let server = server_replying(openai_reply("ok")).await;
        let openrouter = config(
            AiProvider::OpenAi,
            &format!("{}/openrouter.ai/v1", server.uri()),
            Some(KEY),
        );
        let other = config(AiProvider::OpenAi, &server.uri(), Some(KEY));

        ask(&openrouter, true).await.unwrap();
        ask(&openrouter, false).await.unwrap();
        ask(&other, true).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let bodies: Vec<Value> = requests
            .iter()
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect();
        assert_eq!(
            bodies[0]["reasoning"],
            json!({"effort": "low"}),
            "OpenRouter + quick"
        );
        assert!(
            bodies[1].get("reasoning").is_none(),
            "OpenRouter, not quick"
        );
        assert!(bodies[2].get("reasoning").is_none(), "another host, quick");
    }

    #[tokio::test]
    async fn anthropic_never_gets_the_reasoning_parameter() {
        let server = server_replying(anthropic_reply("ok")).await;
        let c = config(
            AiProvider::Anthropic,
            &format!("{}/openrouter.ai", server.uri()),
            Some(KEY),
        );

        ask(&c, true).await.unwrap();

        let (_, body) = only_request(&server).await;
        assert!(body.get("reasoning").is_none());
    }

    #[tokio::test]
    async fn error_message_from_provider() {
        let server = server_replying(
            ResponseTemplate::new(401).set_body_json(json!({"error": {"message": "bad key"}})),
        )
        .await;
        let c = config(AiProvider::OpenAi, &server.uri(), Some(KEY));

        let err = ask(&c, false).await.unwrap_err();

        assert_eq!(
            err,
            ApiError::Status {
                status: 401,
                message: "bad key".into()
            }
        );
    }

    #[tokio::test]
    async fn html_error_body_is_cut_to_300_chars() {
        let html = format!("<html><body>{}</body></html>", "gateway ".repeat(100));
        let server =
            server_replying(ResponseTemplate::new(502).set_body_string(html.clone())).await;
        let c = config(AiProvider::Anthropic, &server.uri(), Some(KEY));

        let ApiError::Status { status, message } = ask(&c, false).await.unwrap_err() else {
            panic!("expected a Status error");
        };

        assert_eq!(status, 502);
        assert_eq!(message, html.chars().take(300).collect::<String>());
    }

    #[tokio::test]
    async fn the_api_key_never_appears_in_an_error() {
        let key = "sk-very-secret-key";
        let server = server_replying(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": {"message": format!("Incorrect API key: {key}")}})),
        )
        .await;
        let c = config(AiProvider::OpenAi, &server.uri(), Some(key));

        let err = ask(&c, false).await.unwrap_err();

        assert!(!err.to_string().contains(key), "{err}");
        assert!(!format!("{err:?}").contains(key), "{err:?}");
    }

    #[tokio::test]
    async fn ok_without_content_is_an_error() {
        let empty_choices =
            server_replying(ResponseTemplate::new(200).set_body_json(json!({"choices": []}))).await;
        let openai = config(AiProvider::OpenAi, &empty_choices.uri(), None);
        assert_eq!(ask(&openai, false).await.unwrap_err(), ApiError::Empty);

        let null_content =
            server_replying(ResponseTemplate::new(200).set_body_json(
                json!({"choices": [{"message": {"content": null, "tool_calls": []}}]}),
            ))
            .await;
        let openai = config(AiProvider::OpenAi, &null_content.uri(), None);
        assert_eq!(ask(&openai, false).await.unwrap_err(), ApiError::Empty);

        let no_text = server_replying(
            ResponseTemplate::new(200)
                .set_body_json(json!({"content": [{"type": "tool_use", "id": "x"}]})),
        )
        .await;
        let anthropic = config(AiProvider::Anthropic, &no_text.uri(), Some(KEY));
        assert_eq!(ask(&anthropic, false).await.unwrap_err(), ApiError::Empty);

        let html =
            server_replying(ResponseTemplate::new(200).set_body_string("<html>welcome</html>"))
                .await;
        let openai = config(AiProvider::OpenAi, &html.uri(), None);
        assert_eq!(
            ask(&openai, false).await.unwrap_err(),
            ApiError::Invalid("<html>welcome</html>".into())
        );

        let not_an_object = server_replying(ResponseTemplate::new(200).set_body_string("[]")).await;
        let openai = config(AiProvider::OpenAi, &not_an_object.uri(), None);
        assert!(matches!(
            ask(&openai, false).await.unwrap_err(),
            ApiError::Invalid(_)
        ));
    }

    #[tokio::test]
    async fn a_base_url_that_is_not_a_url_is_invalid() {
        let c = config(AiProvider::OpenAi, "  //  ", None);
        assert!(matches!(
            ask(&c, false).await.unwrap_err(),
            ApiError::Invalid(_)
        ));
    }

    #[tokio::test]
    async fn unreachable_endpoint_fails_fast() {
        // A port that was just free and has no listener refuses the connection at once.
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let c = config(
            AiProvider::OpenAi,
            &format!("http://127.0.0.1:{port}"),
            None,
        );

        let started = Instant::now();
        let err = complete(&c, "s", "u", 16, Duration::from_secs(10), false)
            .await
            .unwrap_err();

        assert!(
            started.elapsed() < Duration::from_secs(15),
            "took {:?}",
            started.elapsed()
        );
        assert!(matches!(err, ApiError::Network(_)), "{err:?}");
        assert!(err.is_transient());
        assert!(
            !err.to_string().contains("127.0.0.1"),
            "the URL stays out of the message: {err}"
        );
    }

    #[tokio::test]
    async fn a_reply_slower_than_the_timeout_is_a_network_error() {
        let server = server_replying(openai_reply("late").set_delay(Duration::from_secs(5))).await;
        let c = config(AiProvider::OpenAi, &server.uri(), None);

        let started = Instant::now();
        let err = complete(&c, "s", "u", 16, Duration::from_millis(300), false)
            .await
            .unwrap_err();

        assert!(
            started.elapsed() < Duration::from_secs(3),
            "took {:?}",
            started.elapsed()
        );
        assert!(matches!(err, ApiError::Network(_)), "{err:?}");
    }

    const BASE_PROMPT: &str = "You are an editor for raw speech-to-text dictation. The user message contains a transcript produced by automatic speech recognition: it may contain misrecognized words (often homophones), missing punctuation, filler words and repetitions, and may mix languages (e.g. Vietnamese with English technical terms).\nApply the requested edit. Unless asked to translate, keep the original language(s) — do not translate English technical terms that the speaker used.\nOutput ONLY the final text. No preamble, no explanations, no surrounding quotes, no markdown code fences.";

    #[test]
    fn system_prompt_appends_vocabulary() {
        assert_eq!(system_prompt(""), BASE_PROMPT);
        assert_eq!(system_prompt(" \n\t "), BASE_PROMPT);
        assert_eq!(
            system_prompt("  Kubernetes, Tauri \n"),
            format!(
                "{BASE_PROMPT}\n\nCustom vocabulary / context from the user (prefer these spellings when the transcript sounds similar):\nKubernetes, Tauri"
            )
        );
    }

    #[tokio::test]
    async fn refine_sends_the_edit_instruction_and_the_transcript() {
        let server = server_replying(anthropic_reply("  Fixed text.\n")).await;
        let mut c = config(AiProvider::Anthropic, &server.uri(), Some(KEY));
        c.vocabulary = "Dictate".to_string();

        let out = refine(&c, "um hello wrold", "Fix mistakes.").await.unwrap();

        assert_eq!(out, "Fixed text.");
        let (_, body) = only_request(&server).await;
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["system"], system_prompt("Dictate"));
        assert_eq!(
            body["messages"][0]["content"],
            "Edit instruction: Fix mistakes.\n\n<transcript>\num hello wrold\n</transcript>"
        );
    }

    #[tokio::test]
    async fn refine_is_a_quick_call() {
        let server = server_replying(openai_reply("ok")).await;
        let c = config(
            AiProvider::OpenAi,
            &format!("{}/openrouter.ai/v1", server.uri()),
            None,
        );

        refine(&c, "text", "instruction").await.unwrap();

        let (_, body) = only_request(&server).await;
        assert_eq!(body["reasoning"], json!({"effort": "low"}));
    }

    #[tokio::test]
    async fn check_sends_a_tiny_probe_and_accepts_any_reply() {
        let server = server_replying(anthropic_reply("Sure")).await;
        let c = config(AiProvider::Anthropic, &server.uri(), Some(KEY));

        check(&c).await.unwrap();

        let (_, body) = only_request(&server).await;
        assert_eq!(body["system"], "Reply with OK.");
        assert_eq!(body["messages"][0]["content"], "OK?");
        assert_eq!(body["max_tokens"], 16);
    }

    #[tokio::test]
    async fn check_reports_a_provider_error() {
        let server = server_replying(
            ResponseTemplate::new(404)
                .set_body_json(json!({"error": {"message": "model not found"}})),
        )
        .await;
        let c = config(AiProvider::OpenAi, &server.uri(), None);

        assert_eq!(
            check(&c).await.unwrap_err(),
            ApiError::Status {
                status: 404,
                message: "model not found".into()
            }
        );
    }

    #[tokio::test]
    async fn check_fails_when_the_reply_is_empty() {
        let server =
            server_replying(ResponseTemplate::new(200).set_body_json(json!({"choices": []}))).await;
        let c = config(AiProvider::OpenAi, &server.uri(), None);

        assert_eq!(check(&c).await.unwrap_err(), ApiError::Empty);
    }

    #[test]
    fn debug_output_hides_the_api_key() {
        let c = config(
            AiProvider::OpenAi,
            "http://localhost:8317/v1",
            Some("sk-very-secret"),
        );

        let text = format!("{c:?}");

        assert!(!text.contains("sk-very-secret"), "{text}");
        assert!(text.contains("http://localhost:8317/v1"), "{text}");
        assert!(text.contains("test-model"), "{text}");
    }
}
