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
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSScreen, NSWorkspace};
use objc2_application_services::{
    kAXTrustedCheckOptionPrompt, AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
    AXUIElement, AXValue, AXValueType,
};
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRange, CFRetained, CFString, CFType, CGPoint, CGRect,
    CGSize,
};

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
}
