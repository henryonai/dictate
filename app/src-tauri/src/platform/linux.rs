//! Linux: X11 through x11rb, and Wayland, which tells nothing.
//!
//! X11 gives the window that has the focus (`_NET_ACTIVE_WINDOW`) and its class, which is enough to know a
//! terminal. It doesn't give what has the focus inside the window, and AT-SPI, which does, is heavy and
//! unreliable, so the field is "unknown" unless the app is a terminal: the text may well have a place to go.
//! Wayland has no way at all to ask another app's window for anything, and no pointer position either: it
//! answers "don't know" to every question.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, GetPropertyReply, KeyButMask};
use x11rb::rust_connection::RustConnection;

use super::keys::press_paste;
use super::paste::{
    find_on_path, is_app, pick_wayland_tool, wayland_args, wayland_env, ydotool_sockets, KeySender,
    PasteError,
};
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

    fn screen_recording(&self) -> Permission {
        Permission::NotNeeded
    }

    fn request_screen_recording(&self) {}
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
        let terminal = is_terminal(Os::current(), &class) || is_terminal(Os::current(), &instance);
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

// ---- The keys ------------------------------------------------------------------------------------------------

/// How long a tool of Wayland (`wtype`, `ydotool`) has to press the keys.
const TOOL_TIME: Duration = Duration::from_secs(2);

/// Presses Ctrl+V (Ctrl+Shift+V in a terminal): through the XTest extension on X11, and through `wtype` or
/// `ydotool` on Wayland. Wayland lets no app press keys in another, so without one of the two tools there is
/// nothing to do but copy.
pub struct LinuxKeys {
    session: Session,
    last_post: Mutex<Option<Instant>>,
}

impl LinuxKeys {
    pub fn new() -> LinuxKeys {
        LinuxKeys {
            session: session(&|name| std::env::var(name).ok()),
            last_post: Mutex::new(None),
        }
    }

    fn on_x11(&self) -> bool {
        self.session == Session::X11
    }
}

impl KeySender for LinuxKeys {
    fn bring_forward(&self, app: &AppId) -> bool {
        // Wayland doesn't say who is in front, and an app that wasn't found by name can't be waited for: the
        // keys go to whoever has the focus, which is the app that the dictation started in.
        if !self.on_x11() || (app.id.is_empty() && app.pid.is_none()) {
            return true;
        }
        match x11_front() {
            Some(front) => is_app(&front.app, app),
            // No window manager that says which window is active.
            None => true,
        }
    }

    fn modifiers_released(&self) -> bool {
        // Wayland doesn't say which keys are down.
        if !self.on_x11() {
            return true;
        }
        let held = run_with_timeout(ASK_TIMEOUT, || {
            let (connection, screen) = RustConnection::connect(None).ok()?;
            let root = connection.setup().roots.get(screen)?.root;
            let mask = connection.query_pointer(root).ok()?.reply().ok()?.mask;
            let shortcut =
                KeyButMask::SHIFT | KeyButMask::CONTROL | KeyButMask::MOD1 | KeyButMask::MOD4;
            Some(mask.intersects(shortcut))
        })
        .flatten();
        !held.unwrap_or(false)
    }

    fn send_paste(&self, terminal: bool) -> Result<(), PasteError> {
        let sent = if self.on_x11() {
            press_paste(terminal)
        } else {
            wayland_paste(terminal)
        };
        if sent.is_ok() {
            *self
                .last_post
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
        }
        sent
    }

    fn last_post(&self) -> Option<Instant> {
        *self
            .last_post
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Presses the keys with the tool that this Wayland desktop has: `wtype` when it is installed, else `ydotool`
/// with its daemon running.
fn wayland_paste(terminal: bool) -> Result<(), PasteError> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let on_path = |name: &str| find_on_path(name, &path, Path::is_file);
    let uid = std::fs::metadata("/proc/self").ok().map(|own| own.uid());
    let sockets = ydotool_sockets(
        std::env::var("YDOTOOL_SOCKET").ok().as_deref(),
        std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
        uid,
    );
    let listening = sockets.into_iter().find(|socket| socket.exists());
    let tool = pick_wayland_tool(on_path("wtype"), on_path("ydotool"), listening)
        .ok_or(PasteError::NoWaylandTool)?;
    let program = tool.program();
    let mut command = Command::new(program);
    command
        .args(wayland_args(&tool, terminal))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // ydotool looks for its daemon in a place of its own unless it is told where it was found.
    if let Some((name, value)) = wayland_env(&tool) {
        command.env(name, value);
    }
    let mut child = command
        .spawn()
        .map_err(|error| PasteError::Failed(format!("{}: {error}", program.display())))?;
    // A tool that hangs (a daemon that doesn't answer) must not hold the paste.
    let deadline = Instant::now() + TOOL_TIME;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(PasteError::Failed(format!(
                    "{} failed: {status}",
                    program.display()
                )))
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(PasteError::Failed(format!(
                    "{} did not answer",
                    program.display()
                )));
            }
            Err(error) => return Err(PasteError::Failed(error.to_string())),
        }
    }
}
