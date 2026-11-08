//! Watching the keyboard and the mouse of the whole system, to hide the result chip when the user goes on typing or
//! clicks somewhere: the chip is by the text that was written, and what the user does next makes it stale.
//!
//! Each system watches its own way. macOS has a global event monitor of AppKit, which hears the events that go to other
//! apps, and for that it needs the Accessibility permission that dictation has asked for already. Windows has two low
//! level hooks that run on a thread of their own. X11 and Wayland have nothing that every desktop allows, and the chip
//! goes by its timer and by a change of the app in front. Only that something was pressed is reported, never which key:
//! this is not a keylogger, and nothing is kept.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, PoisonError};

/// Reports that the user pressed a key or a mouse button, in any app.
pub trait InputMonitor: Send + Sync {
    /// Starts watching: `on_input` is called for every key that goes down and every left or right mouse button that goes
    /// down, on whatever thread the system reports from (the main thread on macOS, a thread of the monitor's own on
    /// Windows), and it must be quick: hand the work to another thread. Starting again replaces the handler of the one
    /// before.
    fn start(&self, on_input: Box<dyn Fn() + Send + Sync>);
    /// Stops watching. After it returns the handler is not called again (one that is running now finishes), and stopping a
    /// monitor that is not started changes nothing.
    fn stop(&self);
}

/// The monitor of a system that can't watch: X11 and Wayland.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct NoMonitor;

impl InputMonitor for NoMonitor {
    fn start(&self, _on_input: Box<dyn Fn() + Send + Sync>) {}

    fn stop(&self) {}
}

/// Whether a window message that a Windows hook hears is a press that the chip goes away for: a key going down (with
/// or without Alt held) or the left or right mouse button going down. Releases, moves, the wheel and the other buttons are
/// not. The numbers are those of the system's headers; `windows.rs` checks them against the `windows` crate when it is
/// built.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn is_press_message(message: u32) -> bool {
    matches!(message, 0x0100 | 0x0104 | 0x0201 | 0x0204)
}

/// Where a monitor sends what it hears: the handler that `start` was given, until `stop`. Shared by the monitors that
/// watch, so that what is promised of `stop` is kept in one place.
#[derive(Default)]
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub struct Listener {
    handler: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
impl Listener {
    pub fn set(&self, handler: Box<dyn Fn() + Send + Sync>) {
        *self.slot() = Some(Arc::from(handler));
    }

    pub fn clear(&self) {
        *self.slot() = None;
    }

    /// Calls the handler, if there is one. The lock is let go before the call, so that a handler may stop the monitor.
    /// A handler that panics is told of in the log and goes no further: the call comes from the system (AppKit on the
    /// main thread), which a panic must not unwind into.
    pub fn report(&self) {
        let handler = self.slot().clone();
        if let Some(handler) = handler {
            if catch_unwind(AssertUnwindSafe(|| handler())).is_err() {
                log::warn!("The handler of the keyboard and mouse monitor panicked");
            }
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<Arc<dyn Fn() + Send + Sync>>> {
        // A handler that panicked must not silence the monitor for good.
        self.handler.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn counter() -> (Arc<AtomicUsize>, Box<dyn Fn() + Send + Sync>) {
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        (
            count,
            Box::new(move || {
                seen.fetch_add(1, Ordering::SeqCst);
            }),
        )
    }

    #[test]
    fn what_is_heard_goes_to_the_handler() {
        let listener = Listener::default();
        let (count, handler) = counter();
        listener.set(handler);
        listener.report();
        listener.report();
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn nothing_is_reported_before_start_or_after_stop() {
        let listener = Listener::default();
        listener.report();

        let (count, handler) = counter();
        listener.set(handler);
        listener.clear();
        listener.report();
        assert_eq!(count.load(Ordering::SeqCst), 0);
        // Stopping what is stopped changes nothing.
        listener.clear();
    }

    #[test]
    fn starting_again_replaces_the_handler() {
        let listener = Listener::default();
        let (first, handler) = counter();
        listener.set(handler);
        let (second, handler) = counter();
        listener.set(handler);
        listener.report();
        assert_eq!(
            (first.load(Ordering::SeqCst), second.load(Ordering::SeqCst)),
            (0, 1)
        );
    }

    #[test]
    fn a_handler_may_stop_the_monitor_it_is_called_by() {
        // Hiding the chip stops the monitor, and the chip is hidden by the handler: the lock must not be held.
        let listener = Arc::new(Listener::default());
        let (count, _) = counter();
        let inner = listener.clone();
        let seen = count.clone();
        listener.set(Box::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            inner.clear();
        }));
        listener.report();
        listener.report();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_handler_that_panics_does_not_take_the_monitor_down_with_it() {
        // On macOS the handler is called from AppKit, which a panic would unwind into, and the app would be gone.
        let listener = Listener::default();
        listener.set(Box::new(|| panic!("the handler is broken")));
        listener.report();

        // The monitor goes on: the next handler is heard.
        let (count, handler) = counter();
        listener.set(handler);
        listener.report();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_monitor_of_a_system_that_cannot_watch_never_reports() {
        let monitor = NoMonitor;
        let (count, handler) = counter();
        monitor.start(handler);
        monitor.stop();
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn only_a_key_or_a_left_or_right_button_going_down_is_a_press() {
        // WM_KEYDOWN, WM_SYSKEYDOWN, WM_LBUTTONDOWN, WM_RBUTTONDOWN.
        for message in [0x0100, 0x0104, 0x0201, 0x0204] {
            assert!(is_press_message(message), "{message:#06x}");
        }
        // WM_KEYUP, WM_CHAR, WM_SYSKEYUP, WM_MOUSEMOVE, WM_LBUTTONUP, WM_RBUTTONUP, WM_MBUTTONDOWN, WM_MOUSEWHEEL.
        for message in [
            0x0101, 0x0102, 0x0105, 0x0200, 0x0202, 0x0205, 0x0207, 0x020a,
        ] {
            assert!(!is_press_message(message), "{message:#06x}");
        }
    }
}
