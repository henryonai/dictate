//! The meetings window: made the first time it is asked for, shown again after that.
//!
//! Closing it hides it (the `on_window_event` of the app does that for every window), so that its list and what is selected in
//! it are as they were when it comes back, and the app stays in the tray.

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// The label of the window: the capability file `meetings.json` names it.
pub const LABEL: &str = "meetings";

/// The page of the window, in the folder of the UI's build.
const PAGE: &str = "meetings.html";

/// What the window is at first, and the least it can be made.
const WIDTH: f64 = 1240.0;
const HEIGHT: f64 = 780.0;
const MIN_WIDTH: f64 = 960.0;
const MIN_HEIGHT: f64 = 600.0;

/// Shows the meetings window and brings it to the front, making it when there is none yet.
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    let window = match app.get_webview_window(LABEL) {
        Some(window) => window,
        None => match build(app) {
            Ok(window) => window,
            Err(e) => {
                log::warn!("Could not make the meetings window: {e}");
                return;
            }
        },
    };
    let shown = window
        .show()
        .and_then(|()| window.unminimize())
        .and_then(|()| window.set_focus());
    if let Err(e) = shown {
        log::warn!("Could not show the meetings window: {e}");
    }
}

fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(PAGE.into()))
        .title("Dictate")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .center()
        .build()
}

#[cfg(test)]
mod tests {
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
    use tauri::App;

    use super::*;

    fn app() -> App<MockRuntime> {
        mock_builder()
            .build(mock_context(noop_assets()))
            .expect("a mock app")
    }

    #[test]
    fn the_window_is_made_the_first_time_and_shown_again_after() {
        let app = app();
        let handle = app.handle();
        assert!(handle.get_webview_window(LABEL).is_none());

        show(handle);
        let window = handle.get_webview_window(LABEL).expect("the window");
        assert_eq!(window.label(), "meetings");

        // Asking again shows the same window: closing only hid it, and no second one is made. (The mock window keeps no
        // track of being shown or hidden.)
        window.hide().unwrap();
        show(handle);
        let windows = handle.webview_windows();
        assert_eq!(
            windows
                .keys()
                .filter(|label| label.as_str() == LABEL)
                .count(),
            1
        );
        assert_eq!(windows.len(), 1);
    }

    #[test]
    fn the_page_is_meetings_html() {
        let app = app();
        show(app.handle());
        let window = app.get_webview_window(LABEL).unwrap();

        let url = window.url().unwrap();
        assert!(url.as_str().ends_with("meetings.html"), "{url}");
        // The title and the size (1240 x 780, at least 960 x 600) are the builder's: the mock window reports neither.
    }
}
