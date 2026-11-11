//! The meetings window: made the first time it is asked for, shown again after that.
//!
//! Closing it hides it (the `on_window_event` of the app does that for every window), so that its list and what is selected in
//! it are as they were when it comes back, and the app stays in the tray. A hidden web view goes on playing, so the page is told
//! (`meetings-hidden`) and pauses its player.

use serde::Serialize;
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
/// It reaches a page that is there to hear it, which is a window that was made before.
pub const OPEN_RECORDER_EVENT: &str = "meetings-open-recorder";

/// The global that a window made for the picker has before any script of its page runs, holding the same [`OpenRecorder`] as JSON.
/// A page that is not there yet cannot hear [`OPEN_RECORDER_EVENT`]: Tauri sends an event only to a page that has registered a
/// listener for it, which the page does when it has started, and nothing orders the two.
///
/// The contract for the page: when it mounts it reads `window.__dictateOpenRecorder`, deletes it, and shows the picker (with the
/// error, if there is one) when it was there; and it listens for `meetings-open-recorder` for the asks that come later. The global
/// is set again by a reload of the window, which the app does not do (a development build does).
pub const OPEN_RECORDER_GLOBAL: &str = "__dictateOpenRecorder";

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
/// A window that is there has its page told by the event [`OPEN_RECORDER_EVENT`]. A window that is made now has no page to hear
/// it yet, so it is made with [`OPEN_RECORDER_GLOBAL`], which the page reads when it mounts.
pub fn show_recorder<R: Runtime>(app: &AppHandle<R>, error: Option<&ErrorView>) {
    let open = OpenRecorder {
        error: error.cloned(),
    };
    if let Some(window) = app.get_webview_window(LABEL) {
        bring_forward(&window);
        tell_the_page(&window, &open);
        return;
    }
    match builder(app)
        .initialization_script(open_recorder_script(&open))
        .build()
    {
        Ok(window) => bring_forward(&window),
        Err(e) => log::warn!("Could not make the meetings window: {e}"),
    }
}

/// The script that gives [`OPEN_RECORDER_GLOBAL`] its value: one statement. The error says what a tool or the system said, which
/// is no code: the value is JSON, and the characters that could end a string, a script element or (for older scripts) a line are
/// escaped, so that it is read as data whatever it holds.
fn open_recorder_script(open: &OpenRecorder) -> String {
    let json = serde_json::to_string(open).unwrap_or_else(|_| r#"{"error":null}"#.to_owned());
    let json = json
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    format!("window.{OPEN_RECORDER_GLOBAL} = {json};")
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

    /// What the script gives the global, read back as the JSON it is.
    fn literal_of(script: &str) -> serde_json::Value {
        let prefix = format!("window.{OPEN_RECORDER_GLOBAL} = ");
        let literal = script
            .strip_prefix(&prefix)
            .expect("an assignment to the global");
        let literal = literal.strip_suffix(';').expect("one statement");
        serde_json::from_str(literal).expect("a literal of JSON")
    }

    #[test]
    fn the_page_of_a_window_made_for_the_picker_finds_what_to_open_in_a_global_at_start() {
        // The name is the contract with the page: it reads this global when it mounts and deletes it.
        assert_eq!(OPEN_RECORDER_GLOBAL, "__dictateOpenRecorder");

        let none = open_recorder_script(&OpenRecorder { error: None });
        assert_eq!(literal_of(&none), serde_json::json!({ "error": null }));

        let error = ErrorView {
            kind: "MissingTool".into(),
            key: "recording.error.needs_ffmpeg".into(),
            detail: None,
            command: Some("brew install ffmpeg".into()),
        };
        let some = open_recorder_script(&OpenRecorder { error: Some(error) });
        assert_eq!(
            literal_of(&some),
            serde_json::json!({
                "error": {
                    "kind": "MissingTool",
                    "key": "recording.error.needs_ffmpeg",
                    "detail": null,
                    "command": "brew install ffmpeg",
                }
            })
        );
    }

    #[test]
    fn what_an_error_says_goes_into_the_script_as_data_and_never_as_code() {
        // Quotes and backslashes that would end a string, a comment, a template, a script element, and the line separators that
        // older scripts take for the end of a line: the detail of an error is whatever a tool or the system said.
        let nasty = "\"; alert(1); //\\ ' ` ${x} </script><script>alert(2)</script> <!-- & > \u{2028}\u{2029}\n\r\t";
        let open = OpenRecorder {
            error: Some(ErrorView {
                kind: "Other".into(),
                key: "recording.error.source".into(),
                detail: Some(nasty.into()),
                command: Some(nasty.into()),
            }),
        };

        let script = open_recorder_script(&open);

        // It reads back as exactly what was said, and the whole of it is that one literal.
        assert_eq!(literal_of(&script), serde_json::to_value(&open).unwrap());
        assert_eq!(
            literal_of(&script)["error"]["detail"],
            serde_json::Value::String(nasty.to_owned())
        );
        for raw in ['<', '>', '&', '\u{2028}', '\u{2029}', '\n', '\r'] {
            assert!(!script.contains(raw), "{raw:?} is in {script}");
        }
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
