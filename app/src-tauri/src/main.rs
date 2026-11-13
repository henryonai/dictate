// A tray app: no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod dictation;
mod hotkey;
mod meetings;
mod panel;
// Fixture states for the panels, to look at them without the controller. A debug build only.
#[cfg(debug_assertions)]
mod panel_demo;
mod platform;
mod recording;
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
use recording::Recorder;
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
        // A second launch ends here: `dictate --toggle` toggles dictation, `dictate --record` starts or stops a recording, anything
        // else shows the window of the first. This plugin must come first.
        .plugin(tauri_plugin_single_instance::init(
            move |app, args, _cwd| {
                let (toggles, records) = (hotkey::wants_toggle(&args), hotkey::wants_record(&args));
                if toggles {
                    log::info!("Another launch asked to toggle dictation");
                    let _ = toggle_tx.send(HotkeyEvent::Toggle);
                }
                if records {
                    log::info!("Another launch asked to start or stop a recording");
                    recording::toggle(app);
                }
                if !toggles && !records {
                    log::info!("Another launch was started: showing Settings");
                    tray::show_settings(app);
                }
            },
        ))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_opener::init())
        // The dialogs and the banners of the meetings: opened from Rust, and not by any page (no capability gives them).
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init());
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
                // A hidden web view goes on playing: the meetings page pauses its player.
                meetings::window::window_hidden(window.app_handle(), window.label());
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
            commands::edit_presets,
            commands::transcribe_presets,
            commands::open_meetings_folder,
            commands::permissions_status,
            commands::request_permission,
            commands::open_permission_settings,
            meetings::commands::media_tools,
            meetings::commands::meetings_list,
            meetings::commands::meeting_detail,
            meetings::commands::meetings_pick_files,
            meetings::commands::meetings_import_files,
            meetings::commands::meetings_import_link,
            meetings::commands::meeting_cancel,
            meetings::commands::meeting_rerun,
            meetings::commands::meeting_resummarize,
            meetings::commands::meeting_retry,
            meetings::commands::meeting_set_task,
            meetings::commands::meeting_rename_speakers,
            meetings::commands::meeting_ask,
            meetings::commands::meeting_markdown,
            meetings::commands::meeting_export,
            meetings::commands::meeting_log,
            meetings::commands::meeting_open_folder,
            meetings::commands::meeting_open_media,
            meetings::commands::meeting_delete,
            meetings::commands::copy_text,
            meetings::commands::meetings_open_settings,
            meetings::commands::meetings_readiness,
            recording::commands::recording_sources,
            recording::commands::recording_start,
            recording::commands::recording_pause,
            recording::commands::recording_resume,
            recording::commands::recording_stop,
            recording::commands::recording_discard,
            recording::commands::recording_state,
            recording::commands::recording_bar_resized,
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
            } else if let Some(recorder) = app.try_state::<Recorder>() {
                // A recording that goes on is put together before the app goes; the exit is asked for again when that is done,
                // and finds nothing to wait for.
                let handle = app.clone();
                let waits =
                    recording::before_exit(&recorder, recording::QUIT_WAIT, code, move |code| {
                        handle.exit(code);
                    });
                if waits {
                    log::info!("Quitting: finishing the recording first");
                    api.prevent_exit();
                }
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
    let guide_mark = commands::GuideMark::beside(&settings_path);
    let (store, warning) = SettingsStore::open(settings_path);
    if let Some(warning) = &warning {
        log::warn!("{warning}");
    }
    let models_base =
        state::models_base_dir().ok_or("the system has no folder for the app's data")?;
    let models = state::open_model_dir(&models_base)?;
    let initial = store.get();
    let auto_write = initial.auto_write;
    let lang = tray::language_for(&initial);
    let hotkey = tray::hotkey_label(&initial);
    let accel = initial.hotkey;

    let diarize_dir = state::open_diarize_dir(&models_base)?;
    let state = AppState::new(store, warning, Arc::new(KeyringStore::new()), models);
    let startup = state.clone();
    app.manage(state);
    // One platform layer for the whole app. What the system says about the permissions is read now, to know whether
    // the person needs the guide in Settings (macOS only: the other systems have no permission to grant).
    let platform = platform::SystemPlatform(platform::current(app.handle()));
    let permissions = commands::permissions_view(&*platform.0);
    app.manage(platform);
    // The panels may ask for the controller before it is built: it waits for the key store (a locked Keychain waits for
    // the person), and the handle is there from the start.
    let dictation = DictationHandle::default();
    app.manage(dictation.clone());
    tray::create(app.handle(), lang, &hotkey)?;
    panel::create(app.handle())?;
    // The meetings: their jobs wait until the folders are cleared and the keys are read, off this thread.
    meetings::start(app.handle(), &startup, diarize_dir);
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
    let args = std::env::args().collect::<Vec<_>>();
    if hotkey::wants_toggle(&args) {
        log::info!("Started with --toggle: toggling dictation");
        let _ = hotkey_tx.send(HotkeyEvent::Toggle);
    }
    // The recorder is there since the meetings started.
    if hotkey::wants_record(&args) {
        log::info!("Started with --record: starting a recording");
        recording::toggle(app.handle());
    }

    if first_launch {
        log::info!("First launch: showing Settings");
        // This is the guide too: a missing Accessibility is not offered again at the next start.
        guide_mark.set();
        tray::show_settings(app.handle());
    } else if commands::open_guide_at_start(&permissions, auto_write, &guide_mark) {
        // The General page has the Permissions section: where to turn on what dictation needs. It opens until the
        // microphone has been answered, and once for a missing Accessibility (only when text is written into fields):
        // someone who works with the card alone is not sent there at every login.
        log::info!("A permission is missing: showing Settings");
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
    const SETTINGS_COMMANDS: [&str; 22] = [
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
        // Which tools of the system are installed: the one command the meetings window shares with Settings.
        "media_tools",
    ];

    /// The commands of the meetings window.
    const MEETINGS_COMMANDS: [&str; 30] = [
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
        "media_tools",
        // The pane of System Settings where Screen Recording is turned on: what the picker's button and the error of a start that was
        // refused for the permission open.
        "open_permission_settings",
        // The recording: what the picker offers, and starting, pausing, stopping and throwing away.
        "recording_sources",
        "recording_start",
        "recording_pause",
        "recording_resume",
        "recording_stop",
        "recording_discard",
        "recording_state",
    ];

    /// The commands of the recorder, which are in the list of the meetings window too.
    const RECORDING_COMMANDS: [&str; 7] = [
        "recording_sources",
        "recording_start",
        "recording_pause",
        "recording_resume",
        "recording_stop",
        "recording_discard",
        "recording_state",
    ];

    /// The commands of the recording bar: what its buttons ask, and the state it draws. Starting a recording and what the picker
    /// offers are the meetings window's.
    const RECORDER_COMMANDS: [&str; 5] = [
        "recording_pause",
        "recording_resume",
        "recording_stop",
        "recording_discard",
        "recording_state",
    ];

    /// The commands that Settings and the meetings window both ask: the tools of the system, and the panes of System Settings.
    const SHARED_COMMANDS: [&str; 2] = ["media_tools", "open_permission_settings"];

    /// The one command that the recording bar shares with the floating dictation windows: the language of the UI, which it can't read
    /// from the settings.
    const BAR_SHARED_COMMAND: &str = "panel_language";

    /// The command that only the recording bar has: its page tells how big its capsule is, and the window is sized to it.
    const BAR_ONLY_COMMAND: &str = "recording_bar_resized";

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
        for label in [
            "settings", "bubble", "card", "meetings", "recorder", "stranger",
        ] {
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
        // The library is the meetings window's: Settings has only the tools of the system and the panes of System Settings in common
        // with it.
        for command in MEETINGS_COMMANDS
            .into_iter()
            .filter(|c| !SHARED_COMMANDS.contains(c))
        {
            let refusal = asked_by(&app, "settings", command).expect_err(command);
            assert!(refusal.contains("not allowed"), "{command}: {refusal}");
        }
        // The dialogs and the notifications are Rust's: no page of the app is given them.
        for command in [
            "plugin:dialog|open",
            "plugin:dialog|save",
            "plugin:notification|notify",
        ] {
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
            for command in SETTINGS_COMMANDS.into_iter().chain(MEETINGS_COMMANDS) {
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
        for command in SETTINGS_COMMANDS
            .into_iter()
            .chain(PANEL_COMMANDS)
            .chain(MEETINGS_COMMANDS)
            .chain([BAR_ONLY_COMMAND])
        {
            assert!(asked_by(&app, "stranger", command).is_err(), "{command}");
        }
    }

    #[test]
    fn the_meetings_window_may_ask_for_its_commands_and_for_nothing_of_settings_or_the_panels() {
        let app = app();
        for command in MEETINGS_COMMANDS {
            assert_eq!(asked_by(&app, "meetings", command), Ok(()), "{command}");
        }
        let others = SETTINGS_COMMANDS
            .into_iter()
            .chain(PANEL_COMMANDS)
            .filter(|command| !MEETINGS_COMMANDS.contains(command));
        for command in others {
            let refusal = asked_by(&app, "meetings", command).expect_err(command);
            assert!(refusal.contains("not allowed"), "{command}: {refusal}");
        }
        // The dialogs, the notifications and the opener are opened from Rust: the page is not given them.
        for command in [
            "plugin:dialog|open",
            "plugin:dialog|save",
            "plugin:dialog|message",
            "plugin:notification|notify",
            "plugin:opener|open_path",
            "plugin:opener|open_url",
            "plugin:opener|reveal_item_in_dir",
            "plugin:autostart|enable",
        ] {
            let refusal = asked_by(&app, "meetings", command).expect_err(command);
            assert!(refusal.contains("not allowed"), "{command}: {refusal}");
        }
    }

    #[test]
    fn the_recording_commands_are_the_meetings_windows_and_the_bar_has_only_five_of_them() {
        let app = app();
        for command in RECORDING_COMMANDS {
            assert!(MEETINGS_COMMANDS.contains(&command), "{command}");
            assert_eq!(asked_by(&app, "meetings", command), Ok(()), "{command}");
            // Settings, the floating dictation windows and a window that no capability names can't start or stop a recording.
            for window in ["settings", "bubble", "card", "stranger"] {
                let refusal = asked_by(&app, window, command).expect_err(command);
                assert!(
                    refusal.contains("not allowed"),
                    "{window} {command}: {refusal}"
                );
            }
            // The bar can't start a recording or ask what there is to record: that is the picker's.
            if !RECORDER_COMMANDS.contains(&command) {
                let refusal = asked_by(&app, "recorder", command).expect_err(command);
                assert!(
                    refusal.contains("not allowed"),
                    "recorder {command}: {refusal}"
                );
            }
        }
    }

    #[test]
    fn the_recorder_window_may_only_pause_resume_stop_discard_and_read_state() {
        let app = app();
        for command in RECORDER_COMMANDS {
            assert!(RECORDING_COMMANDS.contains(&command), "{command}");
            assert_eq!(asked_by(&app, "recorder", command), Ok(()), "{command}");
        }
        // It speaks the language of the settings, which it asks for as the floating dictation windows do, and it tells how big its
        // capsule is.
        assert_eq!(asked_by(&app, "recorder", BAR_SHARED_COMMAND), Ok(()));
        assert_eq!(asked_by(&app, "recorder", BAR_ONLY_COMMAND), Ok(()));
        // Not starting a recording, nor what the picker lists, nor anything of the library, Settings or the dictation windows.
        let others = SETTINGS_COMMANDS
            .into_iter()
            .chain(PANEL_COMMANDS)
            .chain(MEETINGS_COMMANDS)
            .filter(|command| {
                !RECORDER_COMMANDS.contains(command) && *command != BAR_SHARED_COMMAND
            });
        for command in others {
            let refusal = asked_by(&app, "recorder", command).expect_err(command);
            assert!(refusal.contains("not allowed"), "{command}: {refusal}");
        }
        // And none of the plugins: the dialogs, the notifications and the opener are opened from Rust.
        for command in [
            "plugin:dialog|open",
            "plugin:dialog|save",
            "plugin:notification|notify",
            "plugin:opener|open_path",
            "plugin:opener|open_url",
            "plugin:autostart|enable",
        ] {
            let refusal = asked_by(&app, "recorder", command).expect_err(command);
            assert!(refusal.contains("not allowed"), "{command}: {refusal}");
        }
        // Nobody else gets what the bar has by being named in its capability.
        for window in ["settings", "bubble", "card", "stranger"] {
            for command in ["recording_pause", "recording_discard"] {
                assert!(
                    asked_by(&app, window, command).is_err(),
                    "{window} {command}"
                );
            }
        }
    }

    #[test]
    fn the_capability_of_the_recorder_is_its_five_commands_the_language_listening_for_events_and_being_dragged(
    ) {
        let (windows, commands, others) = capability(include_str!("../capabilities/recorder.json"));

        assert_eq!(windows, ["recorder"]);
        let wanted: BTreeSet<String> = RECORDER_COMMANDS
            .into_iter()
            .chain([BAR_SHARED_COMMAND, BAR_ONLY_COMMAND])
            .map(str::to_owned)
            .collect();
        assert_eq!(commands, wanted);
        // The state of the recording (`recording-state`), and the page's drag region (`data-tauri-drag-region`) asking the window to be
        // moved; no more of the core.
        assert_eq!(
            others,
            [
                "core:event:allow-listen",
                "core:event:allow-unlisten",
                "core:window:allow-start-dragging"
            ]
        );
    }

    #[test]
    fn only_the_bar_may_tell_how_big_its_capsule_is() {
        let app = app();

        assert_eq!(asked_by(&app, "recorder", BAR_ONLY_COMMAND), Ok(()));
        // The size of the bar's window is its page's to say: no other window may size it, and it is not in their capabilities.
        for window in ["settings", "bubble", "card", "meetings", "stranger"] {
            let refusal = asked_by(&app, window, BAR_ONLY_COMMAND).expect_err(window);
            assert!(refusal.contains("not allowed"), "{window}: {refusal}");
        }
        for file in [
            include_str!("../capabilities/default.json"),
            include_str!("../capabilities/panel.json"),
            include_str!("../capabilities/meetings.json"),
        ] {
            let (_, commands, _) = capability(file);
            assert!(!commands.contains(BAR_ONLY_COMMAND));
        }
    }

    #[test]
    fn only_the_bar_may_ask_for_its_window_to_be_dragged() {
        let app = app();

        // What its page asks when the capsule is pressed: not refused (what the window does with it, in a window that is not
        // real, is not what is tested).
        let drag = asked_by(&app, "recorder", "plugin:window|start_dragging");
        assert!(
            !drag
                .clone()
                .is_err_and(|refusal| refusal.contains("not allowed")),
            "{drag:?}"
        );
        for window in ["settings", "bubble", "card", "meetings", "stranger"] {
            let refusal = asked_by(&app, window, "plugin:window|start_dragging")
                .expect_err("a window that is not the bar");
            assert!(refusal.contains("not allowed"), "{window}: {refusal}");
        }
        // And nothing else of the windows: the bar can't move, close or resize any window by asking.
        for command in [
            "plugin:window|close",
            "plugin:window|set_size",
            "plugin:window|hide",
            "plugin:window|destroy",
            "plugin:window|toggle_maximize",
        ] {
            let refusal = asked_by(&app, "recorder", command).expect_err(command);
            assert!(refusal.contains("not allowed"), "{command}: {refusal}");
        }
    }

    /// The commands a capability file gives (`allow-get-settings` is `get_settings`), and what else it names.
    fn capability(source: &str) -> (Vec<String>, BTreeSet<String>, Vec<String>) {
        let file: Value = serde_json::from_str(source).unwrap();
        let windows = file["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|window| window.as_str().unwrap().to_owned())
            .collect();
        let mut commands = BTreeSet::new();
        let mut others = Vec::new();
        for permission in file["permissions"].as_array().unwrap() {
            match permission.as_str() {
                Some(name) if name.starts_with("allow-") => {
                    commands.insert(name["allow-".len()..].replace('-', "_"));
                }
                Some(name) => others.push(name.to_owned()),
                None => others.push(permission["identifier"].as_str().unwrap().to_owned()),
            }
        }
        (windows, commands, others)
    }

    #[test]
    fn the_capability_of_the_meetings_window_is_the_list_of_its_commands_and_listening_for_events()
    {
        let (windows, commands, others) = capability(include_str!("../capabilities/meetings.json"));

        assert_eq!(windows, ["meetings"]);
        let wanted: BTreeSet<String> = MEETINGS_COMMANDS.into_iter().map(str::to_owned).collect();
        assert_eq!(commands, wanted);
        // The events of the jobs (`meeting-job`, `meetings-changed`) and of the files dropped on it; no more of the core.
        assert_eq!(
            others,
            ["core:event:allow-listen", "core:event:allow-unlisten"]
        );
    }

    #[test]
    fn no_command_of_meetings_is_in_the_other_capabilities_except_the_ones_that_are_shared() {
        let (_, settings, _) = capability(include_str!("../capabilities/default.json"));
        let (_, panels, _) = capability(include_str!("../capabilities/panel.json"));
        for command in MEETINGS_COMMANDS {
            assert_eq!(
                settings.contains(command),
                SHARED_COMMANDS.contains(&command),
                "{command} in default.json"
            );
            assert!(!panels.contains(command), "{command} in panel.json");
        }
        // And every command of Settings is still the settings' own (no panel command crept in).
        let own: BTreeSet<String> = SETTINGS_COMMANDS.into_iter().map(str::to_owned).collect();
        assert_eq!(settings, own);
    }

    #[test]
    fn the_content_security_policy_lets_the_page_play_media_from_the_local_server_and_nothing_else_from_outside(
    ) {
        let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = config["app"]["security"]["csp"].as_str().unwrap();
        assert!(csp.contains("media-src 'self' http://127.0.0.1:*"), "{csp}");
        // The rest is what it was: the page's own files, and the IPC.
        assert!(csp.starts_with("default-src 'self';"), "{csp}");
        assert!(
            csp.contains("connect-src 'self' ipc: http://ipc.localhost"),
            "{csp}"
        );
        assert!(
            !csp.contains("script-src"),
            "scripts are the page's own, by default-src"
        );
        assert!(
            !csp.contains("unsafe-eval") && !csp.contains("https:"),
            "{csp}"
        );
    }

    #[test]
    fn the_meetings_window_is_not_made_at_start_but_by_the_tray() {
        // The window is made when it is asked for (`meetings::window::show`), so it is not in the configuration: its label is
        // only named by the capability.
        let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let labels: Vec<&str> = config["app"]["windows"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|window| window["label"].as_str())
            .collect();
        assert_eq!(labels, ["settings"]);
    }

    #[test]
    fn the_deb_recommends_the_gstreamer_packages_that_play_the_recordings() {
        // WebKitGTK plays the audio of a recording (AAC in MP4) only with GStreamer's good plugins and libav. They are
        // recommended, not required: the app runs without them and says which package to install.
        let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config["bundle"]["linux"]["deb"]["recommends"],
            json!(["gstreamer1.0-plugins-good", "gstreamer1.0-libav"])
        );
        assert!(
            config["bundle"]["linux"]["deb"]["depends"].is_null(),
            "a package that is only recommended is not a dependency"
        );
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
    fn every_command_of_the_app_is_in_the_manifest_that_scopes_it_and_in_a_list_of_these_tests() {
        let wanted: BTreeSet<String> = SETTINGS_COMMANDS
            .into_iter()
            .chain(PANEL_COMMANDS)
            .chain(MEETINGS_COMMANDS)
            .chain([BAR_ONLY_COMMAND])
            .map(str::to_owned)
            .collect();
        for build in as_checked_out(include_str!("../build.rs")) {
            let manifest = quoted_after(&build, "COMMANDS: &[&str] = &[", ']');
            assert_eq!(manifest, wanted, "build.rs");
        }

        // What the app registers: `commands::name,` and `panel::name,` in `generate_handler!`.
        for source in as_checked_out(include_str!("main.rs")) {
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
    }

    /// A file as the checkout of each system gives it: Git turns the line ends into CRLF on Windows.
    fn as_checked_out(text: &str) -> [String; 2] {
        let lf = text.replace("\r\n", "\n");
        [lf.replace('\n', "\r\n"), lf]
    }

    /// A plist without its line ends and indentation, so that what stands between two tags is the same on every system.
    fn plist_in_one_line(plist: &str) -> String {
        plist.lines().map(str::trim).collect()
    }

    #[test]
    fn the_macos_bundle_names_the_microphone_in_its_info_and_in_its_entitlements() {
        // Without the reason in Info.plist macOS ends an app that records; with the hardened runtime (a signed and
        // notarised build) the microphone also needs the entitlement. Tauri merges the first file by itself and
        // signs with the second one when the configuration names it. Both files are read as every system checks
        // them out: Windows has CRLF.
        for info in as_checked_out(include_str!("../Info.plist")) {
            assert!(
                plist_in_one_line(&info).contains(
                    "<key>NSMicrophoneUsageDescription</key><string>Dictate records your voice to turn it into text.</string>"
                ),
                "{info:?}"
            );
        }

        for entitlements in as_checked_out(include_str!("../Entitlements.plist")) {
            assert!(
                plist_in_one_line(&entitlements)
                    .contains("<key>com.apple.security.device.audio-input</key><true/>"),
                "{entitlements:?}"
            );
        }

        let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config["bundle"]["macOS"]["entitlements"],
            json!("Entitlements.plist")
        );
    }

    #[test]
    fn the_macos_bundle_names_the_screen_in_its_info() {
        // Without the reason in Info.plist macOS ends an app that records the screen. Read as every system checks it out.
        for info in as_checked_out(include_str!("../Info.plist")) {
            assert!(
                plist_in_one_line(&info).contains(
                    "<key>NSScreenCaptureUsageDescription</key><string>Dictate records your screen and the sound of your computer for meetings you choose to record.</string>"
                ),
                "{info:?}"
            );
        }
    }

    #[test]
    fn the_minimum_macos_is_13() {
        // The system's audio is recorded with ScreenCaptureKit, which has it from macOS 13 (the Swift bridge of the crate needs
        // 12.3, and audio 13.0).
        let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config["bundle"]["macOS"]["minimumSystemVersion"],
            json!("13.0")
        );
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
