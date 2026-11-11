//! The recording bar: the small window that shows the time, the meters and the buttons while a recording goes on. It is made the
//! first time a recording begins (its page is `recorder.html`), shown when a recording begins and hidden when it is over.
//!
//! It stands at the top and in the middle of the primary screen, 12 points below the top of the part that no menu bar covers, and
//! each time it is shown it goes back there: where the person dragged it is not kept (as in the Swift app). It never takes the
//! focus. On macOS it is a non-activating panel (as the bubble of the dictation is), elsewhere a window that refuses the focus.
//!
//! What a recording of the screen would otherwise show of the app is kept out of it: the bar asks the system to leave its content
//! out of any capture of the screen on every OS, and on Windows so do the other windows of the app while a recording goes on. macOS
//! has the recording leave out the whole app, and Linux can't do either.
//!
//! Showing and hiding call the system for a window, so they are for a blocking thread, not for the thread that runs the interface
//! (the same as the meetings window): the recorder calls them from the thread of whoever starts or ends the recording.

use tauri::{
    AppHandle, LogicalPosition, Manager, PhysicalPosition, PhysicalRect, Runtime, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};

use super::views::Phase;
use super::Recorder;

/// The label of the window of the bar. The recorder tells it its state, as it tells the meetings window.
pub const LABEL: &str = "recorder";

/// The page of the bar, in the folder of the UI's build.
const PAGE: &str = "recorder.html";

/// How big the bar is, in points: the capsule and room around it for its shadow.
const WIDTH: f64 = 520.0;
const HEIGHT: f64 = 64.0;

/// How far below the top of the work area the bar stands, in points.
const TOP_GAP: f64 = 12.0;

/// Shows the bar at its place, making its window the first time.
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = window(app) else { return };
    place(&window);
    keep_other_windows_out(app, true);
    set_visible(app, &window, true);
}

/// Hides the bar. A bar that was never made stays unmade.
pub fn hide<R: Runtime>(app: &AppHandle<R>) {
    keep_other_windows_out(app, false);
    if let Some(window) = app.get_webview_window(LABEL) {
        set_visible(app, &window, false);
    }
}

/// Whether a window that is made now is to be kept out of the recording of the screen: on Windows, while a recording goes on.
pub fn protects_other_windows<R: Runtime>(app: &AppHandle<R>) -> bool {
    let phase = app
        .try_state::<Recorder>()
        .map_or(Phase::Idle, |recorder| recorder.view().phase);
    protects_during(cfg!(windows), phase)
}

fn protects_during(on_windows: bool, phase: Phase) -> bool {
    on_windows && phase != Phase::Idle
}

/// The windows of the app that are kept out of a recording of the screen with the system's own means: the meetings window and
/// Settings, on Windows. (The bar keeps itself out on every OS, from the start.)
fn protected_windows(on_windows: bool) -> &'static [&'static str] {
    if on_windows {
        &[crate::meetings::window::LABEL, "settings"]
    } else {
        &[]
    }
}

/// Puts the windows of the app that show in a recording of the screen out of it while the bar is shown, and back in the picture
/// when it is hidden.
fn keep_other_windows_out<R: Runtime>(app: &AppHandle<R>, out: bool) {
    for label in protected_windows(cfg!(windows)) {
        let Some(window) = app.get_webview_window(label) else {
            continue;
        };
        if let Err(e) = window.set_content_protected(out) {
            log::warn!(
                "Could not change whether the {label} window is kept out of a recording: {e}"
            );
        }
    }
}

/// The window of the bar, made when there is none yet.
fn window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    if let Some(window) = app.get_webview_window(LABEL) {
        return Some(window);
    }
    match build(app) {
        Ok(window) => Some(window),
        Err(e) => {
            log::warn!("Could not make the recording bar: {e}");
            None
        }
    }
}

/// What the bar is as a window: transparent and without a frame or a shadow (the page draws its own), above the other windows, on
/// every desktop, out of the taskbar and out of the focus, and out of any capture of the screen. It starts hidden.
fn shaped<'a, R: Runtime>(
    builder: WebviewWindowBuilder<'a, R, AppHandle<R>>,
) -> WebviewWindowBuilder<'a, R, AppHandle<R>> {
    builder
        .title("Dictate")
        .inner_size(WIDTH, HEIGHT)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .focusable(false)
        .focused(false)
        .content_protected(true)
        .visible(false)
}

/// A window that refuses the focus (a no-activate window on Windows, one that does not accept focus in GTK).
fn plain<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    shaped(WebviewWindowBuilder::new(
        app,
        LABEL,
        WebviewUrl::App(PAGE.into()),
    ))
    .build()
}

/// Windows and Linux.
#[cfg(not(target_os = "macos"))]
fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    plain(app)
}

/// Whether the panels of AppKit are there: the plugin that makes them is the app's, and a bare runtime (a test) has none.
#[cfg(target_os = "macos")]
fn has_panels<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<tauri_nspanel::WebviewPanelManager<R>>()
        .is_some()
}

/// macOS: a non-activating panel that can never become the key window, as the bubble of the dictation is: showing a plain window
/// makes the app the active one, and the window that the person works in loses the keyboard. A panel is made on the main thread.
#[cfg(target_os = "macos")]
fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    if !has_panels(app) {
        return plain(app);
    }
    on_main_thread(app, make_panel)??;
    app.get_webview_window(LABEL)
        .ok_or(tauri::Error::WindowNotFound)
}

#[cfg(target_os = "macos")]
fn make_panel<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    use tauri_nspanel::{tauri_panel, CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};

    tauri_panel! {
        panel!(RecorderPanel {
            config: {
                can_become_key_window: false,
                can_become_main_window: false,
                is_floating_panel: true
            }
        })
    }

    PanelBuilder::<R, RecorderPanel<R>>::new(app, LABEL)
        .url(WebviewUrl::App(PAGE.into()))
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
        // Dragged by its background, as the bar of the Swift app is: the page has no drag handle to give.
        .movable_by_window_background(true)
        .with_window(shaped)
        .build()
        .map(drop)
}

/// Runs `work` on the main thread, where AppKit may be touched, and waits for what it gives (not for long: the main thread has
/// no business with a blocking one). On the main thread already it is run at once.
#[cfg(target_os = "macos")]
fn on_main_thread<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    work: impl FnOnce(&AppHandle<R>) -> T + Send + 'static,
) -> tauri::Result<T> {
    if objc2::MainThreadMarker::new().is_some() {
        return Ok(work(app));
    }
    let (answer, wait) = std::sync::mpsc::sync_channel(1);
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = answer.send(work(&handle));
    })?;
    wait.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| tauri::Error::FailedToReceiveMessage)
}

/// Shows or hides the window. A panel is ordered front or out on the main thread, in the order the calls came.
#[cfg(target_os = "macos")]
fn set_visible<R: Runtime>(app: &AppHandle<R>, window: &WebviewWindow<R>, visible: bool) {
    use tauri_nspanel::ManagerExt;

    if !has_panels(app) {
        return plain_set_visible(window, visible);
    }
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || match handle.get_webview_panel(LABEL) {
        Ok(panel) if visible => panel.show(),
        Ok(panel) => panel.hide(),
        Err(e) => log::warn!("There is no recording bar panel to show or hide: {e:?}"),
    });
    if let Err(e) = queued {
        log::warn!("Could not reach the main thread to show or hide the recording bar: {e}");
    }
}

#[cfg(not(target_os = "macos"))]
fn set_visible<R: Runtime>(_app: &AppHandle<R>, window: &WebviewWindow<R>, visible: bool) {
    plain_set_visible(window, visible);
}

fn plain_set_visible<R: Runtime>(window: &WebviewWindow<R>, visible: bool) {
    let done = if visible {
        window.show()
    } else {
        window.hide()
    };
    if let Err(e) = done {
        log::warn!("Could not show or hide the recording bar: {e}");
    }
}

/// Puts the window at the top and in the middle of the primary screen.
fn place<R: Runtime>(window: &WebviewWindow<R>) {
    let monitor = match window.primary_monitor() {
        Ok(Some(monitor)) => monitor,
        Ok(None) => return,
        Err(e) => {
            log::warn!("Could not find the primary screen to place the recording bar on: {e}");
            return;
        }
    };
    let scale = monitor.scale_factor();
    let at = top_centre(monitor.work_area(), scale);
    // macOS counts in points of one space for every screen, and turns pixels to points with the scale of the screen the window is
    // on now, which is not always this one. Elsewhere the pixels are of the screen the window goes to.
    let placed = if cfg!(target_os = "macos") {
        window.set_position(LogicalPosition::new(
            f64::from(at.x) / scale,
            f64::from(at.y) / scale,
        ))
    } else {
        window.set_position(at)
    };
    if let Err(e) = placed {
        log::warn!("Could not place the recording bar: {e}");
    }
}

/// Where the top left corner of the bar goes, in pixels, for a screen whose work area is `area` and which has `scale` pixels to the
/// point: in the middle, and `TOP_GAP` points below the top. A screen narrower than the bar has it start at its left edge.
fn top_centre(area: &PhysicalRect<i32, u32>, scale: f64) -> PhysicalPosition<i32> {
    let width = (WIDTH * scale).round() as i32;
    let room = i32::try_from(area.size.width).unwrap_or(i32::MAX) - width;
    PhysicalPosition::new(
        area.position.x + room.max(0) / 2,
        area.position.y + (TOP_GAP * scale).round() as i32,
    )
}

#[cfg(test)]
mod tests {
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
    use tauri::{App, PhysicalPosition, PhysicalRect, PhysicalSize};

    use super::*;

    fn app() -> App<MockRuntime> {
        mock_builder()
            .build(mock_context(noop_assets()))
            .expect("a mock app")
    }

    fn area(x: i32, y: i32, width: u32, height: u32) -> PhysicalRect<i32, u32> {
        PhysicalRect {
            position: PhysicalPosition::new(x, y),
            size: PhysicalSize::new(width, height),
        }
    }

    #[test]
    fn the_hud_is_created_once_with_its_label_and_page() {
        let app = app();
        let handle = app.handle();
        assert!(handle.get_webview_window(LABEL).is_none());

        show(handle);
        show(handle);

        let windows = handle.webview_windows();
        assert_eq!(windows.keys().collect::<Vec<_>>(), ["recorder"]);
        let url = windows[LABEL].url().unwrap();
        assert!(url.as_str().ends_with("recorder.html"), "{url}");
    }

    #[test]
    fn hiding_a_bar_that_was_never_shown_makes_none() {
        let app = app();

        hide(app.handle());

        assert!(app.handle().webview_windows().is_empty());
    }

    #[test]
    fn the_bar_can_be_shown_again_after_it_was_hidden_and_stays_one_window() {
        let app = app();
        let handle = app.handle();

        show(handle);
        hide(handle);
        show(handle);

        assert_eq!(handle.webview_windows().len(), 1);
    }

    #[test]
    fn showing_the_bar_leaves_the_other_windows_of_the_app_as_they_are() {
        let app = app();
        let handle = app.handle();
        for label in ["settings", "meetings"] {
            WebviewWindowBuilder::new(handle, label, WebviewUrl::default())
                .build()
                .unwrap();
        }

        show(handle);
        hide(handle);

        let mut labels: Vec<String> = handle.webview_windows().into_keys().collect();
        labels.sort();
        assert_eq!(labels, ["meetings", "recorder", "settings"]);
    }

    #[test]
    fn the_bar_is_12_points_below_the_top_of_the_work_area_and_in_the_middle() {
        // 520 points wide on a screen of 1920 pixels with a menu bar of 25: the centre, and 12 below the bar.
        assert_eq!(
            top_centre(&area(0, 25, 1920, 1055), 1.0),
            PhysicalPosition::new(700, 37)
        );
        // On a screen of twice the pixels the points are twice the pixels too.
        assert_eq!(
            top_centre(&area(0, 50, 3840, 2110), 2.0),
            PhysicalPosition::new(1400, 74)
        );
        // The primary screen need not start at the origin of the desktop.
        assert_eq!(
            top_centre(&area(-1920, 0, 1920, 1040), 1.0),
            PhysicalPosition::new(-1220, 12)
        );
        // A scale of one and a half has pixels that are not whole points: the nearest pixel.
        assert_eq!(
            top_centre(&area(0, 0, 1920, 1080), 1.5),
            PhysicalPosition::new(570, 18)
        );
    }

    #[test]
    fn a_screen_narrower_than_the_bar_does_not_push_it_off_the_left_edge() {
        assert_eq!(
            top_centre(&area(100, 0, 400, 800), 1.0),
            PhysicalPosition::new(100, 12)
        );
    }

    #[test]
    fn the_windows_that_are_kept_out_of_the_video_are_the_meetings_and_settings_only_on_windows() {
        assert_eq!(protected_windows(true), ["meetings", "settings"]);
        assert!(protected_windows(false).is_empty());
    }

    #[test]
    fn a_window_that_opens_during_a_recording_is_kept_out_of_it_on_windows() {
        for phase in [
            Phase::Starting,
            Phase::Recording,
            Phase::Paused,
            Phase::Finishing,
        ] {
            assert!(protects_during(true, phase), "{phase:?}");
            assert!(!protects_during(false, phase), "{phase:?}");
        }
        assert!(!protects_during(true, Phase::Idle));
        // With no recorder (a bare app) there is nothing to keep out.
        assert!(!protects_other_windows(app().handle()));
    }
}
