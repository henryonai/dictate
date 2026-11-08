//! What the operating system knows about where the user is typing: the frontmost app, the text field that has
//! the focus and where its caret is, and whether the app may use the microphone and (on macOS) the Accessibility
//! API. Dictation asks these questions before it shows its bubble and after it writes, and it asks while another
//! app has the focus, so every answer can be a "don't know".

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
pub mod rules;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(any(test, target_os = "macos", target_os = "linux"))]
use std::sync::mpsc;
use std::sync::Arc;
#[cfg(any(test, target_os = "macos", target_os = "linux"))]
use std::thread;
#[cfg(any(test, target_os = "macos", target_os = "linux"))]
use std::time::Duration;

/// How long the caller of a question to the OS waits at most. Another app can hang, and it answers slowly or not
/// at all: dictation then goes on without the answer, so that the hotkey never freezes. (Windows has its own,
/// shorter time, for UI Automation.)
#[cfg(any(test, target_os = "macos", target_os = "linux"))]
pub const ASK_TIMEOUT: Duration = Duration::from_millis(500);

/// The visible frame of a screen when the OS doesn't say what the screens are.
pub const DEFAULT_SCREEN: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 1440.0,
    h: 900.0,
};

/// A rectangle in logical points, in one global space with the origin at the top left of the primary screen
/// (macOS flips its bottom-left origin to this, Windows divides physical pixels by the scale of the screen).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// An app: the bundle id on macOS, the file name of the executable on Windows ("chrome.exe"), the window class
/// (`WM_CLASS`) on X11.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppId {
    pub id: String,
    pub pid: Option<u32>,
}

/// A text field that can take what was dictated.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo {
    pub app: AppId,
    /// The app is a terminal: it takes Ctrl+Shift+V, and no text can be read back from it.
    pub terminal: bool,
    /// Where the caret is, or the field itself, when the app says: where the bubble goes.
    pub anchor: Option<Rect>,
}

/// What looking for the focused field found. Linux never finds `NoField`: it can't tell.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum FieldLookup {
    Field(FieldInfo),
    /// The app has the focus on something that doesn't take text: a button, a password field, the desktop.
    NoField,
    /// The system doesn't say (no permission, a hung app, Wayland): the text may well have a place to go.
    Unknown,
}

/// The text length and the selection of the focused field, to tell whether a paste landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSnapshot {
    pub len: usize,
    pub sel_loc: usize,
    pub sel_len: usize,
}

// Each system builds the answers it can give: only Windows and Linux say `NotNeeded`, only macOS the others.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Granted,
    Denied,
    NotDetermined,
    /// The system has no such permission.
    NotNeeded,
}

/// What dictation asks the system about the place it writes to. Every question may take a few hundred
/// milliseconds when the app that has the focus is slow, and none takes longer than [`ASK_TIMEOUT`]: the answer is
/// then "don't know". The questions can be asked from any thread, the main thread included (the global shortcut
/// handler runs there): a question asked on the main thread doesn't wait for the main thread.
pub trait Platform: Send + Sync {
    /// The app that has the focus, or `None` when the system doesn't say (Wayland has no such question).
    fn frontmost_app(&self) -> Option<AppId>;
    /// The text field that has the focus. Looks at the app that is in front, so the caller hides its own panels
    /// first.
    fn focused_field(&self) -> FieldLookup;
    /// The text length and the selection of the focused field, or `None` when they can't be read.
    fn field_snapshot(&self) -> Option<FieldSnapshot>;
    /// Where the caret of the focused field is now, to put the bubble next to it.
    fn caret_anchor(&self) -> Option<Rect>;
    /// Where the mouse pointer is, when the system says.
    fn mouse_position(&self) -> Option<(f64, f64)>;
    /// The part of the screen that no menu bar or taskbar covers: the screen that has `near`, else the main one.
    fn screen_visible_frame(&self, near: Option<(f64, f64)>) -> Rect;
    /// Whether the app may read other apps' text fields and press keys in them (macOS only).
    fn accessibility(&self) -> Permission;
    /// Shows the system's prompt for the Accessibility permission, which is the way to it on macOS.
    fn request_accessibility(&self);
    fn microphone(&self) -> Permission;
    /// Asks for the microphone, which shows the system's prompt the first time, and calls `done` with the answer.
    fn request_microphone(&self, done: Box<dyn FnOnce(bool) + Send>);
}

/// The platform layer of this system. `app` is the way to its main thread, where macOS answers some questions.
pub fn current(app: &tauri::AppHandle) -> Arc<dyn Platform> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::MacPlatform::new(app.clone()))
    }
    #[cfg(target_os = "windows")]
    {
        let _ = app;
        Arc::new(windows::WindowsPlatform::new())
    }
    #[cfg(target_os = "linux")]
    {
        let app = app.clone();
        Arc::new(linux::LinuxPlatform::new(Box::new(move |near| {
            tauri_screen(&app, near)
        })))
    }
}

/// The visible frame of a screen as Tauri knows the monitors, for the systems that have no question of their own
/// for it (Linux). Asks the main thread, where Tauri reads the monitors.
#[cfg(target_os = "linux")]
fn tauri_screen(app: &tauri::AppHandle, near: Option<(f64, f64)>) -> Rect {
    let (answer, wait) = mpsc::sync_channel(1);
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        let mut monitors = handle.available_monitors().unwrap_or_default();
        // The primary monitor is the main screen, and the main screen goes first.
        if let Ok(Some(primary)) = handle.primary_monitor() {
            let same = |m: &tauri::window::Monitor| {
                m.position() == primary.position() && m.size() == primary.size()
            };
            if let Some(i) = monitors.iter().position(same) {
                monitors.swap(0, i);
            }
        }
        let screens: Vec<rules::Screen> = monitors.iter().map(monitor_screen).collect();
        let _ = answer.send(screens);
    });
    let screens = match queued {
        Ok(()) => wait.recv_timeout(ASK_TIMEOUT).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    rules::pick_visible_frame(&screens, near).unwrap_or(DEFAULT_SCREEN)
}

/// A monitor in logical points: Tauri reports physical pixels.
#[cfg(target_os = "linux")]
fn monitor_screen(monitor: &tauri::window::Monitor) -> rules::Screen {
    let scale = monitor.scale_factor();
    let frame = Rect {
        x: f64::from(monitor.position().x),
        y: f64::from(monitor.position().y),
        w: f64::from(monitor.size().width),
        h: f64::from(monitor.size().height),
    };
    let work = monitor.work_area();
    let visible = Rect {
        x: f64::from(work.position.x),
        y: f64::from(work.position.y),
        w: f64::from(work.size.width),
        h: f64::from(work.size.height),
    };
    rules::Screen {
        frame: rules::to_logical(frame, scale),
        visible: rules::to_logical(visible, scale),
    }
}

/// Runs `job` on a thread of its own and waits `timeout` for its answer. `None` when it takes longer (the thread
/// then finishes on its own and its answer is dropped), or when it panics.
#[cfg(any(test, target_os = "macos", target_os = "linux"))]
pub fn run_with_timeout<T: Send + 'static>(
    timeout: Duration,
    job: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (answer, wait) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("platform-ask".into())
        .spawn(move || {
            // The caller may have given up and gone: nobody needs the answer then.
            let _ = answer.send(job());
        })
        .ok()?;
    wait.recv_timeout(timeout).ok()
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn a_quick_job_gives_its_answer() {
        assert_eq!(run_with_timeout(ASK_TIMEOUT, || 7), Some(7));
    }

    #[test]
    fn a_hung_job_does_not_keep_the_caller_waiting() {
        let started = Instant::now();
        let answer = run_with_timeout(Duration::from_millis(100), || {
            thread::sleep(Duration::from_secs(3));
            7
        });
        assert_eq!(answer, None);
        assert!(started.elapsed() < Duration::from_millis(1500));
    }

    #[test]
    fn a_job_that_panics_gives_no_answer() {
        let answer: Option<u8> = run_with_timeout(ASK_TIMEOUT, || panic!("the app is gone"));
        assert_eq!(answer, None);
    }
}
