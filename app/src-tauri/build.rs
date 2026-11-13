/// Every command the app registers, with a permission made for each (`allow-get-settings`, ...). With this list the
/// app has an ACL of its own: a window may call a command only when its capability file names the permission. The
/// Settings window has the commands of `default.json`, the floating dictation windows those of `panel.json` and the
/// meetings window those of `meetings.json`.
/// `main.rs` registers the same names, and a test there checks that the two agree and that the files scope them.
const COMMANDS: &[&str] = &[
    // Settings.
    "get_settings",
    "save_settings",
    "set_ai_provider",
    "set_transcribe_preset",
    "set_hotkey",
    "hotkey_status",
    "hotkey_recording",
    "set_secret",
    "check_ai",
    "check_transcribe",
    "list_models",
    "download_model",
    "cancel_download",
    "delete_model",
    "models_disk_usage_mb",
    "edit_presets",
    "transcribe_presets",
    "open_meetings_folder",
    "permissions_status",
    "request_permission",
    "open_permission_settings",
    // Which tools of the system are installed. Settings asks it too: the one command it shares with the meetings window.
    "media_tools",
    // The meetings window.
    "meetings_list",
    "meeting_detail",
    "meetings_pick_files",
    "meetings_import_files",
    "meetings_import_link",
    "meeting_cancel",
    "meeting_rerun",
    "meeting_resummarize",
    "meeting_retry",
    "meeting_set_task",
    "meeting_rename_speakers",
    "meeting_ask",
    "meeting_markdown",
    "meeting_export",
    "meeting_log",
    "meeting_open_folder",
    "meeting_open_media",
    "meeting_delete",
    "copy_text",
    "meetings_open_settings",
    "meetings_readiness",
    // The recording, which the meetings window starts and stops (the bar of the recording has some of them too).
    "recording_sources",
    "recording_start",
    "recording_pause",
    "recording_resume",
    "recording_stop",
    "recording_discard",
    "recording_state",
    // The bar's own: its page tells how big its capsule is.
    "recording_bar_resized",
    // The bubble, the chip and the card.
    "dictation_languages",
    "dictation_stop",
    "dictation_cancel",
    "dictation_set_language",
    "dictation_rewrite",
    "dictation_custom",
    "dictation_open_card",
    "dictation_copy",
    "dictation_card_text",
    "dictation_undo",
    "dictation_paste_card",
    "dictation_chip_hover",
    "dictation_menu_open",
    "dictation_grant_accessibility",
    "dictation_open_settings",
    "panel_state",
    "panel_language",
    "panel_resized",
];

#[path = "build_rpath.rs"]
mod build_rpath;

fn main() {
    // The run path of the program and its tests (`build_rpath.rs` says what each part is for). Linux: the shared sherpa-onnx libraries,
    // which `sherpa-onnx-sys` and the build script of `dictate-services` copy into `target/<profile>` (the program is built there, the
    // tests in its `deps`), and which the installed package has in `/usr/lib/dictate`. macOS: the Swift runtime of the system, for the
    // ScreenCaptureKit bridge that `dictate-services` links (the bridge's own build script gives this run path only to the binaries of its
    // own package, which has none, and neither does the one of `dictate-services` reach this package). A build script's link arguments
    // reach only the binaries of its own package, so the libraries' build scripts cannot set this up for them.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    for arg in build_rpath::link_args(&target_os) {
        println!("cargo::rustc-link-arg={arg}");
    }
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("the Tauri build failed");
}
