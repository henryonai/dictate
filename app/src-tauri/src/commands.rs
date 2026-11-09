//! The commands the UI calls. Each one is a thin `async` wrapper over a plain function that is tested here.
//!
//! Errors are `String`s meant for the person using the app. Keys never leave this side: the UI learns whether
//! one is saved, never what it is.
//!
//! Anything that can block (the OS key store, the disk, Whisper) runs on a blocking thread, never on the async
//! runtime's own.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dictate_core::meetings::library::MeetingLibrary;
use dictate_core::text::segments::Segment;
use dictate_core::text::timestamp;
use dictate_services::ai;
use dictate_services::http::ApiError;
use dictate_services::presets::{AiProvider, TranscribePreset, EDIT_PRESETS};
use dictate_services::secrets::{Account, SecretStore};
use dictate_services::settings::{self, Settings, SettingsStore};
use dictate_services::system;
use dictate_services::transcribe_api::{self, TranscribeError};
use dictate_services::whisper::local::{read_wav_16k_mono, LocalWhisper};
use dictate_services::whisper::models::{
    self, DownloadError, ModelDir, ModelStatus, HUGGING_FACE_BASE_URL,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_opener::OpenerExt as _;
use tokio_util::sync::CancellationToken;

use crate::config::{ai_config, local_model, transcribe_config};
use crate::dictation::save::SavePaths;
use crate::hotkey::{ChangeError, HotkeyStatus, Hotkeys, INVALID, TAKEN};
use crate::platform::{Permission, Platform, SystemPlatform};
use crate::state::{lock, AppState, Job, KeyFlags};
use crate::tray;

/// The event that tells how a model download goes.
const DOWNLOAD_EVENT: &str = "model-download";
/// The event that tells the Settings window of settings that changed without it (see [`emit_language`]).
const SETTINGS_EVENT: &str = "settings-changed";

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
    /// What the language choice "System" means on this machine: the language of the tray menu. The window
    /// resolves "System" to this one and not by its own guess, so that both always agree.
    pub system_language: tray::Lang,
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

/// What the system says about the two permissions dictation needs. Only macOS has them: elsewhere both are
/// `NotNeeded`, which is how the Settings window knows to show no Permissions section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PermissionsView {
    pub microphone: Permission,
    pub accessibility: Permission,
}

impl PermissionsView {
    /// Whether the person has something to do about the permissions, which is when Settings opens by itself at the
    /// start. A microphone nobody asked about yet: until it is answered. Accessibility missing: once (`shown_before`
    /// is the guide's mark), and only when text is written into fields (`auto_write`): with the card alone nothing
    /// needs it, and a person who skipped it is not sent to Settings at every login. A microphone that was refused is
    /// the person's answer, and Settings doesn't open at every start for it.
    pub fn needs_a_guide(&self, auto_write: bool, shown_before: bool) -> bool {
        self.microphone == Permission::NotDetermined
            || (auto_write && !shown_before && self.accessibility == Permission::Denied)
    }
}

/// The mark that Settings was opened by itself, at a start, for the permissions. A file beside the settings: the
/// settings are the window's to write whole, and this is not something it should know of.
pub struct GuideMark(PathBuf);

impl GuideMark {
    /// The mark that goes with the settings file `settings`.
    pub fn beside(settings: &Path) -> GuideMark {
        GuideMark(settings.with_file_name("permission-guide-shown"))
    }

    /// Whether the guide was shown before.
    pub fn shown(&self) -> bool {
        self.0.exists()
    }

    /// Remembers that the guide was shown. If that can't be written the guide is shown again at the next start, which
    /// is a nuisance and nothing worse.
    pub fn set(&self) {
        let written = self
            .0
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&self.0, b""));
        if let Err(e) = written {
            log::warn!("Could not remember that the permissions guide was shown: {e}");
        }
    }
}

/// Whether Settings opens by itself at this start, for the permissions; when it does, that is remembered.
pub fn open_guide_at_start(view: &PermissionsView, auto_write: bool, mark: &GuideMark) -> bool {
    if !view.needs_a_guide(auto_write, mark.shown()) {
        return false;
    }
    mark.set();
    true
}

/// Asks the system for both permissions.
pub fn permissions_view(platform: &dyn Platform) -> PermissionsView {
    PermissionsView {
        microphone: platform.microphone(),
        accessibility: platform.accessibility(),
    }
}

/// Shows the system's prompt for the permission `kind` names ("microphone" or "accessibility"), and doesn't wait
/// for the answer: the window asks for the status every second.
fn request_permission_now(platform: &dyn Platform, kind: &str) -> Result<(), String> {
    match kind {
        "microphone" => platform.request_microphone(Box::new(|_| {})),
        "accessibility" => platform.request_accessibility(),
        other => return Err(format!("{other:?} is not a permission of this app")),
    }
    Ok(())
}

/// The link to the pane of System Settings where the permission `kind` names is turned on. A fixed list: the window
/// can't have this command open any other link.
fn permission_settings_url(kind: &str) -> Result<&'static str, String> {
    match kind {
        "microphone" => {
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        }
        "accessibility" => {
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        }
        other => Err(format!("{other:?} is not a permission of this app")),
    }
}

/// The settings plus which keys exist. The flags are what the key store said at the start and what saving or
/// removing a key has done since: asking the key store itself on every view could make the OS show a prompt each
/// time.
fn get_settings_view(store: &SettingsStore, keys: &KeyFlags) -> SettingsView {
    let settings = store.get();
    let has_ai_key = keys.get(Account::Ai);
    let has_transcribe_key = keys.get(Account::Transcribe);
    SettingsView {
        has_ai_key,
        has_transcribe_key,
        shares_openrouter_key: settings::shares_ai_key(&settings, has_ai_key, has_transcribe_key),
        load_warning: None,
        system_language: tray::system_language(),
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

/// Saves a key; an empty one (after trimming what was pasted around it) deletes it. The flag follows only once the
/// key store has done it.
fn set_secret_inner(
    store: &SettingsStore,
    keys: &KeyFlags,
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
    stored.map_err(|e| {
        // The platform's own words, for whoever looks at the log; the key is never in them.
        log::warn!("Could not change the {} key: {e}", account.name());
        e.to_string()
    })?;
    keys.set(account, !value.is_empty());
    Ok(get_settings_view(store, keys))
}

fn saving_failed(e: io::Error) -> String {
    format!("Could not save the settings: {e}")
}

fn set_ai_provider_inner(
    store: &SettingsStore,
    keys: &KeyFlags,
    provider: AiProvider,
) -> Result<SettingsView, String> {
    store
        .update(|settings| settings.set_ai_provider(provider))
        .map_err(saving_failed)?;
    Ok(get_settings_view(store, keys))
}

fn set_transcribe_preset_inner(
    store: &SettingsStore,
    keys: &KeyFlags,
    preset: TranscribePreset,
) -> Result<SettingsView, String> {
    store
        .update(|settings| settings.set_transcribe_preset(preset))
        .map_err(saving_failed)?;
    Ok(get_settings_view(store, keys))
}

/// Registers the shortcut and saves it. The errors that the person can do something about are keys of the UI's
/// texts, which it translates; the rest is a sentence.
fn set_hotkey_inner(
    store: &SettingsStore,
    keys: &KeyFlags,
    hotkeys: &Hotkeys,
    accel: &str,
) -> Result<SettingsView, String> {
    hotkeys.change(store, accel).map_err(|e| match e {
        ChangeError::Invalid => INVALID.to_string(),
        ChangeError::Taken => TAKEN.to_string(),
        ChangeError::Save(e) => saving_failed(e),
    })?;
    Ok(get_settings_view(store, keys))
}

/// Saves `settings`. `apply_launch_at_login` is called, with the new value, only when that setting changes; if it
/// fails nothing is saved, so the file never says something the system doesn't do. The same goes for the shortcut,
/// which only `set_hotkey` changes: what the window sends for it is ignored.
fn save_settings_inner(
    store: &SettingsStore,
    keys: &KeyFlags,
    settings: Settings,
    mut apply_launch_at_login: impl FnMut(bool) -> Result<(), String>,
) -> Result<SettingsView, String> {
    let before = store.get().launch_at_login;
    let changed = settings.launch_at_login != before;
    if changed {
        apply_launch_at_login(settings.launch_at_login)?;
    }
    let saved = store.update(|current| {
        let hotkey = std::mem::take(&mut current.hotkey);
        *current = settings;
        current.hotkey = hotkey;
    });
    if let Err(e) = saved {
        if changed {
            // Put the system back to what the file still says.
            let _ = apply_launch_at_login(before);
        }
        return Err(saving_failed(e));
    }
    Ok(get_settings_view(store, keys))
}

/// An API error as shown to the user. An error status with no message gets the HTTP reason phrase, so that a
/// reply with an empty body doesn't end in "API error 301: ".
pub(crate) fn api_error_text(error: &ApiError) -> String {
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

pub(crate) fn transcribe_error_text(error: &TranscribeError) -> String {
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
        .map_err(|e| format!("Could not delete the model: {e}"))?;
    // Its place in memory goes with its file, when it was the model in use.
    state.whisper.forget(id);
    Ok(())
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

/// Opens a folder in the file manager, creating it first if needed.
fn open_folder(app: &AppHandle, folder: &Path) -> Result<(), String> {
    ensure_directory(folder)?;
    app.opener()
        .open_path(folder.to_string_lossy().into_owned(), None::<String>)
        .map_err(|e| format!("Could not open {}: {e}", folder.display()))
}

/// Opens the meetings folder in the file manager, creating it first if needed.
pub fn open_meetings_folder_now(app: &AppHandle) -> Result<(), String> {
    open_folder(app, &MeetingLibrary::default_directory())
}

/// Opens the folder of the saved dictations (`Dictate/Dictations`), creating it first if needed.
pub fn open_dictations_folder_now(app: &AppHandle) -> Result<(), String> {
    let paths = SavePaths::system().ok_or("The system has no folder for the app's data.")?;
    open_folder(app, &paths.dir)
}

// ---- the commands

/// Tells the Settings window that the dictation language was changed without it (by the language menu of the chip),
/// with only that setting: the window keeps a copy of all of them and sends it back whole with every save.
pub(crate) fn emit_language<R: tauri::Runtime>(app: &AppHandle<R>, lang: &str) {
    #[derive(Serialize, Clone)]
    struct Changed<'a> {
        dictation_language: &'a str,
    }
    let changed = Changed {
        dictation_language: lang,
    };
    if let Err(e) = app.emit(SETTINGS_EVENT, changed) {
        log::warn!("Could not tell the Settings window of the new language: {e}");
    }
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<SettingsView, String> {
    let state = state.inner().clone();
    blocking(move || {
        let mut view = get_settings_view(&state.settings, state.loaded_keys());
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
        save_settings_inner(&state.settings, state.loaded_keys(), settings, |enabled| {
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
    blocking(move || set_ai_provider_inner(&state.settings, state.loaded_keys(), p)).await?
}

#[tauri::command]
pub async fn set_transcribe_preset(
    state: State<'_, AppState>,
    p: TranscribePreset,
) -> Result<SettingsView, String> {
    let state = state.inner().clone();
    blocking(move || set_transcribe_preset_inner(&state.settings, state.loaded_keys(), p)).await?
}

/// Changes the shortcut that starts dictation. The answer is the settings; the errors are `hotkey.invalid`,
/// `hotkey.taken` (another app has the shortcut) or a sentence about the file.
#[tauri::command]
pub async fn set_hotkey(
    app: AppHandle,
    state: State<'_, AppState>,
    hotkeys: State<'_, Hotkeys>,
    accel: String,
) -> Result<SettingsView, String> {
    let state = state.inner().clone();
    let hotkeys = hotkeys.inner().clone();
    let view = blocking({
        let hotkeys = hotkeys.clone();
        move || set_hotkey_inner(&state.settings, state.loaded_keys(), &hotkeys, &accel)
    })
    .await??;
    log::info!("The shortcut is now {}", hotkeys.status().label);
    // The item of the tray menu that starts dictation names the shortcut.
    tray::refresh(&app, &view.settings);
    Ok(view)
}

/// The recorder in Settings starts (`true`) or stops (`false`) listening for a new shortcut. While it listens the
/// current shortcut doesn't start a dictation; the pause ends by itself after a minute.
#[tauri::command]
pub async fn hotkey_recording(hotkeys: State<'_, Hotkeys>, active: bool) -> Result<(), String> {
    hotkeys.set_recording(active);
    Ok(())
}

/// What the shortcut is and whether it works.
#[tauri::command]
pub async fn hotkey_status(hotkeys: State<'_, Hotkeys>) -> Result<HotkeyStatus, String> {
    Ok(hotkeys.status())
}

/// `account` is `"ai"` or `"transcribe"`; an empty `value` deletes the key.
#[tauri::command]
pub async fn set_secret(
    state: State<'_, AppState>,
    account: String,
    value: String,
) -> Result<SettingsView, String> {
    let state = state.inner().clone();
    blocking(move || {
        set_secret_inner(
            &state.settings,
            state.loaded_keys(),
            &*state.secrets,
            &account,
            &value,
        )
    })
    .await?
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

#[tauri::command]
pub async fn permissions_status(
    platform: State<'_, SystemPlatform>,
) -> Result<PermissionsView, String> {
    let platform = platform.0.clone();
    blocking(move || permissions_view(&*platform)).await
}

/// The prompt for a permission, which the system shows once: the person's answer comes with the next status.
#[tauri::command]
pub async fn request_permission(
    kind: String,
    platform: State<'_, SystemPlatform>,
) -> Result<(), String> {
    let platform = platform.0.clone();
    blocking(move || request_permission_now(&*platform, &kind)).await?
}

#[tauri::command]
pub async fn open_permission_settings(kind: String, app: AppHandle) -> Result<(), String> {
    let url = permission_settings_url(&kind)?;
    blocking(move || {
        app.opener()
            .open_url(url, None::<String>)
            .map_err(|e| format!("Could not open System Settings: {e}"))
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictation::controller::fakes::{CallLog, FakePlatform};
    use crate::hotkey::{Backend, Gate, Registry};
    use crate::platform::rules::Os;
    use crate::state::{Job, KeyFlags};
    use crate::test_support::{CountingStore, FakeRegistry, NoKeyring};
    use dictate_services::presets::EDIT_PRESETS;
    use dictate_services::secrets::{Account, MemoryStore, SecretError};
    use dictate_services::settings::UiLanguage;
    use dictate_services::whisper::models::CATALOG;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;

    const AI_KEY: &str = "sk-ai-secret-123";
    const STT_KEY: &str = "sk-stt-secret-456";

    struct Fixture {
        _dir: tempfile::TempDir,
        store: SettingsStore,
        keys: KeyFlags,
        secrets: MemoryStore,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let (store, warning) = SettingsStore::open(dir.path().join("settings.json"));
        assert!(warning.is_none());
        Fixture {
            _dir: dir,
            store,
            keys: KeyFlags::default(),
            secrets: MemoryStore::default(),
        }
    }

    /// A key store whose platform refuses: what the Keychain does when the user denies its prompt.
    struct DeniedKeyring;

    impl SecretStore for DeniedKeyring {
        fn get(&self, _: Account) -> Result<Option<String>, SecretError> {
            Err(denied())
        }
        fn set(&self, _: Account, _: &str) -> Result<(), SecretError> {
            Err(denied())
        }
        fn delete(&self, _: Account) -> Result<(), SecretError> {
            Err(denied())
        }
    }

    fn denied() -> SecretError {
        SecretError::PlatformFailure("The user denied the Keychain prompt (-25293)".into())
    }

    // ---- the settings view and the keys

    #[test]
    fn view_never_contains_key_values() {
        let f = fixture();
        set_secret_inner(&f.store, &f.keys, &f.secrets, "ai", AI_KEY).unwrap();
        set_secret_inner(&f.store, &f.keys, &f.secrets, "transcribe", STT_KEY).unwrap();

        let view = get_settings_view(&f.store, &f.keys);
        let json = serde_json::to_string(&view).unwrap();

        assert!(!json.contains(AI_KEY), "{json}");
        assert!(!json.contains(STT_KEY), "{json}");
        assert!(view.has_ai_key && view.has_transcribe_key);
    }

    #[test]
    fn view_does_not_read_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let counting = Arc::new(CountingStore::default());
        counting.set(Account::Ai, AI_KEY).unwrap();
        let state = state_with(&dir, counting.clone());
        // The start of the app reads the keys.
        state.loaded_keys();
        let read_at_start = counting.gets();

        // The window asks for the settings again and again.
        let view = get_settings_view(&state.settings, state.loaded_keys());
        get_settings_view(&state.settings, state.loaded_keys());

        assert!(view.has_ai_key && !view.has_transcribe_key);
        assert_eq!(counting.gets(), read_at_start, "the view read a key");
    }

    #[test]
    fn view_tells_what_the_system_language_is_as_the_tray_resolves_it() {
        let f = fixture();
        let system = Settings {
            ui_language: UiLanguage::System,
            ..Settings::default()
        };
        let wanted = match tray::language_for(&system) {
            tray::Lang::En => "en",
            tray::Lang::Vi => "vi",
        };
        // Whatever language is chosen: the window needs it to resolve "system" in the tray's language.
        for chosen in [UiLanguage::System, UiLanguage::En, UiLanguage::Vi] {
            let mut s = f.store.get();
            s.ui_language = chosen;
            f.store.save(&s).unwrap();
            let json = serde_json::to_value(get_settings_view(&f.store, &f.keys)).unwrap();
            assert_eq!(json["system_language"], wanted, "with {chosen:?} chosen");
        }
    }

    #[test]
    fn view_has_the_settings_at_the_top_level() {
        let f = fixture();
        let json = serde_json::to_value(get_settings_view(&f.store, &f.keys)).unwrap();
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
        f.keys.set(Account::Ai, true);
        let view = get_settings_view(&f.store, &f.keys);
        assert!(view.has_ai_key);
        assert!(!view.has_transcribe_key);
        f.keys.set(Account::Transcribe, true);
        assert!(get_settings_view(&f.store, &f.keys).has_transcribe_key);
    }

    #[test]
    fn view_flags_the_openrouter_key_that_serves_both() {
        let f = fixture();
        f.keys.set(Account::Ai, true);
        let mut s = f.store.get();
        s.set_transcribe_preset(TranscribePreset::OpenRouter);
        s.ai_base_url = "https://openrouter.ai/api/v1".into();
        f.store.save(&s).unwrap();
        assert!(get_settings_view(&f.store, &f.keys).shares_openrouter_key);

        // A transcription URL that has moved to another host gets nothing of it.
        s.transcribe_base_url = "https://stt.example.com/v1".into();
        f.store.save(&s).unwrap();
        assert!(!get_settings_view(&f.store, &f.keys).shares_openrouter_key);

        // With a key of its own, nothing is shared.
        s.transcribe_base_url = "https://openrouter.ai/api/v1".into();
        f.store.save(&s).unwrap();
        f.keys.set(Account::Transcribe, true);
        assert!(!get_settings_view(&f.store, &f.keys).shares_openrouter_key);
    }

    #[test]
    fn set_secret_stores_the_key_and_the_view_says_so() {
        let f = fixture();
        let view = set_secret_inner(&f.store, &f.keys, &f.secrets, "ai", AI_KEY).unwrap();
        assert!(view.has_ai_key && !view.has_transcribe_key);
        assert_eq!(f.secrets.get(Account::Ai), Ok(Some(AI_KEY.to_string())));

        let view = set_secret_inner(&f.store, &f.keys, &f.secrets, "transcribe", STT_KEY).unwrap();
        assert!(view.has_transcribe_key);
        assert_eq!(
            f.secrets.get(Account::Transcribe),
            Ok(Some(STT_KEY.to_string()))
        );
    }

    #[test]
    fn set_secret_updates_the_flag() {
        let f = fixture();
        let secrets = CountingStore::default();

        let view = set_secret_inner(&f.store, &f.keys, &secrets, "ai", AI_KEY).unwrap();
        assert!(view.has_ai_key);
        assert!(f.keys.get(Account::Ai));

        let view = set_secret_inner(&f.store, &f.keys, &secrets, "ai", "  ").unwrap();
        assert!(!view.has_ai_key);
        assert!(!f.keys.get(Account::Ai));

        assert_eq!(secrets.gets(), 0, "the key store is not read for the view");
    }

    #[test]
    fn set_secret_empty_deletes() {
        let f = fixture();
        f.secrets.set(Account::Ai, AI_KEY).unwrap();
        f.keys.set(Account::Ai, true);

        let view = set_secret_inner(&f.store, &f.keys, &f.secrets, "ai", "").unwrap();

        assert!(!view.has_ai_key);
        // Deleted, not stored as an empty string.
        assert_eq!(f.secrets.get(Account::Ai), Ok(None));
    }

    #[test]
    fn set_secret_trims_what_was_pasted() {
        let f = fixture();
        set_secret_inner(
            &f.store,
            &f.keys,
            &f.secrets,
            "ai",
            &format!("  {AI_KEY}\n"),
        )
        .unwrap();
        assert_eq!(f.secrets.get(Account::Ai), Ok(Some(AI_KEY.to_string())));

        // Only blanks is no key.
        set_secret_inner(&f.store, &f.keys, &f.secrets, "ai", " \n ").unwrap();
        assert_eq!(f.secrets.get(Account::Ai), Ok(None));
    }

    #[test]
    fn set_secret_knows_two_accounts() {
        let f = fixture();
        let err = set_secret_inner(&f.store, &f.keys, &f.secrets, "github", AI_KEY).unwrap_err();
        assert!(err.contains("github"), "{err}");
        assert_eq!(f.secrets.get(Account::Ai), Ok(None));
        assert!(!f.keys.get(Account::Ai));
    }

    #[test]
    fn set_secret_without_a_keyring_says_so() {
        let f = fixture();
        let err = set_secret_inner(&f.store, &f.keys, &NoKeyring, "ai", AI_KEY).unwrap_err();
        assert_eq!(err, "No system keyring available");
        let err = set_secret_inner(&f.store, &f.keys, &NoKeyring, "ai", "").unwrap_err();
        assert_eq!(err, "No system keyring available");
    }

    #[test]
    fn a_key_that_could_not_be_written_or_removed_leaves_the_flag_as_it_was() {
        let f = fixture();

        set_secret_inner(&f.store, &f.keys, &NoKeyring, "ai", AI_KEY).unwrap_err();
        assert!(!f.keys.get(Account::Ai), "a key that was never saved");

        f.keys.set(Account::Ai, true);
        set_secret_inner(&f.store, &f.keys, &NoKeyring, "ai", "").unwrap_err();
        assert!(f.keys.get(Account::Ai), "a key that is still there");
    }

    #[test]
    fn set_secret_tells_what_the_platform_said_when_it_refuses() {
        let f = fixture();
        let err = set_secret_inner(&f.store, &f.keys, &DeniedKeyring, "ai", AI_KEY).unwrap_err();
        assert!(err.starts_with("No system keyring available"), "{err}");
        assert!(
            err.contains("The user denied the Keychain prompt (-25293)"),
            "{err}"
        );
        assert!(!err.contains(AI_KEY), "{err}");
    }

    // ---- saving settings

    #[test]
    fn a_language_that_changed_without_the_window_reaches_the_window_as_the_setting_that_changed() {
        use tauri::{test::mock_builder, Listener};
        let app = mock_builder().build(crate::context()).unwrap();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let sink = heard.clone();
        app.listen(SETTINGS_EVENT, move |event| {
            sink.lock().unwrap().push(event.payload().to_owned());
        });

        emit_language(app.handle(), "vi-VN");

        // Only the setting that changed: the window's own edits of the others stay as they are.
        assert_eq!(
            *heard.lock().unwrap(),
            [r#"{"dictation_language":"vi-VN"}"#]
        );
    }

    #[test]
    fn save_settings_writes_and_returns_the_view() {
        let f = fixture();
        let mut s = f.store.get();
        s.vocabulary = "Dictate, Whisper".into();
        let view = save_settings_inner(&f.store, &f.keys, s, |_| Ok(())).unwrap();
        assert_eq!(view.settings.vocabulary, "Dictate, Whisper");
        assert_eq!(f.store.get().vocabulary, "Dictate, Whisper");
    }

    #[test]
    fn launch_at_login_is_applied_only_when_it_changes() {
        let f = fixture();
        let applied = Mutex::new(Vec::new());

        let mut s = f.store.get();
        s.launch_at_login = true;
        save_settings_inner(&f.store, &f.keys, s.clone(), |on| {
            applied.lock().unwrap().push(on);
            Ok(())
        })
        .unwrap();
        // Saving again with the same value leaves the system alone.
        s.vocabulary = "x".into();
        save_settings_inner(&f.store, &f.keys, s.clone(), |on| {
            applied.lock().unwrap().push(on);
            Ok(())
        })
        .unwrap();
        s.launch_at_login = false;
        save_settings_inner(&f.store, &f.keys, s, |on| {
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

        let err = save_settings_inner(&f.store, &f.keys, s, |_| {
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
        let err = save_settings_inner(&store, &KeyFlags::default(), s, |on| {
            applied.lock().unwrap().push(on);
            Ok(())
        })
        .unwrap_err();

        assert!(err.starts_with("Could not save the settings"), "{err}");
        // Turned on, then back off: the system ends up as the file says.
        assert_eq!(*applied.lock().unwrap(), vec![true, false]);
    }

    // ---- the shortcut

    fn registered_hotkeys(registry: &Arc<FakeRegistry>) -> Hotkeys {
        let old = shortcut("Ctrl+Alt+Space");
        registry.register(old).unwrap();
        let status = HotkeyStatus {
            label: "Ctrl+Alt+Space".into(),
            backend: Backend::Global,
            error: None,
        };
        let gate = Gate::shared(Duration::ZERO, unbounded_channel().0);
        Hotkeys::new(Some(registry.clone()), gate, status, Some(old))
    }

    fn shortcut(accel: &str) -> tauri_plugin_global_shortcut::Shortcut {
        accel.parse().unwrap()
    }

    #[test]
    fn set_hotkey_rejects_garbage() {
        let f = fixture();
        let registry = Arc::new(FakeRegistry::default());
        let hotkeys = registered_hotkeys(&registry);

        for garbage in [
            "banana",
            "D",
            "",
            "Ctrl+Alt",
            "Ctrl+",
            "Shift+D",
            "Shift+Space",
        ] {
            let err = set_hotkey_inner(&f.store, &f.keys, &hotkeys, garbage).unwrap_err();
            assert_eq!(err, "hotkey.invalid", "{garbage:?}");
        }

        assert_eq!(f.store.get().hotkey, "Ctrl+Alt+Space");
        assert_eq!(registry.registered(), vec![shortcut("Ctrl+Alt+Space")]);
        assert_eq!(registry.calls(), 1, "only the registration of the start");
    }

    #[test]
    fn set_hotkey_answers_with_the_view_that_has_the_new_shortcut() {
        let f = fixture();
        let registry = Arc::new(FakeRegistry::default());
        let hotkeys = registered_hotkeys(&registry);

        let view = set_hotkey_inner(&f.store, &f.keys, &hotkeys, "F6").unwrap();

        assert_eq!(view.settings.hotkey, "F6");
        assert_eq!(f.store.get().hotkey, "F6");
        assert_eq!(registry.registered(), vec![shortcut("F6")]);
    }

    #[test]
    fn set_hotkey_says_taken_for_a_shortcut_that_another_app_owns() {
        let f = fixture();
        let registry = Arc::new(FakeRegistry::default());
        let hotkeys = registered_hotkeys(&registry);
        registry.refuse(shortcut("F6"));

        let err = set_hotkey_inner(&f.store, &f.keys, &hotkeys, "F6").unwrap_err();

        assert_eq!(err, "hotkey.taken");
        assert_eq!(f.store.get().hotkey, "Ctrl+Alt+Space");
        assert_eq!(registry.registered(), vec![shortcut("Ctrl+Alt+Space")]);
    }

    #[test]
    fn set_hotkey_tells_when_the_file_cannot_be_written() {
        let dir = tempfile::tempdir().unwrap();
        let in_the_way = dir.path().join("in-the-way");
        std::fs::write(&in_the_way, b"x").unwrap();
        let (store, _) = SettingsStore::open(in_the_way.join("settings.json"));
        let registry = Arc::new(FakeRegistry::default());
        let hotkeys = registered_hotkeys(&registry);

        let err = set_hotkey_inner(&store, &KeyFlags::default(), &hotkeys, "F6").unwrap_err();

        assert!(err.starts_with("Could not save the settings"), "{err}");
        assert_eq!(registry.registered(), vec![shortcut("Ctrl+Alt+Space")]);
    }

    #[test]
    fn saving_the_settings_never_changes_the_shortcut() {
        // Only `set_hotkey` does: it registers the shortcut, and a file that says another one would be a lie.
        let f = fixture();
        let mut stale = f.store.get();
        stale.hotkey = "Ctrl+Shift+Z".into();
        stale.vocabulary = "Dictate".into();

        let view = save_settings_inner(&f.store, &f.keys, stale, |_| Ok(())).unwrap();

        assert_eq!(view.settings.vocabulary, "Dictate");
        assert_eq!(view.settings.hotkey, "Ctrl+Alt+Space");
        assert_eq!(f.store.get().hotkey, "Ctrl+Alt+Space");
    }

    #[test]
    fn switching_provider_follows_the_rules_and_is_saved() {
        let f = fixture();
        let view = set_ai_provider_inner(&f.store, &f.keys, AiProvider::OpenAi).unwrap();
        assert_eq!(view.settings.ai_provider, AiProvider::OpenAi);
        assert_eq!(view.settings.ai_base_url, "http://localhost:8317/v1");
        assert_eq!(f.store.get().ai_base_url, "http://localhost:8317/v1");
    }

    #[test]
    fn switching_transcription_service_follows_the_rules_and_is_saved() {
        let f = fixture();
        let view = set_transcribe_preset_inner(&f.store, &f.keys, TranscribePreset::Groq).unwrap();
        assert_eq!(view.settings.transcribe_preset, TranscribePreset::Groq);
        assert_eq!(
            view.settings.transcribe_base_url,
            "https://api.groq.com/openai/v1"
        );
        assert_eq!(f.store.get().transcribe_model, "whisper-large-v3-turbo");
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

    fn state_with(dir: &tempfile::TempDir, secrets: Arc<dyn SecretStore>) -> AppState {
        let (store, warning) = SettingsStore::open(dir.path().join("settings.json"));
        AppState::new(
            store,
            warning,
            secrets,
            ModelDir::new(dir.path().join("models")),
        )
    }

    fn state_in(dir: &tempfile::TempDir) -> AppState {
        state_with(dir, Arc::new(MemoryStore::default()))
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

    // ---- permissions

    fn mac_with(microphone: Permission, accessibility: Permission) -> Arc<FakePlatform> {
        let platform = FakePlatform::new(Os::Mac, Arc::new(CallLog::default()));
        *lock(&platform.microphone) = microphone;
        *lock(&platform.accessibility) = accessibility;
        platform
    }

    #[test]
    fn permissions_view_serialises() {
        let view = PermissionsView {
            microphone: Permission::NotDetermined,
            accessibility: Permission::Granted,
        };
        assert_eq!(
            serde_json::to_value(view).unwrap(),
            serde_json::json!({ "microphone": "not_determined", "accessibility": "granted" })
        );
        // Each state has the word that the window has a text for.
        for (state, word) in [
            (Permission::Granted, "granted"),
            (Permission::Denied, "denied"),
            (Permission::NotDetermined, "not_determined"),
            (Permission::NotNeeded, "not_needed"),
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), word);
        }
    }

    #[test]
    fn permissions_view_reports_what_the_system_says() {
        let platform = mac_with(Permission::NotDetermined, Permission::Denied);
        assert_eq!(
            permissions_view(&*platform),
            PermissionsView {
                microphone: Permission::NotDetermined,
                accessibility: Permission::Denied,
            }
        );
        // The questions change nothing: no prompt is shown by asking.
        assert_eq!(platform.mic_requests.load(Ordering::SeqCst), 0);
        assert_eq!(platform.accessibility_requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_guide_is_for_a_microphone_not_asked_about_and_once_for_a_missing_accessibility() {
        use Permission::{Denied, Granted, NotDetermined, NotNeeded};
        let guide = |microphone, accessibility, auto_write, shown_before| {
            PermissionsView {
                microphone,
                accessibility,
            }
            .needs_a_guide(auto_write, shown_before)
        };
        // A microphone nobody asked about yet: the guide is there until the person answers, shown before or not.
        assert!(guide(NotDetermined, Granted, true, false));
        assert!(guide(NotDetermined, Denied, true, true));
        assert!(guide(NotDetermined, Denied, false, true));
        // Accessibility is missing and the text is written into fields: the guide opens once, and not at every start after.
        assert!(guide(Granted, Denied, true, false));
        assert!(guide(Denied, Denied, true, false));
        assert!(!guide(Granted, Denied, true, true));
        assert!(!guide(Denied, Denied, true, true));
        // Without the setting that writes into fields nothing needs Accessibility: the card is the whole of it.
        assert!(!guide(Granted, Denied, false, false));
        assert!(!guide(Denied, Denied, false, false));
        // Everything is granted: the app starts in the tray, as every time after the first.
        assert!(!guide(Granted, Granted, true, false));
        // A microphone that was refused is the person's answer: Settings doesn't open at every start for it.
        assert!(!guide(Denied, Granted, true, false));
        // Windows and Linux have nothing to grant.
        assert!(!guide(NotNeeded, NotNeeded, true, false));
    }

    #[test]
    fn the_guide_opens_at_a_start_and_is_remembered_so_that_a_missing_accessibility_asks_once() {
        use Permission::{Denied, Granted, NotDetermined};
        let dir = tempfile::tempdir().unwrap();
        let mark = GuideMark::beside(&dir.path().join("settings.json"));
        let view = |microphone, accessibility| PermissionsView {
            microphone,
            accessibility,
        };

        // Accessibility is missing: the first start that finds it opens Settings, and the ones after it don't.
        assert!(open_guide_at_start(&view(Granted, Denied), true, &mark));
        assert!(mark.shown());
        assert!(!open_guide_at_start(&view(Granted, Denied), true, &mark));
        assert!(!open_guide_at_start(&view(Granted, Denied), true, &mark));
        // A microphone that is still not answered opens it again.
        assert!(open_guide_at_start(
            &view(NotDetermined, Denied),
            true,
            &mark
        ));
        // The person who keeps no text in fields is never sent to Settings for Accessibility.
        let card_only = GuideMark::beside(&dir.path().join("other").join("settings.json"));
        assert!(!open_guide_at_start(
            &view(Granted, Denied),
            false,
            &card_only
        ));
        assert!(!card_only.shown());
    }

    #[test]
    fn the_guide_is_remembered_beside_the_settings_and_not_before_it_was_shown() {
        let dir = tempfile::tempdir().unwrap();
        // The folder of the settings is not there at the first start.
        let settings = dir.path().join("Dictate").join("settings.json");
        let mark = GuideMark::beside(&settings);
        assert!(!mark.shown());

        mark.set();

        assert!(mark.shown());
        // A later start reads it again from the disk.
        assert!(GuideMark::beside(&settings).shown());
        // Setting it twice is no trouble.
        mark.set();
        assert!(mark.shown());
        // It is not the settings file, which the window writes whole.
        assert!(!settings.exists());
    }

    #[test]
    fn request_permission_asks_the_system_for_the_one_that_was_named() {
        let platform = mac_with(Permission::NotDetermined, Permission::Denied);

        request_permission_now(&*platform, "microphone").unwrap();
        assert_eq!(platform.mic_requests.load(Ordering::SeqCst), 1);
        assert_eq!(platform.accessibility_requests.load(Ordering::SeqCst), 0);

        request_permission_now(&*platform, "accessibility").unwrap();
        assert_eq!(platform.mic_requests.load(Ordering::SeqCst), 1);
        assert_eq!(platform.accessibility_requests.load(Ordering::SeqCst), 1);

        // A name the window doesn't use is refused, and the system is not asked.
        let error = request_permission_now(&*platform, "camera").unwrap_err();
        assert!(error.contains("camera"), "{error}");
        assert_eq!(platform.mic_requests.load(Ordering::SeqCst), 1);
        assert_eq!(platform.accessibility_requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_permission_links_open_the_privacy_pane_of_each() {
        assert_eq!(
            permission_settings_url("microphone"),
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        );
        assert_eq!(
            permission_settings_url("accessibility"),
            Ok("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        );
        // The window can't make this command open any other link.
        assert!(permission_settings_url("https://example.com").is_err());
    }
}
