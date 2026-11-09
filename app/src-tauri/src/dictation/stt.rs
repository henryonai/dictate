//! Local speech recognition for the dictation: the Whisper model that the settings choose, loaded once and kept.

use std::sync::Arc;

use dictate_services::settings::SettingsStore;
use dictate_services::whisper::local::LocalWhisper;
use dictate_services::whisper::models::ModelDir;
use tokio_util::sync::CancellationToken;

use crate::config::local_model;
use crate::dictation::controller::LocalStt;
use crate::dictation::join_segments;
use crate::state::WhisperCache;

/// The model that the settings choose, or else the one recommended for this machine; it is loaded the first time it is
/// needed and stays in memory, until another one is chosen.
pub struct WhisperStt {
    settings: Arc<SettingsStore>,
    models: ModelDir,
    cache: Arc<WhisperCache>,
    ram_gb: u64,
}

impl WhisperStt {
    /// `ram_gb` is the memory of the machine, which the recommended model depends on.
    pub fn new(
        settings: Arc<SettingsStore>,
        models: ModelDir,
        cache: Arc<WhisperCache>,
        ram_gb: u64,
    ) -> Self {
        Self {
            settings,
            models,
            cache,
            ram_gb,
        }
    }
}

impl LocalStt for WhisperStt {
    fn transcribe(
        &self,
        samples_16k: &[f32],
        language: &str,
        prompt: &str,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        let (id, path) = local_model(&self.settings.get(), &self.models, self.ram_gb)?;
        let model = self
            .cache
            .get(&id, || LocalWhisper::load(&path).map_err(|e| e.to_string()))?;
        let language = Some(language).filter(|language| !language.trim().is_empty());
        let segments = model
            .transcribe(samples_16k, language, prompt, cancel, |_| {})
            .map_err(|e| e.to_string())?;
        Ok(join_segments(&segments, language.unwrap_or_default()))
    }

    fn available(&self) -> bool {
        local_model(&self.settings.get(), &self.models, self.ram_gb).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dictate_services::whisper::models::ModelDir;

    struct Fixture {
        _dir: tempfile::TempDir,
        models: ModelDir,
        settings: Arc<SettingsStore>,
        stt: WhisperStt,
        cache: Arc<WhisperCache>,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let (settings, _) = SettingsStore::open(dir.path().join("settings.json"));
        let settings = Arc::new(settings);
        let models = ModelDir::new(dir.path().join("models"));
        std::fs::create_dir_all(models.root()).unwrap();
        let cache = Arc::new(WhisperCache::default());
        let stt = WhisperStt::new(settings.clone(), models.clone(), cache.clone(), 16);
        Fixture {
            _dir: dir,
            models,
            settings,
            stt,
            cache,
        }
    }

    #[test]
    fn there_is_a_model_when_its_file_is_downloaded_and_not_before() {
        let f = fixture();
        assert!(!f.stt.available());

        // 16 GB: the recommended model is the one that is there.
        let id = dictate_services::whisper::models::recommended(16);
        std::fs::write(f.models.path(id), b"not a real model").unwrap();
        assert!(
            f.stt.available(),
            "a model that is there is available, whether it loads or not"
        );

        // The chosen model wins over the recommended one.
        f.settings
            .update(|s| s.whisper_model = Some("medium".into()))
            .unwrap();
        assert!(!f.stt.available());
    }

    #[test]
    fn a_missing_model_is_an_error_that_says_which() {
        let f = fixture();
        let error = f
            .stt
            .transcribe(&[0.0; 16_000], "en", "", &CancellationToken::new())
            .unwrap_err();
        assert!(error.contains("isn't downloaded"), "{error}");
    }

    #[test]
    fn a_damaged_model_is_an_error_and_is_asked_for_again_next_time() {
        let f = fixture();
        let id = dictate_services::whisper::models::recommended(16);
        std::fs::write(f.models.path(id), b"not a real model").unwrap();

        let error = f
            .stt
            .transcribe(&[0.0; 16_000], "en", "", &CancellationToken::new())
            .unwrap_err();
        assert!(error.contains("model"), "{error}");
        // Nothing was kept in memory for it.
        assert!(f.cache.get(id, || Err("loaded again".into())).unwrap_err() == "loaded again");
    }
}
