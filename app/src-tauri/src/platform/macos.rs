//! macOS: the focused field through the Accessibility API, the frontmost app through NSWorkspace, and the two
//! permissions.
//!
//! The Accessibility questions go to another app and wait for its answer, so they run on a thread of their own
//! and give up after [`ASK_TIMEOUT`]; each call also has a messaging timeout of 0.3 s, so that the thread of a
//! hung app is back soon. NSWorkspace, NSScreen, NSEvent and the permission prompts belong to the main thread,
//! which they reach through `AppHandle::run_on_main_thread`.
//!
//! Accessibility rectangles are in points from the top left of the primary screen, like [`Rect`], so they need no
//! conversion; AppKit's are from the bottom left and are flipped.

use std::ptr::NonNull;
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplicationActivationOptions, NSEvent, NSEventMask, NSPasteboard, NSPasteboardItem,
    NSPasteboardTypeString, NSPasteboardWriting, NSRunningApplication, NSScreen, NSTrackingArea,
    NSTrackingAreaOptions, NSWindow, NSWorkspace,
};
use objc2_application_services::{
    kAXTrustedCheckOptionPrompt, AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
    AXUIElement, AXValue, AXValueType,
};
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRange, CFRetained, CFString, CFType, CGPoint, CGRect,
    CGSize,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
};
use objc2_foundation::{NSArray, NSData, NSString};

use super::monitor::{InputMonitor, Installed, Listener};
use super::paste::{ChangeMark, ClipSnapshot, Clipboard, KeySender, PasteError, PasteboardItem};
use super::rules::{
    accept_caret, ax_focus_is_the_app, caret_after, classify_ax, field_anchor, flip_y,
    is_chromium_browser, is_terminal, on_any_screen, pick_visible_frame, secure_read_outcome,
    terminal_anchor, Editable, Os, Screen, SecureOutcome, SecureRead,
};
use super::{
    run_with_timeout, AppId, FieldInfo, FieldLookup, FieldSnapshot, Permission, Platform, Rect,
    ASK_TIMEOUT, DEFAULT_SCREEN,
};

/// The messaging timeout of every Accessibility call to an app, in seconds: a hung app must not freeze the hotkey.
const AX_TIMEOUT: f32 = 0.3;
/// How long a call that fails with "cannot complete" must have waited to be a hung app, and not an app that
/// refuses the question at once.
const HUNG_AFTER: Duration = Duration::from_millis(250);
/// What Chromium and Electron are given to build their accessibility tree before the question is asked again.
const TREE_WAIT: Duration = Duration::from_millis(150);

pub struct MacPlatform {
    app: tauri::AppHandle,
}

impl MacPlatform {
    pub fn new(app: tauri::AppHandle) -> MacPlatform {
        MacPlatform { app }
    }

    /// Reads the frontmost app on the caller's thread, then runs `work` on a thread of its own. `None` when the
    /// answer takes longer than [`ASK_TIMEOUT`] in all, when Accessibility isn't granted or when `work` fails.
    ///
    /// The desktop is read here and not on the worker: on the main thread (where the global shortcut handler
    /// runs) the read happens at once, whereas a worker's request would queue behind the very thread that
    /// waits for the worker.
    fn ask_app<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Front, &[Screen], Instant) -> Result<T, Fail> + Send + 'static,
    ) -> Option<T> {
        let deadline = Instant::now() + ASK_TIMEOUT;
        // SAFETY: a call with no arguments.
        if !unsafe { AXIsProcessTrusted() } {
            return None;
        }
        let desktop = ask_main(&self.app, deadline, read_desktop)?;
        let front = desktop.front?;
        let screens = desktop.screens;
        run_with_timeout(
            deadline.saturating_duration_since(Instant::now()),
            move || work(&front, &screens, deadline).ok(),
        )
        .flatten()
    }
}

impl Platform for MacPlatform {
    fn frontmost_app(&self) -> Option<AppId> {
        ask_main(&self.app, Instant::now() + ASK_TIMEOUT, read_desktop)?
            .front
            .map(|front| front.app_id())
    }

    fn focused_field(&self) -> FieldLookup {
        self.ask_app(lookup).unwrap_or(FieldLookup::Unknown)
    }

    fn field_snapshot(&self) -> Option<FieldSnapshot> {
        self.ask_app(snapshot)
    }

    fn caret_anchor(&self) -> Option<Rect> {
        match self.focused_field() {
            FieldLookup::Field(field) => field.anchor,
            FieldLookup::NoField | FieldLookup::Unknown => None,
        }
    }

    fn mouse_position(&self) -> Option<(f64, f64)> {
        ask_main(&self.app, Instant::now() + ASK_TIMEOUT, read_desktop).map(|d| d.mouse)
    }

    fn screen_visible_frame(&self, near: Option<(f64, f64)>) -> Rect {
        ask_main(&self.app, Instant::now() + ASK_TIMEOUT, read_desktop)
            .and_then(|d| pick_visible_frame(&d.screens, near))
            .unwrap_or(DEFAULT_SCREEN)
    }

    fn accessibility(&self) -> Permission {
        // SAFETY: a call with no arguments.
        if unsafe { AXIsProcessTrusted() } {
            Permission::Granted
        } else {
            Permission::Denied
        }
    }

    fn request_accessibility(&self) {
        let _ = self.app.run_on_main_thread(|| {
            let prompt = CFBoolean::new(true);
            // SAFETY: the options are the documented key with a CFBoolean.
            let options =
                CFDictionary::from_slices(&[unsafe { kAXTrustedCheckOptionPrompt }], &[prompt]);
            // The call shows the prompt, once for each build of the app, and says whether it is trusted already.
            let _ = unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) };
        });
    }

    fn microphone(&self) -> Permission {
        // SAFETY: the media type is a constant of the framework.
        let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
            return Permission::NotDetermined;
        };
        permission_from(unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) })
    }

    fn request_microphone(&self, done: Box<dyn FnOnce(bool) + Send>) {
        // The handler runs on a queue of the system, once: the prompt is answered, or the answer is known already.
        let done = Arc::new(Mutex::new(Some(done)));
        let on_main = done.clone();
        let queued = self.app.run_on_main_thread(move || {
            // SAFETY: the media type is a constant of the framework.
            let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
                finish(&on_main, false);
                return;
            };
            let handler = {
                let on_main = on_main.clone();
                RcBlock::new(move |granted: Bool| finish(&on_main, granted.as_bool()))
            };
            // SAFETY: the block takes one BOOL.
            unsafe {
                AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler)
            };
        });
        if queued.is_err() {
            finish(&done, false);
        }
    }
}

/// What waits for the answer to a request for the microphone.
type Done = Box<dyn FnOnce(bool) + Send>;

/// Gives the answer to whoever asked, once.
fn finish(done: &Mutex<Option<Done>>, granted: bool) {
    if let Some(done) = done.lock().ok().and_then(|mut done| done.take()) {
        done(granted);
    }
}

fn permission_from(status: AVAuthorizationStatus) -> Permission {
    match status {
        AVAuthorizationStatus::Authorized => Permission::Granted,
        AVAuthorizationStatus::NotDetermined => Permission::NotDetermined,
        // A restriction (parental controls, a profile) is a no that the user can't change.
        _ => Permission::Denied,
    }
}

// ---- The main thread -----------------------------------------------------------------------------------------

/// The frontmost app.
struct Front {
    id: String,
    pid: i32,
}

impl Front {
    fn app_id(&self) -> AppId {
        AppId {
            id: self.id.clone(),
            pid: u32::try_from(self.pid).ok(),
        }
    }
}

/// What AppKit says, read in one go on the main thread.
struct Desktop {
    front: Option<Front>,
    /// The main screen first.
    screens: Vec<Screen>,
    mouse: (f64, f64),
}

fn read_desktop(mtm: MainThreadMarker) -> Desktop {
    let front = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| Front {
            id: app
                .bundleIdentifier()
                .map(|id| id.to_string())
                .unwrap_or_default(),
            pid: app.processIdentifier(),
        });

    let list = NSScreen::screens(mtm);
    // The first screen of the list is the one with the menu bar, the origin of both coordinate systems.
    let primary_height = list
        .iter()
        .next()
        .map_or(0.0, |screen| screen.frame().size.height);
    let flipped = |r: CGRect| {
        flip_y(
            Rect {
                x: r.origin.x,
                y: r.origin.y,
                w: r.size.width,
                h: r.size.height,
            },
            primary_height,
        )
    };
    let mut screens: Vec<Screen> = list
        .iter()
        .map(|screen| Screen {
            frame: flipped(screen.frame()),
            visible: flipped(screen.visibleFrame()),
        })
        .collect();
    if let Some(main) = NSScreen::mainScreen(mtm) {
        let main = flipped(main.frame());
        if let Some(i) = screens.iter().position(|screen| screen.frame == main) {
            screens.swap(0, i);
        }
    }

    let mouse = NSEvent::mouseLocation();
    Desktop {
        front,
        screens,
        mouse: (mouse.x, primary_height - mouse.y),
    }
}

/// Runs `job` on the main thread and waits for its answer until `deadline`.
fn ask_main<T: Send + 'static>(
    app: &tauri::AppHandle,
    deadline: Instant,
    job: impl FnOnce(MainThreadMarker) -> T + Send + 'static,
) -> Option<T> {
    let (answer, wait) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            let _ = answer.send(job(mtm));
        }
    })
    .ok()?;
    wait.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()
}

// ---- Accessibility -------------------------------------------------------------------------------------------

/// Why an Accessibility call gave nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fail {
    /// The app doesn't answer, or the time is up: nothing more is asked of it.
    Hung,
    /// The app answered with this `AXError`: there is no such attribute or value.
    Error(i32),
}

fn fail_from(code: i32, took: Duration) -> Fail {
    if code == AXError::CannotComplete.0 && took >= HUNG_AFTER {
        Fail::Hung
    } else {
        Fail::Error(code)
    }
}

/// Whether a failed read of the focused element is the app saying that nothing has the focus, rather than the
/// app not saying anything.
fn means_no_focus(code: i32) -> bool {
    code == AXError::NoValue.0 || code == AXError::AttributeUnsupported.0
}

/// How a failed read of the subrole (the password fact) is to be taken.
fn secure_read_of(code: i32) -> SecureRead {
    if code == AXError::NoValue.0 || code == AXError::AttributeUnsupported.0 {
        SecureRead::Unsupported
    } else {
        SecureRead::Failed
    }
}

/// Keeps a failure of "no such value" as an answer (`None`) and passes a hung app on.
fn absent<T>(read: Result<T, Fail>) -> Result<Option<T>, Fail> {
    match read {
        Ok(value) => Ok(Some(value)),
        Err(Fail::Error(_)) => Ok(None),
        Err(Fail::Hung) => Err(Fail::Hung),
    }
}

fn check(deadline: Instant) -> Result<(), Fail> {
    if Instant::now() < deadline {
        Ok(())
    } else {
        Err(Fail::Hung)
    }
}

/// The element of an app, with the messaging timeout set.
fn app_element(pid: i32) -> CFRetained<AXUIElement> {
    // SAFETY: plain calls with a pid and a number.
    unsafe {
        let element = AXUIElement::new_application(pid);
        element.set_messaging_timeout(AX_TIMEOUT);
        element
    }
}

fn copy(element: &AXUIElement, name: &str, deadline: Instant) -> Result<CFRetained<CFType>, Fail> {
    check(deadline)?;
    let name = CFString::from_str(name);
    let mut value: *const CFType = std::ptr::null();
    let started = Instant::now();
    // SAFETY: `value` is a place for a pointer.
    let error = unsafe { element.copy_attribute_value(&name, NonNull::from(&mut value)) };
    if error != AXError::Success {
        return Err(fail_from(error.0, started.elapsed()));
    }
    // SAFETY: the call gave a pointer that is ours to release (the "copy" rule), or none.
    NonNull::new(value.cast_mut())
        .map(|value| unsafe { CFRetained::from_raw(value) })
        .ok_or(Fail::Error(AXError::NoValue.0))
}

fn copy_parameterized(
    element: &AXUIElement,
    name: &str,
    parameter: &CFType,
    deadline: Instant,
) -> Result<CFRetained<CFType>, Fail> {
    check(deadline)?;
    let name = CFString::from_str(name);
    let mut value: *const CFType = std::ptr::null();
    let started = Instant::now();
    // SAFETY: `value` is a place for a pointer.
    let error = unsafe {
        element.copy_parameterized_attribute_value(&name, parameter, NonNull::from(&mut value))
    };
    if error != AXError::Success {
        return Err(fail_from(error.0, started.elapsed()));
    }
    // SAFETY: as in `copy`.
    NonNull::new(value.cast_mut())
        .map(|value| unsafe { CFRetained::from_raw(value) })
        .ok_or(Fail::Error(AXError::NoValue.0))
}

fn wrong_type() -> Fail {
    Fail::Error(AXError::IllegalArgument.0)
}

fn element(
    parent: &AXUIElement,
    name: &str,
    deadline: Instant,
) -> Result<CFRetained<AXUIElement>, Fail> {
    copy(parent, name, deadline)?
        .downcast::<AXUIElement>()
        .map_err(|_| wrong_type())
}

fn string(element: &AXUIElement, name: &str, deadline: Instant) -> Result<String, Fail> {
    let value = copy(element, name, deadline)?;
    value
        .downcast_ref::<CFString>()
        .map(|string| string.to_string())
        .ok_or_else(wrong_type)
}

/// A C structure out of an `AXValue`.
fn structure<T: Copy>(value: &CFType, kind: AXValueType, zero: T) -> Result<T, Fail> {
    let value = value.downcast_ref::<AXValue>().ok_or_else(wrong_type)?;
    let mut out = zero;
    // SAFETY: `out` is a `T`, which is what `kind` decodes into.
    if unsafe { value.value(kind, NonNull::from(&mut out).cast()) } {
        Ok(out)
    } else {
        Err(wrong_type())
    }
}

fn selected_range(element: &AXUIElement, deadline: Instant) -> Result<CFRange, Fail> {
    let value = copy(element, "AXSelectedTextRange", deadline)?;
    structure(
        &value,
        AXValueType::CFRange,
        CFRange {
            location: 0,
            length: 0,
        },
    )
}

fn is_settable(element: &AXUIElement, name: &str) -> bool {
    let name = CFString::from_str(name);
    let mut settable: u8 = 0;
    // SAFETY: `settable` is a place for a byte.
    let error = unsafe { element.is_attribute_settable(&name, NonNull::from(&mut settable)) };
    error == AXError::Success && settable != 0
}

/// The position and size of an element, in points from the top left of the primary screen.
fn frame(element: &AXUIElement, deadline: Instant) -> Result<Rect, Fail> {
    let at = copy(element, "AXPosition", deadline)?;
    let at = structure(&at, AXValueType::CGPoint, CGPoint::default())?;
    let size = copy(element, "AXSize", deadline)?;
    let size = structure(&size, AXValueType::CGSize, CGSize::default())?;
    Ok(Rect {
        x: at.x,
        y: at.y,
        w: size.width,
        h: size.height,
    })
}

/// The rectangle of `length` characters from `location`.
fn bounds_for_range(
    element: &AXUIElement,
    location: isize,
    length: isize,
    deadline: Instant,
) -> Result<Rect, Fail> {
    let mut range = CFRange { location, length };
    // SAFETY: `range` is a CFRange, which is what the type says.
    let parameter = unsafe { AXValue::new(AXValueType::CFRange, NonNull::from(&mut range).cast()) }
        .ok_or_else(wrong_type)?;
    let bounds = copy_parameterized(element, "AXBoundsForRange", &parameter, deadline)?;
    let r = structure(&bounds, AXValueType::CGRect, CGRect::default())?;
    Ok(Rect {
        x: r.origin.x,
        y: r.origin.y,
        w: r.size.width,
        h: r.size.height,
    })
}

/// The caret of the field: the rectangle of the empty range at the end of the selection, else the right edge of
/// the character before it. A rectangle that is no line of text, or that is off every screen, doesn't count.
fn caret_rect(
    element: &AXUIElement,
    screens: &[Screen],
    deadline: Instant,
) -> Result<Option<Rect>, Fail> {
    let Some(selection) = absent(selected_range(element, deadline))? else {
        return Ok(None);
    };
    let end = selection.location + selection.length;
    let mut tries = vec![(end, 0, false)];
    if end > 0 {
        tries.push((end - 1, 1, true));
    }
    for (location, length, character_before) in tries {
        let Some(bounds) = absent(bounds_for_range(element, location, length, deadline))? else {
            continue;
        };
        if !accept_caret(&bounds) {
            continue;
        }
        let caret = if character_before {
            caret_after(&bounds)
        } else {
            Rect {
                w: bounds.w.max(1.0),
                ..bounds
            }
        };
        if on_any_screen(screens, &caret) {
            return Ok(Some(caret));
        }
    }
    Ok(None)
}

/// The field itself, when it is small enough to say where the caret is.
fn field_rect(
    element: &AXUIElement,
    screens: &[Screen],
    deadline: Instant,
) -> Result<Option<Rect>, Fail> {
    let Some(field) = absent(frame(element, deadline))? else {
        return Ok(None);
    };
    Ok(field_anchor(&field).filter(|anchor| on_any_screen(screens, anchor)))
}

/// Near the bottom left of the focused window of a terminal, where its prompt usually is.
fn window_anchor(
    app: &AXUIElement,
    screens: &[Screen],
    deadline: Instant,
) -> Result<Option<Rect>, Fail> {
    let Some(window) = absent(element(app, "AXFocusedWindow", deadline))? else {
        return Ok(None);
    };
    let Some(window) = absent(frame(&window, deadline))? else {
        return Ok(None);
    };
    let anchor = terminal_anchor(&window);
    Ok(on_any_screen(screens, &anchor).then_some(anchor))
}

/// Tells Electron and Chromium that a client of the accessibility tree is there: they build it only then.
fn enable_accessibility(app: &AXUIElement, bundle_id: &str) {
    let mut names = vec!["AXManualAccessibility"];
    if is_chromium_browser(bundle_id) {
        names.push("AXEnhancedUserInterface");
    }
    for name in names {
        let name = CFString::from_str(name);
        // SAFETY: the value is a CFBoolean, which both attributes take. Failing is nothing to act on.
        let _ = unsafe { app.set_attribute_value(&name, CFBoolean::new(true)) };
    }
}

/// The focused element of the app. Chromium and Electron answer with nothing, or with the app itself, until they
/// are told that a client of the accessibility tree is there: then they build it, and the question is asked again
/// once, after a short wait.
fn focused_element(
    app: &AXUIElement,
    front: &Front,
    deadline: Instant,
) -> Result<CFRetained<AXUIElement>, Fail> {
    match element(app, "AXFocusedUIElement", deadline) {
        Ok(focus) if !is_the_app(&focus, deadline) => return Ok(focus),
        Err(Fail::Hung) => return Err(Fail::Hung),
        Ok(_) | Err(Fail::Error(_)) => {}
    }
    enable_accessibility(app, &front.id);
    thread::sleep(TREE_WAIT);
    element(app, "AXFocusedUIElement", deadline)
}

/// Whether the element is the application itself, which an app names as its focus when it has nothing to show.
fn is_the_app(element: &AXUIElement, deadline: Instant) -> bool {
    string(element, "AXRole", deadline).is_ok_and(|role| ax_focus_is_the_app(&role))
}

/// R4: the text field that has the focus in the frontmost app.
fn lookup(front: &Front, screens: &[Screen], deadline: Instant) -> Result<FieldLookup, Fail> {
    let app = front.app_id();
    let terminal = is_terminal(Os::current(), &front.id);
    let root = app_element(front.pid);
    if terminal {
        // The prompt is a grid of characters, not a text element: whatever has the focus takes typing.
        // A hung terminal is still a terminal: it only has no anchor.
        let anchor = window_anchor(&root, screens, deadline).ok().flatten();
        return Ok(FieldLookup::Field(FieldInfo {
            app,
            terminal,
            anchor,
        }));
    }

    let field = match focused_element(&root, front, deadline) {
        Ok(field) => field,
        Err(Fail::Error(code)) if means_no_focus(code) => return Ok(FieldLookup::NoField),
        Err(fail) => return Err(fail),
    };
    let role = absent(string(&field, "AXRole", deadline))?.unwrap_or_default();
    if ax_focus_is_the_app(&role) {
        return Ok(FieldLookup::Unknown);
    }
    // A password field must never be taken for a text field: a subrole that can't be read is not "no subrole".
    let subrole = match string(&field, "AXSubrole", deadline) {
        Ok(subrole) => subrole,
        Err(Fail::Hung) => return Err(Fail::Hung),
        Err(Fail::Error(code)) => match secure_read_outcome(secure_read_of(code)) {
            SecureOutcome::Classify => String::new(),
            SecureOutcome::NoField => return Ok(FieldLookup::NoField),
        },
    };
    let has_selection = absent(selected_range(&field, deadline))?.is_some();
    let settable = is_settable(&field, "AXValue");
    match classify_ax(&role, &subrole, settable, has_selection) {
        Editable::Yes => {
            let anchor = match caret_rect(&field, screens, deadline)? {
                Some(caret) => Some(caret),
                None => field_rect(&field, screens, deadline)?,
            };
            Ok(FieldLookup::Field(FieldInfo {
                app,
                terminal,
                anchor,
            }))
        }
        Editable::No | Editable::Secure => Ok(FieldLookup::NoField),
    }
}

/// The text length and the selection of the focused field. A terminal has none to read.
fn snapshot(front: &Front, _screens: &[Screen], deadline: Instant) -> Result<FieldSnapshot, Fail> {
    if is_terminal(Os::current(), &front.id) {
        return Err(Fail::Error(AXError::NoValue.0));
    }
    let root = app_element(front.pid);
    let field = element(&root, "AXFocusedUIElement", deadline)?;
    let selection = selected_range(&field, deadline)?;
    let len = match absent(copy(&field, "AXNumberOfCharacters", deadline))?
        .and_then(|count| count.downcast_ref::<CFNumber>().and_then(CFNumber::as_i64))
    {
        Some(count) => count,
        None => {
            let text = copy(&field, "AXValue", deadline)?;
            let text = text.downcast_ref::<CFString>().ok_or_else(wrong_type)?;
            text.length() as i64
        }
    };
    let count = |n: i64| usize::try_from(n).unwrap_or(0);
    Ok(FieldSnapshot {
        len: count(len),
        sel_loc: count(selection.location as i64),
        sel_len: count(selection.length as i64),
    })
}

// ---- The clipboard -------------------------------------------------------------------------------------------

/// The type that clipboard managers skip (nspasteboard.org): text put there only to be pasted.
const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";

/// The pasteboard. Its calls are safe from any thread.
pub struct MacClipboard {
    /// Which pasteboard: the general one, always, outside the tests.
    board: fn() -> Retained<NSPasteboard>,
}

impl MacClipboard {
    pub fn new() -> MacClipboard {
        MacClipboard {
            board: NSPasteboard::generalPasteboard,
        }
    }
}

impl Clipboard for MacClipboard {
    fn snapshot(&self) -> ClipSnapshot {
        let board = (self.board)();
        // A type that gives no bytes (a promise that nobody keeps) can't be kept.
        let item_of = |item: &NSPasteboardItem| -> PasteboardItem {
            item.types()
                .iter()
                .filter_map(|kind| {
                    let data = item.dataForType(&kind)?;
                    Some((kind.to_string(), data.to_vec()))
                })
                .collect()
        };
        let items = board
            .pasteboardItems()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item_of(&item))
                    .filter(|types| !types.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        ClipSnapshot::Pasteboard(items)
    }

    fn write_transient(&self, text: &str) -> Result<ChangeMark, String> {
        let board = (self.board)();
        board.clearContents();
        // SAFETY: a constant of AppKit.
        let string_type = unsafe { NSPasteboardTypeString };
        if !board.setString_forType(&NSString::from_str(text), string_type) {
            return Err("the pasteboard did not take the text".into());
        }
        // The empty data of the transient type is all the mark is: without it the text is still pasted.
        board.setData_forType(Some(&NSData::new()), &NSString::from_str(TRANSIENT_TYPE));
        Ok(ChangeMark(change_count(&board)))
    }

    fn changed_since(&self, mark: &ChangeMark) -> bool {
        change_count(&(self.board)()) != mark.0
    }

    fn restore(&self, snapshot: ClipSnapshot) {
        let ClipSnapshot::Pasteboard(items) = snapshot else {
            return;
        };
        let board = (self.board)();
        board.clearContents();
        let objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = items
            .iter()
            .map(|types| {
                let item = NSPasteboardItem::new();
                for (kind, bytes) in types {
                    item.setData_forType(&NSData::with_bytes(bytes), &NSString::from_str(kind));
                }
                ProtocolObject::from_retained(item)
            })
            .collect();
        if !objects.is_empty() {
            board.writeObjects(&NSArray::from_retained_slice(&objects));
        }
    }

    fn copy(&self, text: &str) -> Result<(), String> {
        let board = (self.board)();
        board.clearContents();
        // SAFETY: a constant of AppKit.
        let string_type = unsafe { NSPasteboardTypeString };
        if board.setString_forType(&NSString::from_str(text), string_type) {
            Ok(())
        } else {
            Err("the pasteboard did not take the text".into())
        }
    }
}

/// The number that the pasteboard counts up whenever something is put on it.
fn change_count(board: &NSPasteboard) -> u64 {
    u64::try_from(board.changeCount()).unwrap_or(0)
}

// ---- The keys ------------------------------------------------------------------------------------------------

/// The key V on every keyboard layout, as the system numbers the keys by where they are.
const KEY_V: u16 = 9;
/// What every key event of ours is tagged with ("DICT"), so that whoever watches the keyboard can tell it from the
/// user's own typing.
pub const EVENT_TAG: i64 = 0x4449_4354;
/// How long one question about the app in front waits for the main thread, which has to answer it.
const FORWARD_ASK: Duration = Duration::from_millis(100);

/// Presses Cmd+V and brings apps forward.
pub struct MacKeys {
    app: tauri::AppHandle,
    last_post: Mutex<Option<Instant>>,
}

impl MacKeys {
    pub fn new(app: tauri::AppHandle) -> MacKeys {
        MacKeys {
            app,
            last_post: Mutex::new(None),
        }
    }
}

impl KeySender for MacKeys {
    fn bring_forward(&self, app: &AppId) -> bool {
        let app = app.clone();
        // The activation state of an app is kept by the main thread, so that is where it is asked.
        ask_main(&self.app, Instant::now() + FORWARD_ASK, move |_| {
            activate(&app)
        })
        .unwrap_or(false)
    }

    fn modifiers_released(&self) -> bool {
        !modifiers_held(CGEventSource::flags_state(
            CGEventSourceStateID::CombinedSessionState,
        ))
    }

    fn send_paste(&self, _terminal: bool) -> Result<(), PasteError> {
        // Without the permission the system drops the keys and says nothing.
        // SAFETY: a call with no arguments.
        if !unsafe { AXIsProcessTrusted() } {
            return Err(PasteError::Failed(
                "Accessibility access is not granted".into(),
            ));
        }
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState);
        let key = |down: bool| {
            let event = CGEvent::new_keyboard_event(source.as_deref(), KEY_V, down)
                .ok_or_else(|| PasteError::Failed("could not make the key event".into()))?;
            CGEvent::set_flags(Some(&event), paste_flags(down));
            CGEvent::set_integer_value_field(
                Some(&event),
                CGEventField::EventSourceUserData,
                EVENT_TAG,
            );
            Ok(event)
        };
        // Both events are made before one is sent: a key that went down always comes up.
        let (down, up) = (key(true)?, key(false)?);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down));
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
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

/// Asks the app to come forward if it isn't, and says whether it is in front now. Runs on the main thread.
fn activate(target: &AppId) -> bool {
    let by_pid = target
        .pid
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier);
    let running = by_pid.or_else(|| {
        let id = NSString::from_str(&target.id);
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&id)
            .iter()
            .next()
    });
    let Some(running) = running else {
        // The app quit.
        return false;
    };
    if running.isTerminated() {
        return false;
    }
    if running.isActive() {
        return true;
    }
    // Only the main and key windows come forward. The system may refuse (and then the next question gets the
    // same answer); it needs no more than a request.
    running.activateWithOptions(NSApplicationActivationOptions::empty());
    false
}

/// The flags of a key event of the paste shortcut.
fn paste_flags(down: bool) -> CGEventFlags {
    if down {
        CGEventFlags::MaskCommand
    } else {
        CGEventFlags::empty()
    }
}

/// Whether one of the modifiers of a shortcut is held (the caps lock and the fn key are none).
fn modifiers_held(flags: CGEventFlags) -> bool {
    flags.intersects(
        CGEventFlags::MaskCommand
            | CGEventFlags::MaskControl
            | CGEventFlags::MaskAlternate
            | CGEventFlags::MaskShift,
    )
}

// ---- The mouse over a window ---------------------------------------------------------------------------------

/// What a [`MouseWatcher`] tells.
struct MouseWatch {
    on_change: Box<dyn Fn(bool)>,
}

define_class!(
    /// Hears the mouse come into a tracking area and leave it.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DictateMouseWatcher"]
    #[ivars = MouseWatch]
    struct MouseWatcher;

    unsafe impl NSObjectProtocol for MouseWatcher {}

    impl MouseWatcher {
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            (self.ivars().on_change)(true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            (self.ivars().on_change)(false);
        }
    }
);

/// Tells `on_change` when the mouse comes over the window and when it leaves it, whichever app is the active one, with a
/// tracking area on the window's content view. The page of a panel that is never the key window gets no mouse movement, so
/// the page can't tell. The window keeps the delegate it has (Tauri's, which turns what the system says of the window
/// into the events of the app): this adds to the window and replaces nothing. `ns_window` is the `NSWindow` that
/// `WebviewWindow::ns_window` gives. On the main thread. The watcher lives as long as the window, which is as long as the app.
pub fn watch_mouse(ns_window: *mut std::ffi::c_void, on_change: Box<dyn Fn(bool)>) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    // SAFETY: the pointer is the window that Tauri has made, which is alive while this runs, on the main thread.
    let Some(window) = (unsafe { ns_window.cast::<NSWindow>().as_ref() }) else {
        return false;
    };
    let Some(view) = window.contentView() else {
        return false;
    };
    let watcher = MouseWatcher::alloc(mtm).set_ivars(MouseWatch { on_change });
    // SAFETY: `init` of NSObject.
    let watcher: Retained<MouseWatcher> = unsafe { msg_send![super(watcher), init] };
    // `InVisibleRect`: the area is always the whole of the view, whatever its size is as the window is sized to its page.
    let options = NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;
    // SAFETY: the owner is an object that answers `mouseEntered:` and `mouseExited:`, as a tracking area needs.
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            CGRect::ZERO,
            options,
            Some(&watcher),
            None,
        )
    };
    view.addTrackingArea(&area);
    // The area does not keep its owner alive for sure: the watcher is let go of by nobody, as the window is the app's for good.
    std::mem::forget(watcher);
    true
}

// ---- The keyboard and the mouse ------------------------------------------------------------------------------

/// Hears the keys that go down and the mouse buttons that go down in other apps, through a global event monitor of
/// AppKit. The monitor doesn't hear what goes to this app (a click on the chip is no reason to hide it), it hears the
/// keys only while the Accessibility permission is granted, which dictation asks for, and it never says which key.
/// Each `MacMonitor` has its monitor of its own: the one of another is not touched by it.
pub struct MacMonitor {
    app: tauri::AppHandle,
    listener: Arc<Listener>,
    installed: Arc<Installed<Token>>,
}

/// What AppKit gave back for the monitor, to give back to it. It is made and removed on the main thread, and it is
/// only held (not used) anywhere else.
struct Token(Retained<AnyObject>);

// SAFETY: the object is touched on the main thread only (`install_monitor`, `remove_monitor`); other threads keep it
// in its place and let it go, and an Objective-C object may be retained and released from any thread.
unsafe impl Send for Token {}

impl MacMonitor {
    pub fn new(app: tauri::AppHandle) -> MacMonitor {
        MacMonitor {
            app,
            listener: Arc::new(Listener::default()),
            installed: Arc::new(Installed::default()),
        }
    }
}

impl InputMonitor for MacMonitor {
    fn start(&self, on_input: Box<dyn Fn() + Send + Sync>) {
        self.listener.set(on_input);
        let (listener, installed) = (self.listener.clone(), self.installed.clone());
        let queued = self
            .app
            .run_on_main_thread(move || install_monitor(&listener, &installed));
        if queued.is_err() {
            self.listener.clear();
        }
    }

    fn stop(&self) {
        // The handler is gone at once, whatever the main thread is busy with.
        self.listener.clear();
        let installed = self.installed.clone();
        let _ = self
            .app
            .run_on_main_thread(move || remove_monitor(&installed));
    }
}

impl Drop for MacMonitor {
    fn drop(&mut self) {
        // A monitor that is let go of listens no more.
        self.stop();
    }
}

/// On the main thread. One monitor serves every start of this `MacMonitor`: it reports to whatever its listener has.
fn install_monitor(listener: &Arc<Listener>, installed: &Installed<Token>) {
    let made = installed.install(|| {
        let listener = listener.clone();
        let heard = RcBlock::new(move |_event: NonNull<NSEvent>| listener.report());
        NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            NSEventMask::KeyDown | NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
            &heard,
        )
        .map(Token)
    });
    if !made {
        log::warn!("The system did not give a monitor for the keyboard and the mouse");
    }
}

/// On the main thread.
fn remove_monitor(installed: &Installed<Token>) {
    if let Some(Token(monitor)) = installed.take() {
        // SAFETY: this is the object that `addGlobalMonitorForEventsMatchingMask:handler:` returned.
        unsafe { NSEvent::removeMonitor(&monitor) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_that_waited_out_the_timeout_is_a_hung_app() {
        let cannot = AXError::CannotComplete.0;
        assert_eq!(fail_from(cannot, Duration::from_millis(300)), Fail::Hung);
        // The same error at once is an app that refuses (Chromium, before it builds its tree).
        assert_eq!(
            fail_from(cannot, Duration::from_millis(5)),
            Fail::Error(cannot)
        );
        // Only that error means waiting for nothing.
        let no_value = AXError::NoValue.0;
        assert_eq!(
            fail_from(no_value, Duration::from_millis(300)),
            Fail::Error(no_value)
        );
    }

    #[test]
    fn no_value_or_attribute_means_that_nothing_has_the_focus() {
        assert!(means_no_focus(AXError::NoValue.0));
        assert!(means_no_focus(AXError::AttributeUnsupported.0));
        // These say that the app can't tell, not that nothing is focused.
        assert!(!means_no_focus(AXError::CannotComplete.0));
        assert!(!means_no_focus(AXError::NotImplemented.0));
        assert!(!means_no_focus(AXError::APIDisabled.0));
        assert!(!means_no_focus(AXError::Failure.0));
    }

    #[test]
    fn the_microphone_status_maps_to_a_permission() {
        assert_eq!(
            permission_from(AVAuthorizationStatus::Authorized),
            Permission::Granted
        );
        assert_eq!(
            permission_from(AVAuthorizationStatus::Denied),
            Permission::Denied
        );
        // A restriction (parental controls, a profile) is a no that the user can't change.
        assert_eq!(
            permission_from(AVAuthorizationStatus::Restricted),
            Permission::Denied
        );
        assert_eq!(
            permission_from(AVAuthorizationStatus::NotDetermined),
            Permission::NotDetermined
        );
    }

    #[test]
    fn only_no_value_or_no_attribute_means_that_there_is_no_subrole() {
        assert_eq!(secure_read_of(AXError::NoValue.0), SecureRead::Unsupported);
        assert_eq!(
            secure_read_of(AXError::AttributeUnsupported.0),
            SecureRead::Unsupported
        );
        // Any other failure doesn't say that the field is no password field.
        for error in [
            AXError::CannotComplete,
            AXError::Failure,
            AXError::IllegalArgument,
            AXError::InvalidUIElement,
            AXError::NotImplemented,
            AXError::APIDisabled,
        ] {
            assert_eq!(secure_read_of(error.0), SecureRead::Failed, "{error:?}");
        }
    }

    // ---- The clipboard, on a pasteboard of its own so that the user's is left alone ----

    /// The tests share one private pasteboard, so they take turns.
    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        TURN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn private_board() -> Retained<NSPasteboard> {
        // One name for the whole run: `pasteboardWithName` gives the same board for it every time.
        NSPasteboard::pasteboardWithName(&NSString::from_str(&format!(
            "io.github.henryonai.dictate.test.{}",
            std::process::id()
        )))
    }

    fn private_clipboard() -> MacClipboard {
        MacClipboard {
            board: private_board,
        }
    }

    /// Puts items on the pasteboard the way an app that copies does: one `NSPasteboardItem` for each.
    fn put_items(board: &NSPasteboard, items: &[Vec<(&str, &[u8])>]) {
        board.clearContents();
        let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = items
            .iter()
            .map(|types| {
                let item = NSPasteboardItem::new();
                for (kind, bytes) in types {
                    item.setData_forType(&NSData::with_bytes(bytes), &NSString::from_str(kind));
                }
                ProtocolObject::from_retained(item)
            })
            .collect();
        assert!(board.writeObjects(&NSArray::from_retained_slice(&items)));
    }

    /// What the pasteboard has: each item with its types, in order of type name.
    fn items_of(board: &NSPasteboard) -> Vec<Vec<(String, Vec<u8>)>> {
        board
            .pasteboardItems()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let mut types: Vec<(String, Vec<u8>)> = item
                            .types()
                            .iter()
                            .map(|kind| {
                                let bytes = item.dataForType(&kind).map(|d| d.to_vec());
                                (kind.to_string(), bytes.unwrap_or_default())
                            })
                            .collect();
                        types.sort();
                        types
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn a_snapshot_gives_back_every_item_with_every_type() {
        let _turn = exclusive();
        let clip = private_clipboard();
        let board = private_board();
        let png = [0x89, b'P', b'N', b'G', 0, 1, 2, 3, 255];
        put_items(
            &board,
            &[
                vec![
                    ("public.utf8-plain-text", b"hello".as_slice()),
                    ("public.html", b"<b>hello</b>".as_slice()),
                ],
                vec![("public.png", png.as_slice())],
            ],
        );
        let before = items_of(&board);
        assert_eq!(before.len(), 2);

        let snapshot = clip.snapshot();
        clip.write_transient("dictated").unwrap();
        assert_eq!(items_of(&board).len(), 1);
        clip.restore(snapshot);

        assert_eq!(items_of(&board), before);
    }

    #[test]
    fn restoring_an_empty_clipboard_takes_the_dictated_text_away() {
        let _turn = exclusive();
        let clip = private_clipboard();
        let board = private_board();
        board.clearContents();
        let snapshot = clip.snapshot();
        clip.write_transient("dictated").unwrap();

        clip.restore(snapshot);

        assert!(items_of(&board).is_empty());
    }

    #[test]
    fn dictated_text_is_marked_so_that_clipboard_managers_skip_it() {
        let _turn = exclusive();
        let clip = private_clipboard();
        let board = private_board();

        clip.write_transient("dictated").unwrap();

        let items = items_of(&board);
        assert_eq!(items.len(), 1);
        let kinds: Vec<&str> = items[0].iter().map(|(kind, _)| kind.as_str()).collect();
        assert!(
            kinds.contains(&"org.nspasteboard.TransientType"),
            "{kinds:?}"
        );
        assert_eq!(
            board
                .stringForType(unsafe { NSPasteboardTypeString })
                .map(|text| text.to_string()),
            Some("dictated".to_string())
        );
    }

    #[test]
    fn the_mark_tells_a_later_copy_from_our_own_write() {
        let _turn = exclusive();
        let clip = private_clipboard();

        let mark = clip.write_transient("dictated").unwrap();
        assert!(!clip.changed_since(&mark));

        // Putting the user's clipboard back counts as a change too: nothing is restored twice.
        clip.copy("something the user copied").unwrap();
        assert!(clip.changed_since(&mark));
    }

    #[test]
    fn a_plain_copy_has_no_transient_mark() {
        let _turn = exclusive();
        let clip = private_clipboard();
        let board = private_board();

        clip.copy("kept").unwrap();

        let items = items_of(&board);
        let kinds: Vec<&str> = items[0].iter().map(|(kind, _)| kind.as_str()).collect();
        assert!(!kinds.contains(&TRANSIENT_TYPE), "{kinds:?}");
        assert_eq!(
            board
                .stringForType(unsafe { NSPasteboardTypeString })
                .map(|text| text.to_string()),
            Some("kept".to_string())
        );
    }

    #[test]
    fn each_modifier_of_a_shortcut_counts_as_held() {
        for flag in [
            CGEventFlags::MaskCommand,
            CGEventFlags::MaskControl,
            CGEventFlags::MaskAlternate,
            CGEventFlags::MaskShift,
        ] {
            assert!(modifiers_held(flag), "{flag:?}");
        }
        assert!(modifiers_held(
            CGEventFlags::MaskControl | CGEventFlags::MaskAlternate
        ));
    }

    #[test]
    fn caps_lock_and_the_fn_key_are_no_modifiers_of_a_shortcut() {
        assert!(!modifiers_held(CGEventFlags::empty()));
        assert!(!modifiers_held(CGEventFlags::MaskAlphaShift));
        assert!(!modifiers_held(
            CGEventFlags::MaskSecondaryFn | CGEventFlags::MaskNumericPad
        ));
    }

    #[test]
    fn the_paste_key_goes_down_with_only_the_command_flag() {
        // Only Command: the Ctrl and Alt of the hotkey, if they are still held, must not turn it into another
        // shortcut.
        assert_eq!(paste_flags(true), CGEventFlags::MaskCommand);
    }

    #[test]
    fn the_paste_key_comes_up_with_no_flags() {
        // The system takes the flags of the last event that was posted for the state of the keyboard, and no key
        // of Command ever comes up after ours: an up event that still says Command leaves the system thinking that
        // Command is held (every later `flags_state` says so, and the next paste waits a second for it).
        assert_eq!(paste_flags(false), CGEventFlags::empty());
    }

    #[test]
    fn a_keyboard_at_rest_has_no_modifier_held_whatever_else_the_system_sets() {
        // What `CGEventSourceFlagsState` gave on macOS 26 with nothing held: a bit that is no modifier.
        assert!(!modifiers_held(CGEventFlags(0x2000_0000)));
        // The same after our own up event had said Command, with the Command key reported as down.
        assert!(modifiers_held(CGEventFlags(0x2010_0000)));
        // Only the bits that don't depend on the device count: the left-Command bit of a device alone does not,
        // and with the Command bit it is held like any Command.
        assert!(!modifiers_held(CGEventFlags(0x2000_0008)));
        assert!(modifiers_held(CGEventFlags(0x2010_0008)));
    }
}
