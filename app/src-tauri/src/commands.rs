//! The commands the UI calls. Each one is a thin `async` wrapper over a plain function that is tested here.
//!
//! Errors are `String`s meant for the person using the app. Keys never leave this side: the UI learns whether
//! one is saved, never what it is.
//!
//! Anything that can block (the OS key store, the disk, Whisper) runs on a blocking thread, never on the async
//! runtime's own.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dictate_core::meetings::library::MeetingLibrary;
use dictate_core::text::segments::Segment;
use dictate_core::text::timestamp;
use dictate_services::ai::{self, AiConfig};
use dictate_services::http::ApiError;
use dictate_services::presets::{AiProvider, TranscribePreset, EDIT_PRESETS};
use dictate_services::secrets::{Account, SecretStore};
use dictate_services::settings::{self, Settings, SettingsStore};
use dictate_services::system;
use dictate_services::transcribe_api::{self, TranscribeConfig, TranscribeError};
use dictate_services::whisper::local::{read_wav_16k_mono, LocalWhisper};
use dictate_services::whisper::models::{
    self, DownloadError, ModelDir, ModelStatus, HUGGING_FACE_BASE_URL,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_opener::OpenerExt as _;
use tokio_util::sync::CancellationToken;

use crate::state::{lock, AppState, Job};
use crate::tray;

/// The event that tells how a model download goes.
const DOWNLOAD_EVENT: &str = "model-download";

/// The settings plus what the UI may know about the keys.
///
/// The settings fields sit at the top level of the JSON, next to the extra fields.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    #[serde(flatten)]
    pub settings: Settings,
    pub has_ai_key: bool,
    pub has_transcribe_key: bool,
    /// No transcription key of its own, but the AI key is used for it (OpenRouter for both).
    pub shares_openrouter_key: bool,
    /// Why the settings file wasn't used as it was. Only the first view after the start carries it.
    pub load_warning: Option<String>,
}

/// What `model-download` events tell about a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    Running,
    Done,
    Failed,
    Cancelled,
}

/// The payload of the `model-download` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownloadEvent {
    pub id: String,
    pub done: u64,
    pub total: Option<u64>,
    pub state: DownloadState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One Whisper model of the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelView {
    pub id: &'static str,
    pub size_mb: u32,
    pub downloaded: bool,
    pub recommended: bool,
}

impl From<ModelStatus> for ModelView {
    fn from(status: ModelStatus) -> Self {
        ModelView {
            id: status.id,
            size_mb: status.size_mb,
            downloaded: status.downloaded,
            recommended: status.recommended,
        }
    }
}

/// One style of rewriting the transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditPresetView {
    pub id: &'static str,
    pub instruction: &'static str,
}

/// One transcription service, with what the UI shows beyond the settings: its models and the page that hands
/// out its keys. The names of the services are the UI's to translate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TranscribePresetView {
    pub id: TranscribePreset,
    pub models: &'static [&'static str],
    pub key_url: Option<&'static str>,
}

// ---- the plain functions behind the commands

/// The key saved for an account. A key store that fails counts as no key: the settings stay usable and saving a
/// key says what is wrong.
fn saved_key(secrets: &dyn SecretStore, account: Account) -> Option<String> {
    match secrets.get(account) {
        Ok(key) => key.filter(|key| !key.is_empty()),
        Err(e) => {
            log::warn!("Could not read the {} key: {e}", account.name());
            None
        }
    }
}

fn get_settings_view(store: &SettingsStore, secrets: &dyn SecretStore) -> SettingsView {
    let settings = store.get();
    let ai_key = saved_key(secrets, Account::Ai);
    let own_key = saved_key(secrets, Account::Transcribe);
    let used_key =
        settings::effective_transcribe_key(&settings, ai_key.as_deref(), own_key.as_deref());
    SettingsView {
        has_ai_key: ai_key.is_some(),
        has_transcribe_key: own_key.is_some(),
        shares_openrouter_key: own_key.is_none() && used_key.is_some(),
        load_warning: None,
        settings,
    }
}

fn parse_account(name: &str) -> Result<Account, String> {
    match name {
        "ai" => Ok(Account::Ai),
        "transcribe" => Ok(Account::Transcribe),
        other => Err(format!("Unknown account: {other}")),
    }
}

/// Saves a key; an empty one (after trimming what was pasted around it) deletes it.
fn set_secret_inner(
    store: &SettingsStore,
    secrets: &dyn SecretStore,
    account: &str,
    value: &str,
) -> Result<SettingsView, String> {
    let account = parse_account(account)?;
    let value = value.trim();
    // Saving an empty string would keep an entry that holds nothing.
    let stored = if value.is_empty() {
        secrets.delete(account)
    } else {
        secrets.set(account, value)
    };
    stored.map_err(|e| e.to_string())?;
    Ok(get_settings_view(store, secrets))
}

fn save(store: &SettingsStore, settings: &Settings) -> Result<(), String> {
    store
        .save(settings)
        .map_err(|e| format!("Could not save the settings: {e}"))
}

fn set_ai_provider_inner(
    store: &SettingsStore,
    secrets: &dyn SecretStore,
    provider: AiProvider,
) -> Result<SettingsView, String> {
    let mut settings = store.get();
    settings.set_ai_provider(provider);
    save(store, &settings)?;
    Ok(get_settings_view(store, secrets))
}

fn set_transcribe_preset_inner(
    store: &SettingsStore,
    secrets: &dyn SecretStore,
    preset: TranscribePreset,
) -> Result<SettingsView, String> {
    let mut settings = store.get();
    settings.set_transcribe_preset(preset);
    save(store, &settings)?;
    Ok(get_settings_view(store, secrets))
}

/// Saves `settings`. `apply_launch_at_login` is called, with the new value, only when that setting changes; if it
/// fails nothing is saved, so the file never says something the system doesn't do.
fn save_settings_inner(
    store: &SettingsStore,
    secrets: &dyn SecretStore,
    settings: Settings,
    mut apply_launch_at_login: impl FnMut(bool) -> Result<(), String>,
) -> Result<SettingsView, String> {
    let before = store.get().launch_at_login;
    let changed = settings.launch_at_login != before;
    if changed {
        apply_launch_at_login(settings.launch_at_login)?;
    }
    if let Err(e) = save(store, &settings) {
        if changed {
            // Put the system back to what the file still says.
            let _ = apply_launch_at_login(before);
        }
        return Err(e);
    }
    Ok(get_settings_view(store, secrets))
}

/// The AI provider as configured, with its key.
fn ai_config(store: &SettingsStore, secrets: &dyn SecretStore) -> Result<AiConfig, String> {
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
fn transcribe_config(store: &SettingsStore, secrets: &dyn SecretStore) -> TranscribeConfig {
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

/// An API error as shown to the user. An error status with no message gets the HTTP reason phrase, so that a
/// reply with an empty body doesn't end in "API error 301: ".
fn api_error_text(error: &ApiError) -> String {
    match error {
        ApiError::Status { status, message } if message.trim().is_empty() => {
            match reqwest::StatusCode::from_u16(*status)
                .ok()
                .and_then(|code| code.canonical_reason())
            {
                Some(reason) => format!("API error {status}: {reason}"),
                None => format!("API error {status}"),
            }
        }
        other => other.to_string(),
    }
}

fn transcribe_error_text(error: &TranscribeError) -> String {
    match error {
        TranscribeError::Api(e) => api_error_text(e),
        other => other.to_string(),
    }
}

fn download_error_text(error: &DownloadError) -> String {
    match error {
        DownloadError::Http(e) => api_error_text(e),
        other => other.to_string(),
    }
}

/// `[mm:ss] text` lines.
fn format_transcript(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|segment| format!("[{}] {}", timestamp::format(segment.start), segment.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The model local transcription uses: the chosen one, else the one recommended for this much RAM. It must be
/// downloaded.
fn local_model(
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

/// The last event of a download, from how it ended and how far it had come.
fn final_event(
    id: &str,
    outcome: &Result<(), DownloadError>,
    progress: (u64, Option<u64>),
) -> DownloadEvent {
    let (state, error) = match outcome {
        Ok(()) => (DownloadState::Done, None),
        Err(DownloadError::Cancelled) => (DownloadState::Cancelled, None),
        Err(e) => (DownloadState::Failed, Some(download_error_text(e))),
    };
    DownloadEvent {
        id: id.to_string(),
        done: progress.0,
        total: progress.1,
        state,
        error,
    }
}

/// Downloads a model, telling `emit` how it goes. The download is over for good only once the event `Done`
/// has been sent; a cancelled download is not an error.
async fn run_download(
    state: &AppState,
    id: &str,
    base_url: &str,
    emit: impl Fn(DownloadEvent) + Send + Sync,
) -> Result<(), String> {
    if models::find(id).is_none() {
        return Err(format!("There is no model called {id}."));
    }
    let guard = state
        .jobs
        .start(Job::Download(id.to_string()))
        .map_err(|_| "Already downloading".to_string())?;

    let progress = Mutex::new((0_u64, None::<u64>));
    let outcome = models::download(
        &state.models,
        id,
        base_url,
        |done, total| {
            *lock(&progress) = (done, total);
            emit(DownloadEvent {
                id: id.to_string(),
                done,
                total,
                state: DownloadState::Running,
                error: None,
            });
        },
        guard.token(),
    )
    .await
    .map(|_| ());

    // Free the model before the last event: whoever hears it may start the next download at once.
    drop(guard);
    let reached = *lock(&progress);
    emit(final_event(id, &outcome, reached));
    match outcome {
        Ok(()) | Err(DownloadError::Cancelled) => Ok(()),
        Err(e) => Err(download_error_text(&e)),
    }
}

/// Transcribes a WAV file with the local model the settings choose. Blocks until it is done or cancelled.
fn transcribe_file(
    settings: &Settings,
    models: &ModelDir,
    ram_gb: u64,
    wav: &Path,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let (id, model_path) = local_model(settings, models, ram_gb)?;
    let samples = read_wav_16k_mono(wav).map_err(|e| e.to_string())?;
    let whisper = LocalWhisper::load(&model_path).map_err(|e| e.to_string())?;
    let language = settings.transcribe_language.trim();
    let segments = whisper
        .transcribe(
            &samples,
            (!language.is_empty()).then_some(language),
            &settings.vocabulary,
            cancel,
            |_| {},
        )
        .map_err(|e| e.to_string())?;
    log::info!(
        "Transcribed a WAV file with the {id} model: {} segments",
        segments.len()
    );
    Ok(format_transcript(&segments))
}

/// Removes a model, unless it is being downloaded.
fn delete_model_inner(state: &AppState, id: &str) -> Result<(), String> {
    if state.jobs.is_active(&Job::Download(id.to_string())) {
        return Err("The model is being downloaded. Cancel the download first.".to_string());
    }
    state
        .models
        .delete(id)
        .map_err(|e| format!("Could not delete the model: {e}"))
}

fn edit_presets_list() -> Vec<EditPresetView> {
    EDIT_PRESETS
        .iter()
        .map(|preset| EditPresetView {
            id: preset.id,
            instruction: preset.instruction,
        })
        .collect()
}

/// Every transcription service, in the order the UI offers them.
fn transcribe_presets_list() -> Vec<TranscribePresetView> {
    [
        TranscribePreset::OpenRouter,
        TranscribePreset::OpenAi,
        TranscribePreset::Groq,
        TranscribePreset::Custom,
    ]
    .into_iter()
    .map(|preset| TranscribePresetView {
        id: preset,
        models: preset.models(),
        key_url: preset.key_url(),
    })
    .collect()
}

/// Makes sure the folder is there.
fn ensure_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("Could not create {}: {e}", path.display()))
}

/// Runs `work` on a blocking thread.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("The work stopped unexpectedly: {e}"))
}

/// Turns launch at login on or off in the system, when it isn't already as asked.
fn set_launch_at_login(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    let current = manager
        .is_enabled()
        .map_err(|e| format!("Could not read the launch at login setting: {e}"))?;
    if current == enabled {
        return Ok(());
    }
    let changed = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    changed.map_err(|e| format!("Could not change launch at login: {e}"))
}

/// Opens the meetings folder in the file manager, creating it first if needed.
pub fn open_meetings_folder_now(app: &AppHandle) -> Result<(), String> {
    let folder = MeetingLibrary::default_directory();
    ensure_directory(&folder)?;
    app.opener()
        .open_path(folder.to_string_lossy().into_owned(), None::<String>)
        .map_err(|e| format!("Could not open {}: {e}", folder.display()))
}

// ---- the commands

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<SettingsView, String> {
    let state = state.inner().clone();
    blocking(move || {
        let mut view = get_settings_view(&state.settings, &*state.secrets);
        view.load_warning = state.take_load_warning();
        view
    })
    .await
}

#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<SettingsView, String> {
    let state = state.inner().clone();
    let handle = app.clone();
    let view = blocking(move || {
        save_settings_inner(&state.settings, &*state.secrets, settings, |enabled| {
            set_launch_at_login(&handle, enabled)
        })
    })
    .await??;
    tray::refresh(&app, &view.settings);
    Ok(view)
}

#[tauri::command]
pub async fn set_ai_provider(
    state: State<'_, AppState>,
    p: AiProvider,
) -> Result<SettingsView, String> {
    let state = state.inner().clone();
    blocking(move || set_ai_provider_inner(&state.settings, &*state.secrets, p)).await?
}

#[tauri::command]
pub async fn set_transcribe_preset(
    state: State<'_, AppState>,
    p: TranscribePreset,
) -> Result<SettingsView, String> {
    let state = state.inner().clone();
    blocking(move || set_transcribe_preset_inner(&state.settings, &*state.secrets, p)).await?
}

/// `account` is `"ai"` or `"transcribe"`; an empty `value` deletes the key.
#[tauri::command]
pub async fn set_secret(
    state: State<'_, AppState>,
    account: String,
    value: String,
) -> Result<SettingsView, String> {
    let state = state.inner().clone();
    blocking(move || set_secret_inner(&state.settings, &*state.secrets, &account, &value)).await?
}

#[tauri::command]
pub async fn check_ai(state: State<'_, AppState>) -> Result<String, String> {
    let state = state.inner().clone();
    let config = blocking(move || ai_config(&state.settings, &*state.secrets)).await??;
    ai::check(&config).await.map_err(|e| api_error_text(&e))?;
    Ok(format!("Connected: {} answered.", config.model))
}

#[tauri::command]
pub async fn check_transcribe(state: State<'_, AppState>) -> Result<String, String> {
    let state = state.inner().clone();
    let config = blocking(move || transcribe_config(&state.settings, &*state.secrets)).await?;
    transcribe_api::check(&config)
        .await
        .map_err(|e| transcribe_error_text(&e))?;
    Ok(format!(
        "Connected: {} accepted the test audio.",
        config.model
    ))
}

#[tauri::command]
pub async fn list_models(state: State<'_, AppState>) -> Result<Vec<ModelView>, String> {
    let state = state.inner().clone();
    blocking(move || {
        state
            .models
            .list()
            .into_iter()
            .map(ModelView::from)
            .collect()
    })
    .await
}

/// Resolves when the download is over; `model-download` events say how it went on the way.
#[tauri::command]
pub async fn download_model(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let state = state.inner().clone();
    run_download(&state, &id, HUGGING_FACE_BASE_URL, move |event| {
        if let Err(e) = app.emit(DOWNLOAD_EVENT, event) {
            log::warn!("Could not send a download event: {e}");
        }
    })
    .await
}

#[tauri::command]
pub async fn cancel_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.jobs.cancel(&Job::Download(id));
    Ok(())
}

#[tauri::command]
pub async fn delete_model(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let state = state.inner().clone();
    blocking(move || delete_model_inner(&state, &id)).await?
}

#[tauri::command]
pub async fn models_disk_usage_mb(state: State<'_, AppState>) -> Result<u64, String> {
    let state = state.inner().clone();
    blocking(move || state.models.disk_usage_mb()).await
}

/// Dev helper: the chosen local model transcribes a WAV file; the result is `[mm:ss] text` lines. The meetings
/// screens replace it later.
#[tauri::command]
pub async fn transcribe_wav(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let state = state.inner().clone();
    let guard = state
        .jobs
        .start(Job::Transcribe)
        .map_err(|_| "Already transcribing".to_string())?;
    blocking(move || {
        // The guard lives as long as the work does.
        let guard = guard;
        transcribe_file(
            &state.settings.get(),
            &state.models,
            system::total_ram_gb(),
            Path::new(&path),
            &guard.token(),
        )
    })
    .await?
}

/// Stops a running `transcribe_wav`. It ends within about one encoder pass, so the UI should say "Cancelling…".
#[tauri::command]
pub async fn cancel_transcribe(state: State<'_, AppState>) -> Result<(), String> {
    state.jobs.cancel(&Job::Transcribe);
    Ok(())
}

#[tauri::command]
pub async fn edit_presets() -> Result<Vec<EditPresetView>, String> {
    Ok(edit_presets_list())
}

#[tauri::command]
pub async fn transcribe_presets() -> Result<Vec<TranscribePresetView>, String> {
    Ok(transcribe_presets_list())
}

#[tauri::command]
pub async fn open_meetings_folder(app: AppHandle) -> Result<(), String> {
    blocking(move || open_meetings_folder_now(&app)).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Job;
    use dictate_services::presets::EDIT_PRESETS;
    use dictate_services::secrets::{Account, MemoryStore, SecretError};
    use dictate_services::whisper::models::CATALOG;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

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

    /// A key store that is down, like a Linux desktop without a Secret Service.
    struct NoKeyring;

    impl SecretStore for NoKeyring {
        fn get(&self, _: Account) -> Result<Option<String>, SecretError> {
            Err(SecretError::Unavailable)
        }
        fn set(&self, _: Account, _: &str) -> Result<(), SecretError> {
            Err(SecretError::Unavailable)
        }
        fn delete(&self, _: Account) -> Result<(), SecretError> {
            Err(SecretError::Unavailable)
        }
    }

    // ---- the settings view and the keys

    #[test]
    fn view_never_contains_key_values() {
        let f = fixture();
        f.secrets.set(Account::Ai, AI_KEY).unwrap();
        f.secrets.set(Account::Transcribe, STT_KEY).unwrap();

        let view = get_settings_view(&f.store, &f.secrets);
        let json = serde_json::to_string(&view).unwrap();

        assert!(!json.contains(AI_KEY), "{json}");
        assert!(!json.contains(STT_KEY), "{json}");
        assert!(view.has_ai_key && view.has_transcribe_key);
    }

    #[test]
    fn view_has_the_settings_at_the_top_level() {
        let f = fixture();
        let json = serde_json::to_value(get_settings_view(&f.store, &f.secrets)).unwrap();
        assert_eq!(json["ai_model"], "claude-haiku-5-5");
        assert_eq!(json["hotkey"], "Ctrl+Alt+Space");
        assert_eq!(json["has_ai_key"], false);
        assert_eq!(json["has_transcribe_key"], false);
        assert_eq!(json["shares_openrouter_key"], false);
        assert_eq!(json["load_warning"], serde_json::Value::Null);
    }

    #[test]
    fn view_reports_which_keys_are_saved() {
        let f = fixture();
        f.secrets.set(Account::Ai, AI_KEY).unwrap();
        let view = get_settings_view(&f.store, &f.secrets);
        assert!(view.has_ai_key);
        assert!(!view.has_transcribe_key);
        // An empty value counts as no key.
        f.secrets.set(Account::Transcribe, "").unwrap();
        assert!(!get_settings_view(&f.store, &f.secrets).has_transcribe_key);
    }

    #[test]
    fn view_flags_the_openrouter_key_that_serves_both() {
        let f = fixture();
        f.secrets.set(Account::Ai, AI_KEY).unwrap();
        let mut s = f.store.get();
        s.transcribe_preset = TranscribePreset::OpenRouter;
        s.ai_base_url = "https://openrouter.ai/api/v1".into();
        f.store.save(&s).unwrap();
        assert!(get_settings_view(&f.store, &f.secrets).shares_openrouter_key);

        // With a key of its own, nothing is shared.
        f.secrets.set(Account::Transcribe, STT_KEY).unwrap();
        assert!(!get_settings_view(&f.store, &f.secrets).shares_openrouter_key);
    }

    #[test]
    fn a_keyring_that_is_down_does_not_break_the_view() {
        let f = fixture();
        let view = get_settings_view(&f.store, &NoKeyring);
        assert!(!view.has_ai_key && !view.has_transcribe_key);
    }

    #[test]
    fn set_secret_stores_the_key_and_the_view_says_so() {
        let f = fixture();
        let view = set_secret_inner(&f.store, &f.secrets, "ai", AI_KEY).unwrap();
        assert!(view.has_ai_key && !view.has_transcribe_key);
        assert_eq!(f.secrets.get(Account::Ai), Ok(Some(AI_KEY.to_string())));

        let view = set_secret_inner(&f.store, &f.secrets, "transcribe", STT_KEY).unwrap();
        assert!(view.has_transcribe_key);
        assert_eq!(
            f.secrets.get(Account::Transcribe),
            Ok(Some(STT_KEY.to_string()))
        );
    }

    #[test]
    fn set_secret_empty_deletes() {
        let f = fixture();
        f.secrets.set(Account::Ai, AI_KEY).unwrap();

        let view = set_secret_inner(&f.store, &f.secrets, "ai", "").unwrap();

        assert!(!view.has_ai_key);
        // Deleted, not stored as an empty string.
        assert_eq!(f.secrets.get(Account::Ai), Ok(None));
    }

    #[test]
    fn set_secret_trims_what_was_pasted() {
        let f = fixture();
        set_secret_inner(&f.store, &f.secrets, "ai", &format!("  {AI_KEY}\n")).unwrap();
        assert_eq!(f.secrets.get(Account::Ai), Ok(Some(AI_KEY.to_string())));

        // Only blanks is no key.
        set_secret_inner(&f.store, &f.secrets, "ai", " \n ").unwrap();
        assert_eq!(f.secrets.get(Account::Ai), Ok(None));
    }

    #[test]
    fn set_secret_knows_two_accounts() {
        let f = fixture();
        let err = set_secret_inner(&f.store, &f.secrets, "github", AI_KEY).unwrap_err();
        assert!(err.contains("github"), "{err}");
        assert_eq!(f.secrets.get(Account::Ai), Ok(None));
    }

    #[test]
    fn set_secret_without_a_keyring_says_so() {
        let f = fixture();
        let err = set_secret_inner(&f.store, &NoKeyring, "ai", AI_KEY).unwrap_err();
        assert_eq!(err, "No system keyring available");
        let err = set_secret_inner(&f.store, &NoKeyring, "ai", "").unwrap_err();
        assert_eq!(err, "No system keyring available");
    }

    // ---- saving settings

    #[test]
    fn save_settings_writes_and_returns_the_view() {
        let f = fixture();
        let mut s = f.store.get();
        s.vocabulary = "Dictate, Whisper".into();
        let view = save_settings_inner(&f.store, &f.secrets, s, |_| Ok(())).unwrap();
        assert_eq!(view.settings.vocabulary, "Dictate, Whisper");
        assert_eq!(f.store.get().vocabulary, "Dictate, Whisper");
    }

    #[test]
    fn launch_at_login_is_applied_only_when_it_changes() {
        let f = fixture();
        let applied = Mutex::new(Vec::new());

        let mut s = f.store.get();
        s.launch_at_login = true;
        save_settings_inner(&f.store, &f.secrets, s.clone(), |on| {
            applied.lock().unwrap().push(on);
            Ok(())
        })
        .unwrap();
        // Saving again with the same value leaves the system alone.
        s.vocabulary = "x".into();
        save_settings_inner(&f.store, &f.secrets, s.clone(), |on| {
            applied.lock().unwrap().push(on);
            Ok(())
        })
        .unwrap();
        s.launch_at_login = false;
        save_settings_inner(&f.store, &f.secrets, s, |on| {
            applied.lock().unwrap().push(on);
            Ok(())
        })
        .unwrap();

        assert_eq!(*applied.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn a_failed_launch_at_login_saves_nothing() {
        let f = fixture();
        let mut s = f.store.get();
        s.launch_at_login = true;
        s.vocabulary = "changed".into();

        let err = save_settings_inner(&f.store, &f.secrets, s, |_| {
            Err("Could not add the login item".to_string())
        })
        .unwrap_err();

        assert_eq!(err, "Could not add the login item");
        let kept = f.store.get();
        assert!(!kept.launch_at_login);
        assert_eq!(kept.vocabulary, "");
    }

    #[test]
    fn a_failed_save_puts_launch_at_login_back() {
        // The settings file cannot be written: its folder is a file.
        let dir = tempfile::tempdir().unwrap();
        let in_the_way = dir.path().join("in-the-way");
        std::fs::write(&in_the_way, b"x").unwrap();
        let (store, _) = SettingsStore::open(in_the_way.join("settings.json"));
        let applied = Mutex::new(Vec::new());

        let mut s = store.get();
        s.launch_at_login = true;
        let err = save_settings_inner(&store, &MemoryStore::default(), s, |on| {
            applied.lock().unwrap().push(on);
            Ok(())
        })
        .unwrap_err();

        assert!(err.starts_with("Could not save the settings"), "{err}");
        // Turned on, then back off: the system ends up as the file says.
        assert_eq!(*applied.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn switching_provider_follows_the_rules_and_is_saved() {
        let f = fixture();
        let view = set_ai_provider_inner(&f.store, &f.secrets, AiProvider::OpenAi).unwrap();
        assert_eq!(view.settings.ai_provider, AiProvider::OpenAi);
        assert_eq!(view.settings.ai_base_url, "http://localhost:8317/v1");
        assert_eq!(f.store.get().ai_base_url, "http://localhost:8317/v1");
    }

    #[test]
    fn switching_transcription_service_follows_the_rules_and_is_saved() {
        let f = fixture();
        let view =
            set_transcribe_preset_inner(&f.store, &f.secrets, TranscribePreset::Groq).unwrap();
        assert_eq!(view.settings.transcribe_preset, TranscribePreset::Groq);
        assert_eq!(
            view.settings.transcribe_base_url,
            "https://api.groq.com/openai/v1"
        );
        assert_eq!(f.store.get().transcribe_model, "whisper-large-v3-turbo");
    }

    // ---- the checks

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
    fn no_key_means_an_empty_key_and_no_language() {
        let f = fixture();
        let config = transcribe_config(&f.store, &f.secrets);
        assert_eq!(config.api_key, "");
        assert_eq!(config.language, None);
    }

    // ---- error text

    #[test]
    fn an_error_status_without_a_message_gets_the_reason_phrase() {
        let error = ApiError::Status {
            status: 301,
            message: String::new(),
        };
        assert_eq!(api_error_text(&error), "API error 301: Moved Permanently");
        let blank = ApiError::Status {
            status: 502,
            message: " \n".into(),
        };
        assert_eq!(api_error_text(&blank), "API error 502: Bad Gateway");
    }

    #[test]
    fn a_status_the_registry_does_not_know_has_no_dangling_colon() {
        let error = ApiError::Status {
            status: 299,
            message: String::new(),
        };
        assert_eq!(api_error_text(&error), "API error 299");
    }

    #[test]
    fn the_message_of_the_provider_is_kept() {
        let error = ApiError::Status {
            status: 401,
            message: "Invalid API key".into(),
        };
        assert_eq!(api_error_text(&error), "API error 401: Invalid API key");
        assert_eq!(
            api_error_text(&ApiError::Network("connection refused".into())),
            "Network error: connection refused"
        );
        assert_eq!(
            api_error_text(&ApiError::Empty),
            "The API returned no content."
        );
    }

    #[test]
    fn transcription_api_errors_get_the_reason_phrase_too() {
        let error = TranscribeError::Api(ApiError::Status {
            status: 404,
            message: String::new(),
        });
        assert_eq!(transcribe_error_text(&error), "API error 404: Not Found");
        assert_eq!(
            transcribe_error_text(&TranscribeError::Io("x.wav: gone".into())),
            "Could not read the audio file: x.wav: gone"
        );
    }

    // ---- local transcription

    #[test]
    fn the_transcript_has_a_minutes_and_seconds_mark_per_line() {
        let segments = vec![
            Segment {
                start: 0.0,
                speaker: None,
                text: "Hello there.".into(),
            },
            Segment {
                start: 65.4,
                speaker: None,
                text: "Second line.".into(),
            },
            Segment {
                start: 3725.0,
                speaker: None,
                text: "Much later.".into(),
            },
        ];
        assert_eq!(
            format_transcript(&segments),
            "[00:00] Hello there.\n[01:05] Second line.\n[1:02:05] Much later."
        );
        assert_eq!(format_transcript(&[]), "");
    }

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

    #[test]
    fn transcribing_with_a_missing_model_names_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let models = ModelDir::new(dir.path().join("models"));
        let settings = Settings {
            whisper_model: Some("small".into()),
            ..Settings::default()
        };
        let err = transcribe_file(
            &settings,
            &models,
            16,
            Path::new("none.wav"),
            &CancellationToken::new(),
        )
        .unwrap_err();
        assert!(
            err.contains("small") && err.contains("isn't downloaded"),
            "{err}"
        );
    }

    #[test]
    fn a_file_that_is_no_wav_is_reported_before_the_model_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let models = ModelDir::new(dir.path().join("models"));
        // Not a real model: loading it would fail differently, so this shows the audio is read first.
        put_model(&models, "tiny");
        let audio = dir.path().join("talk.wav");
        std::fs::write(&audio, b"this is not audio").unwrap();
        let settings = Settings {
            whisper_model: Some("tiny".into()),
            ..Settings::default()
        };

        let err =
            transcribe_file(&settings, &models, 16, &audio, &CancellationToken::new()).unwrap_err();

        assert!(err.starts_with("Could not read the audio"), "{err}");
    }

    /// The real thing: needs a Whisper model. Run with
    /// `DICTATE_WHISPER_TEST_MODEL=/path/to/ggml-tiny.bin cargo test -p dictate-app -- --ignored`.
    fn real_model_dir(dir: &tempfile::TempDir) -> ModelDir {
        let source = std::env::var_os("DICTATE_WHISPER_TEST_MODEL")
            .expect("set DICTATE_WHISPER_TEST_MODEL to a ggml-tiny.bin file");
        let models = ModelDir::new(dir.path().join("models"));
        std::fs::create_dir_all(models.root()).unwrap();
        std::fs::copy(source, models.path("tiny")).unwrap();
        models
    }

    fn hello_wav() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/dictate-services/tests/fixtures/hello.wav")
    }

    #[test]
    #[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
    fn a_real_model_transcribes_the_hello_clip_into_timed_lines() {
        let dir = tempfile::tempdir().unwrap();
        let models = real_model_dir(&dir);
        let settings = Settings {
            whisper_model: Some("tiny".into()),
            transcribe_language: "en".into(),
            ..Settings::default()
        };

        let text = transcribe_file(
            &settings,
            &models,
            16,
            &hello_wav(),
            &CancellationToken::new(),
        )
        .unwrap();

        assert!(text.starts_with("[00:0"), "{text}");
        assert!(text.to_lowercase().contains("hello"), "{text}");
    }

    #[test]
    #[ignore = "needs a Whisper model: set DICTATE_WHISPER_TEST_MODEL"]
    fn a_cancelled_transcription_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let models = real_model_dir(&dir);
        let settings = Settings {
            whisper_model: Some("tiny".into()),
            ..Settings::default()
        };
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = transcribe_file(&settings, &models, 16, &hello_wav(), &cancel).unwrap_err();

        assert_eq!(err, "The transcription was cancelled.");
    }

    // ---- model downloads

    /// A server that answers the first request with `length` bytes announced and `sent` of them sent, then holds
    /// the connection until the client goes away. Returns the base URL.
    fn serve_model(length: usize, sent: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    return;
                }
                request.push(byte[0]);
            }
            let head =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&vec![b'x'; sent]);
            let _ = stream.flush();
            // Wait for the client to hang up.
            let mut rest = [0u8; 64];
            while stream.read(&mut rest).unwrap_or(0) > 0 {}
        });
        format!("http://127.0.0.1:{port}")
    }

    /// A base URL nothing listens on.
    fn dead_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        format!("http://127.0.0.1:{port}")
    }

    fn state_in(dir: &tempfile::TempDir) -> AppState {
        let (store, warning) = SettingsStore::open(dir.path().join("settings.json"));
        AppState::new(
            store,
            warning,
            Arc::new(MemoryStore::default()),
            ModelDir::new(dir.path().join("models")),
        )
    }

    type Events = Arc<Mutex<Vec<DownloadEvent>>>;

    fn recorder() -> (Events, impl Fn(DownloadEvent) + Send + Sync) {
        let events: Events = Arc::default();
        let sink = events.clone();
        (events, move |event| sink.lock().unwrap().push(event))
    }

    #[test]
    fn a_finished_download_ends_with_done_and_leaves_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(&dir);
        let url = serve_model(10, 10);
        let (events, emit) = recorder();

        let result = tauri::async_runtime::block_on(run_download(&state, "tiny", &url, emit));

        assert_eq!(result, Ok(()));
        let events = events.lock().unwrap();
        let last = events.last().unwrap();
        assert_eq!(
            *last,
            DownloadEvent {
                id: "tiny".into(),
                done: 10,
                total: Some(10),
                state: DownloadState::Done,
                error: None,
            }
        );
        // Progress alone, even when it reaches the total, is never "done".
        let before = &events[..events.len() - 1];
        assert!(!before.is_empty());
        assert!(before.iter().all(|e| e.state == DownloadState::Running));
        assert!(state.models.is_downloaded("tiny"));
        // The model can be fetched again (or deleted) now.
        assert!(state.jobs.start(Job::Download("tiny".into())).is_ok());
    }

    #[test]
    fn a_failed_download_says_why_and_frees_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(&dir);
        let (events, emit) = recorder();

        let result =
            tauri::async_runtime::block_on(run_download(&state, "tiny", &dead_url(), emit));

        let message = result.unwrap_err();
        assert!(message.starts_with("Network error"), "{message}");
        let events = events.lock().unwrap();
        let last = events.last().unwrap();
        assert_eq!(last.state, DownloadState::Failed);
        assert_eq!(last.error.as_deref(), Some(message.as_str()));
        assert!(events.iter().all(|e| e.state != DownloadState::Done));
        assert!(!state.models.is_downloaded("tiny"));
        assert!(state.jobs.start(Job::Download("tiny".into())).is_ok());
    }

    #[test]
    fn a_cancelled_download_is_reported_as_cancelled_not_as_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(&dir);
        // 1000 bytes announced, 100 sent: it waits for the rest until it is cancelled.
        let url = serve_model(1000, 100);
        let events: Events = Arc::default();
        let sink = events.clone();
        let jobs = state.jobs.clone();
        let emit = move |event: DownloadEvent| {
            if event.state == DownloadState::Running {
                jobs.cancel(&Job::Download("tiny".into()));
            }
            sink.lock().unwrap().push(event);
        };

        let result = tauri::async_runtime::block_on(run_download(&state, "tiny", &url, emit));

        assert_eq!(result, Ok(()));
        let events = events.lock().unwrap();
        assert_eq!(events.last().unwrap().state, DownloadState::Cancelled);
        assert_eq!(events.last().unwrap().error, None);
        assert!(events.iter().all(|e| e.state != DownloadState::Done));
        assert!(!state.models.is_downloaded("tiny"));
    }

    #[test]
    fn a_second_download_of_the_same_model_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(&dir);
        let _running = state.jobs.start(Job::Download("tiny".into())).unwrap();
        let (events, emit) = recorder();

        let result =
            tauri::async_runtime::block_on(run_download(&state, "tiny", &dead_url(), emit));

        assert_eq!(result, Err("Already downloading".to_string()));
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn a_model_that_is_not_in_the_catalog_is_refused_up_front() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(&dir);
        let (events, emit) = recorder();

        let result =
            tauri::async_runtime::block_on(run_download(&state, "gpt-5", &dead_url(), emit));

        assert_eq!(result, Err("There is no model called gpt-5.".to_string()));
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn final_events_carry_how_far_it_came() {
        assert_eq!(
            final_event("base", &Err(DownloadError::Cancelled), (40, Some(100))),
            DownloadEvent {
                id: "base".into(),
                done: 40,
                total: Some(100),
                state: DownloadState::Cancelled,
                error: None,
            }
        );
        let failed = final_event("base", &Err(DownloadError::SizeMismatch), (100, Some(100)));
        assert_eq!(failed.state, DownloadState::Failed);
        assert_eq!(
            failed.error.as_deref(),
            Some("The downloaded file is not the size the server announced.")
        );
    }

    #[test]
    fn events_serialize_the_way_the_ui_reads_them() {
        let running = serde_json::to_value(DownloadEvent {
            id: "tiny".into(),
            done: 5,
            total: None,
            state: DownloadState::Running,
            error: None,
        })
        .unwrap();
        assert_eq!(
            running,
            serde_json::json!({"id": "tiny", "done": 5, "total": null, "state": "running"})
        );
        let failed = serde_json::to_value(DownloadEvent {
            id: "tiny".into(),
            done: 5,
            total: Some(9),
            state: DownloadState::Failed,
            error: Some("no space".into()),
        })
        .unwrap();
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["error"], "no space");
    }

    // ---- small things

    #[test]
    fn deleting_a_model_removes_it_and_its_unfinished_download() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(&dir);
        put_model(&state.models, "tiny");
        let part = state.models.root().join("ggml-tiny.bin.part");
        std::fs::write(&part, b"half").unwrap();

        delete_model_inner(&state, "tiny").unwrap();

        assert!(!state.models.is_downloaded("tiny"));
        assert!(!part.exists());
    }

    #[test]
    fn a_model_that_is_being_downloaded_is_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(&dir);
        put_model(&state.models, "tiny");
        let _running = state.jobs.start(Job::Download("tiny".into())).unwrap();

        let err = delete_model_inner(&state, "tiny").unwrap_err();

        assert_eq!(
            err,
            "The model is being downloaded. Cancel the download first."
        );
        assert!(state.models.is_downloaded("tiny"));
    }

    #[test]
    fn deleting_a_model_that_does_not_exist_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(&dir);
        let err = delete_model_inner(&state, "nope").unwrap_err();
        assert_eq!(err, "Could not delete the model: There is no such model.");
    }

    #[test]
    fn the_edit_presets_are_the_eight_in_order() {
        let list = edit_presets_list();
        assert_eq!(list.len(), 8);
        for (view, preset) in list.iter().zip(EDIT_PRESETS.iter()) {
            assert_eq!(view.id, preset.id);
            assert_eq!(view.instruction, preset.instruction);
        }
        assert_eq!(list[0].id, "clean");
    }

    #[test]
    fn the_transcription_services_come_with_their_models_and_key_pages() {
        let list = transcribe_presets_list();
        assert_eq!(
            list.iter().map(|view| view.id).collect::<Vec<_>>(),
            [
                TranscribePreset::OpenRouter,
                TranscribePreset::OpenAi,
                TranscribePreset::Groq,
                TranscribePreset::Custom
            ]
        );

        let json = serde_json::to_value(&list).unwrap();
        assert_eq!(json[0]["id"], "openrouter");
        assert_eq!(json[0]["models"][0], "google/gemini-3.5-flash");
        assert_eq!(json[0]["key_url"], "https://openrouter.ai/keys");
        assert_eq!(json[1]["models"][0], "whisper-1");
        assert_eq!(json[2]["key_url"], "https://console.groq.com/keys");
        // A custom endpoint takes any model name and has no page that hands out keys.
        assert_eq!(json[3]["id"], "custom");
        assert_eq!(json[3]["models"], serde_json::json!([]));
        assert!(json[3]["key_url"].is_null());
    }

    #[test]
    fn the_models_list_serializes_the_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_in(&dir);
        let list: Vec<ModelView> = state
            .models
            .list()
            .into_iter()
            .map(ModelView::from)
            .collect();
        assert_eq!(list.len(), CATALOG.len());
        let json = serde_json::to_value(&list[0]).unwrap();
        assert_eq!(json["id"], CATALOG[0].id);
        assert_eq!(json["downloaded"], false);
        assert!(json["size_mb"].is_number() && json["recommended"].is_boolean());
    }

    #[test]
    fn the_meetings_folder_is_created_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Dictate Meetings");
        ensure_directory(&folder).unwrap();
        assert!(folder.is_dir());
        // It is fine when it is there already.
        ensure_directory(&folder).unwrap();
    }

    #[test]
    fn a_folder_that_cannot_be_created_is_an_error_with_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("in-the-way");
        std::fs::write(&file, b"x").unwrap();
        let err = ensure_directory(&file.join("Dictate Meetings")).unwrap_err();
        assert!(err.contains("Dictate Meetings"), "{err}");
    }
}
