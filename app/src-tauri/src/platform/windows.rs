//! Windows: the focused field through UI Automation, the frontmost app through the foreground window.
//!
//! UI Automation calls into the app that has the focus and wait for it, so they run on one thread of their own
//! (in the multithreaded COM apartment, with one `IUIAutomation` that lives as long as the thread) and the caller
//! gives up after [`UIA_TIMEOUT`]. A hung app keeps that thread for as long as UI Automation's own timeouts
//! (set to the same 300 ms where Windows allows it); the questions that queue up behind it are dropped when
//! their time is up. Windows has no permission for what is done here, neither for reading other apps' fields nor
//! for the microphone that desktop apps use.
//!
//! UI Automation and the window functions give physical pixels. [`Rect`] has logical points, so each rectangle
//! is divided by the scale of the screen it is on.

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::size_of;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{Interface, BOOL, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    SAFEARRAY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenProcess, OpenProcessToken,
    QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation6, IUIAutomationElement,
    IUIAutomationTextPattern, IUIAutomationTextPattern2, IUIAutomationTextRange,
    IUIAutomationValuePattern, TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start,
    TextUnit_Character, UIA_TextPattern2Id, UIA_TextPatternId, UIA_ValuePatternId,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetAncestor, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetMessageW,
    GetWindowRect, GetWindowThreadProcessId, PeekMessageW, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, GUITHREADINFO, HC_ACTION, HHOOK,
    MONITORINFOF_PRIMARY, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_USER,
};

use super::keys::press_paste;
use super::monitor::{is_press_message, InputMonitor, Listener};
use super::paste::{blocked_by_uipi, is_app, KeySender, PasteError};
use super::rules::{
    accept_caret, bounded_text_len, caret_after, classify_uia, field_anchor, first_rect,
    is_terminal, on_any_screen, pick_visible_frame, secure_read_outcome, terminal_anchor,
    to_logical, Editable, Os, Screen, SecureOutcome, SecureRead,
};
use super::{
    AppId, FieldInfo, FieldLookup, FieldSnapshot, Permission, Platform, Rect, DEFAULT_SCREEN,
};

/// How long the caller waits for UI Automation.
const UIA_TIMEOUT: Duration = Duration::from_millis(300);

/// A question for the UI Automation thread, which answers it with the `IUIAutomation` that it keeps.
type Job = Box<dyn FnOnce(&IUIAutomation) + Send>;

pub struct WindowsPlatform {
    jobs: mpsc::Sender<Job>,
}

impl WindowsPlatform {
    pub fn new() -> WindowsPlatform {
        let (jobs, queue) = mpsc::channel();
        // Without the thread no question gets an answer, and each one says so at once.
        let _ = thread::Builder::new()
            .name("uia".into())
            .spawn(move || serve(queue));
        WindowsPlatform { jobs }
    }

    /// Runs `work` on the UI Automation thread and waits [`UIA_TIMEOUT`] for its answer.
    fn ask<T: Send + 'static>(
        &self,
        work: impl FnOnce(&IUIAutomation, Instant) -> Option<T> + Send + 'static,
    ) -> Option<T> {
        let deadline = Instant::now() + UIA_TIMEOUT;
        let (answer, wait) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |uia| {
            // The caller has gone when the thread was busy with a hung app for longer than it waited.
            if Instant::now() < deadline {
                let _ = answer.send(work(uia, deadline));
            }
        });
        self.jobs.send(job).ok()?;
        wait.recv_timeout(UIA_TIMEOUT).ok().flatten()
    }
}

/// The `IUIAutomation` of this thread. `CUIAutomation8` is the class of Windows 8 and later, whose object has the
/// newer interfaces (`IUIAutomation6`, with the timeouts); the first class is the fallback.
fn automation() -> Option<IUIAutomation> {
    // SAFETY: plain calls with a class id.
    unsafe {
        CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))
    }
    .ok()
}

/// The UI Automation thread: starts COM, makes the one `IUIAutomation` and answers the questions in turn.
fn serve(queue: mpsc::Receiver<Job>) {
    // SAFETY: COM is started once on this thread, and ended on it after the last use.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let uia = automation();
    if let Some(uia) = &uia {
        // Windows 10 1809 and later: a call to a hung app fails after the same time that the caller waits.
        if let Ok(uia) = uia.cast::<IUIAutomation6>() {
            let ms = UIA_TIMEOUT.as_millis() as u32;
            // SAFETY: plain calls with a number.
            unsafe {
                let _ = uia.SetConnectionTimeout(ms);
                let _ = uia.SetTransactionTimeout(ms);
            }
        }
    }
    for job in queue {
        // A job that is dropped drops its channel, and the caller sees that there is no answer.
        if let Some(uia) = &uia {
            // A job that panics must not end the thread: every later question would then go unanswered.
            if catch_unwind(AssertUnwindSafe(|| job(uia))).is_err() {
                log::warn!("A UI Automation question panicked");
            }
        }
    }
    drop(uia);
    if com {
        // SAFETY: matches the successful `CoInitializeEx` above.
        unsafe { CoUninitialize() };
    }
}

impl Platform for WindowsPlatform {
    fn frontmost_app(&self) -> Option<AppId> {
        let front = foreground()?;
        Some(AppId {
            id: front.exe,
            pid: Some(front.pid),
        })
    }

    fn focused_field(&self) -> FieldLookup {
        self.ask(|uia, _| Some(lookup(uia)))
            .unwrap_or(FieldLookup::Unknown)
    }

    fn field_snapshot(&self) -> Option<FieldSnapshot> {
        self.ask(snapshot)
    }

    fn caret_anchor(&self) -> Option<Rect> {
        match self.focused_field() {
            FieldLookup::Field(field) => field.anchor,
            FieldLookup::NoField | FieldLookup::Unknown => None,
        }
    }

    fn mouse_position(&self) -> Option<(f64, f64)> {
        let mut at = POINT::default();
        // SAFETY: `at` is a place for a point.
        unsafe { GetCursorPos(&mut at) }.ok()?;
        let at = logical(Rect {
            x: f64::from(at.x),
            y: f64::from(at.y),
            w: 0.0,
            h: 0.0,
        });
        Some((at.x, at.y))
    }

    fn screen_visible_frame(&self, near: Option<(f64, f64)>) -> Rect {
        pick_visible_frame(&screens(), near).unwrap_or(DEFAULT_SCREEN)
    }

    fn accessibility(&self) -> Permission {
        Permission::NotNeeded
    }

    fn request_accessibility(&self) {}

    fn microphone(&self) -> Permission {
        Permission::NotNeeded
    }

    fn request_microphone(&self, done: Box<dyn FnOnce(bool) + Send>) {
        done(true);
    }
}

// ---- The foreground window -----------------------------------------------------------------------------------

/// The window that has the focus and the app it belongs to.
struct Foreground {
    hwnd: HWND,
    pid: u32,
    /// The file name of the executable: "chrome.exe".
    exe: String,
}

/// The foreground window. `None` when there is none, or when the app is out of reach (an app that runs as an
/// administrator can't be opened by one that doesn't).
fn foreground() -> Option<Foreground> {
    // SAFETY: plain calls with handles that the system gave.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut name = [0u16; 1024];
        let mut size = name.len() as u32;
        let named = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(name.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(process);
        named.ok()?;
        let path = String::from_utf16_lossy(name.get(..size as usize)?);
        let exe = path.rsplit(['\\', '/']).next()?.to_owned();
        Some(Foreground { hwnd, pid, exe })
    }
}

// ---- Pixels and points ---------------------------------------------------------------------------------------

fn from_rect(r: RECT) -> Rect {
    Rect {
        x: f64::from(r.left),
        y: f64::from(r.top),
        w: f64::from(r.right - r.left),
        h: f64::from(r.bottom - r.top),
    }
}

/// How many pixels make a point on a screen: 1.5 at a scale of 150 %.
fn scale_of(monitor: HMONITOR) -> f64 {
    let (mut x, mut y) = (0u32, 0u32);
    // SAFETY: `x` and `y` are places for numbers.
    match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) } {
        Ok(()) if x > 0 => f64::from(x) / 96.0,
        _ => 1.0,
    }
}

/// A rectangle of physical pixels in logical points, on the scale of the screen where it begins.
fn logical(r: Rect) -> Rect {
    let at = POINT {
        x: r.x as i32,
        y: r.y as i32,
    };
    // SAFETY: a plain call with a point.
    let monitor = unsafe { MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST) };
    to_logical(r, scale_of(monitor))
}

/// The screens in logical points, the primary one first.
fn screens() -> Vec<Screen> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        list: LPARAM,
    ) -> BOOL {
        // SAFETY: `list` is the vector that `screens` passes, which is alive during the call.
        let list = unsafe { &mut *(list.0 as *mut Vec<HMONITOR>) };
        list.push(monitor);
        BOOL::from(true)
    }

    let mut monitors: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes into the vector, which outlives the call.
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut monitors as *mut Vec<HMONITOR> as isize),
        )
    };
    let mut screens = Vec::new();
    for monitor in monitors {
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `info` says its size, as the call needs.
        if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            continue;
        }
        let scale = scale_of(monitor);
        let screen = Screen {
            frame: to_logical(from_rect(info.rcMonitor), scale),
            visible: to_logical(from_rect(info.rcWork), scale),
        };
        if info.dwFlags & MONITORINFOF_PRIMARY != 0 {
            screens.insert(0, screen);
        } else {
            screens.push(screen);
        }
    }
    screens
}

// ---- UI Automation -------------------------------------------------------------------------------------------

/// The rectangles of a text range, in physical pixels: groups of four numbers, one for each line.
fn range_rect(range: &IUIAutomationTextRange) -> Option<Rect> {
    // SAFETY: the call gives a SAFEARRAY of doubles, which is read and then freed here.
    unsafe {
        let array: *mut SAFEARRAY = range.GetBoundingRectangles().ok()?;
        if array.is_null() {
            return None;
        }
        let mut numbers: Vec<f64> = Vec::new();
        if let (Ok(first), Ok(last)) = (SafeArrayGetLBound(array, 1), SafeArrayGetUBound(array, 1))
        {
            let count = usize::try_from(last - first + 1).unwrap_or(0);
            let mut data: *mut c_void = std::ptr::null_mut();
            if count > 0 && SafeArrayAccessData(array, &mut data).is_ok() {
                numbers.extend_from_slice(std::slice::from_raw_parts(data as *const f64, count));
                let _ = SafeArrayUnaccessData(array);
            }
        }
        let _ = SafeArrayDestroy(array);
        first_rect(&numbers)
    }
}

/// The caret as the end of `range`: the rectangle of the empty range there, else the right edge of the character
/// before it. A rectangle that is no line of text, or is off every screen, doesn't count.
fn caret_of_range(range: &IUIAutomationTextRange, screens: &[Screen]) -> Option<Rect> {
    // SAFETY: calls on a range that is ours.
    unsafe {
        let end = range.Clone().ok()?;
        end.MoveEndpointByRange(
            TextPatternRangeEndpoint_Start,
            range,
            TextPatternRangeEndpoint_End,
        )
        .ok()?;
        if let Some(own) = range_rect(&end).map(logical) {
            if accept_caret(&own) {
                let caret = Rect {
                    w: own.w.max(1.0),
                    ..own
                };
                if on_any_screen(screens, &caret) {
                    return Some(caret);
                }
            }
        }
        let before = end.Clone().ok()?;
        before
            .MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -1)
            .ok()?;
        let character = range_rect(&before).map(logical)?;
        let caret = caret_after(&character);
        (accept_caret(&character) && on_any_screen(screens, &caret)).then_some(caret)
    }
}

/// The caret of an edit control of the classic kind, which has no text pattern: the foreground thread knows it.
fn caret_of_thread(front: &Foreground, screens: &[Screen]) -> Option<Rect> {
    // SAFETY: plain calls with handles that the system gave.
    unsafe {
        let thread = GetWindowThreadProcessId(front.hwnd, None);
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(thread, &mut info).ok()?;
        if info.hwndCaret.is_invalid() {
            return None;
        }
        // The caret is in the coordinates of the window that has it.
        let mut top_left = POINT {
            x: info.rcCaret.left,
            y: info.rcCaret.top,
        };
        if !ClientToScreen(info.hwndCaret, &mut top_left).as_bool() {
            return None;
        }
        let caret = logical(Rect {
            x: f64::from(top_left.x),
            y: f64::from(top_left.y),
            w: f64::from((info.rcCaret.right - info.rcCaret.left).max(1)),
            h: f64::from(info.rcCaret.bottom - info.rcCaret.top),
        });
        (accept_caret(&caret) && on_any_screen(screens, &caret)).then_some(caret)
    }
}

/// Where the bubble goes for a field: its caret from the text pattern, else from the selection, else from the
/// foreground thread, else the field itself when it is small enough.
fn anchor_of(field: &IUIAutomationElement, front: &Foreground, screens: &[Screen]) -> Option<Rect> {
    // SAFETY: calls on an element that is ours.
    unsafe {
        if let Ok(pattern) =
            field.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
        {
            let mut active = BOOL::default();
            if let Some(caret) = pattern
                .GetCaretRange(&mut active)
                .ok()
                .and_then(|range| caret_of_range(&range, screens))
            {
                return Some(caret);
            }
        }
        if let Ok(pattern) =
            field.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        {
            if let Some(caret) = pattern
                .GetSelection()
                .ok()
                .and_then(|ranges| ranges.GetElement(0).ok())
                .and_then(|range| caret_of_range(&range, screens))
            {
                return Some(caret);
            }
        }
        if let Some(caret) = caret_of_thread(front, screens) {
            return Some(caret);
        }
        let frame = logical(from_rect(field.CurrentBoundingRectangle().ok()?));
        field_anchor(&frame).filter(|anchor| on_any_screen(screens, anchor))
    }
}

/// Near the bottom left of the window of a terminal, where its prompt usually is.
fn window_anchor(front: &Foreground, screens: &[Screen]) -> Option<Rect> {
    let mut frame = RECT::default();
    // SAFETY: `frame` is a place for a rectangle.
    unsafe { GetWindowRect(front.hwnd, &mut frame) }.ok()?;
    let anchor = terminal_anchor(&logical(from_rect(frame)));
    on_any_screen(screens, &anchor).then_some(anchor)
}

/// R4: the text field that has the focus. UI Automation tells the control type, whether it is a password field,
/// and whether the value pattern (the text) and the text pattern are there.
fn lookup(uia: &IUIAutomation) -> FieldLookup {
    let Some(front) = foreground() else {
        return FieldLookup::Unknown;
    };
    let screens = screens();
    let app = AppId {
        id: front.exe.clone(),
        pid: Some(front.pid),
    };
    if is_terminal(Os::current(), &front.exe) {
        // The prompt is a grid of characters, not a text element: whatever has the focus takes typing.
        return FieldLookup::Field(FieldInfo {
            anchor: window_anchor(&front, &screens),
            app,
            terminal: true,
        });
    }

    // SAFETY: calls on the focused element, which is ours.
    unsafe {
        let Ok(field) = uia.GetFocusedElement() else {
            return FieldLookup::Unknown;
        };
        let Ok(control_type) = field.CurrentControlType() else {
            return FieldLookup::Unknown;
        };
        // A password field must never be taken for a text field: a failed read is not "no password".
        let password = field.CurrentIsPassword();
        let read = if password.is_ok() {
            SecureRead::Answered
        } else {
            SecureRead::Failed
        };
        if secure_read_outcome(read) == SecureOutcome::NoField {
            return FieldLookup::NoField;
        }
        let is_password = password.map(|password| password.as_bool()).unwrap_or(false);
        let value = field
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .ok();
        let read_only = value
            .as_ref()
            .and_then(|value| value.CurrentIsReadOnly().ok())
            .map(|read_only| read_only.as_bool())
            .unwrap_or(false);
        let has_text = field
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .is_ok();
        match classify_uia(
            control_type.0,
            is_password,
            value.is_some(),
            read_only,
            has_text,
        ) {
            Editable::Yes => FieldLookup::Field(FieldInfo {
                anchor: anchor_of(&field, &front, &screens),
                app,
                terminal: false,
            }),
            Editable::No | Editable::Secure => FieldLookup::NoField,
        }
    }
}

/// How many characters of the text of a field are read to tell whether a paste landed. A longer text isn't read
/// whole (a document of a megabyte would take more than the time there is): it has no snapshot, and the landing
/// check is left out.
const TEXT_CAP: i32 = 50_000;

/// The text length and the selection of the focused field, from its text pattern. The selection starts where
/// the text before it ends. A control without a text pattern, or with more than [`TEXT_CAP`] characters, has
/// none to read.
fn snapshot(uia: &IUIAutomation, deadline: Instant) -> Option<FieldSnapshot> {
    // Every read is asked for at most `TEXT_CAP` characters, and none starts after the time is up.
    let in_time = || (Instant::now() < deadline).then_some(());
    let cap = TEXT_CAP as usize;
    // SAFETY: calls on the focused element, which is ours.
    unsafe {
        let field = uia.GetFocusedElement().ok()?;
        let pattern: IUIAutomationTextPattern =
            field.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
        let document = pattern.DocumentRange().ok()?;
        in_time()?;
        let len = bounded_text_len(document.GetText(TEXT_CAP).ok()?.len(), cap)?;
        let selections = pattern.GetSelection().ok()?;
        if selections.Length().ok()? < 1 {
            return None;
        }
        let selection = selections.GetElement(0).ok()?;
        in_time()?;
        let sel_len = bounded_text_len(selection.GetText(TEXT_CAP).ok()?.len(), cap)?;
        let before = document.Clone().ok()?;
        before
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &selection,
                TextPatternRangeEndpoint_Start,
            )
            .ok()?;
        in_time()?;
        let sel_loc = bounded_text_len(before.GetText(TEXT_CAP).ok()?.len(), cap)?;
        Some(FieldSnapshot {
            len,
            sel_loc,
            sel_len,
        })
    }
}

// ---- The keys ------------------------------------------------------------------------------------------------

/// Presses Ctrl+V with `SendInput` (through enigo). Windows doesn't let a normal app send keys to an elevated
/// one, and says nothing when it drops them, so that case is looked for before the keys are sent.
pub struct WindowsKeys {
    last_post: Mutex<Option<Instant>>,
}

impl WindowsKeys {
    pub fn new() -> WindowsKeys {
        WindowsKeys {
            last_post: Mutex::new(None),
        }
    }
}

impl KeySender for WindowsKeys {
    fn bring_forward(&self, app: &AppId) -> bool {
        // The windows of dictation never take the focus, so the app that had it is the one in front. When it is
        // not (a window of ours is still going away), the next question finds it. Windows doesn't let a
        // background app take the focus anyway.
        let Some(pid) = foreground_pid() else {
            return false;
        };
        let front = AppId {
            id: foreground().map(|front| front.exe).unwrap_or_default(),
            pid: Some(pid),
        };
        is_app(&front, app)
    }

    fn modifiers_released(&self) -> bool {
        [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
            .into_iter()
            // SAFETY: a call with a key code. The high bit says that the key is down.
            .all(|key| unsafe { GetAsyncKeyState(i32::from(key.0)) } >= 0)
    }

    fn send_paste(&self, _terminal: bool) -> Result<(), PasteError> {
        let pid =
            foreground_pid().ok_or_else(|| PasteError::Failed("no window has the focus".into()))?;
        // SAFETY: the pseudo handle of this process needs no closing.
        let ours = token_elevated(unsafe { GetCurrentProcess() }).unwrap_or(false);
        if blocked_by_uipi(ours, process_elevated(pid)) {
            return Err(PasteError::Elevated);
        }
        // Windows Terminal and the console take Ctrl+V like every other app.
        press_paste(false)?;
        *self
            .last_post
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
        Ok(())
    }

    fn last_post(&self) -> Option<Instant> {
        *self
            .last_post
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

// ---- The keyboard and the mouse ------------------------------------------------------------------------------

// `is_press_message` has the numbers of the system's headers: they must be the ones the `windows` crate has.
const _: () = {
    assert!(WM_KEYDOWN == 0x0100);
    assert!(WM_SYSKEYDOWN == 0x0104);
    assert!(WM_LBUTTONDOWN == 0x0201);
    assert!(WM_RBUTTONDOWN == 0x0204);
};

/// How long `start` waits for the thread of the hooks to say that they are in.
const HOOKS_WAIT: Duration = Duration::from_secs(2);

/// What a hook tells the thread that dispatches: a key went down, or a mouse button did, at this point of the screen.
#[derive(Debug, Clone, Copy)]
enum Press {
    Key,
    Click { x: i32, y: i32 },
}

thread_local! {
    /// Where the hooks put what they hear. The hook procedures are called on the thread that installed the hooks, which
    /// is the one this belongs to, so that they need no lock. It does no more than send: Windows drops a hook whose
    /// procedure is slow (about 300 ms), so everything else happens on another thread.
    static HEARD: RefCell<Option<mpsc::Sender<Press>>> = const { RefCell::new(None) };
}

fn heard(press: Press) {
    // Without a listener, or while the thread goes away, there is nobody to tell.
    let _ = HEARD.try_with(|heard| {
        if let Ok(heard) = heard.try_borrow() {
            if let Some(heard) = heard.as_ref() {
                let _ = heard.send(press);
            }
        }
    });
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && is_press_message(wparam.0 as u32) {
        heard(Press::Key);
    }
    // SAFETY: the next hook gets what this one got.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && is_press_message(wparam.0 as u32) {
        // SAFETY: for a low-level mouse hook, `lparam` points to an `MSLLHOOKSTRUCT` for the length of the call.
        let at = unsafe { (*(lparam.0 as *const MSLLHOOKSTRUCT)).pt };
        heard(Press::Click { x: at.x, y: at.y });
    }
    // SAFETY: the next hook gets what this one got.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Hears the keys and the mouse buttons that go down in every app with two low-level hooks. The hooks live on a thread
/// of their own that runs a message loop, as Windows needs, and what they hear is sent on to a second thread that checks
/// it and calls the handler: the hook procedures themselves do nothing slow. A click on one of our own windows (the chip
/// itself) is not reported, as macOS doesn't report it either.
pub struct WindowsMonitor {
    listener: Arc<Listener>,
    /// The thread of the hooks, while they are installed.
    hooks: Mutex<Option<u32>>,
}

impl WindowsMonitor {
    pub fn new() -> WindowsMonitor {
        WindowsMonitor {
            listener: Arc::new(Listener::default()),
            hooks: Mutex::new(None),
        }
    }
}

impl InputMonitor for WindowsMonitor {
    fn start(&self, on_input: Box<dyn Fn() + Send + Sync>) {
        self.listener.set(on_input);
        let mut hooks = self.hooks.lock().unwrap_or_else(PoisonError::into_inner);
        if hooks.is_none() {
            *hooks = install_hooks(self.listener.clone());
            if hooks.is_none() {
                self.listener.clear();
            }
        }
    }

    fn stop(&self) {
        // The handler is gone at once; the thread of the hooks is told to leave and takes them out on its way.
        self.listener.clear();
        let thread = self
            .hooks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            // SAFETY: a message to a thread by its id. It fails, harmlessly, when the thread is gone already.
            let _ = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
    }
}

/// Starts the thread of the hooks and the one that dispatches, and gives the id of the first when the hooks are in.
fn install_hooks(listener: Arc<Listener>) -> Option<u32> {
    let (heard_tx, heard_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("input-hooks".into())
        .spawn(move || run_hooks(heard_tx, ready_tx))
        .ok()?;
    let thread = ready_rx.recv_timeout(HOOKS_WAIT).ok().flatten()?;
    // It ends when the thread of the hooks does: the channel is closed then.
    let dispatched = thread::Builder::new()
        .name("input-dispatch".into())
        .spawn(move || {
            for press in heard_rx {
                if !is_ours(press) {
                    listener.report();
                }
            }
        });
    if dispatched.is_err() {
        // SAFETY: as in `stop`.
        let _ = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        return None;
    }
    Some(thread)
}

/// The thread of the hooks: installs them, tells `ready` the id of the thread (`None` when a hook can't be installed),
/// and runs the message loop until it is told to quit.
fn run_hooks(heard_tx: mpsc::Sender<Press>, ready: mpsc::SyncSender<Option<u32>>) {
    HEARD.with(|heard| *heard.borrow_mut() = Some(heard_tx));
    // A message queue is made for the thread by looking into it, so that a quit message sent at once isn't lost.
    let mut message = MSG::default();
    // SAFETY: `message` is a buffer for the call.
    let _ = unsafe { PeekMessageW(&mut message, None, WM_USER, WM_USER, PM_NOREMOVE) };

    let hooks = set_hooks();
    // SAFETY: a call with no arguments.
    let thread = unsafe { GetCurrentThreadId() };
    let _ = ready.send(hooks.as_ref().map(|_| thread));
    if let Some((keyboard, mouse)) = hooks {
        // SAFETY: `message` is a buffer for the call. 0 is the quit message, and -1 an error: both end the loop.
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {}
        // SAFETY: the hooks that were installed above, taken out once.
        unsafe {
            let _ = UnhookWindowsHookEx(keyboard);
            let _ = UnhookWindowsHookEx(mouse);
        }
    }
    // The dispatcher has nothing more to wait for.
    let _ = HEARD.try_with(|heard| heard.borrow_mut().take());
}

fn set_hooks() -> Option<(HHOOK, HHOOK)> {
    // SAFETY: the module of this process, which the procedures are in.
    let module = unsafe { GetModuleHandleW(None) }.ok()?;
    let module = Some(HINSTANCE(module.0));
    // SAFETY: both procedures have the signature of a hook, and live as long as the process.
    let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0) };
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0) };
    match (keyboard, mouse) {
        (Ok(keyboard), Ok(mouse)) => Some((keyboard, mouse)),
        (keyboard, mouse) => {
            log::warn!("Could not hook the keyboard and the mouse: {keyboard:?} {mouse:?}");
            // SAFETY: the hook that was installed, if one was.
            unsafe {
                if let Ok(hook) = keyboard {
                    let _ = UnhookWindowsHookEx(hook);
                }
                if let Ok(hook) = mouse {
                    let _ = UnhookWindowsHookEx(hook);
                }
            }
            None
        }
    }
}

/// Whether the press was on one of this app's windows: a click on the chip. The window under the point is asked on the
/// dispatching thread, where a window that doesn't answer holds up nothing else; the page of a window is a child window
/// of the process of the webview, so the top-level window is the one asked about.
fn is_ours(press: Press) -> bool {
    let Press::Click { x, y } = press else {
        return false;
    };
    // SAFETY: plain calls with a point and the handles that the system gave.
    unsafe {
        let under = WindowFromPoint(POINT { x, y });
        if under.is_invalid() {
            return false;
        }
        let top = GetAncestor(under, GA_ROOT);
        let window = if top.is_invalid() { under } else { top };
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        pid == GetCurrentProcessId()
    }
}

/// The process of the foreground window: no handle to the process is needed.
fn foreground_pid() -> Option<u32> {
    // SAFETY: plain calls with a handle that the system gave.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}

/// Whether the process runs with the full rights of an administrator. `None` when Windows won't say, which it
/// does for a process of a higher level than ours.
fn process_elevated(pid: u32) -> Option<bool> {
    // SAFETY: the handle is closed again.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let elevated = token_elevated(process);
        let _ = CloseHandle(process);
        elevated
    }
}

fn token_elevated(process: HANDLE) -> Option<bool> {
    // SAFETY: `elevation` is the buffer that `TokenElevation` fills, and the token is closed again.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let read = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        let _ = CloseHandle(token);
        read.ok()?;
        Some(elevation.TokenIsElevated != 0)
    }
}
