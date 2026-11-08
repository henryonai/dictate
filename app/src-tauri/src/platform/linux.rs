//! Linux: X11 through x11rb, and Wayland, which tells nothing.
//!
//! X11 gives the window that has the focus (`_NET_ACTIVE_WINDOW`) and its class, which is enough to know a
//! terminal. It doesn't give what has the focus inside the window, and AT-SPI, which does, is heavy and
//! unreliable, so the field is "unknown" unless the app is a terminal: the text may well have a place to go.
//! Wayland has no way at all to ask another app's window for anything, and no pointer position either: it
//! answers "don't know" to every question.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, GetPropertyReply};
use x11rb::rust_connection::RustConnection;

use super::rules::{is_terminal, parse_wm_class, session, Os, Session};
use super::{
    run_with_timeout, AppId, FieldInfo, FieldLookup, FieldSnapshot, Permission, Platform, Rect,
    ASK_TIMEOUT,
};

/// Where the visible frame of a screen comes from: Tauri knows the monitors.
type ScreenFrame = Box<dyn Fn(Option<(f64, f64)>) -> Rect + Send + Sync>;

pub struct LinuxPlatform {
    session: Session,
    screen: ScreenFrame,
}

impl LinuxPlatform {
    pub fn new(screen: ScreenFrame) -> LinuxPlatform {
        LinuxPlatform {
            session: session(&|name| std::env::var(name).ok()),
            screen,
        }
    }

    fn on_x11(&self) -> bool {
        self.session == Session::X11
    }
}

impl Platform for LinuxPlatform {
    fn frontmost_app(&self) -> Option<AppId> {
        if !self.on_x11() {
            return None;
        }
        x11_front().map(|front| front.app)
    }

    fn focused_field(&self) -> FieldLookup {
        if !self.on_x11() {
            return FieldLookup::Unknown;
        }
        match x11_front() {
            // A terminal always takes typing, which needs no field to be found.
            Some(front) if front.terminal => FieldLookup::Field(FieldInfo {
                app: front.app,
                terminal: true,
                anchor: None,
            }),
            _ => FieldLookup::Unknown,
        }
    }

    fn field_snapshot(&self) -> Option<FieldSnapshot> {
        None
    }

    fn caret_anchor(&self) -> Option<Rect> {
        None
    }

    fn mouse_position(&self) -> Option<(f64, f64)> {
        if !self.on_x11() {
            return None;
        }
        run_with_timeout(ASK_TIMEOUT, || {
            let (connection, screen) = RustConnection::connect(None).ok()?;
            let root = connection.setup().roots.get(screen)?.root;
            let pointer = connection.query_pointer(root).ok()?.reply().ok()?;
            Some((f64::from(pointer.root_x), f64::from(pointer.root_y)))
        })
        .flatten()
    }

    fn screen_visible_frame(&self, near: Option<(f64, f64)>) -> Rect {
        (self.screen)(near)
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

/// The window that has the focus on X11.
struct Front {
    app: AppId,
    terminal: bool,
}

/// The active window and its class, read from the X server. `None` when there is no X server, no window manager
/// that says which window is active, or the server takes longer than [`ASK_TIMEOUT`].
fn x11_front() -> Option<Front> {
    run_with_timeout(ASK_TIMEOUT, || {
        let (connection, screen) = RustConnection::connect(None).ok()?;
        let root = connection.setup().roots.get(screen)?.root;
        let atom = |name: &[u8]| -> Option<Atom> {
            Some(connection.intern_atom(false, name).ok()?.reply().ok()?.atom)
        };

        let active = connection
            .get_property(
                false,
                root,
                atom(b"_NET_ACTIVE_WINDOW")?,
                AtomEnum::WINDOW,
                0,
                1,
            )
            .ok()?
            .reply()
            .ok()?;
        let window = first_value(&active).filter(|window| *window != 0)?;

        let class = connection
            .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
            .ok()?
            .reply()
            .ok()?;
        let (instance, class) = parse_wm_class(&class.value)?;
        let pid = atom(b"_NET_WM_PID").and_then(|pid| {
            let reply = connection
                .get_property(false, window, pid, AtomEnum::CARDINAL, 0, 1)
                .ok()?
                .reply()
                .ok()?;
            first_value(&reply)
        });

        // Some terminals name only the instance well ("gnome-terminal-server" is the instance of the class
        // "Gnome-terminal"), so both names are looked at.
        let terminal = is_terminal(Os::Linux, &class) || is_terminal(Os::Linux, &instance);
        Some(Front {
            app: AppId { id: class, pid },
            terminal,
        })
    })
    .flatten()
}

/// The first number of a property of numbers.
fn first_value(reply: &GetPropertyReply) -> Option<u32> {
    reply.value32()?.next()
}
