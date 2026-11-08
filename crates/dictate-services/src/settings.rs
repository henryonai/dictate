//! The settings file and the rules that derive state from it.
//!
//! Only plain settings live here. API keys belong to the OS credential store (see `secrets`) and
//! never reach this file.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::presets::{AiProvider, TranscribePreset};

/// Which engine turns speech into text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// On-device Whisper model.
    #[default]
    Whisper,
    /// The configured transcription API.
    Api,
}

/// Language of the app's own UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiLanguage {
    /// Follow the OS language.
    #[default]
    System,
    En,
    Vi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(deserialize_with = "lenient")]
    pub ui_language: UiLanguage,
    /// BCP-47 locale for dictation, e.g. `en-US`.
    pub dictation_language: String,
    /// Global shortcut, e.g. `Ctrl+Alt+Space`.
    pub hotkey: String,
    pub push_to_talk: bool,
    pub auto_refine: bool,
    /// Write the dictation result straight into the focused text field.
    pub auto_write: bool,
    pub save_recordings: bool,
    pub launch_at_login: bool,
    #[serde(deserialize_with = "lenient")]
    pub ai_provider: AiProvider,
    pub ai_base_url: String,
    pub ai_model: String,
    /// Empty = use `ai_model`.
    pub summary_model: String,
    pub vocabulary: String,
    pub default_edit_preset: String,
    #[serde(deserialize_with = "lenient")]
    pub dictation_engine: Engine,
    #[serde(deserialize_with = "lenient")]
    pub file_engine: Engine,
    /// A Whisper model name; `None` = the recommended one for this machine.
    pub whisper_model: Option<String>,
    pub diarize: bool,
    #[serde(deserialize_with = "lenient")]
    pub transcribe_preset: TranscribePreset,
    pub transcribe_base_url: String,
    /// Model for file transcripts.
    pub transcribe_model: String,
    /// Model for dictation: fast, where `transcribe_model` suits long audio.
    pub dictation_model: String,
    /// ISO-639-1 code for file transcripts; empty = detect it.
    pub transcribe_language: String,
}

/// An unknown or malformed value becomes the type's default instead of failing the whole file.
fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

impl Default for Settings {
    fn default() -> Self {
        let provider = AiProvider::default();
        let preset = TranscribePreset::default();
        let model = preset.models().first().copied().unwrap_or("whisper-1");
        Settings {
            ui_language: UiLanguage::default(),
            dictation_language: "en-US".into(),
            hotkey: "Ctrl+Alt+Space".into(),
            push_to_talk: false,
            auto_refine: true,
            auto_write: true,
            save_recordings: true,
            launch_at_login: false,
            ai_provider: provider,
            ai_base_url: provider.default_base_url().into(),
            ai_model: provider.default_model().into(),
            summary_model: String::new(),
            vocabulary: String::new(),
            default_edit_preset: "clean".into(),
            dictation_engine: Engine::default(),
            file_engine: Engine::default(),
            whisper_model: None,
            diarize: true,
            transcribe_preset: preset,
            transcribe_base_url: preset.base_url().into(),
            transcribe_model: model.into(),
            dictation_model: model.into(),
            transcribe_language: String::new(),
        }
    }
}

impl Settings {
    /// Switch provider; the base URL follows unless the user has changed it.
    pub fn set_ai_provider(&mut self, p: AiProvider) {
        let old = self.ai_provider;
        self.ai_provider = p;
        if self.ai_base_url == old.default_base_url() || self.ai_base_url.is_empty() {
            self.ai_base_url = p.default_base_url().into();
        }
    }

    /// Switch transcription service; the base URL follows unless the user has changed it, and a
    /// model the new service doesn't offer becomes its first one.
    pub fn set_transcribe_preset(&mut self, p: TranscribePreset) {
        let old = self.transcribe_preset;
        self.transcribe_preset = p;
        if p == old {
            return;
        }
        if self.transcribe_base_url == old.base_url() || self.transcribe_base_url.is_empty() {
            self.transcribe_base_url = p.base_url().into();
        }
        let models = p.models();
        if let Some(first) = models.first() {
            for model in [&mut self.transcribe_model, &mut self.dictation_model] {
                if !models.contains(&model.as_str()) {
                    *model = (*first).into();
                }
            }
        }
    }
}

/// With OpenRouter for both AI and transcripts one key is enough: the AI key is reused when no
/// separate transcription key is set. Empty keys count as absent.
///
/// The AI key only ever goes to OpenRouter itself: both base URLs must be `https` URLs whose host is
/// exactly `openrouter.ai`. A transcription URL that points anywhere else gets no key of the AI part.
pub fn effective_transcribe_key(
    s: &Settings,
    ai_key: Option<&str>,
    transcribe_key: Option<&str>,
) -> Option<String> {
    let own = transcribe_key.filter(|k| !k.is_empty());
    let ai = ai_key.filter(|k| !k.is_empty());
    match (own, ai) {
        (Some(key), _) => Some(key.into()),
        (None, Some(key))
            if s.transcribe_preset == TranscribePreset::OpenRouter
                && is_openrouter_url(&s.ai_base_url)
                && is_openrouter_url(&s.transcribe_base_url) =>
        {
            Some(key.into())
        }
        _ => None,
    }
}

/// Whether the URL is an `https` one on the host `openrouter.ai` itself, not a path, a query, a user name or a
/// longer host name that merely contains it.
fn is_openrouter_url(url: &str) -> bool {
    reqwest::Url::parse(url.trim())
        .is_ok_and(|url| url.scheme() == "https" && url.host_str() == Some("openrouter.ai"))
}

/// Local proxies (CLIProxyAPI, Ollama) work without a key; hosted endpoints such as OpenRouter
/// need one.
pub fn is_ai_configured(s: &Settings, has_ai_key: bool) -> bool {
    match s.ai_provider {
        AiProvider::Anthropic => has_ai_key,
        AiProvider::OpenAi => {
            !s.ai_base_url.is_empty() && (has_ai_key || is_local_url(&s.ai_base_url))
        }
    }
}

/// Whether the URL's host is this machine. Only the host counts, not the path or query.
fn is_local_url(url: &str) -> bool {
    let url = url.trim();
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if host_port.starts_with('[') {
        // IPv6 literal: keep the brackets, drop the port.
        host_port
            .find(']')
            .map_or(host_port, |end| &host_port[..=end])
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    ["localhost", "127.0.0.1", "[::1]"]
        .iter()
        .any(|local| host.eq_ignore_ascii_case(local))
}

/// Why the settings file wasn't used as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadWarning {
    pub message: String,
    /// Where the unreadable file was kept, if it could be.
    pub backup: Option<PathBuf>,
}

impl fmt::Display for LoadWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The settings file plus an in-memory copy of what it holds.
#[derive(Debug)]
pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsStore {
    /// `<config dir>/Dictate/settings.json`.
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_default()
            .join("Dictate")
            .join("settings.json")
    }

    /// Load the file. A missing file is normal (defaults, no warning).
    ///
    /// A file that isn't valid JSON, or isn't a JSON object, gives all defaults plus a warning. A
    /// field that can't be read (say `"auto_refine": "yes"`) falls back to its own default while
    /// every other field is kept, again with a warning. In both cases the file is first copied to
    /// `<name>.bak` so nothing the user typed is lost. Unknown fields are ignored and unknown enum
    /// values silently become their default.
    pub fn open(path: PathBuf) -> (SettingsStore, Option<LoadWarning>) {
        let (settings, warning) = match fs::read(&path) {
            Ok(bytes) => match parse(&bytes) {
                Ok((settings, dropped)) if dropped.is_empty() => (settings, None),
                Ok((settings, dropped)) => {
                    let lead = format!(
                        "These settings couldn't be read and were reset to their defaults: {}",
                        dropped.join(", ")
                    );
                    (settings, Some(keep_backup(&path, &lead)))
                }
                Err(why) => {
                    let lead = format!("The settings file was reset to defaults because {why}");
                    (Settings::default(), Some(keep_backup(&path, &lead)))
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => (
                Settings::default(),
                Some(LoadWarning {
                    message: format!("The settings file couldn't be read ({e}); using defaults."),
                    backup: None,
                }),
            ),
        };
        let store = SettingsStore {
            path,
            current: Mutex::new(settings),
        };
        (store, warning)
    }

    pub fn get(&self) -> Settings {
        lock(&self.current).clone()
    }

    /// Write the file atomically (temp file, then rename) and make `s` the current settings.
    pub fn save(&self, s: &Settings) -> io::Result<()> {
        let mut current = lock(&self.current);
        let mut json = serde_json::to_vec_pretty(s)?;
        json.push(b'\n');
        write_atomic(&self.path, &json)?;
        *current = s.clone();
        Ok(())
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `path` with `suffix` added to its file name: `settings.json` + `.bak`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.file_name().unwrap_or_default().to_owned();
    name.push(suffix);
    path.with_file_name(name)
}

/// Parse the file one field at a time, so one bad value never costs the others. Returns the
/// settings plus the names of the fields that were dropped, or why the file isn't usable at all.
fn parse(bytes: &[u8]) -> Result<(Settings, Vec<String>), String> {
    let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let value: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("it isn't valid JSON ({e})"))?;
    let serde_json::Value::Object(fields) = value else {
        return Err("it isn't a JSON object".into());
    };

    // Add the fields one by one, keeping each only if the file still reads as `Settings`. An
    // unknown field always passes (it is ignored), so only a known field with a bad value is dropped.
    let mut settings = Settings::default();
    let mut accepted = serde_json::Map::new();
    let mut dropped = Vec::new();
    for (name, value) in fields {
        accepted.insert(name.clone(), value);
        match serde_json::from_value::<Settings>(serde_json::Value::Object(accepted.clone())) {
            Ok(parsed) => settings = parsed,
            Err(_) => {
                accepted.remove(&name);
                dropped.push(name);
            }
        }
    }
    Ok((settings, dropped))
}

/// Copy the unusable file to `<name>.bak`; `lead` says what went wrong, without a final period.
fn keep_backup(path: &Path, lead: &str) -> LoadWarning {
    let backup = with_suffix(path, ".bak");
    match fs::copy(path, &backup) {
        Ok(_) => LoadWarning {
            message: format!("{lead}. The old file was kept as {}.", backup.display()),
            backup: Some(backup),
        },
        Err(e) => LoadWarning {
            message: format!("{lead}, and the old file couldn't be kept ({e})."),
            backup: None,
        },
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let tmp = with_suffix(path, ".tmp");
    let written = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::{AiProvider, TranscribePreset};
    use std::fs;

    fn store_in(dir: &tempfile::TempDir) -> (SettingsStore, Option<LoadWarning>) {
        SettingsStore::open(dir.path().join("settings.json"))
    }

    #[test]
    fn defaults_match_spec() {
        let s = Settings::default();
        assert_eq!(s.ui_language, UiLanguage::System);
        assert_eq!(s.dictation_language, "en-US");
        assert_eq!(s.hotkey, "Ctrl+Alt+Space");
        assert_eq!(s.ai_provider, AiProvider::Anthropic);
        assert_eq!(s.ai_base_url, "https://api.anthropic.com");
        assert_eq!(s.ai_model, "claude-haiku-5-5");
        assert_eq!(s.summary_model, "");
        assert_eq!(s.vocabulary, "");
        assert_eq!(s.default_edit_preset, "clean");
        assert!(s.auto_refine && s.auto_write && s.save_recordings && s.diarize);
        assert!(!s.push_to_talk && !s.launch_at_login);
        assert_eq!(s.dictation_engine, Engine::Whisper);
        assert_eq!(s.file_engine, Engine::Whisper);
        assert_eq!(s.whisper_model, None);
        assert_eq!(s.transcribe_preset, TranscribePreset::OpenAi);
        assert_eq!(s.transcribe_base_url, "https://api.openai.com/v1");
        assert_eq!(s.transcribe_model, "whisper-1");
        assert_eq!(s.dictation_model, "whisper-1");
        assert_eq!(s.transcribe_language, "");
    }

    #[test]
    fn switching_preset_moves_untouched_url_and_models() {
        let mut s = Settings::default();
        s.set_transcribe_preset(TranscribePreset::Groq);
        assert_eq!(s.transcribe_preset, TranscribePreset::Groq);
        assert_eq!(s.transcribe_base_url, "https://api.groq.com/openai/v1");
        assert_eq!(s.transcribe_model, "whisper-large-v3-turbo");
        assert_eq!(s.dictation_model, "whisper-large-v3-turbo");

        // An edited URL stays where the user put it, and so does a model the new preset lists.
        let mut s = Settings {
            transcribe_base_url: "https://stt.example.com/v1".into(),
            transcribe_model: "whisper-large-v3".into(),
            ..Settings::default()
        };
        s.set_transcribe_preset(TranscribePreset::Groq);
        assert_eq!(s.transcribe_base_url, "https://stt.example.com/v1");
        assert_eq!(s.transcribe_model, "whisper-large-v3");
        assert_eq!(s.dictation_model, "whisper-large-v3-turbo");
    }

    #[test]
    fn switching_preset_fills_an_empty_url() {
        let mut s = Settings::default();
        s.transcribe_base_url.clear();
        s.set_transcribe_preset(TranscribePreset::OpenRouter);
        assert_eq!(s.transcribe_base_url, "https://openrouter.ai/api/v1");
        assert_eq!(s.transcribe_model, "google/gemini-3.5-flash");
    }

    #[test]
    fn switching_to_custom_keeps_the_models_it_cannot_list() {
        let mut s = Settings::default();
        s.set_transcribe_preset(TranscribePreset::Custom);
        assert_eq!(s.transcribe_base_url, "");
        assert_eq!(s.transcribe_model, "whisper-1");
        assert_eq!(s.dictation_model, "whisper-1");

        // From custom, the empty URL follows the next preset.
        s.set_transcribe_preset(TranscribePreset::Groq);
        assert_eq!(s.transcribe_base_url, "https://api.groq.com/openai/v1");
    }

    #[test]
    fn choosing_the_same_preset_changes_nothing() {
        let mut s = Settings {
            transcribe_base_url: "https://proxy.example.com/v1".into(),
            transcribe_model: "my-model".into(),
            ..Settings::default()
        };
        let before = s.clone();
        s.set_transcribe_preset(TranscribePreset::OpenAi);
        assert_eq!(s, before);
    }

    #[test]
    fn switching_provider_moves_untouched_url() {
        let mut s = Settings::default();
        s.set_ai_provider(AiProvider::OpenAi);
        assert_eq!(s.ai_provider, AiProvider::OpenAi);
        assert_eq!(s.ai_base_url, "http://localhost:8317/v1");
        assert_eq!(s.ai_model, "claude-haiku-5-5");

        s.set_ai_provider(AiProvider::Anthropic);
        assert_eq!(s.ai_base_url, "https://api.anthropic.com");

        let mut s = Settings {
            ai_base_url: "https://openrouter.ai/api/v1".into(),
            ..Settings::default()
        };
        s.set_ai_provider(AiProvider::OpenAi);
        assert_eq!(s.ai_base_url, "https://openrouter.ai/api/v1");

        let mut s = Settings {
            ai_base_url: String::new(),
            ..Settings::default()
        };
        s.set_ai_provider(AiProvider::OpenAi);
        assert_eq!(s.ai_base_url, "http://localhost:8317/v1");
    }

    /// Settings with OpenRouter as the transcription service, the two base URLs as given.
    fn openrouter_with(ai_base_url: &str, transcribe_base_url: &str) -> Settings {
        Settings {
            transcribe_preset: TranscribePreset::OpenRouter,
            ai_base_url: ai_base_url.into(),
            transcribe_base_url: transcribe_base_url.into(),
            ..Settings::default()
        }
    }

    #[test]
    fn openrouter_key_is_shared() {
        let s = openrouter_with(
            "https://openrouter.ai/api/v1",
            "https://openrouter.ai/api/v1",
        );

        assert_eq!(
            effective_transcribe_key(&s, Some("ai-key"), None),
            Some("ai-key".to_string())
        );
        assert_eq!(
            effective_transcribe_key(&s, Some("ai-key"), Some("")),
            Some("ai-key".to_string())
        );
        // A key of its own wins over the shared one.
        assert_eq!(
            effective_transcribe_key(&s, Some("ai-key"), Some("stt-key")),
            Some("stt-key".to_string())
        );
        assert_eq!(effective_transcribe_key(&s, None, None), None);
        assert_eq!(effective_transcribe_key(&s, Some(""), Some("")), None);
    }

    #[test]
    fn the_ai_key_is_not_shared_outside_openrouter() {
        let s = openrouter_with("https://api.anthropic.com", "https://openrouter.ai/api/v1");
        assert_eq!(effective_transcribe_key(&s, Some("ai-key"), None), None);

        let s = Settings {
            ai_base_url: "https://openrouter.ai/api/v1".into(),
            transcribe_preset: TranscribePreset::Groq,
            ..Settings::default()
        };
        assert_eq!(effective_transcribe_key(&s, Some("ai-key"), None), None);
        assert_eq!(
            effective_transcribe_key(&s, Some("ai-key"), Some("groq-key")),
            Some("groq-key".to_string())
        );
    }

    #[test]
    fn the_ai_key_goes_only_to_openrouter_itself() {
        const OPENROUTER: &str = "https://openrouter.ai/api/v1";
        // Each of these merely mentions or resembles the host, or is not the secure site.
        for impostor in [
            "https://evil.example/v1",
            "https://openrouter.ai.evil.example/api/v1",
            "https://evil-openrouter.ai/api/v1",
            "https://evil.example/openrouter.ai",
            "https://evil.example/v1?via=openrouter.ai",
            "https://openrouter.ai@evil.example/v1",
            "http://openrouter.ai/api/v1",
            "ftp://openrouter.ai/api/v1",
            "openrouter.ai/api/v1",
            "https://openrouter.ai./api/v1",
            "",
        ] {
            let on_transcribe = openrouter_with(OPENROUTER, impostor);
            assert_eq!(
                effective_transcribe_key(&on_transcribe, Some("ai-key"), None),
                None,
                "transcription at {impostor:?} must not get the AI key"
            );
            let on_ai = openrouter_with(impostor, OPENROUTER);
            assert_eq!(
                effective_transcribe_key(&on_ai, Some("ai-key"), None),
                None,
                "an AI key for {impostor:?} is not an OpenRouter key"
            );
        }
    }

    #[test]
    fn only_the_host_decides_whether_the_key_is_shared() {
        // The spelling around the host doesn't matter: case, a port, a path, blanks, a trailing slash.
        for spelling in [
            "https://openrouter.ai",
            "https://openrouter.ai/",
            "https://OpenRouter.AI/api/v1",
            "https://openrouter.ai:443/api/v1/",
            "  https://openrouter.ai/api/v1  ",
        ] {
            let s = openrouter_with(spelling, spelling);
            assert_eq!(
                effective_transcribe_key(&s, Some("ai-key"), None),
                Some("ai-key".to_string()),
                "{spelling:?}"
            );
        }
    }

    #[test]
    fn local_openai_needs_no_key() {
        let mut s = Settings::default();
        s.set_ai_provider(AiProvider::OpenAi);

        s.ai_base_url = "http://localhost:11434/v1".into();
        assert!(is_ai_configured(&s, false));
        s.ai_base_url = "http://127.0.0.1:8317/v1".into();
        assert!(is_ai_configured(&s, false));
        s.ai_base_url = "http://[::1]:11434/v1".into();
        assert!(is_ai_configured(&s, false));

        s.ai_base_url = "https://openrouter.ai/api/v1".into();
        assert!(!is_ai_configured(&s, false));
        assert!(is_ai_configured(&s, true));

        // Only the host counts, not a path or query that happens to mention localhost.
        s.ai_base_url = "https://example.com/localhost".into();
        assert!(!is_ai_configured(&s, false));
        s.ai_base_url = "https://localhost.example.com/v1".into();
        assert!(!is_ai_configured(&s, false));

        s.ai_base_url.clear();
        assert!(!is_ai_configured(&s, true));
    }

    #[test]
    fn anthropic_always_needs_a_key() {
        let mut s = Settings::default();
        assert!(!is_ai_configured(&s, false));
        assert!(is_ai_configured(&s, true));
        s.ai_base_url = "http://localhost:8317".into();
        assert!(!is_ai_configured(&s, false));
    }

    #[test]
    fn round_trip_save_and_open() {
        let dir = tempfile::tempdir().unwrap();
        let (store, warning) = store_in(&dir);
        assert!(warning.is_none());

        let mut s = store.get();
        s.ui_language = UiLanguage::Vi;
        s.dictation_language = "vi-VN".into();
        s.hotkey = "Ctrl+Shift+D".into();
        s.push_to_talk = true;
        s.auto_refine = false;
        s.launch_at_login = true;
        s.set_ai_provider(AiProvider::OpenAi);
        s.vocabulary = "Dictate, Tauri".into();
        s.default_edit_preset = "email".into();
        s.dictation_engine = Engine::Api;
        s.whisper_model = Some("small".into());
        s.set_transcribe_preset(TranscribePreset::Groq);
        s.transcribe_language = "vi".into();
        store.save(&s).unwrap();
        assert_eq!(store.get(), s);

        let (reopened, warning) = store_in(&dir);
        assert!(warning.is_none());
        assert_eq!(reopened.get(), s);
    }

    #[test]
    fn missing_file_gives_defaults_without_warning() {
        let dir = tempfile::tempdir().unwrap();
        let (store, warning) = store_in(&dir);
        assert!(warning.is_none());
        assert_eq!(store.get(), Settings::default());
        assert!(!dir.path().join("settings.json.bak").exists());
    }

    #[test]
    fn save_creates_missing_folders_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("nested")
            .join("Dictate")
            .join("settings.json");
        let (store, _) = SettingsStore::open(path.clone());
        store.save(&Settings::default()).unwrap();
        assert!(path.is_file());
        assert!(!path.with_file_name("settings.json.tmp").exists());
    }

    #[test]
    fn save_replaces_a_stale_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("settings.json.tmp");
        fs::write(&tmp, "half-writ").unwrap();
        let (store, _) = store_in(&dir);
        let mut s = store.get();
        s.vocabulary = "kept".into();
        store.save(&s).unwrap();
        assert!(!tmp.exists());
        let (reopened, warning) = store_in(&dir);
        assert!(warning.is_none());
        assert_eq!(reopened.get().vocabulary, "kept");
    }

    #[test]
    fn corrupt_file_falls_back_and_keeps_backup() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.json"), "{oops").unwrap();

        let (store, warning) = store_in(&dir);
        assert_eq!(store.get(), Settings::default());
        let warning = warning.expect("a corrupt file is reported");
        assert_eq!(warning.backup, Some(dir.path().join("settings.json.bak")));
        assert!(!warning.to_string().is_empty());
        assert_eq!(
            fs::read_to_string(dir.path().join("settings.json.bak")).unwrap(),
            "{oops"
        );

        // The app keeps working: the next save replaces the bad file, the backup stays.
        store.save(&Settings::default()).unwrap();
        let (_, warning) = store_in(&dir);
        assert!(warning.is_none());
        assert_eq!(
            fs::read_to_string(dir.path().join("settings.json.bak")).unwrap(),
            "{oops"
        );
    }

    #[test]
    fn wrong_typed_field_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let original = r#"{"auto_refine":"yes","ai_model":"m1","push_to_talk":true}"#;
        fs::write(dir.path().join("settings.json"), original).unwrap();

        let (store, warning) = store_in(&dir);
        let s = store.get();
        assert_eq!(s.auto_refine, Settings::default().auto_refine);
        assert!(s.auto_refine);
        assert_eq!(s.ai_model, "m1");
        assert!(s.push_to_talk);

        let warning = warning.expect("a dropped field is reported");
        assert!(warning.message.contains("auto_refine"));
        assert!(!warning.message.contains("ai_model"));
        assert_eq!(warning.backup, Some(dir.path().join("settings.json.bak")));
        assert_eq!(
            fs::read_to_string(dir.path().join("settings.json.bak")).unwrap(),
            original
        );
    }

    #[test]
    fn every_unreadable_field_is_dropped_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("settings.json"),
            r#"{"summary_model":null,"push_to_talk":"true","vocabulary":"Dictate",
                "dictation_language":7,"hotkey":"Ctrl+D","whisper_model":null}"#,
        )
        .unwrap();

        let (store, warning) = store_in(&dir);
        let s = store.get();
        assert_eq!(s.summary_model, "");
        assert!(!s.push_to_talk);
        assert_eq!(s.dictation_language, "en-US");
        assert_eq!(s.vocabulary, "Dictate");
        assert_eq!(s.hotkey, "Ctrl+D");
        assert_eq!(s.whisper_model, None);

        let warning = warning.expect("dropped fields are reported");
        for field in ["summary_model", "push_to_talk", "dictation_language"] {
            assert!(warning.message.contains(field), "{field} not named");
        }
        // Fields that were fine, including a null for an optional one, aren't blamed.
        for field in ["vocabulary", "hotkey", "whisper_model"] {
            assert!(!warning.message.contains(field), "{field} wrongly named");
        }
    }

    #[test]
    fn saving_after_a_dropped_field_leaves_a_clean_file_and_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let original = r#"{"auto_refine":"yes","ai_model":"m1"}"#;
        fs::write(dir.path().join("settings.json"), original).unwrap();
        let (store, _) = store_in(&dir);
        store.save(&store.get()).unwrap();

        let (reopened, warning) = store_in(&dir);
        assert!(warning.is_none());
        assert_eq!(reopened.get().ai_model, "m1");
        assert_eq!(
            fs::read_to_string(dir.path().join("settings.json.bak")).unwrap(),
            original
        );
    }

    #[test]
    fn not_a_json_object_counts_as_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.json"), "[1, 2]").unwrap();
        let (store, warning) = store_in(&dir);
        assert!(warning.is_some());
        assert_eq!(store.get(), Settings::default());
    }

    #[test]
    fn utf8_bom_from_a_windows_editor_is_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("settings.json"),
            b"\xEF\xBB\xBF{\"push_to_talk\":true}",
        )
        .unwrap();
        let (store, warning) = store_in(&dir);
        assert!(warning.is_none());
        assert!(store.get().push_to_talk);
    }

    #[test]
    fn unknown_values_use_defaults() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("settings.json"),
            r#"{"ai_provider":"gemini","auto_refine":false,"transcribe_preset":7,
                "dictation_engine":null,"file_engine":"apple","ui_language":"fr",
                "vocabulary":"Dictate"}"#,
        )
        .unwrap();

        let (store, warning) = store_in(&dir);
        assert!(warning.is_none());
        let s = store.get();
        assert_eq!(s.ai_provider, AiProvider::Anthropic);
        assert!(!s.auto_refine);
        assert_eq!(s.transcribe_preset, TranscribePreset::OpenAi);
        assert_eq!(s.dictation_engine, Engine::Whisper);
        assert_eq!(s.file_engine, Engine::Whisper);
        assert_eq!(s.ui_language, UiLanguage::System);
        assert_eq!(s.vocabulary, "Dictate");
        assert_eq!(s.ai_base_url, "https://api.anthropic.com");
    }

    #[test]
    fn missing_fields_use_defaults() {
        let s: Settings = serde_json::from_str(r#"{"push_to_talk":true}"#).unwrap();
        assert!(s.push_to_talk);
        assert_eq!(
            Settings {
                push_to_talk: false,
                ..s
            },
            Settings::default()
        );
    }

    #[test]
    fn settings_file_never_contains_keys() {
        let json = serde_json::to_value(Settings::default()).unwrap();
        let fields = json.as_object().unwrap();
        assert_eq!(fields.len(), 23);
        for name in fields.keys() {
            let n = name.to_lowercase();
            assert!(
                n != "key"
                    && !n.ends_with("_key")
                    && !n.contains("apikey")
                    && !n.contains("secret")
                    && !n.contains("token")
                    && !n.contains("password"),
                "{name} looks like a credential"
            );
        }
    }

    #[test]
    fn saving_drops_unknown_fields_such_as_a_pasted_key() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("settings.json"),
            r#"{"api_key":"sk-secret-123","auto_write":false}"#,
        )
        .unwrap();
        let (store, warning) = store_in(&dir);
        assert!(warning.is_none());
        assert!(!store.get().auto_write);
        store.save(&store.get()).unwrap();
        let saved = fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert!(!saved.contains("sk-secret-123"));
        assert!(!saved.contains("api_key"));
    }

    #[test]
    fn default_path_is_under_the_config_dir() {
        let path = SettingsStore::default_path();
        assert!(path.ends_with("Dictate/settings.json"));
        if let Some(config) = dirs::config_dir() {
            assert!(path.starts_with(config));
        }
    }
}
