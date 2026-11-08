//! The floating windows of dictation: two windows with one page (`panel.html`), where they go, and what they are
//! told.
//!
//! `bubble` sits next to the text field the text will be written into. It never takes the focus, so that the
//! person goes on typing in the field: on macOS it is a non-activating panel that can never become the key window,
//! elsewhere a window that refuses focus. `card` is the other one: the pill while dictating with no field to write
//! to, and (later) the review card. It does take the focus, for Return, Esc and typing.
//!
//! The controller sends one [`PanelState`] to both windows with [`emit_state`], which shows the window of its view
//! and hides the other. The page measures what it draws and tells the Rust side with `panel_resized`, which sizes
//! the window to it and puts it where [`place`] says.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use dictate_services::dictation::lang::{FAVOURITES, OTHERS};
use dictate_services::dictation::machine::Phase;
use dictate_services::dictation::pipeline::Notice;
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
};

use crate::platform::{self, Platform, Rect};
use crate::state::{lock, AppState};
use crate::tray;

/// The window next to the text field. It is also the name the page is opened with (`?view=bubble`).
pub const BUBBLE: &str = "bubble";
/// The window that takes the focus.
pub const CARD: &str = "card";
const WINDOWS: [&str; 2] = [BUBBLE, CARD];
/// The event that carries a [`PanelState`] to both windows.
pub const STATE_EVENT: &str = "panel-state";

/// What a window is before its page has measured itself: room for a bubble or a pill with its padding.
const BUBBLE_START: (f64, f64) = (428.0, 120.0);
const CARD_START: (f64, f64) = (460.0, 240.0);
/// No panel is bigger than this on a side (logical points): a page that reports more has gone wrong.
const MAX_SIDE: f64 = 4000.0;

/// The padding around the visible shape of a panel, for its shadow (the page draws it, 14 CSS pixels).
const INSET: f64 = 14.0;
/// How far left of the anchor the visible shape starts: the inset and 10 more.
const X_OFFSET: f64 = INSET + 10.0;
/// The gap between the anchor line and the visible shape.
const GAP: f64 = 8.0;
/// A panel goes below its anchor when this much of the screen is left under it.
const ROOM_BELOW: f64 = 240.0;
/// Without an anchor, the visible shape stands this far above the bottom of the screen.
const BOTTOM_LIFT: f64 = 60.0;

// ---- Where ----------------------------------------------------------------------------------------------------------

/// Where the window of a panel of `size` goes, as the position of its top left corner in the global space of
/// the screens (logical points, origin at the top left), and which side of its anchor it took.
///
/// `anchor` is the caret or the field to stay next to. `visible` is the part of the screen that no menu bar or
/// taskbar covers. `below` is the side decided for this anchor already: it is decided once, so that a panel that
/// grows doesn't jump to the other side. Without an anchor the panel stands at the bottom centre of `visible`.
pub fn place(
    anchor: Option<Rect>,
    size: (f64, f64),
    visible: Rect,
    below: Option<bool>,
) -> (f64, f64, bool) {
    let (w, h) = size;
    let (x, y, below) = match anchor {
        Some(a) => {
            let below = below.unwrap_or(visible.y + visible.h - (a.y + a.h) >= ROOM_BELOW);
            let y = if below {
                a.y + a.h + GAP - INSET
            } else {
                a.y - GAP + INSET - h
            };
            (a.x - X_OFFSET, y, below)
        }
        None => (
            visible.x + visible.w / 2.0 - w / 2.0,
            visible.y + visible.h - BOTTOM_LIFT - h,
            below.unwrap_or(false),
        ),
    };
    // `min(max())` and not `clamp`: a panel bigger than the screen has no room, and `clamp` panics on that. The
    // right and the bottom edge win then, which is what the Swift app did.
    let x = x
        .max(visible.x - INSET)
        .min(visible.x + visible.w - w + INSET);
    let y = y
        .max(visible.y - INSET)
        .min(visible.y + visible.h - h + INSET);
    (x, y, below)
}

/// Where the panels are put: next to one anchor, on the side that was decided for it.
#[derive(Debug, Default)]
struct Layout {
    anchor: Option<Rect>,
    /// Below the anchor or above it, once it is decided.
    below: Option<bool>,
}

impl Layout {
    /// A new dictation has a new anchor, and decides its side anew.
    fn set_anchor(&mut self, anchor: Option<Rect>) {
        self.anchor = anchor;
        self.below = None;
    }

    /// The point whose screen is the one to ask about, when there is an anchor.
    fn near(&self) -> Option<(f64, f64)> {
        self.anchor.map(|a| (a.x, a.y))
    }

    /// The top left corner for a panel of `size`, remembering the side.
    fn position(&mut self, size: (f64, f64), visible: Rect) -> (f64, f64) {
        let (x, y, below) = place(self.anchor, size, visible, self.below);
        if self.anchor.is_some() {
            self.below = Some(below);
        }
        (x, y)
    }
}

/// Whether the system lets an app put its windows where it wants. Wayland doesn't: the compositor chooses.
fn positioning_works(wayland: bool) -> bool {
    !wayland
}

fn on_wayland() -> bool {
    #[cfg(target_os = "linux")]
    {
        use platform::rules::{session, Session};
        session(&|name| std::env::var(name).ok()) == Session::Wayland
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

// ---- What the windows are told ----------------------------------------------------------------------------------------

/// What is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    Hidden,
    /// The live text and what happens to it before it is written, by the text field.
    Bubble,
    /// The recording, when there is no field to write to.
    Pill,
    /// What was written, with the ⋯ menu.
    Chip,
    /// The review card.
    Card,
}

impl View {
    /// The window that draws this view. The bubble and the chip stay out of the way of the field, so they are in
    /// the window that never takes the focus. The pill is stopped with Return and cancelled with Esc, and the card is
    /// typed in: they are in the one that does.
    pub fn window(self) -> Option<&'static str> {
        match self {
            View::Hidden => None,
            View::Bubble | View::Chip => Some(BUBBLE),
            View::Pill | View::Card => Some(CARD),
        }
    }
}

/// What the app is working on while the bubble waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkKind {
    Writing,
    Editing,
    Rewriting,
    Retranscribing,
}

/// The work under way, for the title of the bubble. The page has the words of every title; this has only what goes
/// into them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Working {
    pub kind: WorkKind,
    /// A preset id, or the words of a custom request.
    pub style: Option<String>,
    /// The transcription model, while it transcribes again.
    pub model: Option<String>,
    /// The transcription also edits, in the same call.
    pub editing: bool,
}

/// Everything the two windows draw from, sent as the `panel-state` event. Rust sends keys and values, never words:
/// the page has the words, in both languages.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelState {
    pub view: View,
    pub phase: Phase,
    /// The text that is final, and the latest guess for the rest.
    pub committed: String,
    pub partial: String,
    /// The loudness now, 0 to 1.
    pub level: f32,
    /// BCP-47, as in the settings.
    pub language: String,
    pub duration_s: f64,
    /// The dictation shortcut as a person reads it ("⌃⌥Space").
    pub hotkey: String,
    pub push_to_talk: bool,
    pub working: Option<Working>,
    /// What was written, or what the card edits.
    pub text: String,
    pub original: Option<String>,
    pub style: Option<String>,
    pub notice: Option<Notice>,
    pub from_chip: bool,
    pub can_write: bool,
    pub accessibility_missing: bool,
}

impl PanelState {
    /// Nothing to show: both windows hide.
    pub fn hidden() -> Self {
        Self {
            view: View::Hidden,
            phase: Phase::Hidden,
            committed: String::new(),
            partial: String::new(),
            level: 0.0,
            language: "en-US".into(),
            duration_s: 0.0,
            hotkey: String::new(),
            push_to_talk: false,
            working: None,
            text: String::new(),
            original: None,
            style: None,
            notice: None,
            from_chip: false,
            can_write: false,
            accessibility_missing: false,
        }
    }
}

/// Which window shows and which hides for a view: each window, and whether it is on.
fn wanted(view: View) -> [(&'static str, bool); 2] {
    let shown = view.window();
    WINDOWS.map(|label| (label, shown == Some(label)))
}

// ---- What is on screen ------------------------------------------------------------------------------------------------

/// What setting a window to shown or hidden changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Appeared,
    Disappeared,
    Same,
}

/// Which windows are shown. Asking a window is a round trip to the main thread, which the controller can't always
/// make: this is what the app last asked for.
#[derive(Debug, Default)]
struct Shown {
    bubble: bool,
    card: bool,
}

impl Shown {
    fn set(&mut self, label: &str, on: bool) -> Change {
        let flag = if label == BUBBLE {
            &mut self.bubble
        } else {
            &mut self.card
        };
        let was = std::mem::replace(flag, on);
        match (was, on) {
            (false, true) => Change::Appeared,
            (true, false) => Change::Disappeared,
            _ => Change::Same,
        }
    }

    fn is_on(&self, label: &str) -> bool {
        if label == BUBBLE {
            self.bubble
        } else {
            self.card
        }
    }
}

struct Inner {
    layout: Layout,
    shown: Shown,
    /// What each window measures, in logical points.
    sizes: HashMap<&'static str, (f64, f64)>,
}

/// The windows' state, managed by the app.
pub struct Panels {
    platform: Arc<dyn Platform>,
    inner: Mutex<Inner>,
}

impl Panels {
    fn new(platform: Arc<dyn Platform>) -> Self {
        Self {
            platform,
            inner: Mutex::new(Inner {
                layout: Layout::default(),
                shown: Shown::default(),
                sizes: HashMap::from([(BUBBLE, BUBBLE_START), (CARD, CARD_START)]),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        lock(&self.inner)
    }
}

/// The window of this name, as it is named in the page's URL and in the capability: `None` for any other.
fn window_label(label: &str) -> Option<&'static str> {
    WINDOWS.into_iter().find(|window| *window == label)
}

/// A size the page measured, when it is a size a window can have: nothing drawn is 0, which leaves the window as it was.
fn sane_size(width: f64, height: f64) -> Option<(f64, f64)> {
    let fits = |side: f64| side.is_finite() && side > 0.0 && side <= MAX_SIDE;
    (fits(width) && fits(height)).then_some((width, height))
}

// ---- The windows ------------------------------------------------------------------------------------------------------

/// Creates both windows, hidden, and the state that goes with them. Called once, when the app starts, on the main
/// thread.
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    create_bubble(app)?;
    create_card(app)?;
    app.manage(Panels::new(platform::current(app)));
    log::info!("The panel windows are ready");
    Ok(())
}

/// What both windows are: transparent, without a frame or a shadow of their own (the page draws the shadows), above
/// the other windows, on every desktop, and out of the taskbar. They start hidden.
fn floating(
    builder: WebviewWindowBuilder<'_, tauri::Wry, AppHandle>,
    size: (f64, f64),
) -> WebviewWindowBuilder<'_, tauri::Wry, AppHandle> {
    builder
        .title("Dictate")
        .inner_size(size.0, size.1)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .visible(false)
}

/// Windows and Linux: a window that refuses the focus (a no-activate window on Windows, a window that doesn't accept
/// focus in GTK), so that showing it leaves the text field where it is.
#[cfg(not(target_os = "macos"))]
fn create_bubble(app: &AppHandle) -> tauri::Result<()> {
    let url = WebviewUrl::App("panel.html?view=bubble".into());
    floating(WebviewWindowBuilder::new(app, BUBBLE, url), BUBBLE_START)
        .focusable(false)
        .focused(false)
        .build()?;
    Ok(())
}

/// macOS: a window that doesn't take the focus can't be made with Tauri's window flags, because showing a window
/// activates the app and the field loses its caret. A non-activating panel can never become the key window, and
/// it is shown with `orderFrontRegardless` and nothing else.
#[cfg(target_os = "macos")]
fn create_bubble(app: &AppHandle) -> tauri::Result<()> {
    use tauri_nspanel::{tauri_panel, CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};

    tauri_panel! {
        panel!(BubblePanel {
            config: {
                can_become_key_window: false,
                can_become_main_window: false,
                is_floating_panel: true
            }
        })
    }

    PanelBuilder::<_, BubblePanel>::new(app, BUBBLE)
        .url(WebviewUrl::App("panel.html?view=bubble".into()))
        .level(PanelLevel::Floating)
        .style_mask(StyleMask::empty().nonactivating_panel())
        // On every desktop, and next to a full-screen app.
        .collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary(),
        )
        .no_activate(true)
        .transparent(true)
        .has_shadow(false)
        .hides_on_deactivate(false)
        .with_window(|window| {
            floating(window, BUBBLE_START)
                .focused(false)
                .focusable(false)
        })
        .build()?;
    Ok(())
}

fn create_card(app: &AppHandle) -> tauri::Result<()> {
    let url = WebviewUrl::App("panel.html?view=card".into());
    floating(WebviewWindowBuilder::new(app, CARD, url), CARD_START)
        .focusable(true)
        .focused(false)
        .build()?;
    Ok(())
}

/// Sends the state to both windows, and shows the window of its view and hides the other. A window that is shown
/// already is left alone: it doesn't take the focus again with every level of the meter.
pub fn emit_state(app: &AppHandle, state: &PanelState) {
    for label in WINDOWS {
        if let Err(e) = app.emit_to(label, STATE_EVENT, state) {
            log::warn!("Could not send the panel state to {label}: {e}");
        }
    }
    let Some(panels) = app.try_state::<Panels>() else {
        log::warn!("The panel windows don't exist: nothing to show");
        return;
    };
    for (label, on) in wanted(state.view) {
        // The lock is let go before anything is done to a window.
        let change = panels.lock().shown.set(label, on);
        match change {
            Change::Appeared => {
                place_window(app, label);
                on_main_thread(app, move |app| show_now(app, label));
            }
            Change::Disappeared => on_main_thread(app, move |app| hide_now(app, label)),
            Change::Same => {}
        }
    }
}

/// Puts the next panels next to `anchor` (the caret or the field), or at the bottom centre of the screen without
/// one, and decides their side again. Called when a dictation starts; the windows that are shown move at once.
pub fn set_anchor(app: &AppHandle, anchor: Option<Rect>) {
    let Some(panels) = app.try_state::<Panels>() else {
        return;
    };
    panels.lock().layout.set_anchor(anchor);
    for label in WINDOWS {
        if panels.lock().shown.is_on(label) {
            place_window(app, label);
        }
    }
}

/// Moves a window to where it belongs for its size. Asks the system where the screens are, which can take a moment:
/// not for the main thread's own work.
fn place_window(app: &AppHandle, label: &'static str) {
    if !positioning_works(on_wayland()) {
        return;
    }
    let Some(panels) = app.try_state::<Panels>() else {
        return;
    };
    let near = panels.lock().layout.near();
    let visible = panels.platform.screen_visible_frame(near);
    let (x, y) = {
        let mut inner = panels.lock();
        let size = inner.sizes.get(label).copied().unwrap_or(BUBBLE_START);
        inner.layout.position(size, visible)
    };
    if let Some(window) = app.get_webview_window(label) {
        if let Err(e) = window.set_position(LogicalPosition::new(x, y)) {
            log::warn!("Could not place the {label} window: {e}");
        }
    }
}

/// Runs `work` on the main thread, where the windows of macOS may be touched.
fn on_main_thread(app: &AppHandle, work: impl FnOnce(&AppHandle) + Send + 'static) {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || work(&handle)) {
        log::warn!("Could not reach the main thread: {e}");
    }
}

/// On the main thread. The bubble is shown, never made the key window; the card is shown and takes the focus.
fn show_now(app: &AppHandle, label: &'static str) {
    #[cfg(target_os = "macos")]
    if label == BUBBLE {
        use tauri_nspanel::ManagerExt;
        match app.get_webview_panel(BUBBLE) {
            Ok(panel) => panel.show(),
            Err(e) => log::warn!("There is no bubble panel to show: {e:?}"),
        }
        return;
    }
    let Some(window) = app.get_webview_window(label) else {
        log::warn!("There is no {label} window to show");
        return;
    };
    if let Err(e) = window.show() {
        log::warn!("Could not show the {label} window: {e}");
    }
    if label == CARD {
        if let Err(e) = window.set_focus() {
            log::warn!("Could not focus the {label} window: {e}");
        }
    }
}

/// On the main thread.
fn hide_now(app: &AppHandle, label: &'static str) {
    #[cfg(target_os = "macos")]
    if label == BUBBLE {
        use tauri_nspanel::ManagerExt;
        match app.get_webview_panel(BUBBLE) {
            Ok(panel) => panel.hide(),
            Err(e) => log::warn!("There is no bubble panel to hide: {e:?}"),
        }
        return;
    }
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    if let Err(e) = window.hide() {
        log::warn!("Could not hide the {label} window: {e}");
    }
}

/// Sizes a window to what its page measured and puts it where it belongs. The page measures again whenever what it
/// draws changes size.
fn resize_and_place(app: &AppHandle, label: &'static str, size: (f64, f64)) {
    let Some(panels) = app.try_state::<Panels>() else {
        return;
    };
    let shown = {
        let mut inner = panels.lock();
        inner.sizes.insert(label, size);
        inner.shown.is_on(label)
    };
    if let Some(window) = app.get_webview_window(label) {
        if let Err(e) = window.set_size(LogicalSize::new(size.0, size.1)) {
            log::warn!("Could not size the {label} window: {e}");
        }
    }
    if shown {
        place_window(app, label);
    }
}

// ---- Commands -------------------------------------------------------------------------------------------------------------

/// The languages of the menu on the chip, in the order it shows them. The page names them in the language of the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LanguageLists {
    pub favourites: Vec<String>,
    pub others: Vec<String>,
}

fn language_lists() -> LanguageLists {
    let own = |codes: &[&str]| codes.iter().map(|code| (*code).to_owned()).collect();
    LanguageLists {
        favourites: own(&FAVOURITES),
        others: own(OTHERS),
    }
}

#[tauri::command]
pub async fn dictation_languages() -> Result<LanguageLists, String> {
    Ok(language_lists())
}

/// The language of the UI that the settings ask for: the panels speak it, and the settings are not theirs to read.
#[tauri::command]
pub async fn panel_language(state: tauri::State<'_, AppState>) -> Result<tray::Lang, String> {
    let state = state.inner().clone();
    // The settings are locked while they are written to disk.
    tauri::async_runtime::spawn_blocking(move || tray::language_for(&state.settings.get()))
        .await
        .map_err(|e| format!("The work stopped unexpectedly: {e}"))
}

/// The page measured what it draws: the window is sized to it and put where it belongs.
#[tauri::command]
pub async fn panel_resized(
    app: AppHandle,
    label: String,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let window = window_label(&label).ok_or_else(|| format!("There is no panel called {label}"))?;
    let Some(size) = sane_size(width, height) else {
        return Ok(());
    };
    tauri::async_runtime::spawn_blocking(move || resize_and_place(&app, window, size))
        .await
        .map_err(|e| format!("The work stopped unexpectedly: {e}"))
}

// The next three are for the dictation controller (Task 10 of the dictation plan) to answer; until it exists they
// only say that they were asked.

/// Stop and write: the Stop button of the pill and Return.
#[tauri::command]
pub async fn dictation_stop() -> Result<(), String> {
    log::info!("The panel asked to stop the dictation");
    Ok(())
}

/// Drop the dictation: the cross of the bubble and Esc in the pill.
#[tauri::command]
pub async fn dictation_cancel() -> Result<(), String> {
    log::info!("The panel asked to cancel the dictation");
    Ok(())
}

/// The language menu: save the language and apply it to what is said from now on.
#[tauri::command]
pub async fn dictation_set_language(lang: String) -> Result<(), String> {
    log::info!("The panel asked to change the dictation language to {lang}");
    Ok(())
}

/// A way to look at the windows without the controller: in a debug build, `DICTATE_PANEL_DEMO=1` cycles fixture states
/// through [`emit_state`]. Debug builds only: a release build has none of it.
///
/// - `DICTATE_PANEL_DEMO_ANCHOR=x,y` puts the panels next to a caret there (logical points from the top left of the
///   main screen), where without it they stand at the bottom centre of the screen.
/// - `DICTATE_PANEL_DEMO_STEP_MS` is how long each state stays (3000).
/// - `DICTATE_PANEL_DEMO_HOLD=n` stays on the n-th state (from 0) and doesn't cycle.
#[cfg(debug_assertions)]
pub mod demo {
    use std::time::Duration;

    use super::*;

    /// How often the level and the timer of a state that listens are updated.
    const TICK: Duration = Duration::from_millis(100);

    fn base() -> PanelState {
        PanelState {
            hotkey: "\u{2303}\u{2325}Space".into(),
            ..PanelState::hidden()
        }
    }

    fn working(kind: WorkKind, style: Option<&str>, model: Option<&str>, editing: bool) -> Working {
        Working {
            kind,
            style: style.map(str::to_owned),
            model: model.map(str::to_owned),
            editing,
        }
    }

    /// One state of each shape the page draws now, in the order a dictation goes through them.
    pub fn fixtures() -> Vec<PanelState> {
        let listening = PanelState {
            view: View::Bubble,
            phase: Phase::Recording,
            ..base()
        };
        let words = PanelState {
            committed: "This is the part that is final, and".into(),
            partial: "this is what is still a guess".into(),
            ..listening.clone()
        };
        let pill = PanelState {
            view: View::Pill,
            phase: Phase::Recording,
            duration_s: 65.0,
            ..base()
        };
        let edited = PanelState {
            view: View::Bubble,
            phase: Phase::Working,
            text: "This is the part that is final, and this is what was heard.".into(),
            ..base()
        };
        vec![
            listening,
            words.clone(),
            PanelState {
                push_to_talk: true,
                ..words.clone()
            },
            PanelState {
                phase: Phase::Finishing,
                ..words
            },
            PanelState {
                working: Some(working(WorkKind::Editing, Some("concise"), None, false)),
                ..edited.clone()
            },
            PanelState {
                working: Some(working(WorkKind::Writing, None, None, false)),
                ..edited.clone()
            },
            PanelState {
                working: Some(working(
                    WorkKind::Retranscribing,
                    Some("clean"),
                    Some("whisper-1"),
                    true,
                )),
                ..edited
            },
            pill.clone(),
            PanelState {
                committed: "The pill shows the words above it,".into(),
                partial: "and a guess for the rest".into(),
                ..pill.clone()
            },
            PanelState {
                phase: Phase::Finishing,
                ..pill
            },
            PanelState::hidden(),
        ]
    }

    /// Starts cycling when the environment asks for it.
    pub fn start(app: &AppHandle) {
        if std::env::var_os("DICTATE_PANEL_DEMO").is_none() {
            return;
        }
        let anchor = std::env::var("DICTATE_PANEL_DEMO_ANCHOR")
            .ok()
            .and_then(|value| parse_anchor(&value));
        let step = std::env::var("DICTATE_PANEL_DEMO_STEP_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map_or(Duration::from_secs(3), Duration::from_millis);
        let hold = std::env::var("DICTATE_PANEL_DEMO_HOLD")
            .ok()
            .and_then(|value| value.parse::<usize>().ok());
        let app = app.clone();
        log::info!("Panel demo: cycling the fixture states (anchor {anchor:?})");
        let spawned = std::thread::Builder::new()
            .name("panel-demo".into())
            .spawn(move || run(&app, anchor, step, hold));
        if let Err(e) = spawned {
            log::warn!("Could not start the panel demo: {e}");
        }
    }

    /// "x,y": a caret at that point, 2 wide and a line of text high.
    pub fn parse_anchor(value: &str) -> Option<Rect> {
        let (x, y) = value.split_once(',')?;
        Some(Rect {
            x: x.trim().parse().ok()?,
            y: y.trim().parse().ok()?,
            w: 2.0,
            h: 18.0,
        })
    }

    fn run(app: &AppHandle, anchor: Option<Rect>, step: Duration, hold: Option<usize>) {
        // The pages need a moment to load before they hear a state.
        std::thread::sleep(Duration::from_secs(2));
        set_anchor(app, anchor);
        let fixtures = fixtures();
        loop {
            let shown: Vec<&PanelState> = match hold {
                Some(n) => fixtures.get(n).into_iter().collect(),
                None => fixtures.iter().collect(),
            };
            for fixture in shown {
                log::info!("Panel demo: {:?} {:?}", fixture.view, fixture.phase);
                let started = std::time::Instant::now();
                while started.elapsed() < step {
                    let mut state = fixture.clone();
                    let t = started.elapsed().as_secs_f32();
                    // Something like speech: a level that comes and goes.
                    state.level = ((t * 6.0).sin() * 0.5 + 0.5) * ((t * 2.3).sin().abs());
                    state.duration_s += f64::from(t);
                    emit_state(app, &state);
                    std::thread::sleep(TICK);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_demo_shows_every_shape_the_page_draws_and_ends_hidden() {
            let fixtures = fixtures();
            let has = |view: View| fixtures.iter().any(|f| f.view == view);
            assert!(has(View::Bubble) && has(View::Pill));
            assert_eq!(fixtures.last().map(|f| f.view), Some(View::Hidden));
        }

        #[test]
        fn the_demo_shows_each_title_of_the_bubble_and_both_hints() {
            let fixtures = fixtures();
            let kinds: Vec<WorkKind> = fixtures
                .iter()
                .filter_map(|f| f.working.as_ref().map(|w| w.kind))
                .collect();
            for kind in [
                WorkKind::Writing,
                WorkKind::Editing,
                WorkKind::Retranscribing,
            ] {
                assert!(kinds.contains(&kind), "{kind:?}");
            }
            assert!(fixtures.iter().any(|f| f.push_to_talk));
            assert!(fixtures.iter().any(|f| !f.push_to_talk));
            assert!(fixtures.iter().any(|f| f.phase == Phase::Finishing));
        }

        #[test]
        fn an_anchor_is_x_comma_y() {
            assert_eq!(
                parse_anchor("300, 200"),
                Some(Rect {
                    x: 300.0,
                    y: 200.0,
                    w: 2.0,
                    h: 18.0
                })
            );
            assert_eq!(parse_anchor("300"), None);
            assert_eq!(parse_anchor("a,b"), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use dictate_services::dictation::pipeline::NoticeKey;
    use serde_json::{json, Value};

    use super::*;

    /// A screen of 1440 x 900 points with a menu bar of 25.
    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 25.0,
        w: 1440.0,
        h: 875.0,
    };
    /// A panel: a bubble of 400 and its inset on each side.
    const SIZE: (f64, f64) = (428.0, 100.0);

    fn caret(x: f64, y: f64) -> Rect {
        Rect {
            x,
            y,
            w: 2.0,
            h: 18.0,
        }
    }

    #[test]
    fn place_below_when_room() {
        // 18 tall at y 200: its bottom is at 218, and 682 points of screen are under it.
        let (x, y, below) = place(Some(caret(300.0, 200.0)), SIZE, SCREEN, None);
        assert!(below);
        // The shape starts 24 left of the caret and 8 under its bottom; the window starts 14 before the shape.
        assert_eq!((x, y), (276.0, 218.0 + 8.0 - 14.0));
    }

    #[test]
    fn place_above_near_bottom() {
        // The bottom of the caret is at 818, 82 above the bottom of the screen: not the 240 that it takes.
        let (x, y, below) = place(Some(caret(300.0, 800.0)), SIZE, SCREEN, None);
        assert!(!below);
        // The shape ends 8 above the top of the caret; the window ends 14 after the shape.
        assert_eq!((x, y), (276.0, 800.0 - 8.0 + 14.0 - SIZE.1));
    }

    #[test]
    fn place_decides_at_exactly_240_points() {
        // The bottom of the caret is at 660: 240 are left under it. One less goes above.
        let (_, _, enough) = place(Some(caret(300.0, 642.0)), SIZE, SCREEN, None);
        let (_, _, short) = place(Some(caret(300.0, 643.0)), SIZE, SCREEN, None);
        assert!(enough);
        assert!(!short);
    }

    #[test]
    fn place_keeps_side_for_same_anchor() {
        // Little room, but the panel went below when there was more: it stays there while the anchor is the same.
        let near_bottom = caret(300.0, 800.0);
        let (_, y, below) = place(Some(near_bottom), SIZE, SCREEN, Some(true));
        assert!(below);
        assert_eq!(y, 818.0 + 8.0 - 14.0);
        // And the other way round: plenty of room, but it went above.
        let (_, y, below) = place(Some(caret(300.0, 400.0)), SIZE, SCREEN, Some(false));
        assert!(!below);
        assert_eq!(y, 400.0 - 8.0 + 14.0 - SIZE.1);
    }

    #[test]
    fn place_unanchored_bottom_centre_plus_60() {
        let (x, y, _) = place(None, SIZE, SCREEN, None);
        assert_eq!(x, 1440.0 / 2.0 - SIZE.0 / 2.0);
        // The window ends 60 above the bottom of the visible frame (900).
        assert_eq!(y, 900.0 - 60.0 - SIZE.1);
    }

    #[test]
    fn place_unanchored_on_a_screen_that_is_not_at_the_origin() {
        // A second screen on the right, 1920 wide, whose top is 100 points above the main one.
        let right = Rect {
            x: 1440.0,
            y: -100.0,
            w: 1920.0,
            h: 1080.0,
        };
        let (x, y, _) = place(None, SIZE, right, None);
        assert_eq!(x, 1440.0 + 1920.0 / 2.0 - SIZE.0 / 2.0);
        assert_eq!(y, 980.0 - 60.0 - SIZE.1);
    }

    #[test]
    fn place_clamps_to_screen() {
        // Too far right: the shape ends at the edge of the screen, and the inset may stick out of it.
        let (x, _, _) = place(Some(caret(1430.0, 200.0)), SIZE, SCREEN, None);
        assert_eq!(x, 1440.0 - SIZE.0 + 14.0);
        // Too far left: the shape starts at the edge.
        let (x, _, _) = place(Some(caret(5.0, 200.0)), SIZE, SCREEN, None);
        assert_eq!(x, -14.0);
        // Above, near the top: under the menu bar, the shape starts at the top of the visible frame.
        let (_, y, _) = place(Some(caret(300.0, 30.0)), SIZE, SCREEN, Some(false));
        assert_eq!(y, 25.0 - 14.0);
        // Below, with a panel too tall for what is left: it ends at the bottom.
        let tall = (428.0, 700.0);
        let (_, y, _) = place(Some(caret(300.0, 300.0)), tall, SCREEN, None);
        assert_eq!(y, 900.0 - tall.1 + 14.0);
    }

    // ---- What the windows are told ------------------------------------------------------------------------------

    fn state() -> PanelState {
        PanelState {
            view: View::Bubble,
            phase: Phase::Working,
            committed: "Hello".into(),
            partial: "there".into(),
            level: 0.5,
            language: "en-US".into(),
            duration_s: 12.5,
            hotkey: "\u{2303}\u{2325}Space".into(),
            push_to_talk: true,
            working: Some(Working {
                kind: WorkKind::Retranscribing,
                style: Some("concise".into()),
                model: Some("whisper-1".into()),
                editing: true,
            }),
            text: "Hello there".into(),
            original: Some("hello there".into()),
            style: Some("concise".into()),
            notice: Some(Notice {
                key: NoticeKey::ApiFailed,
                detail: Some("401".into()),
            }),
            from_chip: true,
            can_write: true,
            accessibility_missing: true,
        }
    }

    #[test]
    fn the_state_is_sent_to_the_page_with_the_names_the_page_reads() {
        assert_eq!(
            serde_json::to_value(state()).unwrap(),
            json!({
                "view": "bubble",
                "phase": "working",
                "committed": "Hello",
                "partial": "there",
                "level": 0.5,
                "language": "en-US",
                "durationS": 12.5,
                "hotkey": "\u{2303}\u{2325}Space",
                "pushToTalk": true,
                "working": {
                    "kind": "retranscribing",
                    "style": "concise",
                    "model": "whisper-1",
                    "editing": true
                },
                "text": "Hello there",
                "original": "hello there",
                "style": "concise",
                "notice": { "key": "api_failed", "detail": "401" },
                "fromChip": true,
                "canWrite": true,
                "accessibilityMissing": true
            })
        );
    }

    #[test]
    fn what_is_missing_is_sent_as_null() {
        let state = PanelState {
            working: None,
            original: None,
            style: None,
            notice: None,
            ..state()
        };
        let sent = serde_json::to_value(state).unwrap();
        for key in ["working", "original", "style", "notice"] {
            assert_eq!(sent[key], Value::Null, "{key}");
        }
    }

    #[test]
    fn every_view_and_kind_of_work_has_the_lowercase_name_the_page_expects() {
        let views = [
            (View::Hidden, "hidden"),
            (View::Bubble, "bubble"),
            (View::Pill, "pill"),
            (View::Chip, "chip"),
            (View::Card, "card"),
        ];
        for (view, name) in views {
            assert_eq!(serde_json::to_value(view).unwrap(), name);
        }
        let kinds = [
            (WorkKind::Writing, "writing"),
            (WorkKind::Editing, "editing"),
            (WorkKind::Rewriting, "rewriting"),
            (WorkKind::Retranscribing, "retranscribing"),
        ];
        for (kind, name) in kinds {
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
        }
    }

    #[test]
    fn a_hidden_state_shows_nothing() {
        let hidden = PanelState::hidden();
        assert_eq!(hidden.view, View::Hidden);
        assert_eq!(hidden.phase, Phase::Hidden);
        assert_eq!(hidden.view.window(), None);
    }

    #[test]
    fn the_bubble_and_the_chip_are_in_the_window_that_never_takes_focus() {
        assert_eq!(View::Bubble.window(), Some(BUBBLE));
        assert_eq!(View::Chip.window(), Some(BUBBLE));
    }

    #[test]
    fn the_pill_and_the_card_are_in_the_window_that_does() {
        // The pill is stopped with Return and cancelled with Esc, and the card is typed in.
        assert_eq!(View::Pill.window(), Some(CARD));
        assert_eq!(View::Card.window(), Some(CARD));
    }

    #[test]
    fn the_windows_are_named_as_the_page_and_the_capability_name_them() {
        let capability: Value =
            serde_json::from_str(include_str!("../capabilities/panel.json")).unwrap();
        assert_eq!(capability["windows"], json!([BUBBLE, CARD]));
        assert_eq!(WINDOWS, [BUBBLE, CARD]);
        assert_eq!(window_label("bubble"), Some(BUBBLE));
        assert_eq!(window_label("card"), Some(CARD));
        // The settings window is not one of ours to resize.
        assert_eq!(window_label("settings"), None);
        assert_eq!(window_label(""), None);
    }

    #[test]
    fn the_panel_windows_are_given_events_and_the_panel_commands_and_nothing_else() {
        // What the permissions let through is tested where the app is built (`main.rs`: the windows ask for every
        // command); this pins what the file grants: they listen for the state and stop listening, and they ask for
        // the commands of this file. They may not open pages, move windows or emit events.
        let capability: Value =
            serde_json::from_str(include_str!("../capabilities/panel.json")).unwrap();
        let commands = [
            "allow-dictation-languages",
            "allow-dictation-stop",
            "allow-dictation-cancel",
            "allow-dictation-set-language",
            "allow-panel-language",
            "allow-panel-resized",
        ];
        let mut wanted = vec!["core:event:allow-listen", "core:event:allow-unlisten"];
        wanted.extend(commands);
        assert_eq!(capability["permissions"], json!(wanted));
    }

    #[test]
    fn the_language_menu_lists_the_favourites_then_the_others_in_the_order_of_the_lists() {
        let lists = language_lists();
        assert_eq!(lists.favourites, FAVOURITES);
        assert_eq!(lists.others, OTHERS);
        assert_eq!(lists.favourites[0], "en-US");
        assert_eq!(
            serde_json::to_value(&lists).unwrap()["favourites"][1],
            "vi-VN"
        );
    }

    // ---- Sizes ---------------------------------------------------------------------------------------------------

    #[test]
    fn a_size_the_page_measures_is_taken_when_it_is_a_size() {
        assert_eq!(sane_size(428.0, 100.0), Some((428.0, 100.0)));
        assert_eq!(sane_size(1.0, 1.0), Some((1.0, 1.0)));
    }

    #[test]
    fn a_page_that_has_nothing_drawn_or_measures_nonsense_does_not_resize_the_window() {
        for (w, h) in [
            (0.0, 100.0),
            (428.0, 0.0),
            (-1.0, 100.0),
            (f64::NAN, 100.0),
            (428.0, f64::INFINITY),
            (MAX_SIDE + 1.0, 100.0),
            (428.0, MAX_SIDE + 1.0),
        ] {
            assert_eq!(sane_size(w, h), None, "{w} x {h}");
        }
    }

    // ---- Where ---------------------------------------------------------------------------------------------------

    #[test]
    fn a_new_anchor_decides_the_side_again() {
        let mut layout = Layout::default();
        layout.set_anchor(Some(caret(300.0, 200.0)));
        assert_eq!(layout.position(SIZE, SCREEN).1, 218.0 + 8.0 - 14.0);

        // A caret near the bottom of the screen: above it.
        layout.set_anchor(Some(caret(300.0, 800.0)));
        assert_eq!(layout.position(SIZE, SCREEN).1, 800.0 - 8.0 + 14.0 - SIZE.1);
    }

    #[test]
    fn the_side_is_decided_once_for_an_anchor_even_when_the_panel_grows() {
        let mut layout = Layout::default();
        layout.set_anchor(Some(caret(300.0, 400.0)));
        // Room below, so it goes below.
        let (_, below) = (layout.position(SIZE, SCREEN), layout.below);
        assert_eq!(below, Some(true));
        // A much taller panel doesn't change its mind about the side: it is pushed up by the clamp instead.
        let (_, y) = layout.position((428.0, 600.0), SCREEN);
        assert_eq!(layout.below, Some(true));
        assert_eq!(y, 900.0 - 600.0 + 14.0);
    }

    #[test]
    fn with_no_anchor_the_panel_is_at_the_bottom_centre_and_decides_nothing() {
        let mut layout = Layout::default();
        layout.set_anchor(None);
        let (x, y) = layout.position(SIZE, SCREEN);
        assert_eq!((x, y), (1440.0 / 2.0 - SIZE.0 / 2.0, 900.0 - 60.0 - SIZE.1));
        assert_eq!(layout.below, None);
    }

    #[test]
    fn the_screen_to_ask_about_is_the_one_of_the_anchor() {
        let mut layout = Layout::default();
        assert_eq!(layout.near(), None);
        layout.set_anchor(Some(caret(300.0, 200.0)));
        assert_eq!(layout.near(), Some((300.0, 200.0)));
    }

    #[test]
    fn wayland_has_no_place_to_put_a_window() {
        assert!(!positioning_works(true));
        // X11, Windows and macOS do.
        assert!(positioning_works(false));
    }

    #[test]
    fn a_window_is_shown_when_hidden_and_hidden_when_shown_and_otherwise_left_alone() {
        let mut shown = Shown::default();
        assert_eq!(shown.set(BUBBLE, true), Change::Appeared);
        assert_eq!(shown.set(BUBBLE, true), Change::Same);
        assert_eq!(shown.set(CARD, false), Change::Same);
        assert_eq!(shown.set(BUBBLE, false), Change::Disappeared);
        assert_eq!(shown.set(BUBBLE, false), Change::Same);
    }

    #[test]
    fn the_windows_to_show_and_hide_for_a_view() {
        assert_eq!(wanted(View::Bubble), [(BUBBLE, true), (CARD, false)]);
        assert_eq!(wanted(View::Chip), [(BUBBLE, true), (CARD, false)]);
        assert_eq!(wanted(View::Pill), [(BUBBLE, false), (CARD, true)]);
        assert_eq!(wanted(View::Card), [(BUBBLE, false), (CARD, true)]);
        assert_eq!(wanted(View::Hidden), [(BUBBLE, false), (CARD, false)]);
    }

    #[test]
    fn place_a_panel_bigger_than_the_screen_does_not_panic() {
        let (x, y, _) = place(Some(caret(300.0, 300.0)), (2000.0, 1200.0), SCREEN, None);
        // The right and bottom edges win, as they did in the Swift app.
        assert_eq!((x, y), (1440.0 - 2000.0 + 14.0, 900.0 - 1200.0 + 14.0));
    }
}
