//! What the services are given: the settings and the saved keys, put together.
//!
//! Reading a key here asks the OS key store, which can be slow or even show a prompt. The settings view
//! doesn't: it uses the flags in `state::KeyFlags`. Only what needs a key's value comes here.

use std::path::PathBuf;

use dictate_services::ai::AiConfig;
use dictate_services::presets::AiProvider;
use dictate_services::secrets::{Account, SecretStore};
use dictate_services::settings::{self, Settings, SettingsStore};
use dictate_services::transcribe_api::TranscribeConfig;
use dictate_services::whisper::models::{self, ModelDir};

/// How many characters of the vocabulary go to the transcription API as its prompt.
const PROMPT_LIMIT: usize = 800;

/// The key saved for an account. A key store that fails counts as no key: the settings stay usable and saving a
/// key says what is wrong.
pub fn saved_key(secrets: &dyn SecretStore, account: Account) -> Option<String> {
    match secrets.get(account) {
        Ok(key) => key.filter(|key| !key.is_empty()),
        Err(e) => {
            log::warn!("Could not read the {} key: {e}", account.name());
            None
        }
    }
}

/// The AI provider as configured, with its key.
pub fn ai_config(store: &SettingsStore, secrets: &dyn SecretStore) -> Result<AiConfig, String> {
    let settings = store.get();
    let key = saved_key(secrets, Account::Ai);
    if !settings::is_ai_configured(&settings, key.is_some()) {
        return Err(
            if settings.ai_provider == AiProvider::OpenAi && key.is_some() {
                "Enter the base URL of the AI provider first.".to_string()
            } else {
                "Add the API key of the AI provider first.".to_string()
            },
        );
    }
    Ok(AiConfig {
        provider: settings.ai_provider,
        base_url: settings.ai_base_url,
        model: settings.ai_model,
        api_key: key,
        vocabulary: settings.vocabulary,
    })
}

/// The transcription service as configured, with the key it uses.
pub fn transcribe_config(store: &SettingsStore, secrets: &dyn SecretStore) -> TranscribeConfig {
    let settings = store.get();
    let ai_key = saved_key(secrets, Account::Ai);
    let own_key = saved_key(secrets, Account::Transcribe);
    let key = settings::effective_transcribe_key(&settings, ai_key.as_deref(), own_key.as_deref());
    let language = settings.transcribe_language.trim();
    TranscribeConfig {
        base_url: settings.transcribe_base_url,
        model: settings.transcribe_model,
        api_key: key.unwrap_or_default(),
        language: (!language.is_empty()).then(|| language.to_string()),
        prompt: settings.vocabulary,
        chat: settings.transcribe_preset.uses_chat(),
    }
}

/// The transcription service as dictation uses it: the dictation model (the one for files when none is set), the
/// dictation language and the start of the vocabulary as the prompt. `None` when there is no URL, no model or no
/// key to send: then dictation stays on the local model.
// Nothing calls it before the dictation controller lands; remove the attribute then.
#[allow(dead_code)]
pub fn dictation_transcribe_config(
    s: &Settings,
    secrets: &dyn SecretStore,
) -> Option<TranscribeConfig> {
    let model = if s.dictation_model.trim().is_empty() {
        &s.transcribe_model
    } else {
        &s.dictation_model
    };
    if model.trim().is_empty() || s.transcribe_base_url.trim().is_empty() {
        return None;
    }
    let ai_key = saved_key(secrets, Account::Ai);
    let own_key = saved_key(secrets, Account::Transcribe);
    let api_key = settings::effective_transcribe_key(s, ai_key.as_deref(), own_key.as_deref())?;
    let language = whisper_language(&s.dictation_language);
    Some(TranscribeConfig {
        base_url: s.transcribe_base_url.clone(),
        model: model.clone(),
        api_key,
        language: (!language.is_empty()).then_some(language),
        prompt: s.vocabulary.chars().take(PROMPT_LIMIT).collect(),
        chat: s.transcribe_preset.uses_chat(),
    })
}

/// The language code Whisper and the transcription APIs take, from a BCP-47 locale: `vi-VN` is `vi`.
// Nothing calls it before the dictation controller lands; remove the attribute then.
#[allow(dead_code)]
pub fn whisper_language(bcp47: &str) -> String {
    let language = bcp47.split('-').next().unwrap_or_default();
    language.to_lowercase()
}

/// The model local transcription uses: the chosen one, else the one recommended for this much RAM. It must be
/// downloaded.
pub fn local_model(
    settings: &Settings,
    models: &ModelDir,
    ram_gb: u64,
) -> Result<(String, PathBuf), String> {
    let id = settings
        .whisper_model
        .clone()
        .unwrap_or_else(|| models::recommended(ram_gb).to_string());
    if !models.is_downloaded(&id) {
        return Err(format!(
            "The {id} model isn't downloaded yet. Download it in Settings first."
        ));
    }
    let path = models.path(&id);
    Ok((id, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dictate_services::presets::TranscribePreset;
    use dictate_services::secrets::MemoryStore;

    const AI_KEY: &str = "sk-ai-secret-123";
    const STT_KEY: &str = "sk-stt-secret-456";

    struct Fixture {
        _dir: tempfile::TempDir,
        store: SettingsStore,
        secrets: MemoryStore,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let (store, warning) = SettingsStore::open(dir.path().join("settings.json"));
        assert!(warning.is_none());
        Fixture {
            _dir: dir,
            store,
            secrets: MemoryStore::default(),
        }
    }

    // ---- the AI check

    #[test]
    fn the_ai_check_needs_a_key_for_anthropic() {
        let f = fixture();
        let err = ai_config(&f.store, &f.secrets).unwrap_err();
        assert!(err.to_lowercase().contains("key"), "{err}");
    }

    #[test]
    fn the_ai_config_carries_the_key_and_the_vocabulary() {
        let f = fixture();
        f.secrets.set(Account::Ai, AI_KEY).unwrap();
        let mut s = f.store.get();
        s.vocabulary = "Dictate".into();
        s.ai_model = "my-model".into();
        f.store.save(&s).unwrap();

        let config = ai_config(&f.store, &f.secrets).unwrap();
        assert_eq!(config.api_key.as_deref(), Some(AI_KEY));
        assert_eq!(config.vocabulary, "Dictate");
        assert_eq!(config.model, "my-model");
        assert_eq!(config.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn a_local_ai_server_needs_no_key() {
        let f = fixture();
        let mut s = f.store.get();
        s.set_ai_provider(AiProvider::OpenAi);
        f.store.save(&s).unwrap();
        let config = ai_config(&f.store, &f.secrets).unwrap();
        assert_eq!(config.api_key, None);
    }

    // ---- the transcription check

    #[test]
    fn the_transcription_config_uses_its_own_key_first() {
        let f = fixture();
        f.secrets.set(Account::Ai, AI_KEY).unwrap();
        f.secrets.set(Account::Transcribe, STT_KEY).unwrap();
        let config = transcribe_config(&f.store, &f.secrets);
        assert_eq!(config.api_key, STT_KEY);
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.model, "whisper-1");
        assert!(!config.chat);
    }

    #[test]
    fn the_transcription_config_shares_the_openrouter_key() {
        let f = fixture();
        f.secrets.set(Account::Ai, AI_KEY).unwrap();
        let mut s = f.store.get();
        s.ai_base_url = "https://openrouter.ai/api/v1".into();
        s.set_transcribe_preset(TranscribePreset::OpenRouter);
        s.transcribe_language = "vi".into();
        s.vocabulary = "Dictate".into();
        f.store.save(&s).unwrap();

        let config = transcribe_config(&f.store, &f.secrets);
        assert_eq!(config.api_key, AI_KEY);
        assert!(config.chat);
        assert_eq!(config.language.as_deref(), Some("vi"));
        assert_eq!(config.prompt, "Dictate");
    }

    #[test]
    fn the_transcription_config_keeps_the_ai_key_from_another_host() {
        let f = fixture();
        f.secrets.set(Account::Ai, AI_KEY).unwrap();
        let mut s = f.store.get();
        s.ai_base_url = "https://openrouter.ai/api/v1".into();
        s.set_transcribe_preset(TranscribePreset::OpenRouter);
        s.transcribe_base_url = "https://stt.example.com/v1".into();
        f.store.save(&s).unwrap();

        let config = transcribe_config(&f.store, &f.secrets);
        assert_eq!(config.base_url, "https://stt.example.com/v1");
        assert_eq!(
            config.api_key, "",
            "the key of OpenRouter stays with OpenRouter"
        );
    }

    #[test]
    fn no_key_means_an_empty_key_and_no_language() {
        let f = fixture();
        let config = transcribe_config(&f.store, &f.secrets);
        assert_eq!(config.api_key, "");
        assert_eq!(config.language, None);
    }

    // ---- the transcription API for dictation

    /// Settings with a key for the transcription service, as a user who turned the API on has.
    fn with_stt_key() -> (Settings, MemoryStore) {
        let secrets = MemoryStore::default();
        secrets.set(Account::Transcribe, STT_KEY).unwrap();
        (Settings::default(), secrets)
    }

    #[test]
    fn dictation_config_uses_dictation_model_and_language() {
        let (mut s, secrets) = with_stt_key();
        s.set_transcribe_preset(TranscribePreset::Groq);
        s.dictation_model = "whisper-large-v3-turbo".into();
        s.transcribe_model = "whisper-large-v3".into();
        s.dictation_language = "vi-VN".into();
        s.transcribe_language = "en".into();

        let config = dictation_transcribe_config(&s, &secrets).unwrap();

        assert_eq!(config.model, "whisper-large-v3-turbo");
        assert_eq!(
            config.language.as_deref(),
            Some("vi"),
            "the dictation language, not the one for files"
        );
        assert_eq!(config.base_url, "https://api.groq.com/openai/v1");
        assert_eq!(config.api_key, STT_KEY);
        assert!(!config.chat);
    }

    #[test]
    fn dictation_config_falls_back_to_the_transcribe_model_when_blank() {
        let (mut s, secrets) = with_stt_key();
        s.dictation_model = "  ".into();
        s.transcribe_model = "whisper-1".into();

        let config = dictation_transcribe_config(&s, &secrets).unwrap();

        assert_eq!(config.model, "whisper-1");
    }

    #[test]
    fn dictation_config_none_without_key() {
        let (s, _) = with_stt_key();
        assert!(dictation_transcribe_config(&s, &MemoryStore::default()).is_none());

        // An empty key counts as none.
        let secrets = MemoryStore::default();
        secrets.set(Account::Transcribe, "").unwrap();
        assert!(dictation_transcribe_config(&s, &secrets).is_none());
    }

    #[test]
    fn dictation_config_none_without_a_model_or_a_url() {
        let (mut s, secrets) = with_stt_key();
        s.dictation_model.clear();
        s.transcribe_model.clear();
        assert!(dictation_transcribe_config(&s, &secrets).is_none());

        let (mut s, secrets) = with_stt_key();
        s.transcribe_base_url = " ".into();
        assert!(dictation_transcribe_config(&s, &secrets).is_none());
    }

    #[test]
    fn dictation_config_shares_the_openrouter_key_and_sends_chat() {
        let secrets = MemoryStore::default();
        secrets.set(Account::Ai, AI_KEY).unwrap();
        let mut s = Settings {
            ai_base_url: "https://openrouter.ai/api/v1".into(),
            ..Settings::default()
        };
        s.set_transcribe_preset(TranscribePreset::OpenRouter);

        let config = dictation_transcribe_config(&s, &secrets).unwrap();

        assert_eq!(config.api_key, AI_KEY);
        assert!(config.chat);
    }

    #[test]
    fn dictation_config_keeps_the_ai_key_from_another_host() {
        let secrets = MemoryStore::default();
        secrets.set(Account::Ai, AI_KEY).unwrap();
        let mut s = Settings {
            ai_base_url: "https://openrouter.ai/api/v1".into(),
            ..Settings::default()
        };
        s.set_transcribe_preset(TranscribePreset::OpenRouter);
        s.transcribe_base_url = "https://stt.example.com/v1".into();

        assert!(dictation_transcribe_config(&s, &secrets).is_none());
    }

    #[test]
    fn dictation_config_has_no_language_when_none_is_set() {
        let (mut s, secrets) = with_stt_key();
        s.dictation_language.clear();

        let config = dictation_transcribe_config(&s, &secrets).unwrap();

        assert_eq!(config.language, None);
    }

    #[test]
    fn dictation_config_cuts_the_vocabulary_to_800_characters() {
        let (mut s, secrets) = with_stt_key();
        // Characters, not bytes: each of these is three bytes long.
        s.vocabulary = "ế".repeat(900);

        let config = dictation_transcribe_config(&s, &secrets).unwrap();

        assert_eq!(config.prompt, "ế".repeat(800));

        s.vocabulary = "Dictate, Whisper".into();
        let config = dictation_transcribe_config(&s, &secrets).unwrap();
        assert_eq!(config.prompt, "Dictate, Whisper");
    }

    #[test]
    fn the_language_is_the_part_of_the_locale_before_the_dash_in_lower_case() {
        assert_eq!(whisper_language("vi-VN"), "vi");
        assert_eq!(whisper_language("EN"), "en");
        assert_eq!(whisper_language("en-US"), "en");
        assert_eq!(whisper_language("zh-CN"), "zh");
        assert_eq!(whisper_language(""), "");
    }

    // ---- the local model

    fn put_model(models: &ModelDir, id: &str) {
        std::fs::create_dir_all(models.root()).unwrap();
        std::fs::write(models.path(id), b"not a real model").unwrap();
    }

    #[test]
    fn the_chosen_model_is_used_when_it_is_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let models = ModelDir::new(dir.path().join("models"));
        put_model(&models, "tiny");
        let settings = Settings {
            whisper_model: Some("tiny".into()),
            ..Settings::default()
        };
        let (id, path) = local_model(&settings, &models, 32).unwrap();
        assert_eq!(id, "tiny");
        assert_eq!(path, models.path("tiny"));
    }

    #[test]
    fn the_recommended_model_is_used_when_none_is_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let models = ModelDir::new(dir.path().join("models"));
        put_model(&models, "small");
        let (id, _) = local_model(&Settings::default(), &models, 8).unwrap();
        assert_eq!(id, "small");
    }

    #[test]
    fn a_model_that_is_not_downloaded_is_named_in_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let models = ModelDir::new(dir.path().join("models"));
        let settings = Settings {
            whisper_model: Some("medium".into()),
            ..Settings::default()
        };
        let err = local_model(&settings, &models, 32).unwrap_err();
        assert_eq!(
            err,
            "The medium model isn't downloaded yet. Download it in Settings first."
        );
        // The recommended one for 4 GB is `base`.
        let err = local_model(&Settings::default(), &models, 4).unwrap_err();
        assert!(err.contains("base"), "{err}");
    }
}
