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
    /// A good answer that holds no content.
    Empty,
}

impl ApiError {
    /// Whether asking again may well work: 408, 429, 5xx, a 200 that carried nothing usable, and any
    /// network failure. A bad key, missing credit or a model that cannot do the job do not pass.
    pub fn is_transient(&self) -> bool {
        match self {
            ApiError::Status { status, .. } => {
                [0, 200, 408, 429].contains(status) || *status >= 500
            }
            ApiError::Network(_) => true,
            ApiError::Invalid(_) | ApiError::Empty => false,
        }
    }

    /// Hides `secret` in the messages, in case a provider or proxy echoes it back.
    pub fn redact(self, secret: Option<&str>) -> ApiError {
        let Some(secret) = secret.filter(|s| !s.is_empty()) else {
            return self;
        };
        let hide = |m: String| m.replace(secret, "***");
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
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Dictate/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .build()
        // With rustls and its bundled root certificates there is nothing here that can fail in practice; the
        // fallback keeps the signature infallible for callers.
        .unwrap_or_default()
}

/// Builds the error for a reply that is not a success: the provider's `error.message` when the body has one,
/// else the first 300 characters of the body.
pub fn status_error(status: u16, body: &[u8]) -> ApiError {
    let message = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|json| json.pointer("/error/message")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| snippet(body));
    ApiError::Status { status, message }
}

/// The first 300 characters of a body, for error messages.
pub fn snippet(body: &[u8]) -> String {
    String::from_utf8_lossy(body).chars().take(300).collect()
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
    fn network_errors_are_transient_and_bad_replies_are_not() {
        assert!(ApiError::Network("refused".into()).is_transient());
        assert!(!ApiError::Invalid("<html>".into()).is_transient());
        assert!(!ApiError::Empty.is_transient());
    }

    #[test]
    fn status_error_prefers_the_provider_message() {
        let body = br#"{"error":{"message":"bad key","type":"auth"}}"#;
        assert_eq!(
            status_error(401, body),
            ApiError::Status {
                status: 401,
                message: "bad key".into()
            }
        );
    }

    #[test]
    fn status_error_falls_back_to_the_start_of_the_body() {
        let html = format!("<html>{}</html>", "x".repeat(500));
        let ApiError::Status { status, message } = status_error(502, html.as_bytes()) else {
            panic!("expected a Status error");
        };
        assert_eq!(status, 502);
        assert_eq!(message.chars().count(), 300);
        assert!(message.starts_with("<html>xxx"));
    }

    #[test]
    fn status_error_counts_characters_not_bytes() {
        let body = "ế".repeat(400);
        let ApiError::Status { message, .. } = status_error(500, body.as_bytes()) else {
            panic!("expected a Status error");
        };
        assert_eq!(message, "ế".repeat(300));
    }

    #[test]
    fn status_error_uses_the_body_when_the_error_has_no_message() {
        let body = br#"{"error":"rate limited"}"#;
        assert_eq!(
            status_error(429, body),
            ApiError::Status {
                status: 429,
                message: r#"{"error":"rate limited"}"#.into()
            }
        );
    }

    #[test]
    fn status_error_of_an_empty_body_has_an_empty_message() {
        assert_eq!(status_error(503, b""), status(503));
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

        client().get(server.uri()).send().await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let agent = requests[0]
            .headers
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(agent, format!("Dictate/{}", env!("CARGO_PKG_VERSION")));
    }
}
