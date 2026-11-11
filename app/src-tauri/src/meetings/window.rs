//! The meetings window: made the first time it is asked for, shown again after that.
//!
//! Closing it hides it (the `on_window_event` of the app does that for every window), so that its list and what is selected in
//! it are as they were when it comes back, and the app stays in the tray. A hidden web view goes on playing, so the page is told
//! (`meetings-hidden`) and pauses its player.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::webview::PageLoadEvent;
use tauri::{
    AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use super::views::ErrorView;

/// The label of the window: the capability file `meetings.json` names it.
pub const LABEL: &str = "meetings";

/// The page of the window, in the folder of the UI's build.
const PAGE: &str = "meetings.html";

/// What the window is at first, and the least it can be made.
const WIDTH: f64 = 1240.0;
const HEIGHT: f64 = 780.0;
const MIN_WIDTH: f64 = 960.0;
const MIN_HEIGHT: f64 = 600.0;

/// The event that tells the page its window was closed. Closing only hides the window, and a hidden web view goes on playing:
/// the page pauses the player on this.
pub const HIDDEN_EVENT: &str = "meetings-hidden";

/// The event that asks the page to show the picker of the recording ("Record & summarize"), with an [`OpenRecorder`] as its payload.
pub const OPEN_RECORDER_EVENT: &str = "meetings-open-recorder";

/// What the page is told when the picker is asked for.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpenRecorder {
    /// Why the recording did not begin, when the picker is opened because it could not (a start from the shortcut): the picker
    /// shows it above its choices.
    pub error: Option<ErrorView>,
}

/// A window was hidden by a request to close it: when it is the meetings window, its page is told so.
pub fn window_hidden<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if label != LABEL {
        return;
    }
    if let Err(e) = app.emit_to(LABEL, HIDDEN_EVENT, ()) {
        log::warn!("Could not tell the meetings page that its window was closed: {e}");
    }
}

/// Shows the meetings window and brings it to the front, making it when there is none yet.
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    let window = match app.get_webview_window(LABEL) {
        Some(window) => window,
        None => match builder(app).build() {
            Ok(window) => window,
            Err(e) => {
                log::warn!("Could not make the meetings window: {e}");
                return;
            }
        },
    };
    bring_forward(&window);
}

/// Shows the meetings window with the picker of the recording open, making the window when there is none yet. `error` is why a
/// recording did not begin, for the picker to show.
///
/// A window that is made now has no page to hear the event yet: it is sent when the page has loaded.
pub fn show_recorder<R: Runtime>(app: &AppHandle<R>, error: Option<&ErrorView>) {
    let open = OpenRecorder {
        error: error.cloned(),
    };
    if let Some(window) = app.get_webview_window(LABEL) {
        bring_forward(&window);
        tell_the_page(&window, &open);
        return;
    }
    // The page is loaded once for the life of the window: the event goes with that load and with no later one.
    let told = Arc::new(AtomicBool::new(false));
    let on_load = {
        let open = open.clone();
        move |window: WebviewWindow<R>, payload: tauri::webview::PageLoadPayload<'_>| {
            if payload.event() == PageLoadEvent::Finished && !told.swap(true, Ordering::SeqCst) {
                tell_the_page(&window, &open);
            }
        }
    };
    match builder(app).on_page_load(on_load).build() {
        Ok(window) => bring_forward(&window),
        Err(e) => log::warn!("Could not make the meetings window: {e}"),
    }
}

fn tell_the_page<R: Runtime>(window: &WebviewWindow<R>, open: &OpenRecorder) {
    if let Err(e) = window.emit_to(LABEL, OPEN_RECORDER_EVENT, open) {
        log::warn!("Could not ask the meetings page for the picker: {e}");
    }
}

fn bring_forward<R: Runtime>(window: &WebviewWindow<R>) {
    let shown = window
        .show()
        .and_then(|()| window.unminimize())
        .and_then(|()| window.set_focus());
    if let Err(e) = shown {
        log::warn!("Could not show the meetings window: {e}");
    }
}

fn builder<R: Runtime>(app: &AppHandle<R>) -> WebviewWindowBuilder<'_, R, AppHandle<R>> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(PAGE.into()))
        .title("Dictate")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .center()
        // Opened while a recording goes on, it must not be in the recording of the screen either.
        .content_protected(crate::recording::hud::protects_other_windows(app))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
    use tauri::{App, Listener};

    use super::*;
    use crate::meetings::views::ErrorView;

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
    fn closing_the_meetings_window_tells_its_page_and_no_other_window_does() {
        let app = app();
        let handle = app.handle();
        show(handle);
        let window = handle.get_webview_window(LABEL).expect("the window");
        let heard = Arc::new(AtomicUsize::new(0));
        let count = heard.clone();
        window.listen(HIDDEN_EVENT, move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        });

        // The settings window is hidden by the same handler: the meetings page has no news of it.
        window_hidden(handle, "settings");
        assert_eq!(heard.load(Ordering::SeqCst), 0);

        window_hidden(handle, LABEL);
        assert_eq!(heard.load(Ordering::SeqCst), 1);
    }

    /// The payloads of the events of the picker that the window hears.
    fn listen_for_the_picker(window: &WebviewWindow<MockRuntime>) -> Arc<Mutex<Vec<String>>> {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let sink = heard.clone();
        window.listen(OPEN_RECORDER_EVENT, move |event| {
            sink.lock().unwrap().push(event.payload().to_owned());
        });
        heard
    }

    #[test]
    fn the_picker_is_asked_for_by_an_event_that_carries_the_error_if_there_is_one() {
        let app = app();
        let handle = app.handle();
        show(handle);
        let window = handle.get_webview_window(LABEL).expect("the window");
        let heard = listen_for_the_picker(&window);

        show_recorder(handle, None);
        let error = ErrorView {
            kind: "Other".into(),
            key: "recording.error.needs_ffmpeg".into(),
            detail: Some("no ffmpeg".into()),
            command: Some("brew install ffmpeg".into()),
        };
        show_recorder(handle, Some(&error));

        let heard = heard.lock().unwrap().clone();
        assert_eq!(heard.len(), 2, "{heard:?}");
        let payload = |text: &str| serde_json::from_str::<serde_json::Value>(text).unwrap();
        assert_eq!(payload(&heard[0]), serde_json::json!({ "error": null }));
        assert_eq!(
            payload(&heard[1]),
            serde_json::json!({
                "error": {
                    "kind": "Other",
                    "key": "recording.error.needs_ffmpeg",
                    "detail": "no ffmpeg",
                    "command": "brew install ffmpeg",
                }
            })
        );
    }

    #[test]
    fn asking_for_the_picker_makes_the_window_when_there_is_none_and_makes_no_second() {
        let app = app();
        let handle = app.handle();
        assert!(handle.get_webview_window(LABEL).is_none());

        show_recorder(handle, None);
        assert!(handle.get_webview_window(LABEL).is_some());
        show_recorder(handle, None);

        assert_eq!(handle.webview_windows().len(), 1);
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
