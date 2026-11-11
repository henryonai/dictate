//! The commands of the recorder, which the meetings window asks (and the bar, which has `recording_pause`, `recording_resume`,
//! `recording_stop`, `recording_discard` and `recording_state`). Each is in `capabilities/meetings.json` and in the manifest of
//! `build.rs`.
//!
//! What can take long (starting, stopping, asking the system what can be recorded) is done on a blocking thread.

use dictate_services::record::RecordOptions;
use tauri::State;

use super::{Recorder, RecordingView, SourcesView};
use crate::commands::blocking;
use crate::meetings::views::ErrorView;

/// What the picker offers, and what it had last.
#[tauri::command]
pub async fn recording_sources(recorder: State<'_, Recorder>) -> Result<SourcesView, String> {
    let recorder = recorder.inner().clone();
    blocking(move || recorder.sources()).await
}

/// Starts a recording. Answers when it has begun, which can be as long as a person takes to answer the desktop.
#[tauri::command]
pub async fn recording_start(
    recorder: State<'_, Recorder>,
    options: RecordOptions,
) -> Result<(), ErrorView> {
    let recorder = recorder.inner().clone();
    blocking(move || recorder.start(options))
        .await
        .map_err(ErrorView::other)?
}

#[tauri::command]
pub async fn recording_pause(recorder: State<'_, Recorder>) -> Result<(), String> {
    recorder.pause();
    Ok(())
}

#[tauri::command]
pub async fn recording_resume(recorder: State<'_, Recorder>) -> Result<(), String> {
    recorder.resume();
    Ok(())
}

/// Ends the recording and starts its meeting. Answers when the recording is put together.
#[tauri::command]
pub async fn recording_stop(recorder: State<'_, Recorder>) -> Result<(), ErrorView> {
    let recorder = recorder.inner().clone();
    blocking(move || recorder.stop())
        .await
        .map_err(ErrorView::other)?
}

/// Throws the recording away.
#[tauri::command]
pub async fn recording_discard(recorder: State<'_, Recorder>) -> Result<(), ErrorView> {
    let recorder = recorder.inner().clone();
    blocking(move || recorder.discard())
        .await
        .map_err(ErrorView::other)?
}

/// How the recording is now: what a window that opens in the middle of one shows.
#[tauri::command]
pub async fn recording_state(recorder: State<'_, Recorder>) -> Result<RecordingView, String> {
    Ok(recorder.view())
}
