//! The global shortcut that starts and stops dictation, and `dictate --toggle`, which does the same from a script.
//!
//! Every way of hearing the key ends as [`HotkeyEvent`]s on one channel, which the dictation controller reads.

use std::io;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dictate_services::dictation::lang::display_hotkey;
use dictate_services::settings::SettingsStore;
use serde::Serialize;
#[cfg(target_os = "linux")]
use tauri::Emitter;
use tauri::{AppHandle, Manager};
#[cfg(any(test, target_os = "linux"))]
use tauri_plugin_global_shortcut::Modifiers;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tokio::sync::mpsc::UnboundedSender;

use crate::state::lock;

#[cfg(target_os = "linux")]
mod portal;

/// What the shortcut, or `dictate --toggle`, tells the dictation controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// The key went down (once: holding it doesn't repeat this).
    Pressed,
    /// The key went up.
    Released,
    /// `dictate --toggle`: start when idle, stop when recording.
    Toggle,
}

/// How long on X11 a release waits for a press to follow it. Holding a key there sends a release and a press for
/// every repeat, and the two come within a few milliseconds of each other.
pub const X11_RELEASE_DEBOUNCE: Duration = Duration::from_millis(40);

/// Decides which presses and releases of the key are real. It has no clock and no thread: the caller says what
/// time it is and when to ask again.
#[derive(Debug)]
pub struct HotkeyFilter {
    debounce: Duration,
    down: bool,
    release_due: Option<Instant>,
}

impl HotkeyFilter {
    /// `debounce` is how long a release waits for a press that would make it a repeat of the key held down.
    pub fn new(debounce: Duration) -> HotkeyFilter {
        HotkeyFilter {
            debounce,
            down: false,
            release_due: None,
        }
    }

    /// Whether this press is a new one. Call `poll` first, or a release that is due is lost.
    pub fn on_pressed(&mut self, now: Instant) -> bool {
        match self.release_due.take() {
            // A press soon after a release: the key is repeating, it never went up.
            Some(due) if now < due => false,
            // The release was due and nobody asked: the key did go up, and this is a new press.
            Some(_) => {
                self.down = true;
                true
            }
            None if self.down => false,
            None => {
                self.down = true;
                true
            }
        }
    }

    /// The time at which this release counts, unless a press comes before: `poll` at that time. `None` for a
    /// release of a key that wasn't down, or of one whose release is waiting already.
    pub fn on_released(&mut self, now: Instant) -> Option<Instant> {
        if !self.down || self.release_due.is_some() {
            return None;
        }
        let due = now + self.debounce;
        self.release_due = Some(due);
        Some(due)
    }

    /// Whether a release that was waiting has come due: the key is up.
    pub fn poll(&mut self, now: Instant) -> bool {
        match self.release_due {
            Some(due) if now >= due => {
                self.release_due = None;
                self.down = false;
                true
            }
            _ => false,
        }
    }
}

/// How the shortcut is reached: by a grab the app makes itself, by the desktop's portal, or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Backend {
    #[serde(rename = "global")]
    Global,
    /// Only a Wayland session makes this one; it is built everywhere so that the window has one answer to read.
    #[serde(rename = "portal")]
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Portal,
    #[serde(rename = "none")]
    Unavailable,
}

/// What Settings shows about the shortcut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HotkeyStatus {
    /// The shortcut as the user reads it.
    pub label: String,
    pub backend: Backend,
    /// A key of the UI's texts, when the shortcut doesn't work.
    pub error: Option<String>,
}

pub const INVALID: &str = "hotkey.invalid";
pub const TAKEN: &str = "hotkey.taken";
/// The desktop of a Wayland session has no global shortcuts for apps.
#[cfg(target_os = "linux")]
pub const WAYLAND_UNAVAILABLE: &str = "hotkey.wayland_unavailable";

/// Why the shortcut could not be changed.
#[derive(Debug)]
pub enum ChangeError {
    Invalid,
    Taken,
    Save(io::Error),
}

/// What changing the shortcut needs from the system: to grab a shortcut for the whole desktop and let it go.
pub trait Registry: Send + Sync {
    fn register(&self, shortcut: Shortcut) -> Result<(), String>;
    fn unregister(&self, shortcut: Shortcut) -> Result<(), String>;
}

/// The shortcut of the app, as Settings changes it.
#[derive(Clone)]
pub struct Hotkeys {
    registry: Option<Arc<dyn Registry>>,
    state: Arc<Mutex<State>>,
}

struct State {
    status: HotkeyStatus,
    registered: Option<Shortcut>,
}

impl Hotkeys {
    pub fn new(
        registry: Option<Arc<dyn Registry>>,
        status: HotkeyStatus,
        registered: Option<Shortcut>,
    ) -> Hotkeys {
        Hotkeys {
            registry,
            state: Arc::new(Mutex::new(State { status, registered })),
        }
    }

    pub fn status(&self) -> HotkeyStatus {
        lock(&self.state).status.clone()
    }

    /// Changes the status as the desktop's portal tells things. The new status.
    #[cfg(target_os = "linux")]
    fn update_status(&self, change: impl FnOnce(&mut HotkeyStatus)) -> HotkeyStatus {
        let mut state = lock(&self.state);
        change(&mut state.status);
        state.status.clone()
    }

    /// Registers `accel`, saves it and lets the old shortcut go. Whatever fails, the old shortcut is as it was.
    pub fn change(&self, store: &SettingsStore, accel: &str) -> Result<(), ChangeError> {
        let accel = accel.trim();
        let shortcut = parse_accelerator(accel)?;
        let save = || {
            store
                .update(|settings| settings.hotkey = accel.to_string())
                .map(drop)
                .map_err(ChangeError::Save)
        };
        let Some(registry) = &self.registry else {
            // The desktop owns the shortcut: what is saved is the one Dictate asks for the next time it binds.
            return save();
        };

        let mut state = lock(&self.state);
        if state.registered != Some(shortcut) {
            // The new one first: a shortcut that can't be had must cost the old one nothing.
            registry.register(shortcut).map_err(|e| {
                log::warn!("Could not register the shortcut {accel}: {e}");
                ChangeError::Taken
            })?;
            if let Err(e) = save() {
                if let Err(undo) = registry.unregister(shortcut) {
                    log::warn!("Could not let go of the shortcut {accel}: {undo}");
                }
                return Err(e);
            }
            if let Some(old) = state.registered.replace(shortcut) {
                if let Err(e) = registry.unregister(old) {
                    log::warn!("Could not let go of the old shortcut: {e}");
                }
            }
        } else {
            // The same shortcut, maybe written another way.
            save()?;
        }
        state.status = HotkeyStatus {
            label: label_of(accel),
            backend: Backend::Global,
            error: None,
        };
        Ok(())
    }
}

/// The shortcut as the user reads it: symbols on the Mac, names elsewhere.
fn label_of(accel: &str) -> String {
    display_hotkey(accel, cfg!(target_os = "macos"))
}

/// The accelerator as a shortcut, when Dictate can use it: modifiers, then one key; and at least one modifier unless
/// the key is a function key, so that a shortcut can't swallow a letter in every app.
pub fn parse_accelerator(accel: &str) -> Result<Shortcut, ChangeError> {
    let shortcut = Shortcut::from_str(accel).map_err(|_| ChangeError::Invalid)?;
    if shortcut.mods.is_empty() && !is_function_key(&shortcut) {
        return Err(ChangeError::Invalid);
    }
    Ok(shortcut)
}

/// F1 to F24.
fn is_function_key(shortcut: &Shortcut) -> bool {
    let name = shortcut.key.to_string();
    name.strip_prefix('F')
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=24).contains(&number))
}

/// Whether the arguments of a launch ask for `--toggle`.
pub fn wants_toggle<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter().any(|arg| arg.as_ref() == "--toggle")
}

/// The shortcut as the portal is asked for it, in the words of the freedesktop shortcuts specification: modifiers
/// `CTRL`, `ALT`, `SHIFT` and `LOGO`, then the name of the key in X11's list of key names (`CTRL+ALT+space`).
/// `None` for a key whose name this doesn't know.
#[cfg(any(test, target_os = "linux"))]
fn portal_trigger(shortcut: &Shortcut) -> Option<String> {
    let name = shortcut.key.to_string();
    let key = if let Some(letter) = name.strip_prefix("Key") {
        letter.to_ascii_lowercase()
    } else if let Some(digit) = name.strip_prefix("Digit") {
        digit.to_string()
    } else if is_function_key(shortcut) {
        name
    } else {
        match name.as_str() {
            "Space" => "space",
            "Enter" => "Return",
            "Tab" => "Tab",
            "Backspace" => "BackSpace",
            "Delete" => "Delete",
            "Insert" => "Insert",
            "Home" => "Home",
            "End" => "End",
            "PageUp" => "Page_Up",
            "PageDown" => "Page_Down",
            "ArrowUp" => "Up",
            "ArrowDown" => "Down",
            "ArrowLeft" => "Left",
            "ArrowRight" => "Right",
            _ => return None,
        }
        .to_string()
    };
    let mods = shortcut.mods;
    let mut parts: Vec<&str> = Vec::new();
    for (modifier, name) in [
        (Modifiers::CONTROL, "CTRL"),
        (Modifiers::ALT, "ALT"),
        (Modifiers::SHIFT, "SHIFT"),
        (Modifiers::SUPER, "LOGO"),
    ] {
        if mods.contains(modifier) {
            parts.push(name);
        }
    }
    parts.push(&key);
    Some(parts.join("+"))
}

/// The events of the keys, in order: the filter and the channel behind one lock.
struct Gate {
    filter: HotkeyFilter,
    tx: UnboundedSender<HotkeyEvent>,
}

impl Gate {
    fn new(debounce: Duration, tx: UnboundedSender<HotkeyEvent>) -> Gate {
        Gate {
            filter: HotkeyFilter::new(debounce),
            tx,
        }
    }

    /// The key went down. A release that came due before is sent first: it was a real one.
    fn pressed(&mut self, now: Instant) {
        self.tick(now);
        if self.filter.on_pressed(now) {
            self.send(HotkeyEvent::Pressed);
        }
    }

    /// The key went up. When the release can't be told from a repeat yet: the time to call `tick` at.
    fn released(&mut self, now: Instant) -> Option<Instant> {
        self.tick(now);
        let due = self.filter.on_released(now)?;
        if due > now {
            return Some(due);
        }
        self.tick(now);
        None
    }

    /// Time has passed: sends the release that has come due, if there is one.
    fn tick(&mut self, now: Instant) {
        if self.filter.poll(now) {
            self.send(HotkeyEvent::Released);
        }
    }

    fn send(&self, event: HotkeyEvent) {
        // The controller is gone only while the app quits.
        let _ = self.tx.send(event);
    }
}

/// A key went down or up. When the release can't be told from a repeat yet, a timer sends it once it can.
fn key_event(gate: &Arc<Mutex<Gate>>, down: bool, now: Instant) {
    if down {
        lock(gate).pressed(now);
        return;
    }
    let due = lock(gate).released(now);
    if let Some(due) = due {
        let gate = Arc::clone(gate);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep_until(due.into()).await;
            lock(&gate).tick(Instant::now());
        });
    }
}

/// How long a release waits for a press that would make it a repeat: on X11, which holds a key down as a series of
/// releases and presses. Windows and macOS tell the press and the release of a key once, and Wayland's portal too.
fn release_debounce() -> Duration {
    if cfg!(target_os = "linux") {
        X11_RELEASE_DEBOUNCE
    } else {
        Duration::ZERO
    }
}

/// The event of the settings window that tells the shortcut changed by itself: the desktop answered the portal.
#[cfg(target_os = "linux")]
const STATUS_EVENT: &str = "hotkey-status";

/// Whether this process runs in a Wayland session, where an app can't grab a key itself.
fn wayland_session() -> bool {
    #[cfg(target_os = "linux")]
    {
        use crate::platform::rules::{session, Session};
        session(&|name| std::env::var(name).ok()) == Session::Wayland
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Whether the global shortcut plugin goes into the app. Not in a Wayland session: the plugin grabs keys through
/// X11, and a session with no XWayland would fail to start the whole app over it.
pub fn uses_global_plugin() -> bool {
    !wayland_session()
}

/// The shortcut of the plugin: grabs a key for the whole desktop, and sends what happens to it to the gate.
struct PluginRegistry {
    app: AppHandle,
    gate: Arc<Mutex<Gate>>,
}

impl Registry for PluginRegistry {
    fn register(&self, shortcut: Shortcut) -> Result<(), String> {
        let gate = Arc::clone(&self.gate);
        self.app
            .global_shortcut()
            .on_shortcut(shortcut, move |_app, _shortcut, event| {
                key_event(
                    &gate,
                    event.state() == ShortcutState::Pressed,
                    Instant::now(),
                );
            })
            .map_err(|e| e.to_string())
    }

    fn unregister(&self, shortcut: Shortcut) -> Result<(), String> {
        self.app
            .global_shortcut()
            .unregister(shortcut)
            .map_err(|e| e.to_string())
    }
}

/// Starts listening for the shortcut `accel`: its presses and releases go to `tx`. (`dictate --toggle` is not heard
/// here: the single-instance callback and the first launch send that one themselves.) The answer says whether the
/// shortcut works, and the app manages the [`Hotkeys`] that the commands `set_hotkey` and `hotkey_status` use. A
/// shortcut that can't be had is not an error: the app goes on without it and Settings says why.
pub fn start(app: &AppHandle, accel: &str, tx: UnboundedSender<HotkeyEvent>) -> HotkeyStatus {
    #[cfg(target_os = "linux")]
    let hotkeys = if wayland_session() {
        start_portal(app, accel, tx)
    } else {
        start_global(app, accel, tx)
    };
    #[cfg(not(target_os = "linux"))]
    let hotkeys = start_global(app, accel, tx);

    let status = hotkeys.status();
    app.manage(hotkeys);
    status
}

/// The shortcut is grabbed through the plugin (macOS, Windows and X11).
fn start_global(app: &AppHandle, accel: &str, tx: UnboundedSender<HotkeyEvent>) -> Hotkeys {
    let gate = Arc::new(Mutex::new(Gate::new(release_debounce(), tx)));
    let registry = Arc::new(PluginRegistry {
        app: app.clone(),
        gate,
    });
    let label = label_of(accel);
    let (registered, status) = match parse_accelerator(accel) {
        Err(_) => (None, unavailable(label, INVALID)),
        Ok(shortcut) => match registry.register(shortcut) {
            Ok(()) => (
                Some(shortcut),
                HotkeyStatus {
                    label,
                    backend: Backend::Global,
                    error: None,
                },
            ),
            Err(e) => {
                log::warn!("Could not register the shortcut {accel}: {e}");
                (None, unavailable(label, TAKEN))
            }
        },
    };
    Hotkeys::new(Some(registry), status, registered)
}

/// The shortcut is the desktop's (Wayland): the app asks the portal to bind it and hears it from there. The portal can
/// take as long as the user takes to confirm, so the status starts as the shortcut that was asked for and changes
/// when the desktop answers.
#[cfg(target_os = "linux")]
fn start_portal(app: &AppHandle, accel: &str, tx: UnboundedSender<HotkeyEvent>) -> Hotkeys {
    let gate = Arc::new(Mutex::new(Gate::new(Duration::ZERO, tx)));
    let status = HotkeyStatus {
        label: label_of(accel),
        backend: Backend::Portal,
        error: None,
    };
    let hotkeys = Hotkeys::new(None, status, None);
    let preferred = parse_accelerator(accel)
        .ok()
        .and_then(|shortcut| portal_trigger(&shortcut));
    let events = PortalEvents {
        app: app.clone(),
        hotkeys: hotkeys.clone(),
        gate,
    };
    tauri::async_runtime::spawn(portal::run(events, preferred));
    hotkeys
}

/// What the portal says, as the status of the shortcut and as the keys.
#[cfg(target_os = "linux")]
struct PortalEvents {
    app: AppHandle,
    hotkeys: Hotkeys,
    gate: Arc<Mutex<Gate>>,
}

#[cfg(target_os = "linux")]
impl PortalEvents {
    /// Changes the status and tells the window.
    fn publish(&self, change: impl FnOnce(&mut HotkeyStatus)) {
        let status = self.hotkeys.update_status(change);
        if let Err(e) = self.app.emit(STATUS_EVENT, &status) {
            log::warn!("Could not tell the window how the shortcut stands: {e}");
        }
    }

    /// The desktop describes the shortcut like this: that is what Settings shows.
    fn described(&self, trigger: String) {
        self.publish(|status| {
            status.backend = Backend::Portal;
            status.error = None;
            if !trigger.is_empty() {
                status.label = trigger;
            }
        });
    }
}

#[cfg(target_os = "linux")]
impl portal::Events for PortalEvents {
    fn bound(&self, trigger: String) {
        log::info!("The desktop bound the shortcut: {trigger}");
        self.described(trigger);
    }

    fn changed(&self, trigger: String) {
        log::info!("The desktop changed the shortcut: {trigger}");
        self.described(trigger);
    }

    fn failed(&self, reason: String) {
        log::warn!("The desktop gives no global shortcut: {reason}");
        self.publish(|status| {
            status.backend = Backend::Unavailable;
            status.error = Some(WAYLAND_UNAVAILABLE.to_string());
        });
    }

    fn key(&self, down: bool) {
        key_event(&self.gate, down, Instant::now());
    }
}

/// A shortcut that doesn't work, for the reason that the key of the UI's texts tells.
fn unavailable(label: String, error: &str) -> HotkeyStatus {
    HotkeyStatus {
        label,
        backend: Backend::Unavailable,
        error: Some(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeRegistry;
    use dictate_services::dictation::lang::display_hotkey;
    use std::str::FromStr;
    use tokio::sync::mpsc::unbounded_channel;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    fn shortcut(accel: &str) -> Shortcut {
        Shortcut::from_str(accel).unwrap()
    }

    fn label(accel: &str) -> String {
        display_hotkey(accel, cfg!(target_os = "macos"))
    }

    // ---- the filter

    #[test]
    fn hotkey_filter_drops_repeats() {
        let t = Instant::now();
        let mut filter = HotkeyFilter::new(Duration::ZERO);

        assert!(filter.on_pressed(t), "the first press is a press");
        assert!(!filter.on_pressed(t + MS(30)), "held: a repeat");
        assert!(!filter.on_pressed(t + MS(60)), "held: a repeat");

        // With nothing to wait for, a release is due at once.
        assert_eq!(filter.on_released(t + MS(500)), Some(t + MS(500)));
        assert!(filter.poll(t + MS(500)));

        assert!(
            filter.on_pressed(t + MS(900)),
            "the key went up: a new press"
        );
    }

    #[test]
    fn hotkey_filter_ignores_a_release_of_a_key_that_was_not_down() {
        let t = Instant::now();
        let mut filter = HotkeyFilter::new(Duration::ZERO);

        assert_eq!(filter.on_released(t), None);
        assert!(!filter.poll(t));
    }

    #[test]
    fn hotkey_filter_x11_debounces_release() {
        let t = Instant::now();
        let mut filter = HotkeyFilter::new(X11_RELEASE_DEBOUNCE);
        assert!(filter.on_pressed(t));

        // An auto-repeat on X11: a release, and a press 5 ms later.
        let due = filter
            .on_released(t + MS(500))
            .expect("a release to wait for");
        assert_eq!(due, t + MS(540));
        assert!(!filter.poll(t + MS(504)), "not due yet");
        assert!(
            !filter.on_pressed(t + MS(505)),
            "the press that follows is the repeat"
        );
        assert!(!filter.poll(t + MS(600)), "the release was cancelled");

        // The key is still down: the next release waits again, and nothing came of the first.
        let due = filter
            .on_released(t + MS(1000))
            .expect("a release to wait for");
        assert_eq!(due, t + MS(1040));
    }

    #[test]
    fn hotkey_filter_real_release_after_40ms() {
        let t = Instant::now();
        let mut filter = HotkeyFilter::new(X11_RELEASE_DEBOUNCE);
        assert!(filter.on_pressed(t));

        let due = filter
            .on_released(t + MS(500))
            .expect("a release to wait for");
        assert!(!filter.poll(t + MS(539)), "39 ms: a press could still come");
        assert!(filter.poll(due), "40 ms: nothing came, the key is up");
        assert!(!filter.poll(due + MS(1)), "it is reported once");

        assert!(
            filter.on_pressed(t + MS(700)),
            "a press after that is a new one"
        );
    }

    #[test]
    fn hotkey_filter_a_second_release_while_one_waits_changes_nothing() {
        let t = Instant::now();
        let mut filter = HotkeyFilter::new(X11_RELEASE_DEBOUNCE);
        filter.on_pressed(t);
        let due = filter.on_released(t + MS(10));

        assert_eq!(filter.on_released(t + MS(20)), None);
        assert!(filter.poll(due.unwrap()));
    }

    // ---- the gate: the filter and the channel

    #[test]
    fn gate_sends_one_pressed_and_one_released_for_a_held_key() {
        let t = Instant::now();
        let (tx, mut rx) = unbounded_channel();
        let mut gate = Gate::new(Duration::ZERO, tx);

        gate.pressed(t);
        gate.pressed(t + MS(30));
        assert_eq!(
            gate.released(t + MS(500)),
            None,
            "nothing is left to wait for"
        );

        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Pressed));
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Released));
        assert!(rx.try_recv().is_err(), "and nothing else");
    }

    #[test]
    fn gate_sends_a_due_release_before_the_press_that_follows_it() {
        let t = Instant::now();
        let (tx, mut rx) = unbounded_channel();
        let mut gate = Gate::new(X11_RELEASE_DEBOUNCE, tx);

        gate.pressed(t);
        let due = gate.released(t + MS(10));
        assert_eq!(
            due,
            Some(t + MS(50)),
            "the caller must come back at that time"
        );
        // The timer is late, and the next press is there first.
        gate.pressed(t + MS(80));

        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Pressed));
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Released));
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Pressed));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn gate_sends_nothing_for_an_x11_repeat() {
        let t = Instant::now();
        let (tx, mut rx) = unbounded_channel();
        let mut gate = Gate::new(X11_RELEASE_DEBOUNCE, tx);

        gate.pressed(t);
        let due = gate.released(t + MS(500)).unwrap();
        gate.pressed(t + MS(503));
        gate.tick(due);

        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Pressed));
        assert!(
            rx.try_recv().is_err(),
            "the repeat sent neither a release nor a press"
        );
    }

    #[test]
    fn a_release_that_waits_for_a_repeat_is_sent_by_the_timer() {
        let (tx, mut rx) = unbounded_channel();
        let gate = Arc::new(Mutex::new(Gate::new(MS(100), tx)));

        key_event(&gate, true, Instant::now());
        key_event(&gate, false, Instant::now());
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Pressed));
        assert!(rx.try_recv().is_err(), "the release has to wait");

        std::thread::sleep(MS(500));
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Released));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_repeat_calls_off_the_release_the_timer_would_send() {
        let (tx, mut rx) = unbounded_channel();
        let gate = Arc::new(Mutex::new(Gate::new(MS(100), tx)));

        key_event(&gate, true, Instant::now());
        key_event(&gate, false, Instant::now());
        key_event(&gate, true, Instant::now());

        std::thread::sleep(MS(500));
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Pressed));
        assert!(rx.try_recv().is_err(), "the key was never up");
    }

    // ---- which shortcuts Dictate takes

    #[test]
    fn shortcuts_with_a_modifier_or_a_function_key_are_taken() {
        for accel in [
            "Ctrl+Alt+Space",
            "ctrl+alt+space",
            "Super+Shift+D",
            "Alt+5",
            "Ctrl+ArrowUp",
            "F6",
            "F24",
            "Shift+F1",
        ] {
            assert!(parse_accelerator(accel).is_ok(), "{accel}");
        }
    }

    #[test]
    fn garbage_and_keys_that_would_swallow_typing_are_refused() {
        for accel in [
            "", "banana", "Ctrl+", "+D", "Ctrl+Alt", "Ctrl+D+E", "D", "Space", "Shift+", "5",
        ] {
            assert!(
                matches!(parse_accelerator(accel), Err(ChangeError::Invalid)),
                "{accel:?}"
            );
        }
    }

    /// The names of the keys of `ui/src/lib/accelerator.ts`.
    fn recorder_keys() -> Vec<String> {
        let mut keys: Vec<String> = ('A'..='Z').map(String::from).collect();
        keys.extend((0..=9).map(|digit| digit.to_string()));
        keys.extend((1..=24).map(|n| format!("F{n}")));
        keys.extend(
            [
                "Space",
                "Enter",
                "Tab",
                "Backspace",
                "Delete",
                "Insert",
                "Home",
                "End",
                "PageUp",
                "PageDown",
                "ArrowUp",
                "ArrowDown",
                "ArrowLeft",
                "ArrowRight",
            ]
            .map(String::from),
        );
        keys
    }

    #[test]
    fn every_key_the_recorder_can_produce_is_one_the_plugin_reads() {
        for key in recorder_keys() {
            let accel = format!("Super+Ctrl+Alt+Shift+{key}");
            assert!(parse_accelerator(&accel).is_ok(), "{accel}");
        }
    }

    // ---- what the portal is asked for

    #[test]
    fn the_portal_is_asked_in_the_words_of_the_shortcuts_specification() {
        let trigger = |accel: &str| portal_trigger(&shortcut(accel));

        assert_eq!(trigger("Ctrl+Alt+Space").as_deref(), Some("CTRL+ALT+space"));
        assert_eq!(trigger("Super+Shift+D").as_deref(), Some("SHIFT+LOGO+d"));
        assert_eq!(trigger("F6").as_deref(), Some("F6"));
        assert_eq!(trigger("Ctrl+5").as_deref(), Some("CTRL+5"));
        assert_eq!(trigger("Ctrl+Enter").as_deref(), Some("CTRL+Return"));
        assert_eq!(trigger("Alt+PageUp").as_deref(), Some("ALT+Page_Up"));
        assert_eq!(trigger("Alt+ArrowLeft").as_deref(), Some("ALT+Left"));
        assert_eq!(trigger("Ctrl+Backspace").as_deref(), Some("CTRL+BackSpace"));
        // The plugin reads this key and the recorder never makes it.
        assert_eq!(trigger("Ctrl+Numpad5"), None);
    }

    #[test]
    fn the_portal_knows_every_key_the_recorder_can_produce() {
        for key in recorder_keys() {
            let accel = format!("Ctrl+{key}");
            assert!(portal_trigger(&shortcut(&accel)).is_some(), "{accel}");
        }
    }

    // ---- the arguments of a launch

    #[test]
    fn toggle_is_found_among_the_arguments_of_a_launch() {
        assert!(wants_toggle(&["dictate", "--toggle"]));
        assert!(wants_toggle(&["/usr/bin/dictate", "--other", "--toggle"]));
        assert!(!wants_toggle(&["dictate"]));
        assert!(!wants_toggle(&[
            "dictate",
            "--toggled",
            "toggle",
            "-toggle"
        ]));
        assert!(!wants_toggle::<&str>(&[]));
    }

    // ---- changing the shortcut

    struct Changing {
        _dir: tempfile::TempDir,
        store: SettingsStore,
        registry: Arc<FakeRegistry>,
        hotkeys: Hotkeys,
    }

    /// The default shortcut registered, as after a good start.
    fn changing() -> Changing {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = SettingsStore::open(dir.path().join("settings.json"));
        let registry = Arc::new(FakeRegistry::default());
        let old = shortcut("Ctrl+Alt+Space");
        registry.register(old).unwrap();
        let status = HotkeyStatus {
            label: label("Ctrl+Alt+Space"),
            backend: Backend::Global,
            error: None,
        };
        let hotkeys = Hotkeys::new(Some(registry.clone()), status, Some(old));
        Changing {
            _dir: dir,
            store,
            registry,
            hotkeys,
        }
    }

    #[test]
    fn a_new_shortcut_is_registered_saved_and_shown_and_the_old_one_let_go() {
        let c = changing();

        c.hotkeys.change(&c.store, "Super+Shift+D").unwrap();

        assert_eq!(c.registry.registered(), vec![shortcut("Super+Shift+D")]);
        assert_eq!(c.store.get().hotkey, "Super+Shift+D");
        assert_eq!(
            c.hotkeys.status(),
            HotkeyStatus {
                label: label("Super+Shift+D"),
                backend: Backend::Global,
                error: None
            }
        );
    }

    #[test]
    fn a_shortcut_that_another_app_owns_is_taken_and_the_old_one_stays() {
        let c = changing();
        c.registry.refuse(shortcut("F6"));

        let err = c.hotkeys.change(&c.store, "F6").unwrap_err();

        assert!(matches!(err, ChangeError::Taken), "{err:?}");
        assert_eq!(c.registry.registered(), vec![shortcut("Ctrl+Alt+Space")]);
        assert_eq!(c.store.get().hotkey, "Ctrl+Alt+Space", "nothing was saved");
        assert_eq!(c.hotkeys.status().label, label("Ctrl+Alt+Space"));
    }

    #[test]
    fn garbage_changes_nothing() {
        let c = changing();

        for accel in ["banana", "D", ""] {
            let err = c.hotkeys.change(&c.store, accel).unwrap_err();
            assert!(matches!(err, ChangeError::Invalid), "{accel:?}: {err:?}");
        }

        assert_eq!(c.registry.registered(), vec![shortcut("Ctrl+Alt+Space")]);
        assert_eq!(c.registry.calls(), 1, "only the registration of the start");
        assert_eq!(c.store.get().hotkey, "Ctrl+Alt+Space");
    }

    #[test]
    fn a_shortcut_that_cannot_be_saved_is_given_back() {
        // The settings file cannot be written: its folder is a file.
        let dir = tempfile::tempdir().unwrap();
        let in_the_way = dir.path().join("in-the-way");
        std::fs::write(&in_the_way, b"x").unwrap();
        let (store, _) = SettingsStore::open(in_the_way.join("settings.json"));
        let c = changing();

        let err = c.hotkeys.change(&store, "F6").unwrap_err();

        assert!(matches!(err, ChangeError::Save(_)), "{err:?}");
        assert_eq!(
            c.registry.registered(),
            vec![shortcut("Ctrl+Alt+Space")],
            "the new one is let go and the old one never was"
        );
        assert_eq!(c.hotkeys.status().label, label("Ctrl+Alt+Space"));
    }

    #[test]
    fn the_same_shortcut_in_other_words_is_saved_as_written_and_registered_once() {
        let c = changing();

        c.hotkeys.change(&c.store, "ctrl+alt+space").unwrap();

        assert_eq!(c.registry.registered(), vec![shortcut("Ctrl+Alt+Space")]);
        assert_eq!(c.registry.calls(), 1, "no second registration");
        assert_eq!(c.store.get().hotkey, "ctrl+alt+space");
    }

    #[test]
    fn a_start_that_failed_is_mended_by_choosing_a_shortcut_that_is_free() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = SettingsStore::open(dir.path().join("settings.json"));
        let registry = Arc::new(FakeRegistry::default());
        let failed = HotkeyStatus {
            label: label("Ctrl+Alt+Space"),
            backend: Backend::Unavailable,
            error: Some(TAKEN.into()),
        };
        let hotkeys = Hotkeys::new(Some(registry.clone()), failed, None);

        hotkeys.change(&store, "F6").unwrap();

        assert_eq!(registry.registered(), vec![shortcut("F6")]);
        assert_eq!(
            hotkeys.status(),
            HotkeyStatus {
                label: "F6".into(),
                backend: Backend::Global,
                error: None
            }
        );
    }

    #[test]
    fn where_the_desktop_owns_the_shortcut_a_change_is_saved_and_nothing_is_registered() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = SettingsStore::open(dir.path().join("settings.json"));
        let status = HotkeyStatus {
            label: "Ctrl+Alt+Space".into(),
            backend: Backend::Portal,
            error: None,
        };
        let hotkeys = Hotkeys::new(None, status.clone(), None);

        hotkeys.change(&store, "Super+Shift+D").unwrap();
        assert_eq!(store.get().hotkey, "Super+Shift+D");
        assert_eq!(
            hotkeys.status(),
            status,
            "the desktop says what the shortcut is"
        );

        // Garbage is refused there too.
        assert!(matches!(
            hotkeys.change(&store, "banana"),
            Err(ChangeError::Invalid)
        ));
    }
}
