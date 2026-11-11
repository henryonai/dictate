//! The tray icon and its menu, and the Settings window it opens. While a recording goes on the menu says how to stop it, and on
//! macOS the menu bar counts it (`● 01:05`, `⏸ 01:05` when it is paused).

use std::sync::Mutex;

use dictate_services::dictation::lang::display_hotkey;
use dictate_services::settings::{Settings, UiLanguage};
use serde::Serialize;
use tauri::menu::{IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::commands;
use crate::dictation::system::DictationHandle;
use crate::hotkey::{self, HotkeyEvent};
use crate::meetings;
use crate::recording::{Phase, Recorder, RecordingView};
use crate::state::lock;

const TRAY_ID: &str = "main";
const SETTINGS_WINDOW: &str = "settings";

const ID_DICTATE: &str = "dictate";
const ID_RECORD: &str = "record";
const ID_SETTINGS: &str = "settings";
const ID_MEETINGS: &str = "meetings";
const ID_MEETINGS_FOLDER: &str = "meetings-folder";
const ID_DICTATIONS: &str = "dictations";
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
    /// The meetings window.
    pub meetings: &'static str,
    pub open_meetings: &'static str,
    pub open_dictations: &'static str,
    pub quit: &'static str,
    /// The item that opens the picker of a recording, while none goes on.
    pub record: &'static str,
    /// The same item while one does.
    pub stop_recording: &'static str,
}

pub fn tray_labels(lang: Lang) -> TrayLabels {
    match lang {
        Lang::En => TrayLabels {
            settings: "Settings…",
            meetings: "Meetings…",
            open_meetings: "Open meetings folder",
            open_dictations: "Open dictations folder",
            quit: "Quit",
            record: "Record & summarize…",
            stop_recording: "Stop recording & summarize",
        },
        Lang::Vi => TrayLabels {
            settings: "Cài đặt…",
            meetings: "Cuộc họp…",
            open_meetings: "Mở thư mục cuộc họp",
            open_dictations: "Mở thư mục đọc chính tả",
            quit: "Thoát",
            record: "Quay & tóm tắt…",
            stop_recording: "Dừng quay & tóm tắt",
        },
    }
}

/// The item that starts and stops a dictation, with the shortcut that does the same.
pub fn dictate_label(lang: Lang, hotkey: &str) -> String {
    match lang {
        Lang::En => format!("Start / stop dictation ({hotkey})"),
        Lang::Vi => format!("Bắt đầu / dừng đọc ({hotkey})"),
    }
}

/// The item that opens the picker of a recording, or stops the one that goes on, with the shortcut that does the same.
pub fn record_label(lang: Lang, recording: bool, hotkey: &str) -> String {
    let labels = tray_labels(lang);
    let words = if recording {
        labels.stop_recording
    } else {
        labels.record
    };
    format!("{words} ({hotkey})")
}

/// What the menu bar says of the recording, on macOS: a red dot and the time, or the pause sign and the time. `None` when no
/// recording goes on (the title is empty then), or while one is being started or put together.
pub fn title(view: &RecordingView) -> Option<String> {
    let mark = match view.phase {
        Phase::Recording => "●",
        Phase::Paused => "⏸",
        Phase::Idle | Phase::Starting | Phase::Finishing => return None,
    };
    let time = dictate_core::text::timestamp::format(view.elapsed_ms as f64 / 1000.0);
    Some(format!("{mark} {time}"))
}

/// What the tray shows of a recording: whether the item stops it, and the title of the menu bar.
fn shows(view: &RecordingView) -> (bool, Option<String>) {
    let recording = matches!(view.phase, Phase::Recording | Phase::Paused);
    (recording, title(view))
}

/// The shortcut as the person reads it: symbols on the Mac, names elsewhere.
pub fn hotkey_label(settings: &Settings) -> String {
    display_hotkey(&settings.hotkey, cfg!(target_os = "macos"))
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

/// One item of the tray menu.
struct Entry {
    id: &'static str,
    text: String,
    enabled: bool,
}

/// The items of the menu, from the top; `None` is a separator. `recording` is whether a recording goes on.
fn menu_entries(lang: Lang, hotkey: &str, recording: bool) -> Vec<Option<Entry>> {
    let labels = tray_labels(lang);
    let entry = |id, text: &str, enabled| {
        Some(Entry {
            id,
            text: text.to_owned(),
            enabled,
        })
    };
    vec![
        entry(ID_DICTATE, &dictate_label(lang, hotkey), true),
        entry(
            ID_RECORD,
            &record_label(lang, recording, &hotkey::record_label()),
            true,
        ),
        None,
        entry(ID_SETTINGS, labels.settings, true),
        entry(ID_MEETINGS, labels.meetings, true),
        entry(ID_MEETINGS_FOLDER, labels.open_meetings, true),
        entry(ID_DICTATIONS, labels.open_dictations, true),
        None,
        entry(
            ID_VERSION,
            &format!("Dictate {}", env!("CARGO_PKG_VERSION")),
            false,
        ),
        entry(ID_QUIT, labels.quit, true),
    ]
}

fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
    lang: Lang,
    hotkey: &str,
    recording: bool,
) -> tauri::Result<Menu<R>> {
    let mut items: Vec<Box<dyn IsMenuItem<R>>> = Vec::new();
    for entry in menu_entries(lang, hotkey, recording) {
        items.push(match entry {
            Some(entry) => Box::new(MenuItem::with_id(
                app,
                entry.id,
                entry.text,
                entry.enabled,
                None::<&str>,
            )?),
            None => Box::new(PredefinedMenuItem::separator(app)?),
        });
    }
    let items: Vec<&dyn IsMenuItem<R>> = items.iter().map(Box::as_ref).collect();
    Menu::with_items(app, &items)
}

/// The words of the menu: what a rebuild of it is made from.
#[derive(Debug, Clone, PartialEq)]
struct Words {
    lang: Lang,
    hotkey: String,
    /// Whether the item that starts a recording says that it stops one.
    recording: bool,
}

/// What the tray has to show next, and what it shows: the menu and the title of the menu bar are changed only when what they say
/// changes, which is not as often as the recorder reports (every 50 ms).
struct Look {
    wanted: Words,
    wanted_title: Option<String>,
    shown: Words,
    shown_title: Option<String>,
}

/// What has to be changed on the tray to show what is wanted: the menu is rebuilt from the words, and the title (`Some(None)` is no
/// title) is set.
#[derive(Debug, Default, PartialEq)]
struct Due {
    menu: Option<Words>,
    title: Option<Option<String>>,
}

impl Look {
    /// What `create` builds: no recording.
    fn new(lang: Lang, hotkey: String) -> Look {
        let words = Words {
            lang,
            hotkey,
            recording: false,
        };
        Look {
            wanted: words.clone(),
            wanted_title: None,
            shown: words,
            shown_title: None,
        }
    }

    fn set_words(&mut self, lang: Lang, hotkey: String) {
        self.wanted.lang = lang;
        self.wanted.hotkey = hotkey;
    }

    fn set_recording(&mut self, recording: bool, title: Option<String>) {
        self.wanted.recording = recording;
        self.wanted_title = title;
    }

    fn is_due(&self) -> bool {
        self.wanted != self.shown || self.wanted_title != self.shown_title
    }

    /// What is to be done, which is taken to be done.
    fn take_due(&mut self) -> Due {
        let menu = (self.wanted != self.shown).then(|| self.wanted.clone());
        if let Some(words) = &menu {
            self.shown = words.clone();
        }
        let title = (self.wanted_title != self.shown_title).then(|| self.wanted_title.clone());
        if let Some(title) = &title {
            self.shown_title = title.clone();
        }
        Due { menu, title }
    }
}

/// The look of the tray, kept as state of the app.
struct TrayLook(Mutex<Look>);

/// Puts the icon in the tray. Clicking it opens the menu (on macOS too, as menu bar apps do).
pub fn create(app: &AppHandle, lang: Lang, hotkey: &str) -> tauri::Result<()> {
    app.manage(TrayLook(Mutex::new(Look::new(lang, hotkey.to_owned()))));
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Dictate")
        .menu(&build_menu(app, lang, hotkey, false)?)
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
    let Some(look) = app.try_state::<TrayLook>() else {
        return;
    };
    lock(&look.0).set_words(language_for(settings), hotkey_label(settings));
    apply(app);
}

/// The recording changed (the recorder says so as it goes on): the item says how to stop it, and on macOS the menu bar counts it.
/// This does no more than note what is to be shown, and asks the main thread to show it when that changed.
pub fn recording_changed<R: Runtime>(app: &AppHandle<R>, view: &RecordingView) {
    let Some(look) = app.try_state::<TrayLook>() else {
        return;
    };
    let (recording, title) = shows(view);
    let due = {
        let mut look = lock(&look.0);
        look.set_recording(recording, title);
        look.is_due()
    };
    if due {
        apply(app);
    }
}

/// Makes the tray show what is wanted, on the main thread. What is asked for twice is done once.
fn apply<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        let Some(look) = handle.try_state::<TrayLook>() else {
            return;
        };
        let due = lock(&look.0).take_due();
        let Some(tray) = handle.tray_by_id(TRAY_ID) else {
            return;
        };
        if let Some(words) = due.menu {
            if let Err(e) = build_menu(&handle, words.lang, &words.hotkey, words.recording)
                .and_then(|menu| tray.set_menu(Some(menu)))
            {
                log::warn!("Could not update the tray menu: {e}");
            }
        }
        // Only the menu bar of the Mac has room for a title.
        if let (Some(title), true) = (due.title, cfg!(target_os = "macos")) {
            if let Err(e) = tray.set_title(title.as_deref()) {
                log::warn!("Could not update the tray title: {e}");
            }
        }
    });
    if let Err(e) = queued {
        log::warn!("Could not update the tray: {e}");
    }
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        // The same as the shortcut: starts when idle, stops when recording.
        ID_DICTATE => {
            match app
                .try_state::<DictationHandle>()
                .and_then(|handle| handle.get())
            {
                // Not on the thread that runs the menu: starting a dictation shows windows, which asks that thread.
                Some(controller) => {
                    tauri::async_runtime::spawn_blocking(move || {
                        controller.on_hotkey(HotkeyEvent::Toggle);
                    });
                }
                None => log::info!("The tray asked to toggle dictation before it is ready"),
            }
        }
        // Opens the picker of a recording, or stops the one that goes on. A window is made and a recording put together on a thread
        // that may wait: not the one that runs the menu.
        ID_RECORD => {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || match app.try_state::<Recorder>() {
                Some(recorder) => recorder.tray_item(&app),
                None => log::info!("The tray asked about recording before the recorder is ready"),
            });
        }
        ID_SETTINGS => show_settings(app),
        ID_MEETINGS => {
            // Making a window asks the thread that runs the menu, and on Windows waiting for it there would never end.
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || meetings::window::show(&app));
        }
        ID_MEETINGS_FOLDER => {
            // Starting the file manager can take a moment: not on the thread that runs the menu.
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = commands::open_meetings_folder_now(&app) {
                    log::warn!("{e}");
                }
            });
        }
        ID_DICTATIONS => {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = commands::open_dictations_folder_now(&app) {
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
    use crate::recording::{Phase, RecordingView};

    #[test]
    fn tray_labels_vi_and_en() {
        let en = tray_labels(Lang::En);
        assert_eq!(en.settings, "Settings…");
        assert_eq!(en.meetings, "Meetings…");
        assert_eq!(en.open_meetings, "Open meetings folder");
        assert_eq!(en.open_dictations, "Open dictations folder");
        assert_eq!(en.quit, "Quit");
        assert_eq!(en.record, "Record & summarize…");
        assert_eq!(en.stop_recording, "Stop recording & summarize");

        let vi = tray_labels(Lang::Vi);
        assert_eq!(vi.settings, "Cài đặt…");
        assert_eq!(vi.meetings, "Cuộc họp…");
        assert_eq!(vi.open_meetings, "Mở thư mục cuộc họp");
        assert_eq!(vi.open_dictations, "Mở thư mục đọc chính tả");
        assert_eq!(vi.quit, "Thoát");
        assert_eq!(vi.record, "Quay & tóm tắt…");
        assert_eq!(vi.stop_recording, "Dừng quay & tóm tắt");
    }

    #[test]
    fn the_menu_has_record_after_dictate() {
        // The same items whether or not a recording goes on: only the words of one of them change.
        for recording in [false, true] {
            let entries = menu_entries(Lang::En, "Ctrl+Alt+Space", recording);
            let ids: Vec<Option<&str>> = entries
                .iter()
                .map(|entry| entry.as_ref().map(|entry| entry.id))
                .collect();
            // `None` is a separator.
            assert_eq!(
                ids,
                [
                    Some(ID_DICTATE),
                    Some(ID_RECORD),
                    None,
                    Some(ID_SETTINGS),
                    Some(ID_MEETINGS),
                    Some(ID_MEETINGS_FOLDER),
                    Some(ID_DICTATIONS),
                    None,
                    Some(ID_VERSION),
                    Some(ID_QUIT),
                ],
                "recording: {recording}"
            );
        }
        let text = |lang, id| {
            menu_entries(lang, "Ctrl+Alt+Space", false)
                .into_iter()
                .flatten()
                .find(|entry| entry.id == id)
                .map(|entry| (entry.text, entry.enabled))
        };
        assert_eq!(
            text(Lang::En, ID_MEETINGS),
            Some(("Meetings…".to_owned(), true))
        );
        assert_eq!(
            text(Lang::Vi, ID_MEETINGS),
            Some(("Cuộc họp…".to_owned(), true))
        );
        assert_eq!(
            text(Lang::En, ID_MEETINGS_FOLDER),
            Some(("Open meetings folder".to_owned(), true))
        );
        // The line with the version is a label, not something to click.
        assert_eq!(
            text(Lang::En, ID_VERSION).map(|(_, enabled)| enabled),
            Some(false)
        );
    }

    #[test]
    fn the_record_label_follows_the_state() {
        assert_eq!(
            record_label(Lang::En, false, "⌃⌥R"),
            "Record & summarize… (⌃⌥R)"
        );
        assert_eq!(
            record_label(Lang::En, true, "⌃⌥R"),
            "Stop recording & summarize (⌃⌥R)"
        );
        assert_eq!(
            record_label(Lang::Vi, false, "⌃⌥R"),
            "Quay & tóm tắt… (⌃⌥R)"
        );
        assert_eq!(
            record_label(Lang::Vi, true, "⌃⌥R"),
            "Dừng quay & tóm tắt (⌃⌥R)"
        );

        // The menu has them, with the shortcut as it is read on this system, and the item can be clicked either way.
        let shortcut = crate::hotkey::record_label();
        let item = |lang, recording| {
            menu_entries(lang, "Ctrl+Alt+Space", recording)
                .into_iter()
                .flatten()
                .find(|entry| entry.id == ID_RECORD)
                .map(|entry| (entry.text, entry.enabled))
        };
        assert_eq!(
            item(Lang::En, false),
            Some((format!("Record & summarize… ({shortcut})"), true))
        );
        assert_eq!(
            item(Lang::En, true),
            Some((format!("Stop recording & summarize ({shortcut})"), true))
        );
        assert_eq!(
            item(Lang::Vi, false),
            Some((format!("Quay & tóm tắt… ({shortcut})"), true))
        );
        assert_eq!(
            item(Lang::Vi, true),
            Some((format!("Dừng quay & tóm tắt ({shortcut})"), true))
        );
    }

    fn view(phase: Phase, elapsed_ms: u64) -> RecordingView {
        RecordingView {
            phase,
            elapsed_ms,
            ..RecordingView::idle()
        }
    }

    #[test]
    fn the_menu_bar_counts_the_recording_and_marks_a_pause() {
        assert_eq!(
            title(&view(Phase::Recording, 0)).as_deref(),
            Some("● 00:00")
        );
        assert_eq!(
            title(&view(Phase::Recording, 65_900)).as_deref(),
            Some("● 01:05"),
            "the seconds are whole ones"
        );
        assert_eq!(
            title(&view(Phase::Paused, 125_000)).as_deref(),
            Some("⏸ 02:05")
        );
        assert_eq!(
            title(&view(Phase::Recording, 3_725_000)).as_deref(),
            Some("● 1:02:05"),
            "hours from the first one"
        );
        // Nothing is counted when no recording goes on, or while one is being started or put together.
        for phase in [Phase::Idle, Phase::Starting, Phase::Finishing] {
            assert_eq!(title(&view(phase, 5_000)), None, "{phase:?}");
        }
    }

    #[test]
    fn the_item_says_stop_while_a_recording_goes_on_and_is_paused_too() {
        let recording = |phase| shows(&view(phase, 0)).0;
        assert!(recording(Phase::Recording));
        assert!(recording(Phase::Paused));
        // Being started or put together, the recorder takes no new recording, and none is there to stop: the item opens the picker.
        assert!(!recording(Phase::Idle));
        assert!(!recording(Phase::Starting));
        assert!(!recording(Phase::Finishing));
    }

    #[test]
    fn the_tray_changes_only_what_changed() {
        let mut look = Look::new(Lang::En, "Ctrl+Alt+Space".into());
        // What `create` built is what is shown.
        assert_eq!(look.take_due(), Due::default());

        look.set_recording(true, Some("● 00:00".into()));
        let due = look.take_due();
        assert_eq!(
            due.menu,
            Some(Words {
                lang: Lang::En,
                hotkey: "Ctrl+Alt+Space".into(),
                recording: true
            })
        );
        assert_eq!(due.title, Some(Some("● 00:00".to_owned())));

        // The clock moves on: the title, not the menu.
        look.set_recording(true, Some("● 00:01".into()));
        let due = look.take_due();
        assert_eq!(due.menu, None);
        assert_eq!(due.title, Some(Some("● 00:01".to_owned())));

        // The same again, as every report of the recorder is: nothing to do.
        look.set_recording(true, Some("● 00:01".into()));
        assert_eq!(look.take_due(), Due::default());

        // A pause changes the title and not the menu: the item still stops the recording.
        look.set_recording(true, Some("⏸ 00:01".into()));
        let due = look.take_due();
        assert_eq!(due.menu, None);
        assert_eq!(due.title, Some(Some("⏸ 00:01".to_owned())));

        // New words (the language of the interface changed): the menu, in what the state is now.
        look.set_words(Lang::Vi, "Ctrl+Alt+Space".into());
        let due = look.take_due();
        assert_eq!(
            due.menu.map(|words| (words.lang, words.recording)),
            Some((Lang::Vi, true))
        );
        assert_eq!(due.title, None);

        // The recording is over: the item says what it said, and the title goes.
        look.set_recording(false, None);
        let due = look.take_due();
        assert_eq!(due.menu.map(|words| words.recording), Some(false));
        assert_eq!(due.title, Some(None));
        assert_eq!(look.take_due(), Due::default());
    }

    #[test]
    fn the_item_that_starts_and_stops_dictation_says_which_shortcut_does_it() {
        assert_eq!(
            dictate_label(Lang::En, "\u{2303}\u{2325}Space"),
            "Start / stop dictation (\u{2303}\u{2325}Space)"
        );
        assert_eq!(
            dictate_label(Lang::Vi, "Ctrl+Alt+Space"),
            "Bắt đầu / dừng đọc (Ctrl+Alt+Space)"
        );
    }

    #[test]
    fn the_shortcut_of_the_menu_is_written_as_the_person_reads_it() {
        let settings = Settings {
            hotkey: "Ctrl+Alt+Space".into(),
            ..Settings::default()
        };
        let label = hotkey_label(&settings);
        if cfg!(target_os = "macos") {
            assert_eq!(label, "\u{2303}\u{2325}Space");
        } else {
            assert_eq!(label, "Ctrl+Alt+Space");
        }
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
