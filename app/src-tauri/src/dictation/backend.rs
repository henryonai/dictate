//! The network calls of the stop pipeline, with the app's settings and keys: the speech API that hears a recording again,
//! and the AI model that edits a text.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dictate_services::ai::{self, AiConfig};
use dictate_services::dictation::pipeline::Backend;
use dictate_services::secrets::{Account, SecretStore};
use dictate_services::settings::{self, SettingsStore};
use dictate_services::transcribe_api::{self, TranscribeConfig};

use crate::commands::{api_error_text, transcribe_error_text};
use crate::config::{dictation_transcribe_config, saved_key};
use crate::dictation::join_segments;
use crate::state::KeyFlags;

/// What the person is told when the speech API has no key: the same words as the settings give.
const NO_TRANSCRIBE_KEY: &str = "Add the API key of the transcription service first.";
const NO_AI: &str = "Add the API key of the AI provider first.";

/// The pipeline's calls, made with what the settings and the key store say right now. The key store is asked only when the
/// flags say that a key is there: asking it can show the person a prompt.
pub struct AppBackend {
    settings: Arc<SettingsStore>,
    secrets: Arc<dyn SecretStore>,
    flags: Arc<KeyFlags>,
}

impl AppBackend {
    pub fn new(
        settings: Arc<SettingsStore>,
        secrets: Arc<dyn SecretStore>,
        flags: Arc<KeyFlags>,
    ) -> Self {
        Self {
            settings,
            secrets,
            flags,
        }
    }

    /// The speech API of the dictation, with the language (the first two letters of the dictation language) and the vocabulary
    /// (cut to 800 characters) it is told. `None` when there is no key to send.
    fn transcribe_config(&self) -> Option<TranscribeConfig> {
        let settings = self.settings.get();
        let has_own = self.flags.get(Account::Transcribe);
        let borrows = settings::shares_ai_key(&settings, self.flags.get(Account::Ai), has_own);
        // No key by the flags: there is nothing to look for, and looking could make the system ask the person something.
        if !has_own && !borrows {
            return None;
        }
        dictation_transcribe_config(&settings, &*self.secrets)
    }

    fn ai_config(&self) -> Result<AiConfig, String> {
        let settings = self.settings.get();
        let has_key = self.flags.get(Account::Ai);
        if !settings::is_ai_configured(&settings, has_key) {
            return Err(NO_AI.to_owned());
        }
        Ok(AiConfig {
            provider: settings.ai_provider,
            base_url: settings.ai_base_url,
            model: settings.ai_model,
            api_key: if has_key {
                saved_key(&*self.secrets, Account::Ai)
            } else {
                None
            },
            vocabulary: settings.vocabulary,
        })
    }
}

#[async_trait::async_trait]
impl Backend for AppBackend {
    async fn transcribe(&self, wav: &Path, timeout: Duration) -> Result<String, String> {
        let config = self.transcribe_config().ok_or(NO_TRANSCRIBE_KEY)?;
        let segments = transcribe_api::transcribe_file(&config, wav, None, timeout)
            .await
            .map_err(|e| transcribe_error_text(&e))?;
        let language = self.settings.get().dictation_language;
        Ok(join_segments(&segments, &language))
    }

    async fn transcribe_and_edit(
        &self,
        wav: &Path,
        instruction: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let config = self.transcribe_config().ok_or(NO_TRANSCRIBE_KEY)?;
        transcribe_api::dictate(&config, wav, instruction, timeout)
            .await
            .map_err(|e| transcribe_error_text(&e))
    }

    async fn refine(&self, text: &str, instruction: &str) -> Result<String, String> {
        let config = self.ai_config()?;
        ai::refine(&config, text, instruction)
            .await
            .map_err(|e| api_error_text(&e))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use dictate_services::presets::{AiProvider, TranscribePreset};

    use super::*;
    use crate::test_support::CountingStore;

    const STT_KEY: &str = "sk-stt-secret-456";
    const AI_KEY: &str = "sk-ai-secret-123";

    struct Fixture {
        _dir: tempfile::TempDir,
        settings: Arc<SettingsStore>,
        flags: Arc<KeyFlags>,
        secrets: Arc<CountingStore>,
        backend: AppBackend,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let (settings, _) = SettingsStore::open(dir.path().join("settings.json"));
        let settings = Arc::new(settings);
        let flags = Arc::new(KeyFlags::default());
        let secrets = Arc::new(CountingStore::default());
        let backend = AppBackend::new(settings.clone(), secrets.clone(), flags.clone());
        Fixture {
            _dir: dir,
            settings,
            flags,
            secrets,
            backend,
        }
    }

    /// A server that answers one request with `body` as JSON, and tells the test what it was sent: the head and the body.
    fn serve_once(body: &'static str) -> (String, mpsc::Receiver<(String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sent, received) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    return;
                }
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|n| n.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut request = vec![0u8; length];
            stream.read_exact(&mut request).unwrap();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = sent.send((head, request));
        });
        (format!("http://127.0.0.1:{port}"), received)
    }

    fn a_recording(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("take.wav");
        dictate_services::audio::wav::write_16k_mono(&path, &[0.1; 1600]).unwrap();
        path
    }

    #[tokio::test]
    async fn the_speech_api_is_not_asked_without_a_key_and_the_key_store_is_left_alone() {
        let f = fixture();
        let dir = tempfile::tempdir().unwrap();
        let wav = a_recording(&dir);

        let error = f
            .backend
            .transcribe(&wav, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.to_lowercase().contains("key"), "{error}");
        assert_eq!(
            f.secrets.gets(),
            0,
            "the key store was asked with no key to find"
        );
        let error = f
            .backend
            .transcribe_and_edit(&wav, "clean", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.to_lowercase().contains("key"), "{error}");
    }

    #[tokio::test]
    async fn the_speech_api_hears_the_recording_in_the_language_of_the_dictation() {
        let f = fixture();
        f.secrets.set(Account::Transcribe, STT_KEY).unwrap();
        f.flags.set(Account::Transcribe, true);
        let (url, received) = serve_once(r#"{"text":" xin chào "}"#);
        f.settings
            .update(|s| {
                s.set_transcribe_preset(TranscribePreset::Custom);
                s.transcribe_base_url = url;
                s.dictation_model = "whisper-1".into();
                s.dictation_language = "vi-VN".into();
                // Characters, not bytes: three bytes each.
                s.vocabulary = "ế".repeat(900);
            })
            .unwrap();
        let dir = tempfile::tempdir().unwrap();

        let text = f
            .backend
            .transcribe(&a_recording(&dir), Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(text, "xin chào");

        let (head, body) = received.recv_timeout(Duration::from_secs(5)).unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(head.contains("/audio/transcriptions"), "{head}");
        assert!(head.contains(&format!("Bearer {STT_KEY}")));
        assert!(
            body.contains("name=\"language\"\r\n\r\nvi\r\n"),
            "the language is two letters"
        );
        assert!(body.contains(&format!("\r\n\r\n{}\r\n", "ế".repeat(800))));
        assert!(!body.contains(&"ế".repeat(801)));
    }

    #[tokio::test]
    async fn the_ai_is_not_asked_without_its_key_and_its_key_is_never_in_the_error() {
        let f = fixture();
        let error = f.backend.refine("hello", "clean").await.unwrap_err();
        assert!(error.to_lowercase().contains("key"), "{error}");
        assert_eq!(f.secrets.gets(), 0);

        // A provider that answers with an error that quotes the key it was sent.
        f.secrets.set(Account::Ai, AI_KEY).unwrap();
        f.flags.set(Account::Ai, true);
        let (url, _received) = serve_once(r#"{"error":{"message":"bad key sk-ai-secret-123"}}"#);
        f.settings
            .update(|s| {
                s.set_ai_provider(AiProvider::OpenAi);
                s.ai_base_url = url;
            })
            .unwrap();
        // The server answers 200 with no choices: an empty answer is an error, never a text.
        let error = f.backend.refine("hello", "clean").await.unwrap_err();
        assert!(!error.contains(AI_KEY), "{error}");
        assert!(!error.is_empty());
    }

    #[tokio::test]
    async fn the_ai_edit_goes_to_the_model_with_the_vocabulary() {
        let f = fixture();
        let (url, received) =
            serve_once(r#"{"choices":[{"message":{"content":"Hello, world."}}]}"#);
        // A local server needs no key at all.
        f.settings
            .update(|s| {
                s.set_ai_provider(AiProvider::OpenAi);
                s.ai_base_url = url;
                s.vocabulary = "Dictate".into();
            })
            .unwrap();

        let text = f.backend.refine("hello world", "Fix it.").await.unwrap();
        assert_eq!(text, "Hello, world.");
        let (_, body) = received.recv_timeout(Duration::from_secs(5)).unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("hello world") && body.contains("Fix it.") && body.contains("Dictate")
        );
        assert_eq!(
            f.secrets.gets(),
            0,
            "a server with no key has none to ask for"
        );
    }
}
