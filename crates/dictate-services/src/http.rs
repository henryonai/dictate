//! The HTTP client and the error type shared by the AI and transcription clients.

use std::fmt;
use std::time::Duration;

/// Why a call to a provider failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// The server answered with an error status.
    Status { status: u16, message: String },
    /// The request never got an answer (no route, refused, timed out, broken connection).
    Network(String),
    /// The request could not be built, or the answer was not what the API promises.
    Invalid(String),
    /// A good answer that holds no content (nothing, or only blanks).
    Empty,
}

impl ApiError {
    /// Whether asking again may well work: 408, 429, 5xx, a good reply without content (the provider failed
    /// mid-reply), and any network failure. A bad key, missing credit, a model that cannot do the job or a reply
    /// that is not the API's do not pass.
    pub fn is_transient(&self) -> bool {
        match self {
            ApiError::Status { status, .. } => {
                [0, 200, 408, 429].contains(status) || *status >= 500
            }
            ApiError::Network(_) | ApiError::Empty => true,
            ApiError::Invalid(_) => false,
        }
    }

    /// Hides `secret` in the messages, in case a provider or proxy echoes it back.
    pub fn redact(self, secret: Option<&str>) -> ApiError {
        let hide = |m: String| hide_secret(&m, secret);
        match self {
            ApiError::Status { status, message } => ApiError::Status {
                status,
                message: hide(message),
            },
            ApiError::Network(m) => ApiError::Network(hide(m)),
            ApiError::Invalid(m) => ApiError::Invalid(hide(m)),
            ApiError::Empty => ApiError::Empty,
        }
    }
}

/// `text` with every `secret` in it replaced by `***`; as it is when there is no secret (or an empty one).
fn hide_secret(text: &str, secret: Option<&str>) -> String {
    match secret.filter(|s| !s.is_empty()) {
        Some(secret) => text.replace(secret, "***"),
        None => text.to_owned(),
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Status { status, message } => write!(f, "API error {status}: {message}"),
            ApiError::Network(m) => write!(f, "Network error: {m}"),
            ApiError::Invalid(m) => write!(f, "Invalid API reply: {m}"),
            ApiError::Empty => f.write_str("The API returned no content."),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        // The URL is dropped from the text: it is noise, and a base URL may carry credentials.
        let e = e.without_url();
        if e.is_builder() {
            return ApiError::Invalid(e.to_string());
        }
        // reqwest's own text stops at "error sending request": the cause (refused, timed out, TLS) is below it.
        let mut text = e.to_string();
        let mut source = std::error::Error::source(&e);
        while let Some(cause) = source {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            source = cause.source();
        }
        ApiError::Network(text)
    }
}

/// A client for one burst of calls: rustls, 10 s to connect, and a `Dictate/<version>` user agent. There is no
/// overall timeout here; each call sets its own.
///
/// Redirects are not followed: reqwest would pass headers such as `x-api-key` on to the host a redirect names.
/// A 3xx reply comes back as a `Status` error instead.
pub fn client() -> Result<reqwest::Client, ApiError> {
    reqwest::Client::builder()
        .user_agent(concat!("Dictate/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        // A builder error, so this becomes `ApiError::Invalid`: the request cannot even be set up.
        .map_err(ApiError::from)
}

/// The URL of a call; an error when the base URL the user typed does not make a valid one.
pub(crate) fn parse_url(url: &str) -> Result<reqwest::Url, ApiError> {
    reqwest::Url::parse(url)
        .map_err(|_| ApiError::Invalid("The base URL is not valid.".to_string()))
}

/// Builds the error for a reply that is not a success: the provider's `error.message` when the body has one,
/// else the first 300 characters of the body. `secret`, the key the call was made with, is hidden in either.
pub fn status_error(status: u16, body: &[u8], secret: Option<&str>) -> ApiError {
    let message = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|json| {
            let message = json.pointer("/error/message")?.as_str()?;
            Some(hide_secret(message, secret))
        })
        .unwrap_or_else(|| snippet(body, secret));
    ApiError::Status { status, message }
}

/// The first 300 characters of a body, for error messages, with `secret` hidden.
///
/// The key is hidden in the whole body and only then is the text cut: the other way round the cut could leave
/// the beginning of a key that it split, which no later hiding would recognize.
pub fn snippet(body: &[u8], secret: Option<&str>) -> String {
    hide_secret(&String::from_utf8_lossy(body), secret)
        .chars()
        .take(300)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn status(status: u16) -> ApiError {
        ApiError::Status {
            status,
            message: String::new(),
        }
    }

    #[test]
    fn statuses_worth_a_retry_are_transient() {
        for code in [0, 200, 408, 429, 500, 502, 503, 599] {
            assert!(status(code).is_transient(), "{code} should be transient");
        }
    }

    #[test]
    fn statuses_that_a_retry_cannot_fix_are_not_transient() {
        for code in [400, 401, 402, 403, 404, 422] {
            assert!(!status(code).is_transient(), "{code} should not be");
        }
    }

    #[test]
    fn network_errors_and_empty_replies_are_transient_but_invalid_ones_are_not() {
        assert!(ApiError::Network("refused".into()).is_transient());
        assert!(ApiError::Empty.is_transient());
        assert!(!ApiError::Invalid("<html>".into()).is_transient());
    }

    #[test]
    fn status_error_prefers_the_provider_message() {
        let body = br#"{"error":{"message":"bad key","type":"auth"}}"#;
        assert_eq!(
            status_error(401, body, None),
            ApiError::Status {
                status: 401,
                message: "bad key".into()
            }
        );
    }

    #[test]
    fn status_error_falls_back_to_the_start_of_the_body() {
        let html = format!("<html>{}</html>", "x".repeat(500));
        let ApiError::Status { status, message } = status_error(502, html.as_bytes(), None) else {
            panic!("expected a Status error");
        };
        assert_eq!(status, 502);
        assert_eq!(message.chars().count(), 300);
        assert!(message.starts_with("<html>xxx"));
    }

    #[test]
    fn status_error_counts_characters_not_bytes() {
        let body = "ế".repeat(400);
        let ApiError::Status { message, .. } = status_error(500, body.as_bytes(), None) else {
            panic!("expected a Status error");
        };
        assert_eq!(message, "ế".repeat(300));
    }

    #[test]
    fn status_error_uses_the_body_when_the_error_has_no_message() {
        let body = br#"{"error":"rate limited"}"#;
        assert_eq!(
            status_error(429, body, None),
            ApiError::Status {
                status: 429,
                message: r#"{"error":"rate limited"}"#.into()
            }
        );
    }

    #[test]
    fn status_error_of_an_empty_body_has_an_empty_message() {
        assert_eq!(status_error(503, b"", None), status(503));
    }

    /// A body of `length` characters with `key` starting at character `start`.
    fn body_with_key_at(start: usize, length: usize, key: &str) -> String {
        let mut body = "x".repeat(start);
        body.push_str(key);
        body.push_str(&"y".repeat(length - start - key.chars().count()));
        body
    }

    const KEY: &str = "sk-or-v1-0123456789abcdef0123456789abcdef";

    #[test]
    fn a_key_that_the_cut_would_split_is_hidden_before_it() {
        // The key starts a little before the 300th character: cutting first and hiding after would leave its
        // beginning in the message, where hiding no longer recognizes it.
        for start in 250..=299 {
            let body = body_with_key_at(start, 500, KEY);
            let ApiError::Status { message, .. } = status_error(502, body.as_bytes(), Some(KEY))
            else {
                panic!("expected a Status error");
            };
            let leaked = |text: &str| text.contains(&KEY[..8]);
            assert!(!leaked(&message), "key at {start}: {message}");
            assert!(
                !leaked(&snippet(body.as_bytes(), Some(KEY))),
                "key at {start}"
            );
        }
    }

    #[test]
    fn the_message_is_cut_after_the_key_is_hidden() {
        let body = body_with_key_at(0, 500, KEY);
        let ApiError::Status { message, .. } = status_error(502, body.as_bytes(), Some(KEY)) else {
            panic!("expected a Status error");
        };
        assert_eq!(message.chars().count(), 300);
        assert!(message.starts_with("***yyy"));
        assert_eq!(snippet(body.as_bytes(), Some(KEY)), message);
    }

    #[test]
    fn the_key_is_hidden_in_the_message_of_the_provider_too() {
        let body = format!(r#"{{"error":{{"message":"No access for {KEY}, sorry"}}}}"#);
        assert_eq!(
            status_error(401, body.as_bytes(), Some(KEY)),
            ApiError::Status {
                status: 401,
                message: "No access for ***, sorry".into()
            }
        );
    }

    #[test]
    fn nothing_is_hidden_without_a_key() {
        for secret in [None, Some("")] {
            assert_eq!(snippet(b"plain body", secret), "plain body");
        }
    }

    #[test]
    fn redact_hides_the_secret_in_every_message() {
        let secret = Some("sk-secret-123");
        let echoed = |m: &str| ApiError::Status {
            status: 401,
            message: m.into(),
        };
        assert_eq!(
            echoed("bad key sk-secret-123, really sk-secret-123").redact(secret),
            echoed("bad key ***, really ***")
        );
        assert_eq!(
            ApiError::Invalid("got sk-secret-123".into()).redact(secret),
            ApiError::Invalid("got ***".into())
        );
        assert_eq!(
            ApiError::Network("to sk-secret-123".into()).redact(secret),
            ApiError::Network("to ***".into())
        );
        assert_eq!(ApiError::Empty.redact(secret), ApiError::Empty);
    }

    #[test]
    fn redact_without_a_secret_changes_nothing() {
        let e = ApiError::Invalid("abc".into());
        assert_eq!(e.clone().redact(None), e);
        assert_eq!(e.clone().redact(Some("")), e);
    }

    #[tokio::test]
    async fn client_sends_a_dictate_user_agent() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        client().unwrap().get(server.uri()).send().await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let agent = requests[0]
            .headers
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(agent, format!("Dictate/{}", env!("CARGO_PKG_VERSION")));
    }

    #[tokio::test]
    async fn client_does_not_follow_redirects() {
        // reqwest would pass an `x-api-key` header on to the host a redirect names.
        let target = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200))
            .mount(&target)
            .await;
        let origin = MockServer::start().await;
        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/steal", target.uri())),
            )
            .mount(&origin)
            .await;

        let response = client()
            .unwrap()
            .post(origin.uri())
            .header("x-api-key", "sk-test-key-123")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status().as_u16(), 307);
        assert!(target.received_requests().await.unwrap().is_empty());
    }
}
