// A tray app: no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod dictation;
mod hotkey;
mod panel;
// Fixture states for the panels, to look at them without the controller. A debug build only.
#[cfg(debug_assertions)]
mod panel_demo;
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

use dictation::system::DictationHandle;
use hotkey::HotkeyEvent;
use state::AppState;

/// The app's configuration and its permissions, as compiled in. One function holds the only `generate_context!`, which
/// can't be expanded twice in a program: the tests build an app with the real permissions from it too.
fn context<R: tauri::Runtime>() -> tauri::Context<R> {
    tauri::generate_context!()
}

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
    // The bubble is a panel of AppKit: this is what lets Tauri turn its window into one.
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
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
                // A floating window hidden like this is not shown any more, and the next state shows it again.
                panel::window_closed(window.app_handle(), window.label());
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
            panel::dictation_languages,
            panel::dictation_stop,
            panel::dictation_cancel,
            panel::dictation_set_language,
            panel::dictation_rewrite,
            panel::dictation_custom,
            panel::dictation_open_card,
            panel::dictation_copy,
            panel::dictation_card_text,
            panel::dictation_undo,
            panel::dictation_paste_card,
            panel::dictation_chip_hover,
            panel::dictation_menu_open,
            panel::dictation_grant_accessibility,
            panel::dictation_open_settings,
            panel::panel_state,
            panel::panel_language,
            panel::panel_resized,
        ])
        .build(context())
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
    let initial = store.get();
    let lang = tray::language_for(&initial);
    let hotkey = tray::hotkey_label(&initial);
    let accel = initial.hotkey;

    let state = AppState::new(store, warning, Arc::new(KeyringStore::new()), models);
    let startup = state.clone();
    app.manage(state);
    // The panels may ask for the controller before it is built: it waits for the key store (a locked Keychain waits for
    // the person), and the handle is there from the start.
    let dictation = DictationHandle::default();
    app.manage(dictation.clone());
    tray::create(app.handle(), lang, &hotkey)?;
    panel::create(app.handle())?;
    // A debug build can cycle fixture states through the panels, to look at them without the controller.
    #[cfg(debug_assertions)]
    panel_demo::start(app.handle());

    let status = hotkey::start(app.handle(), &accel, hotkey_tx.clone());
    match &status.error {
        None => log::info!("The shortcut is {} ({:?})", status.label, status.backend),
        Some(error) => log::warn!("The shortcut {} does not work: {error}", status.label),
    }
    // What the shortcut, the tray and `dictate --toggle` say goes to the controller as soon as there is one; until then it
    // waits in the channel, `--toggle` of this very launch included.
    dictation::system::start(app.handle().clone(), startup, dictation, hotkey_rx);
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::{json, Value};
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{mock_builder, MockRuntime, INVOKE_KEY};
    use tauri::webview::InvokeRequest;
    use tauri::{App, WebviewUrl, WebviewWindowBuilder};

    /// The commands of the Settings window.
    const SETTINGS_COMMANDS: [&str; 20] = [
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
    ];

    /// The commands of the two floating windows.
    const PANEL_COMMANDS: [&str; 18] = [
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

    /// An app with the real permissions, in which every command that gets past them answers "ok": what is tested is
    /// who may ask, not what the commands do.
    fn app() -> App<MockRuntime> {
        let app = mock_builder()
            .invoke_handler(|invoke| {
                invoke.resolver.resolve("ok");
                true
            })
            .build(super::context())
            .expect("the app could not be built with its permissions");
        for label in ["settings", "bubble", "card", "stranger"] {
            WebviewWindowBuilder::new(&app, label, WebviewUrl::default())
                .build()
                .expect("a window");
        }
        app
    }

    /// What happens when the page of the window `label` calls `command`.
    fn asked_by(app: &App<MockRuntime>, label: &str, command: &str) -> Result<(), String> {
        use tauri::Manager;
        let window = app.get_webview_window(label).expect("the window");
        let request = InvokeRequest {
            cmd: command.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: if cfg!(windows) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .unwrap(),
            body: InvokeBody::default(),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        };
        tauri::test::get_ipc_response(&window, request)
            .map(|_| ())
            .map_err(|refusal| refusal.to_string())
    }

    #[test]
    fn the_settings_window_may_ask_for_what_settings_uses_and_for_nothing_of_the_panels() {
        let app = app();
        for command in SETTINGS_COMMANDS {
            assert_eq!(asked_by(&app, "settings", command), Ok(()), "{command}");
        }
        for command in PANEL_COMMANDS {
            let refusal = asked_by(&app, "settings", command).expect_err(command);
            assert!(refusal.contains("not allowed"), "{command}: {refusal}");
        }
    }

    #[test]
    fn the_panel_windows_may_ask_for_the_panel_commands_and_for_nothing_else() {
        let app = app();
        for window in ["bubble", "card"] {
            for command in PANEL_COMMANDS {
                assert_eq!(
                    asked_by(&app, window, command),
                    Ok(()),
                    "{window} {command}"
                );
            }
            // The keys, the settings and the downloads are not theirs: a page that went wrong can't reach them.
            for command in SETTINGS_COMMANDS {
                let refusal = asked_by(&app, window, command).expect_err(command);
                assert!(
                    refusal.contains("not allowed"),
                    "{window} {command}: {refusal}"
                );
            }
        }
    }

    #[test]
    fn a_window_no_capability_names_may_ask_for_nothing() {
        let app = app();
        for command in SETTINGS_COMMANDS.into_iter().chain(PANEL_COMMANDS) {
            assert!(asked_by(&app, "stranger", command).is_err(), "{command}");
        }
    }

    /// The names between quotes in the first list of strings after `marker`.
    fn quoted_after(source: &str, marker: &str, end: char) -> BTreeSet<String> {
        let from = source.find(marker).expect(marker) + marker.len();
        let list = &source[from..from + source[from..].find(end).expect("the end of the list")];
        list.split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn every_command_of_the_app_is_in_the_manifest_that_scopes_it_and_in_one_list_of_these_tests() {
        let wanted: BTreeSet<String> = SETTINGS_COMMANDS
            .into_iter()
            .chain(PANEL_COMMANDS)
            .map(str::to_owned)
            .collect();
        let manifest = quoted_after(include_str!("../build.rs"), "COMMANDS: &[&str] = &[", ']');
        assert_eq!(manifest, wanted, "build.rs");

        // What the app registers: `commands::name,` and `panel::name,` in `generate_handler!`.
        let source = include_str!("main.rs");
        let from = source.find("generate_handler![").unwrap();
        let handler = &source[from..from + source[from..].find(']').unwrap()];
        let registered: BTreeSet<String> = handler
            .lines()
            .filter_map(|line| line.trim().strip_suffix(','))
            .filter_map(|path| path.rsplit("::").next())
            .map(str::to_owned)
            .collect();
        assert_eq!(registered, wanted, "generate_handler!");
    }

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
