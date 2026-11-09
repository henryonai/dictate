//! The commands of the meetings window. Each is a thin wrapper over [`Meetings`] (which a test can reach) that runs it on a
//! blocking thread; what needs a window, the dialogs and the opener, is opened here from Rust: the page never holds a
//! path of the disk or a handle to the file manager.
//!
//! Every command named here is in `capabilities/meetings.json` (the one shared with Settings, `media_tools`, is in
//! `default.json` too) and in the manifest of `build.rs`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dictate_services::media::tools::{self, Tools};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;

use super::backend::{ai_ready, whisper_ready};
use super::library::{is_settings_tab, Meetings};
use super::notify;
use super::views::{ErrorView, MeetingDetail, MeetingRow};
use crate::commands::blocking;
use crate::platform::paste::Clipboard;
use crate::state::AppState;
use crate::tray;

/// The event that tells the Settings window to show another of its pages.
pub const SETTINGS_TAB_EVENT: &str = "settings-tab";

/// The extensions of what the file dialog offers first: the audio and video that a recording is made of. The decoder reads
/// what it can and asks ffmpeg for the rest; "All files" is offered too.
const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "3gp", "ts", "m4a",
    "mp3", "wav", "aac", "flac", "ogg", "oga", "opus", "wma", "aiff", "aif", "caf", "amr",
];

/// How long `ffmpeg -version` and `yt-dlp --version` may take.
const VERSION_LIMIT: Duration = Duration::from_secs(10);

/// What the empty state of the window and its "no summary" texts need to know of the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    /// The AI model that writes summaries is set up.
    pub ai: bool,
    /// The Whisper model of this computer is downloaded.
    pub whisper: bool,
}

/// Which of the tools of the system are installed, with their versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsView {
    pub ffmpeg: Option<String>,
    pub yt_dlp: Option<String>,
}

/// What the settings and the saved keys say. The key flags are read once if that has not been done (call it on a blocking
/// thread: that may wait for the person to answer the key store).
pub fn readiness(state: &AppState, ram_gb: u64) -> Readiness {
    let settings = state.settings.get();
    Readiness {
        ai: ai_ready(&settings, state.loaded_keys()),
        whisper: whisper_ready(&settings, &state.models, ram_gb),
    }
}

/// The tools that `tools` found, and the version each says it is. A tool that is there and does not say gets "unknown".
pub async fn tools_view(tools: &Tools) -> ToolsView {
    ToolsView {
        ffmpeg: match &tools.ffmpeg {
            Some(path) => Some(version_of(path, "-version", ffmpeg_version).await),
            None => None,
        },
        yt_dlp: match &tools.yt_dlp {
            Some(path) => Some(version_of(path, "--version", yt_dlp_version).await),
            None => None,
        },
    }
}

/// Asks `program` for its version with `flag` and reads it from the first line it prints with `parse`.
async fn version_of(program: &Path, flag: &str, parse: fn(&str) -> Option<String>) -> String {
    let first = std::sync::Mutex::new(None::<String>);
    let ran = tokio::time::timeout(
        VERSION_LIMIT,
        tools::run(program, &[flag.into()], &CancellationToken::new(), |line| {
            let mut first = first
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if first.is_none() {
                *first = Some(line.to_owned());
            }
        }),
    )
    .await;
    let line = first
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match (ran, line) {
        (Ok(Ok(_)), Some(line)) => parse(&line).unwrap_or_else(|| "unknown".to_owned()),
        _ => "unknown".to_owned(),
    }
}

/// "ffmpeg version 6.1.1 Copyright (c) 2000-2023 the FFmpeg developers" is `6.1.1`.
fn ffmpeg_version(line: &str) -> Option<String> {
    let mut words = line.split_whitespace();
    words.find(|word| word.eq_ignore_ascii_case("version"))?;
    words.next().map(str::to_owned)
}

/// yt-dlp prints its version, such as `2025.01.15`, and nothing else.
fn yt_dlp_version(line: &str) -> Option<String> {
    let version = line.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

/// The copy of the text that the page asks for goes to the clipboard of the system.
pub struct TextClipboard(pub Arc<dyn Clipboard>);

// ---- the commands

#[tauri::command]
pub async fn meetings_list(
    meetings: State<'_, Meetings>,
    query: String,
) -> Result<Vec<MeetingRow>, String> {
    let meetings = meetings.inner().clone();
    blocking(move || meetings.list(&query)).await
}

#[tauri::command]
pub async fn meeting_detail(
    meetings: State<'_, Meetings>,
    id: String,
) -> Result<MeetingDetail, String> {
    let meetings = meetings.inner().clone();
    blocking(move || meetings.detail(&id)).await?
}

/// Opens the dialog for the files to import (the window of the system is never the page's), and imports each. The ids of the
/// recordings made; none when the person closed the dialog.
#[tauri::command]
pub async fn meetings_pick_files(
    app: AppHandle,
    meetings: State<'_, Meetings>,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let meetings = meetings.inner().clone();
    let lang = tray::language_for(&state.settings.get());
    blocking(move || {
        let picked = app
            .dialog()
            .file()
            .set_title(notify::text(lang, "meetings.dialog.pick_title", &[]))
            .add_filter(
                notify::text(lang, "meetings.dialog.filter_media", &[]),
                MEDIA_EXTENSIONS,
            )
            .add_filter(
                notify::text(lang, "meetings.dialog.filter_all", &[]),
                &["*"],
            )
            .blocking_pick_files();
        let paths: Vec<String> = picked
            .unwrap_or_default()
            .into_iter()
            .filter_map(|file| file.into_path().ok())
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        meetings.import_files(&paths)
    })
    .await
}

/// For the files that were dropped on the window.
#[tauri::command]
pub async fn meetings_import_files(
    meetings: State<'_, Meetings>,
    paths: Vec<String>,
) -> Result<Vec<String>, String> {
    let meetings = meetings.inner().clone();
    blocking(move || meetings.import_files(&paths)).await
}

#[tauri::command]
pub async fn meetings_import_link(
    meetings: State<'_, Meetings>,
    url: String,
) -> Result<String, ErrorView> {
    let meetings = meetings.inner().clone();
    blocking(move || meetings.import_link(&url))
        .await
        .map_err(ErrorView::other)?
}

#[tauri::command]
pub async fn meeting_cancel(meetings: State<'_, Meetings>, id: String) -> Result<(), String> {
    meetings.cancel(&id);
    Ok(())
}

/// "Transcribe + summarize again from scratch".
#[tauri::command]
pub async fn meeting_rerun(meetings: State<'_, Meetings>, id: String) -> Result<(), String> {
    let meetings = meetings.inner().clone();
    blocking(move || meetings.rerun(&id)).await?
}

/// "Summarize again".
#[tauri::command]
pub async fn meeting_resummarize(meetings: State<'_, Meetings>, id: String) -> Result<(), String> {
    let meetings = meetings.inner().clone();
    blocking(move || meetings.resummarize(&id)).await?
}

/// "Try again", from the step that failed.
#[tauri::command]
pub async fn meeting_retry(meetings: State<'_, Meetings>, id: String) -> Result<(), String> {
    let meetings = meetings.inner().clone();
    blocking(move || meetings.retry(&id)).await?
}

#[tauri::command]
pub async fn meeting_set_task(
    meetings: State<'_, Meetings>,
    id: String,
    index: usize,
    done: bool,
) -> Result<(), String> {
    let meetings = meetings.inner().clone();
    blocking(move || meetings.set_task(&id, index, done)).await?
}

/// Gives speakers new names (old name to new). The renames that were done, sorted. Refused while the recording has a job.
#[tauri::command]
pub async fn meeting_rename_speakers(
    meetings: State<'_, Meetings>,
    id: String,
    names: HashMap<String, String>,
    resummarize: bool,
) -> Result<Vec<(String, String)>, ErrorView> {
    let meetings = meetings.inner().clone();
    blocking(move || meetings.rename_speakers(&id, &names, resummarize))
        .await
        .map_err(ErrorView::other)?
}

#[tauri::command]
pub async fn meeting_ask(
    meetings: State<'_, Meetings>,
    state: State<'_, AppState>,
    id: String,
    question: String,
) -> Result<String, ErrorView> {
    meetings.ask(&state, &id, &question).await
}

#[tauri::command]
pub async fn meeting_markdown(meetings: State<'_, Meetings>, id: String) -> Result<String, String> {
    let meetings = meetings.inner().clone();
    blocking(move || meetings.markdown(&id)).await?
}

/// Asks where to save the Markdown of the recording and writes it there. Whether it was saved: `false` when the person
/// closed the dialog.
#[tauri::command]
pub async fn meeting_export(
    app: AppHandle,
    meetings: State<'_, Meetings>,
    state: State<'_, AppState>,
    id: String,
) -> Result<bool, ErrorView> {
    let meetings = meetings.inner().clone();
    let lang = tray::language_for(&state.settings.get());
    blocking(move || {
        let name = meetings.export_name(&id).map_err(ErrorView::other)?;
        let chosen = app
            .dialog()
            .file()
            .set_title(notify::text(lang, "meetings.dialog.export_title", &[]))
            .set_file_name(name)
            .add_filter(
                notify::text(lang, "meetings.dialog.filter_markdown", &[]),
                &["md"],
            )
            .blocking_save_file()
            .and_then(|file| file.into_path().ok());
        match chosen {
            Some(destination) => meetings.export_to(&id, &destination).map(|()| true),
            None => Ok(false),
        }
    })
    .await
    .map_err(ErrorView::other)?
}

#[tauri::command]
pub async fn meeting_log(meetings: State<'_, Meetings>, id: String) -> Result<String, String> {
    let meetings = meetings.inner().clone();
    blocking(move || meetings.log(&id)).await?
}

/// Opens the recording's folder in the file manager.
#[tauri::command]
pub async fn meeting_open_folder(
    app: AppHandle,
    meetings: State<'_, Meetings>,
    id: String,
) -> Result<(), String> {
    let meetings = meetings.inner().clone();
    blocking(move || open(&app, &meetings.folder_path(&id)?)).await?
}

/// Opens the recording's video or audio in the program of the system for it.
#[tauri::command]
pub async fn meeting_open_media(
    app: AppHandle,
    meetings: State<'_, Meetings>,
    id: String,
) -> Result<(), String> {
    let meetings = meetings.inner().clone();
    blocking(move || open(&app, &meetings.media_path(&id)?)).await?
}

fn open(app: &AppHandle, path: &Path) -> Result<(), String> {
    app.opener()
        .open_path(path.to_string_lossy().into_owned(), None::<String>)
        .map_err(|e| format!("Could not open {}: {e}", path.display()))
}

/// Stops the recording's job and moves the recording to the Trash.
#[tauri::command]
pub async fn meeting_delete(meetings: State<'_, Meetings>, id: String) -> Result<(), String> {
    meetings.delete(&id).await
}

/// Puts the text on the clipboard.
#[tauri::command]
pub async fn copy_text(clipboard: State<'_, TextClipboard>, text: String) -> Result<(), String> {
    let clipboard = clipboard.0.clone();
    blocking(move || clipboard.copy(&text)).await?
}

/// Shows Settings on one of its pages: `general`, `speech`, `ai` or `accounts`.
#[tauri::command]
pub async fn meetings_open_settings(app: AppHandle, tab: String) -> Result<(), String> {
    if !is_settings_tab(&tab) {
        return Err(format!("There is no settings page called {tab}."));
    }
    tray::show_settings(&app);
    app.emit_to("settings", SETTINGS_TAB_EVENT, tab)
        .map_err(|e| format!("Could not show the page: {e}"))
}

#[tauri::command]
pub async fn meetings_readiness(state: State<'_, AppState>) -> Result<Readiness, String> {
    let state = state.inner().clone();
    blocking(move || readiness(&state, dictate_services::system::total_ram_gb())).await
}

/// Which tools are installed (ffmpeg, yt-dlp) and their versions.
#[tauri::command]
pub async fn media_tools() -> Result<ToolsView, String> {
    let found = blocking(Tools::find).await?;
    Ok(tools_view(&found).await)
}

#[cfg(test)]
mod tests {
    use dictate_services::secrets::{Account, MemoryStore};
    use dictate_services::settings::SettingsStore;
    use dictate_services::whisper::models::{self, ModelDir};

    use super::*;
    use crate::test_support::script;

    fn state(dir: &Path) -> AppState {
        let (store, _) = SettingsStore::open(dir.join("settings.json"));
        let models = ModelDir::new(dir.join("models"));
        std::fs::create_dir_all(models.root()).unwrap();
        AppState::new(store, None, Arc::new(MemoryStore::default()), models)
    }

    #[test]
    fn readiness_says_whether_the_ai_and_the_whisper_model_are_there() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(dir.path());
        // The flags are read once, from the key store: no key is saved.
        assert_eq!(
            readiness(&state, 16),
            Readiness {
                ai: false,
                whisper: false
            }
        );

        state.keys.set(Account::Ai, true);
        std::fs::write(state.models.path(models::recommended(16)), b"a model").unwrap();
        assert_eq!(
            readiness(&state, 16),
            Readiness {
                ai: true,
                whisper: true
            }
        );
    }

    #[test]
    fn readiness_is_sent_in_camel_case() {
        let json = serde_json::to_value(Readiness {
            ai: true,
            whisper: false,
        })
        .unwrap();
        assert_eq!(json, serde_json::json!({ "ai": true, "whisper": false }));
        let json = serde_json::to_value(ToolsView {
            ffmpeg: Some("7.1".into()),
            yt_dlp: None,
        })
        .unwrap();
        assert_eq!(json, serde_json::json!({ "ffmpeg": "7.1", "ytDlp": null }));
    }

    #[test]
    fn the_version_is_read_from_the_first_line_each_tool_prints() {
        assert_eq!(
            ffmpeg_version("ffmpeg version 6.1.1 Copyright (c) 2000-2023 the FFmpeg developers")
                .as_deref(),
            Some("6.1.1")
        );
        assert_eq!(
            ffmpeg_version(
                "ffmpeg version N-117345-g6f0bd51b1f-tessus  https://evermeet.cx/ffmpeg/"
            )
            .as_deref(),
            Some("N-117345-g6f0bd51b1f-tessus")
        );
        assert_eq!(ffmpeg_version("something else"), None);
        assert_eq!(
            yt_dlp_version("  2025.01.15 \n").as_deref(),
            Some("2025.01.15")
        );
        assert_eq!(yt_dlp_version("  "), None);
    }

    #[tokio::test]
    async fn the_tools_view_names_each_tool_that_is_there_with_its_version() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = script(
            dir.path(),
            "ffmpeg",
            "echo 'ffmpeg version 6.1.1 Copyright (c) 2000-2023 the FFmpeg developers'",
            "echo ffmpeg version 6.1.1 Copyright (c) 2000-2023 the FFmpeg developers",
        );
        let yt_dlp = script(dir.path(), "yt-dlp", "echo 2025.01.15", "echo 2025.01.15");

        let both = tools_view(&Tools {
            ffmpeg: Some(ffmpeg.clone()),
            yt_dlp: Some(yt_dlp),
        })
        .await;
        assert_eq!(
            both,
            ToolsView {
                ffmpeg: Some("6.1.1".into()),
                yt_dlp: Some("2025.01.15".into())
            }
        );

        // One that is missing is none; one that is there and says nothing sensible is "unknown".
        let silent = script(dir.path(), "silent", "exit 0", "exit /b 0");
        let view = tools_view(&Tools {
            ffmpeg: None,
            yt_dlp: Some(silent),
        })
        .await;
        assert_eq!(
            view,
            ToolsView {
                ffmpeg: None,
                yt_dlp: Some("unknown".into())
            }
        );
        let view = tools_view(&Tools {
            ffmpeg: Some(dir.path().join("no such program")),
            yt_dlp: None,
        })
        .await;
        assert_eq!(view.ffmpeg.as_deref(), Some("unknown"));
        let _ = ffmpeg;
    }
}
