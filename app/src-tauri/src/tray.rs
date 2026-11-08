//! The tray icon and its menu, and the Settings window it opens.

use dictate_services::settings::{Settings, UiLanguage};
use serde::Serialize;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::commands;

const TRAY_ID: &str = "main";
const SETTINGS_WINDOW: &str = "settings";

const ID_SETTINGS: &str = "settings";
const ID_MEETINGS: &str = "meetings";
const ID_VERSION: &str = "version";
const ID_QUIT: &str = "quit";

/// The languages the app speaks. Sent to the window as `"en"` and `"vi"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    En,
    Vi,
}

/// The language of the UI: the user's choice, else Vietnamese when the OS language starts with `vi`, else
/// English.
pub fn resolve_language(setting: UiLanguage, os_locale: Option<&str>) -> Lang {
    match setting {
        UiLanguage::En => Lang::En,
        UiLanguage::Vi => Lang::Vi,
        UiLanguage::System => match os_locale {
            Some(locale) if locale.trim().to_ascii_lowercase().starts_with("vi") => Lang::Vi,
            _ => Lang::En,
        },
    }
}

/// The words of the tray menu. The version line ("Dictate 1.2.3") has no words to translate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayLabels {
    pub settings: &'static str,
    pub open_meetings: &'static str,
    pub quit: &'static str,
}

pub fn tray_labels(lang: Lang) -> TrayLabels {
    match lang {
        Lang::En => TrayLabels {
            settings: "Settings…",
            open_meetings: "Open meetings folder",
            quit: "Quit",
        },
        Lang::Vi => TrayLabels {
            settings: "Cài đặt…",
            open_meetings: "Mở thư mục cuộc họp",
            quit: "Thoát",
        },
    }
}

/// The language of the UI for these settings, on this OS.
pub fn language_for(settings: &Settings) -> Lang {
    resolve_language(settings.ui_language, sys_locale::get_locale().as_deref())
}

/// The language that the choice "System" means on this OS. The window is told this one, so that it speaks the
/// language of the tray menu and does not guess it again from what its web view reports.
pub fn system_language() -> Lang {
    resolve_language(UiLanguage::System, sys_locale::get_locale().as_deref())
}

fn build_menu(app: &AppHandle, lang: Lang) -> tauri::Result<Menu<tauri::Wry>> {
    let labels = tray_labels(lang);
    let settings = MenuItem::with_id(app, ID_SETTINGS, labels.settings, true, None::<&str>)?;
    let meetings = MenuItem::with_id(app, ID_MEETINGS, labels.open_meetings, true, None::<&str>)?;
    let version = MenuItem::with_id(
        app,
        ID_VERSION,
        format!("Dictate {}", env!("CARGO_PKG_VERSION")),
        false,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, ID_QUIT, labels.quit, true, None::<&str>)?;
    Menu::with_items(
        app,
        &[
            &settings,
            &meetings,
            &PredefinedMenuItem::separator(app)?,
            &version,
            &quit,
        ],
    )
}

/// Puts the icon in the tray. Clicking it opens the menu (on macOS too, as menu bar apps do).
pub fn create(app: &AppHandle, lang: Lang) -> tauri::Result<()> {
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Dictate")
        .menu(&build_menu(app, lang)?)
        .show_menu_on_left_click(true)
        .on_menu_event(on_menu_event);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    log::info!("The tray icon is in place");
    Ok(())
}

/// Rebuilds the tray menu in the language the settings now ask for.
pub fn refresh(app: &AppHandle, settings: &Settings) {
    let lang = language_for(settings);
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        let Some(tray) = handle.tray_by_id(TRAY_ID) else {
            return;
        };
        if let Err(e) = build_menu(&handle, lang).and_then(|menu| tray.set_menu(Some(menu))) {
            log::warn!("Could not update the tray menu: {e}");
        }
    });
    if let Err(e) = queued {
        log::warn!("Could not update the tray menu: {e}");
    }
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        ID_SETTINGS => show_settings(app),
        ID_MEETINGS => {
            // Starting the file manager can take a moment: not on the thread that runs the menu.
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = commands::open_meetings_folder_now(&app) {
                    log::warn!("{e}");
                }
            });
        }
        ID_QUIT => app.exit(0),
        _ => {}
    }
}

/// Shows the Settings window and brings it to the front.
pub fn show_settings(app: &AppHandle) {
    let Some(window) = app.get_webview_window(SETTINGS_WINDOW) else {
        log::warn!("There is no Settings window to show");
        return;
    };
    let shown = window
        .show()
        .and_then(|()| window.unminimize())
        .and_then(|()| window.set_focus());
    if let Err(e) = shown {
        log::warn!("Could not show the Settings window: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_labels_vi_and_en() {
        let en = tray_labels(Lang::En);
        assert_eq!(en.settings, "Settings…");
        assert_eq!(en.open_meetings, "Open meetings folder");
        assert_eq!(en.quit, "Quit");

        let vi = tray_labels(Lang::Vi);
        assert_eq!(vi.settings, "Cài đặt…");
        assert_eq!(vi.open_meetings, "Mở thư mục cuộc họp");
        assert_eq!(vi.quit, "Thoát");
    }

    #[test]
    fn a_language_is_sent_to_the_window_as_en_or_vi() {
        assert_eq!(serde_json::to_value(Lang::En).unwrap(), "en");
        assert_eq!(serde_json::to_value(Lang::Vi).unwrap(), "vi");
    }

    #[test]
    fn the_system_language_is_what_system_resolves_to_here() {
        let system = Settings {
            ui_language: UiLanguage::System,
            ..Settings::default()
        };
        assert_eq!(system_language(), language_for(&system));
    }

    #[test]
    fn the_chosen_language_wins_over_the_os() {
        assert_eq!(resolve_language(UiLanguage::En, Some("vi-VN")), Lang::En);
        assert_eq!(resolve_language(UiLanguage::Vi, Some("en-US")), Lang::Vi);
        assert_eq!(resolve_language(UiLanguage::Vi, None), Lang::Vi);
    }

    #[test]
    fn system_follows_the_os_language() {
        assert_eq!(
            resolve_language(UiLanguage::System, Some("vi-VN")),
            Lang::Vi
        );
        assert_eq!(resolve_language(UiLanguage::System, Some("vi")), Lang::Vi);
        assert_eq!(
            resolve_language(UiLanguage::System, Some("VI_VN")),
            Lang::Vi
        );
        assert_eq!(
            resolve_language(UiLanguage::System, Some("vi_VN.UTF-8")),
            Lang::Vi
        );
        assert_eq!(
            resolve_language(UiLanguage::System, Some("en-GB")),
            Lang::En
        );
        assert_eq!(
            resolve_language(UiLanguage::System, Some("fr-FR")),
            Lang::En
        );
        assert_eq!(resolve_language(UiLanguage::System, None), Lang::En);
    }
}
