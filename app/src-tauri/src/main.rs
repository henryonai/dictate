// A tray app: no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod hotkey;
// Nothing calls the platform layer yet: the dictation controller (Task 10 of the dictation plan) will.
#[allow(dead_code)]
mod platform;
mod state;
#[cfg(test)]
mod test_support;
mod tray;

use std::sync::Arc;
use std::time::Duration;

use dictate_services::secrets::KeyringStore;
use dictate_services::settings::SettingsStore;
use tauri::{Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use hotkey::HotkeyEvent;
use state::AppState;

/// How long quitting waits for downloads to write down what they have, so that they can be resumed.
const QUIT_WAIT: Duration = Duration::from_secs(3);

fn main() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    // What the shortcut and `dictate --toggle` hear goes down this channel, to the dictation controller.
    let (hotkey_tx, hotkey_rx) = unbounded_channel();
    let toggle_tx = hotkey_tx.clone();

    let builder = tauri::Builder::default()
        // A second launch ends here: `dictate --toggle` toggles dictation, anything else shows the window of the
        // first. This plugin must come first.
        .plugin(tauri_plugin_single_instance::init(
            move |app, args, _cwd| {
                if hotkey::wants_toggle(&args) {
                    log::info!("Another launch asked to toggle dictation");
                    let _ = toggle_tx.send(HotkeyEvent::Toggle);
                } else {
                    log::info!("Another launch was started: showing Settings");
                    tray::show_settings(app);
                }
            },
        ))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_opener::init());
    let builder = if hotkey::uses_global_plugin() {
        builder.plugin(tauri_plugin_global_shortcut::Builder::new().build())
    } else {
        builder
    };
    let app = builder
        .setup(move |app| setup(app, hotkey_tx, hotkey_rx))
        // Closing the window hides it: the app stays in the tray.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(e) = window.hide() {
                    log::warn!("Could not hide the window: {e}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::set_ai_provider,
            commands::set_transcribe_preset,
            commands::set_hotkey,
            commands::hotkey_status,
            commands::hotkey_recording,
            commands::set_secret,
            commands::check_ai,
            commands::check_transcribe,
            commands::list_models,
            commands::download_model,
            commands::cancel_download,
            commands::delete_model,
            commands::models_disk_usage_mb,
            commands::transcribe_wav,
            commands::cancel_transcribe,
            commands::edit_presets,
            commands::transcribe_presets,
            commands::open_meetings_folder,
        ])
        .build(tauri::generate_context!())
        .expect("the Dictate app could not start");

    app.run(|app, event| match event {
        // Opening the app again from Finder, Launchpad or Spotlight reaches the running one as this event, not as a
        // second process, so the single-instance plugin never hears of it. Settings is what a tray app shows then.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            log::info!("The app was opened again: showing Settings");
            tray::show_settings(app);
        }
        // Closing the last window doesn't end a tray app; Quit does, and it comes with an exit code.
        RunEvent::ExitRequested { api, code, .. } => {
            if code.is_none() {
                api.prevent_exit();
            }
        }
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<AppState>() {
                if !state.jobs.cancel_all_and_wait(QUIT_WAIT) {
                    log::warn!("Some work did not stop in time");
                }
            }
        }
        _ => {}
    });
}

fn setup(
    app: &mut tauri::App,
    hotkey_tx: UnboundedSender<HotkeyEvent>,
    hotkey_rx: UnboundedReceiver<HotkeyEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    // No Dock icon: the app lives in the menu bar.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    let settings_path =
        SettingsStore::default_path().ok_or("the system has no folder for the app's settings")?;
    // Without a settings file this is the first start: Settings opens so that a new user sees where to begin.
    let first_launch = !settings_path.exists();
    let (store, warning) = SettingsStore::open(settings_path);
    if let Some(warning) = &warning {
        log::warn!("{warning}");
    }
    let models_base =
        state::models_base_dir().ok_or("the system has no folder for the app's data")?;
    let models = state::open_model_dir(&models_base)?;
    let lang = tray::language_for(&store.get());
    let accel = store.get().hotkey;

    let state = AppState::new(store, warning, Arc::new(KeyringStore::new()), models);
    // Ask the key store which keys exist once, now and off this thread: a locked Keychain waits for the user. The
    // settings view waits for the answer, so it never asks again.
    let startup = state.clone();
    tauri::async_runtime::spawn_blocking(move || {
        startup.loaded_keys();
    });
    app.manage(state);
    tray::create(app.handle(), lang)?;

    let status = hotkey::start(app.handle(), &accel, hotkey_tx.clone());
    match &status.error {
        None => log::info!("The shortcut is {} ({:?})", status.label, status.backend),
        Some(error) => log::warn!("The shortcut {} does not work: {error}", status.label),
    }
    log_hotkey_events(hotkey_rx);
    // `dictate --toggle` that starts the app: dictation starts as soon as the controller is there to hear it.
    if hotkey::wants_toggle(&std::env::args().collect::<Vec<_>>()) {
        log::info!("Started with --toggle: toggling dictation");
        let _ = hotkey_tx.send(HotkeyEvent::Toggle);
    }

    if first_launch {
        log::info!("First launch: showing Settings");
        tray::show_settings(app.handle());
    }
    Ok(())
}

/// Until the dictation controller exists (Task 10 of the dictation plan), what the shortcut hears is only logged.
fn log_hotkey_events(mut events: UnboundedReceiver<HotkeyEvent>) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            log::info!("Hotkey event: {event:?}");
        }
    });
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    /// What the settings window may ask of the opener plugin, from the capability file.
    fn opener_permissions() -> Vec<Value> {
        let capability: Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|permission| {
                let name = permission
                    .as_str()
                    .or_else(|| permission["identifier"].as_str());
                name.is_some_and(|name| name.starts_with("opener:"))
            })
            .cloned()
            .collect()
    }

    #[test]
    fn the_window_may_open_https_pages_and_nothing_else_of_the_opener() {
        // "Get a key" opens a page in the browser. Folders are opened by the Rust side, which needs no permission.
        assert_eq!(
            opener_permissions(),
            [json!({ "identifier": "opener:allow-open-url", "allow": [{ "url": "https://*" }] })]
        );
    }
}
