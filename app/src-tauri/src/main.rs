// A tray app: no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
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

use state::AppState;

/// How long quitting waits for downloads to write down what they have, so that they can be resumed.
const QUIT_WAIT: Duration = Duration::from_secs(3);

fn main() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    let app = tauri::Builder::default()
        // A second launch ends here and shows the window of the first. This plugin must come first.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            log::info!("Another launch was started: showing Settings");
            tray::show_settings(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_opener::init())
        .setup(setup)
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

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
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

    let state = AppState::new(store, warning, Arc::new(KeyringStore::new()), models);
    // Ask the key store which keys exist once, now and off this thread: a locked Keychain waits for the user. The
    // settings view waits for the answer, so it never asks again.
    let startup = state.clone();
    tauri::async_runtime::spawn_blocking(move || {
        startup.loaded_keys();
    });
    app.manage(state);
    tray::create(app.handle(), lang)?;
    if first_launch {
        log::info!("First launch: showing Settings");
        tray::show_settings(app.handle());
    }
    Ok(())
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
