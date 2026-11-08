//! Watching the keyboard and the mouse of the whole system, to hide the result chip when the user goes on typing or
//! clicks somewhere: the chip is by the text that was written, and what the user does next makes it stale.
//!
//! Each system watches its own way. macOS has a global event monitor of AppKit, which hears the events that go to other
//! apps, and for that it needs the Accessibility permission that dictation has asked for already. Windows has two low
//! level hooks that run on a thread of their own. X11 and Wayland have nothing that every desktop allows, and the chip
//! goes by its timer and by a change of the app in front. Only that something was pressed is reported, never which key:
//! this is not a keylogger, and nothing is kept.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

/// What a monitor keeps of what the system gave for it (the object that removes an AppKit monitor): one for each
/// monitor, so that two monitors in a process can't take each other's down, and at most one at a time for each.
#[derive(Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct Installed<T> {
    slot: Mutex<Option<T>>,
}

impl<T> Default for Installed<T> {
    fn default() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl<T> Installed<T> {
    /// Keeps what `make` gives, unless something is kept already (then `make` is not called). Says whether there is
    /// something kept now.
    pub fn install(&self, make: impl FnOnce() -> Option<T>) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = make();
        }
        slot.is_some()
    }

    /// Hands over what is kept, to be removed.
    pub fn take(&self) -> Option<T> {
        self.slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// How the thread that puts hooks in and the one that started it agree, when the second waits only so long: whichever
/// comes second sees what the first did. The thread says that it is in with `arrived`, and is told whether it is still
/// wanted; the starter that stops waiting calls `give_up`, and is told the thread to send a quit message to, when it
/// has arrived. One of the two always knows, so that no thread is left running its hooks with nobody that can stop it.
#[derive(Debug, Default)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub struct Handshake {
    thread: AtomicU32,
    given_up: AtomicBool,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl Handshake {
    /// The thread `thread` has its hooks in. False when the starter has given up waiting: take them out and leave.
    pub fn arrived(&self, thread: u32) -> bool {
        // Both sides write what they did and then read what the other did, all in one order (`SeqCst`): they can't both miss.
        self.thread.store(thread, Ordering::SeqCst);
        !self.given_up.load(Ordering::SeqCst)
    }

    /// The starter stops waiting. The thread that has arrived, if one has, is the one to tell to quit; one that has not
    /// will find out by itself.
    pub fn give_up(&self) -> Option<u32> {
        self.given_up.store(true, Ordering::SeqCst);
        match self.thread.load(Ordering::SeqCst) {
            0 => None,
            thread => Some(thread),
        }
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

    #[test]
    fn installed_keeps_the_first_and_does_not_make_another() {
        let installed = Installed::<u8>::default();
        assert!(installed.install(|| Some(1)));
        // Something is kept: the second `make` is not even called.
        assert!(installed.install(|| panic!("a second monitor was made")));
        assert_eq!(installed.take(), Some(1));
    }

    #[test]
    fn installed_gives_what_it_kept_once() {
        let installed = Installed::<u8>::default();
        assert_eq!(installed.take(), None);
        installed.install(|| Some(7));
        assert_eq!(installed.take(), Some(7));
        assert_eq!(installed.take(), None);
        // And can be filled again after.
        assert!(installed.install(|| Some(8)));
        assert_eq!(installed.take(), Some(8));
    }

    #[test]
    fn installed_stays_empty_when_the_system_gives_nothing() {
        let installed = Installed::<u8>::default();
        assert!(!installed.install(|| None));
        assert_eq!(installed.take(), None);
        // The next start may be given one.
        assert!(installed.install(|| Some(2)));
    }

    #[test]
    fn two_monitors_do_not_share_what_was_installed() {
        // Each monitor has its own: one that stops takes only its own monitor down.
        let (first, second) = (Installed::<u8>::default(), Installed::<u8>::default());
        first.install(|| Some(1));
        second.install(|| Some(2));
        assert_eq!(first.take(), Some(1));
        assert_eq!(second.take(), Some(2));
    }

    #[test]
    fn a_thread_that_arrives_before_the_starter_gives_up_is_told_to_quit() {
        let handshake = Handshake::default();
        assert!(handshake.arrived(4242));
        assert_eq!(handshake.give_up(), Some(4242));
    }

    #[test]
    fn a_thread_that_arrives_after_the_starter_gave_up_leaves_by_itself() {
        let handshake = Handshake::default();
        assert_eq!(handshake.give_up(), None);
        assert!(!handshake.arrived(4242));
    }

    #[test]
    fn a_starter_that_gives_up_on_a_thread_that_never_comes_has_nobody_to_tell() {
        assert_eq!(Handshake::default().give_up(), None);
    }

    #[test]
    fn one_of_the_two_always_knows_when_they_meet_at_once() {
        // The thread and the starter race, many times over: either the thread is told to leave, or the starter is told
        // its id. Never neither.
        for _ in 0..2_000 {
            let handshake = Arc::new(Handshake::default());
            let thread = {
                let handshake = handshake.clone();
                std::thread::spawn(move || handshake.arrived(77))
            };
            let told = handshake.give_up();
            let wanted = thread.join().unwrap();
            assert!(
                !wanted || told == Some(77),
                "wanted {wanted}, told {told:?}"
            );
        }
    }
}
