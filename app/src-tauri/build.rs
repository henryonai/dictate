/// Every command the app registers, with a permission made for each (`allow-get-settings`, ...). With this list the
/// app has an ACL of its own: a window may call a command only when its capability file names the permission. The
/// Settings window has the commands of `default.json`, the floating dictation windows those of `panel.json`.
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
    "transcribe_wav",
    "cancel_transcribe",
    "edit_presets",
    "transcribe_presets",
    "open_meetings_folder",
    "permissions_status",
    "request_permission",
    "open_permission_settings",
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

fn main() {
    // Linux: the program and its tests have to find the shared sherpa-onnx library that `sherpa-onnx-sys` and the build
    // script of `dictate-services` copy into `target/<profile>` (the program is built there, the tests in its `deps`).
    // A build script's link arguments reach only the binaries of its own package, so `dictate-services` cannot set this
    // up for them.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo::rustc-link-arg=-Wl,-rpath,$ORIGIN");
        println!("cargo::rustc-link-arg=-Wl,-rpath,$ORIGIN/..");
    }
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("the Tauri build failed");
}
