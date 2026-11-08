//! Writing dictated text into another app: the text goes on the clipboard, the app comes forward, the paste
//! shortcut is pressed, and the user's own clipboard is given back a moment later.
//!
//! This file has no operating system in it. [`Paster`] does the sequence and the waiting; what each system does
//! for it is behind two traits, [`Clipboard`] (the pasteboard, the Windows clipboard, the X11 selection) and
//! [`KeySender`] (the paste shortcut and the questions about who is in front), and every wait goes through a
//! [`Clock`], so that the tests run the whole sequence with a fake one.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::{AppId, FieldSnapshot};

/// How often the app that should take the paste is asked whether it is in front, and for how long.
const FORWARD_POLLS: u32 = 20;
const FORWARD_POLL: Duration = Duration::from_millis(25);
/// What an app that was brought forward is given to hand the focus back to its window.
const FORWARD_SETTLE: Duration = Duration::from_millis(150);
/// How often the keys are checked for the modifiers of the hotkey, and for how long.
const MODIFIER_POLLS: u32 = 40;
const MODIFIER_POLL: Duration = Duration::from_millis(25);
/// How long the pasted text stays on the clipboard before the user's own contents come back: apps read the
/// clipboard some time after the shortcut.
const RESTORE_AFTER: Duration = Duration::from_millis(800);

/// What a paste waits with.
pub trait Clock: Send + Sync {
    fn sleep(&self, duration: Duration);
}

/// The clock of the process: it sleeps.
pub struct SystemClock;

impl Clock for SystemClock {
    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

/// A number that changes whenever the clipboard does: the change count of the pasteboard, the sequence number of
/// the Windows clipboard, or on Linux the number of the last write that we made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeMark(pub u64);

/// One item of the macOS pasteboard: every type that it has, with its bytes.
pub type PasteboardItem = Vec<(String, Vec<u8>)>;

/// A picture on the clipboard, as 8-bit RGBA.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// What Windows and Linux can read from the clipboard and put back: text, HTML, a picture and a list of files.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Contents {
    pub text: Option<String>,
    pub html: Option<String>,
    pub image: Option<Image>,
    pub files: Vec<PathBuf>,
}

/// Everything on the clipboard at one moment, in the form that the system that read it can put back.
// Each system builds one kind: macOS the first, Windows and Linux the second.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum ClipSnapshot {
    /// macOS: every item of the pasteboard with all its types, as the Swift app kept them.
    Pasteboard(Vec<PasteboardItem>),
    Contents(Contents),
}

/// Why a paste did not go through.
// Each system builds the errors it can have: only Windows says `Elevated`, only Wayland `NoWaylandTool`.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteError {
    /// The app in front runs with more rights than we do, and Windows drops the keys that we send it.
    Elevated,
    /// Wayland has no way to press keys in another app, and neither `wtype` nor `ydotool` is there to do it.
    NoWaylandTool,
    Failed(String),
}

/// Why the text was only copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyReason {
    /// The app would not come forward (it quit, or the system refused): the shortcut would land in some other app.
    AppNotForward,
    Elevated,
    NoWaylandTool,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteResult {
    /// The shortcut was sent. Whether the field took the text is for [`landed`] to say.
    Pasted,
    /// The text is on the clipboard, for the user to paste; the user's own clipboard stays replaced.
    CopiedOnly(CopyReason),
}

/// The clipboard of the system.
pub trait Clipboard: Send + Sync {
    /// Everything that is on it now.
    fn snapshot(&self) -> ClipSnapshot;
    /// Puts `text` on it, marked so that clipboard managers don't keep it, and says what the clipboard is like
    /// now.
    fn write_transient(&self, text: &str) -> Result<ChangeMark, String>;
    /// Whether the clipboard is no longer what [`Clipboard::write_transient`] left: someone copied something.
    fn changed_since(&self, mark: &ChangeMark) -> bool;
    /// Puts back what [`Clipboard::snapshot`] took.
    fn restore(&self, snapshot: ClipSnapshot);
    /// Puts `text` on it as an ordinary copy.
    fn copy(&self, text: &str) -> Result<(), String>;
}

/// The keys and the foreground of the system. None of the methods waits: [`Paster`] polls.
pub trait KeySender: Send + Sync {
    /// Asks the app to come forward, if it isn't, and says whether it is in front now.
    fn bring_forward(&self, app: &AppId) -> bool;
    /// Whether none of Ctrl, Alt, Shift and Cmd/Win is held down: the hotkey of dictation has them, and a paste
    /// shortcut sent while they are held would arrive as another one.
    fn modifiers_released(&self) -> bool;
    /// Presses the paste shortcut: Ctrl+V, Cmd+V, or Ctrl+Shift+V for a terminal on Linux.
    fn send_paste(&self, terminal: bool) -> Result<(), PasteError>;
    /// When the last key was sent, so that whoever watches the keyboard can tell ours from the user's.
    fn last_post(&self) -> Option<Instant>;
}

/// The user's clipboard, kept while the dictated text is on it.
struct Held {
    snapshot: ClipSnapshot,
}

#[derive(Default)]
struct State {
    /// `Some` while a restore is waiting.
    held: Option<Held>,
    /// The number of the paste that the restore belongs to: a later paste takes the restore over.
    turn: u64,
}

fn locked(state: &Mutex<State>) -> MutexGuard<'_, State> {
    // A restore that panicked must not stop every later paste.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Pastes text into other apps and gives the clipboard back. One for the whole app: it remembers the clipboard
/// of the user while a restore is waiting.
pub struct Paster {
    clock: Arc<dyn Clock>,
    state: Arc<Mutex<State>>,
}

impl Paster {
    pub fn new(clock: Arc<dyn Clock>) -> Paster {
        Paster {
            clock,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Whether the user's clipboard is waiting to be given back.
    pub fn restore_pending(&self) -> bool {
        locked(&self.state).held.is_some()
    }

    /// Writes `text` into `target`: on the clipboard, `target` forward, the shortcut, and 800 ms later the
    /// clipboard of the user back, if nobody copied in between (on a thread of its own: this returns as soon as the
    /// shortcut is sent). When that can't be done, the text is left on the clipboard and the result says why.
    /// The caller has found out that `target` is not a password field.
    pub fn paste(
        &self,
        text: &str,
        target: &AppId,
        terminal: bool,
        clip: &Arc<dyn Clipboard>,
        keys: &dyn KeySender,
    ) -> PasteResult {
        // 1 and 2: keep the user's clipboard, unless a restore is waiting (what is on it then is the text of the
        // paste before, and the user's clipboard is the one that is kept), and put the text in its place.
        let (turn, mark) = {
            let mut state = locked(&self.state);
            let fresh = state.held.is_none().then(|| clip.snapshot());
            let mark = match clip.write_transient(text) {
                Ok(mark) => mark,
                Err(error) => {
                    // The write may have cleared the clipboard on its way to failing.
                    if let Some(snapshot) = fresh {
                        clip.restore(snapshot);
                    }
                    return PasteResult::CopiedOnly(CopyReason::Failed(error));
                }
            };
            if let Some(snapshot) = fresh {
                state.held = Some(Held { snapshot });
            }
            state.turn += 1;
            (state.turn, mark)
        };

        // 3: the app that is to take the paste.
        if !self.bring_forward(target, keys) {
            self.leave_on_clipboard(turn);
            return PasteResult::CopiedOnly(CopyReason::AppNotForward);
        }
        // 4: the modifiers of the hotkey. Held for longer than a second, they are the user's choice: paste anyway.
        self.wait_modifiers_released(keys);
        // 5
        if let Err(error) = keys.send_paste(terminal) {
            self.leave_on_clipboard(turn);
            return PasteResult::CopiedOnly(match error {
                PasteError::Elevated => CopyReason::Elevated,
                PasteError::NoWaylandTool => CopyReason::NoWaylandTool,
                PasteError::Failed(error) => CopyReason::Failed(error),
            });
        }

        // 6: the clipboard of the user comes back after a while, on a thread of its own.
        let state = self.state.clone();
        let clock = self.clock.clone();
        let board = clip.clone();
        let timer = thread::Builder::new()
            .name("clipboard-restore".into())
            .spawn(move || {
                clock.sleep(RESTORE_AFTER);
                give_back(&state, board.as_ref(), turn, mark);
            });
        if let Err(error) = timer {
            log::warn!("no thread to give the clipboard back: {error}");
            self.leave_on_clipboard(turn);
        }
        PasteResult::Pasted
    }

    /// Asks `target` to come forward and waits until it is: the app that is in front already takes the paste at
    /// once, one that had to be asked gets a moment to hand the focus to its window. False when it never comes.
    fn bring_forward(&self, target: &AppId, keys: &dyn KeySender) -> bool {
        if keys.bring_forward(target) {
            return true;
        }
        for _ in 0..FORWARD_POLLS {
            self.clock.sleep(FORWARD_POLL);
            if keys.bring_forward(target) {
                self.clock.sleep(FORWARD_SETTLE);
                return true;
            }
        }
        false
    }

    fn wait_modifiers_released(&self, keys: &dyn KeySender) {
        for _ in 0..MODIFIER_POLLS {
            if keys.modifiers_released() {
                return;
            }
            self.clock.sleep(MODIFIER_POLL);
        }
    }

    /// The text stays on the clipboard for the user to paste, and nothing is left to take it away: neither this
    /// paste's restore nor the one of an earlier paste. The user's clipboard from before is given up.
    fn leave_on_clipboard(&self, turn: u64) {
        let mut state = locked(&self.state);
        if state.turn == turn {
            state.held = None;
        }
    }
}

/// Puts the user's clipboard back, if the paste that holds it is the last one, and nobody has copied since the
/// text of that paste was written.
fn give_back(state: &Mutex<State>, clip: &dyn Clipboard, turn: u64, mark: ChangeMark) {
    let mut state = locked(state);
    if state.turn != turn {
        // A later paste has the user's clipboard now, and gives it back.
        return;
    }
    if let Some(held) = state.held.take() {
        if !clip.changed_since(&mark) {
            clip.restore(held.snapshot);
        }
    }
}

/// Whether the paste changed the field: any difference in the length of the text or in the selection. When the
/// field can't be read after the paste, it is not known that it didn't take the text.
pub fn landed(before: FieldSnapshot, after: Option<FieldSnapshot>) -> bool {
    after.is_none_or(|after| after != before)
}

// ---- What Linux and Windows decide on their own ---------------------------------------------------------------

/// What of a [`Contents`] goes back on the clipboard. arboard puts one kind of thing on the clipboard at a time
/// (HTML comes with its plain text, nothing else comes together), so the one that matters most is chosen.
#[cfg(any(test, target_os = "windows", target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restore {
    Files,
    /// The text, with its HTML when there is one.
    Text,
    Image,
    /// HTML that has no plain text.
    Html,
    /// There was nothing: the clipboard is cleared.
    Nothing,
}

#[cfg(any(test, target_os = "windows", target_os = "linux"))]
pub fn choose_restore(contents: &Contents) -> Restore {
    let has_text = contents.text.as_ref().is_some_and(|text| !text.is_empty());
    if !contents.files.is_empty() {
        Restore::Files
    } else if has_text {
        // Text with a picture of it (cells copied from a spreadsheet) is the text.
        Restore::Text
    } else if contents.image.is_some() {
        Restore::Image
    } else if contents.html.is_some() {
        Restore::Html
    } else {
        Restore::Nothing
    }
}

/// Whether `front`, the app that the system says is in front, is the app `wanted`: by the process when both are
/// known, else by the name.
#[cfg(any(test, target_os = "windows", target_os = "linux"))]
pub fn is_app(front: &AppId, wanted: &AppId) -> bool {
    match (front.pid, wanted.pid) {
        (Some(front), Some(wanted)) => front == wanted,
        _ => front.id.eq_ignore_ascii_case(&wanted.id),
    }
}

/// The tool that presses keys on Wayland, which has no way for an app to press keys in another one.
#[cfg(any(test, target_os = "linux"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaylandTool {
    /// `wtype`: wlroots compositors, Hyprland and Sway (not GNOME).
    Wtype(PathBuf),
    /// `ydotool`, with the socket of its daemon there: keys come in as a virtual device.
    Ydotool(PathBuf),
}

/// The file of `name` in the first directory of `path` that has one.
#[cfg(any(test, target_os = "linux"))]
pub fn find_on_path(
    name: &str,
    path: &std::ffi::OsStr,
    is_file: impl Fn(&std::path::Path) -> bool,
) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|file| is_file(file))
}

/// Where the daemon of `ydotool` listens: `$YDOTOOL_SOCKET`, else the runtime directory of the user.
#[cfg(any(test, target_os = "linux"))]
pub fn ydotool_sockets(
    env_socket: Option<&str>,
    runtime_dir: Option<&str>,
    uid: Option<u32>,
) -> Vec<PathBuf> {
    let mut places = Vec::new();
    if let Some(socket) = env_socket.filter(|socket| !socket.is_empty()) {
        places.push(PathBuf::from(socket));
    }
    if let Some(dir) = runtime_dir.filter(|dir| !dir.is_empty()) {
        places.push(PathBuf::from(dir).join(".ydotool_socket"));
    } else if let Some(uid) = uid {
        places.push(PathBuf::from(format!("/run/user/{uid}/.ydotool_socket")));
    }
    places
}

/// `wtype` when it is there, else `ydotool` when its daemon is listening.
#[cfg(any(test, target_os = "linux"))]
pub fn pick_wayland_tool(
    wtype: Option<PathBuf>,
    ydotool: Option<PathBuf>,
    ydotool_listening: bool,
) -> Option<WaylandTool> {
    match (wtype, ydotool) {
        (Some(wtype), _) => Some(WaylandTool::Wtype(wtype)),
        (None, Some(ydotool)) if ydotool_listening => Some(WaylandTool::Ydotool(ydotool)),
        _ => None,
    }
}

/// What the tool is told to press: Ctrl+V, or Ctrl+Shift+V for a terminal.
#[cfg(any(test, target_os = "linux"))]
pub fn wayland_args(tool: &WaylandTool, terminal: bool) -> Vec<&'static str> {
    match tool {
        WaylandTool::Wtype(_) if terminal => {
            vec![
                "-M", "ctrl", "-M", "shift", "v", "-m", "shift", "-m", "ctrl",
            ]
        }
        WaylandTool::Wtype(_) => vec!["-M", "ctrl", "v", "-m", "ctrl"],
        // Key codes of the kernel: 29 left Ctrl, 42 left Shift, 47 V; `:1` is down and `:0` is up.
        WaylandTool::Ydotool(_) if terminal => {
            vec!["key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"]
        }
        WaylandTool::Ydotool(_) => vec!["key", "29:1", "47:1", "47:0", "29:0"],
    }
}

/// Whether Windows drops the keys that we send to the app in front: it runs with more rights than we do, or it
/// can't be told (the system refuses to say anything about it).
#[cfg(any(test, target_os = "windows"))]
pub fn blocked_by_uipi(we_are_elevated: bool, app_is_elevated: Option<bool>) -> bool {
    !we_are_elevated && app_is_elevated != Some(false)
}

#[cfg(test)]
mod tests {
    use std::sync::Condvar;
    use std::thread::ThreadId;

    use super::*;

    type Log = Arc<Mutex<Vec<(Duration, String)>>>;

    /// A clock that doesn't wait: the thread that made it moves the time on by what it sleeps. A timer thread
    /// waits to be let go first, in the order in which the timers started, so that a test can look at the world
    /// while a restore is still pending.
    struct FakeClock {
        owner: ThreadId,
        state: Mutex<Time>,
        wake: Condvar,
    }

    struct Time {
        now: Duration,
        /// Timer threads that came to sleep, and how many of them are let go.
        arrived: usize,
        released: usize,
    }

    impl FakeClock {
        fn new(timers_wait: bool) -> Arc<FakeClock> {
            Arc::new(FakeClock {
                owner: thread::current().id(),
                state: Mutex::new(Time {
                    now: Duration::ZERO,
                    arrived: 0,
                    released: if timers_wait { 0 } else { usize::MAX },
                }),
                wake: Condvar::new(),
            })
        }

        fn now(&self) -> Duration {
            self.state.lock().unwrap().now
        }

        /// Lets the next timer that is waiting (and every one after it, when there are none yet) go.
        fn release_next(&self) {
            let mut state = self.state.lock().unwrap();
            state.released = state.released.saturating_add(1);
            self.wake.notify_all();
        }

        fn release(&self) {
            self.state.lock().unwrap().released = usize::MAX;
            self.wake.notify_all();
        }

        /// Waits until `count` timer threads are asleep.
        fn wait_for_timers(&self, count: usize) {
            let give_up = Instant::now() + Duration::from_secs(5);
            while self.state.lock().unwrap().arrived < count {
                assert!(Instant::now() < give_up, "the timer never started");
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl Clock for FakeClock {
        fn sleep(&self, duration: Duration) {
            let mut state = self.state.lock().unwrap();
            if thread::current().id() != self.owner {
                let mine = state.arrived;
                state.arrived += 1;
                while state.released <= mine {
                    state = self.wake.wait(state).unwrap();
                }
            }
            state.now += duration;
        }
    }

    /// A clipboard that holds one text, and counts its changes like the real ones.
    struct FakeClipboard {
        clock: Arc<FakeClock>,
        log: Log,
        state: Mutex<FakeBoard>,
        write_fails: Option<String>,
    }

    struct FakeBoard {
        text: String,
        changes: u64,
        snapshots: usize,
    }

    impl FakeClipboard {
        fn new(clock: &Arc<FakeClock>, log: &Log, text: &str) -> Arc<FakeClipboard> {
            Arc::new(FakeClipboard {
                clock: clock.clone(),
                log: log.clone(),
                state: Mutex::new(FakeBoard {
                    text: text.into(),
                    changes: 0,
                    snapshots: 0,
                }),
                write_fails: None,
            })
        }

        fn note(&self, what: &str) {
            self.log
                .lock()
                .unwrap()
                .push((self.clock.now(), what.into()));
        }

        fn text(&self) -> String {
            self.state.lock().unwrap().text.clone()
        }

        fn snapshots(&self) -> usize {
            self.state.lock().unwrap().snapshots
        }

        /// The user copies something.
        fn user_copies(&self, text: &str) {
            let mut board = self.state.lock().unwrap();
            board.text = text.into();
            board.changes += 1;
        }
    }

    impl Clipboard for FakeClipboard {
        fn snapshot(&self) -> ClipSnapshot {
            self.note("snapshot");
            let mut board = self.state.lock().unwrap();
            board.snapshots += 1;
            ClipSnapshot::Contents(Contents {
                text: Some(board.text.clone()),
                ..Contents::default()
            })
        }

        fn write_transient(&self, text: &str) -> Result<ChangeMark, String> {
            if let Some(error) = &self.write_fails {
                return Err(error.clone());
            }
            self.note(&format!("write {text}"));
            let mut board = self.state.lock().unwrap();
            board.text = text.into();
            board.changes += 1;
            Ok(ChangeMark(board.changes))
        }

        fn changed_since(&self, mark: &ChangeMark) -> bool {
            self.state.lock().unwrap().changes != mark.0
        }

        fn restore(&self, snapshot: ClipSnapshot) {
            let ClipSnapshot::Contents(contents) = snapshot else {
                panic!("the fake only takes what it gave");
            };
            self.note("restore");
            let mut board = self.state.lock().unwrap();
            board.text = contents.text.unwrap_or_default();
            board.changes += 1;
        }

        fn copy(&self, text: &str) -> Result<(), String> {
            self.user_copies(text);
            Ok(())
        }
    }

    struct FakeKeys {
        clock: Arc<FakeClock>,
        log: Log,
        /// Questions to `bring_forward` that get "no" before the first "yes"; `None`: always no.
        forward_after: Option<u32>,
        /// Questions to `modifiers_released` that get "no" before the first "yes"; `None`: always no.
        released_after: Option<u32>,
        paste: Result<(), PasteError>,
        asked: Mutex<(u32, u32)>,
    }

    impl FakeKeys {
        fn new(clock: &Arc<FakeClock>, log: &Log) -> FakeKeys {
            FakeKeys {
                clock: clock.clone(),
                log: log.clone(),
                forward_after: Some(0),
                released_after: Some(0),
                paste: Ok(()),
                asked: Mutex::new((0, 0)),
            }
        }

        fn note(&self, what: &str) {
            self.log
                .lock()
                .unwrap()
                .push((self.clock.now(), what.into()));
        }

        fn forward_questions(&self) -> u32 {
            self.asked.lock().unwrap().0
        }
    }

    impl KeySender for FakeKeys {
        fn bring_forward(&self, _: &AppId) -> bool {
            let mut asked = self.asked.lock().unwrap();
            asked.0 += 1;
            self.note("bring_forward");
            self.forward_after.is_some_and(|n| asked.0 > n)
        }

        fn modifiers_released(&self) -> bool {
            let mut asked = self.asked.lock().unwrap();
            asked.1 += 1;
            self.released_after.is_some_and(|n| asked.1 > n)
        }

        fn send_paste(&self, terminal: bool) -> Result<(), PasteError> {
            self.note(&format!("paste terminal={terminal}"));
            self.paste.clone()
        }

        fn last_post(&self) -> Option<Instant> {
            None
        }
    }

    struct Fixture {
        clock: Arc<FakeClock>,
        log: Log,
        clip: Arc<FakeClipboard>,
        paster: Paster,
    }

    impl Fixture {
        fn new(timers_wait: bool) -> Fixture {
            let clock = FakeClock::new(timers_wait);
            let log: Log = Arc::default();
            let clip = FakeClipboard::new(&clock, &log, "original");
            let paster = Paster::new(clock.clone());
            Fixture {
                clock,
                log,
                clip,
                paster,
            }
        }

        fn keys(&self) -> FakeKeys {
            FakeKeys::new(&self.clock, &self.log)
        }

        fn paste(&self, text: &str, keys: &FakeKeys) -> PasteResult {
            let clip: Arc<dyn Clipboard> = self.clip.clone();
            self.paster.paste(text, &target(), false, &clip, keys)
        }

        /// The events so far, with the time that the fake clock had when they happened.
        fn events(&self) -> Vec<(u64, String)> {
            self.log
                .lock()
                .unwrap()
                .iter()
                .map(|(at, what)| (at.as_millis() as u64, what.clone()))
                .collect()
        }

        /// Lets the timers run and waits for the restore to be over (given back, or dropped).
        fn finish_restore(&self) {
            self.clock.release();
            let give_up = Instant::now() + Duration::from_secs(5);
            while self.paster.restore_pending() {
                assert!(Instant::now() < give_up, "the restore never finished");
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn target() -> AppId {
        AppId {
            id: "com.apple.TextEdit".into(),
            pid: Some(4242),
        }
    }

    fn names(events: &[(u64, String)]) -> Vec<&str> {
        events.iter().map(|(_, what)| what.as_str()).collect()
    }

    #[test]
    fn paster_restores_after_800ms() {
        let f = Fixture::new(true);
        let keys = f.keys();

        let result = f.paste("hello", &keys);

        // The caller is free as soon as the shortcut is sent: the 800 ms are waited on another thread.
        assert_eq!(result, PasteResult::Pasted);
        assert_eq!(f.clip.text(), "hello");
        assert!(f.paster.restore_pending());
        assert_eq!(f.clock.now(), Duration::ZERO);

        f.finish_restore();

        assert_eq!(
            f.events(),
            vec![
                (0, "snapshot".to_string()),
                (0, "write hello".to_string()),
                (0, "bring_forward".to_string()),
                (0, "paste terminal=false".to_string()),
                (800, "restore".to_string()),
            ]
        );
        assert_eq!(f.clip.text(), "original");
    }

    #[test]
    fn paster_skips_restore_when_user_copied() {
        let f = Fixture::new(true);
        let keys = f.keys();
        assert_eq!(f.paste("hello", &keys), PasteResult::Pasted);

        f.clip.user_copies("something else");
        f.finish_restore();

        assert!(!names(&f.events()).contains(&"restore"));
        assert_eq!(f.clip.text(), "something else");
    }

    #[test]
    fn paster_back_to_back_keeps_first_snapshot() {
        let f = Fixture::new(true);
        let keys = f.keys();
        assert_eq!(f.paste("first", &keys), PasteResult::Pasted);
        f.clock.wait_for_timers(1);

        // The clipboard holds our "first" now. A second paste must not take that for the user's clipboard.
        assert_eq!(f.paste("second", &keys), PasteResult::Pasted);
        f.clock.wait_for_timers(2);
        assert_eq!(f.clip.snapshots(), 1);

        // The timer of the first paste goes off while the second text is still on the clipboard: it leaves
        // the user's clipboard to the second paste.
        f.clock.release_next();
        thread::sleep(Duration::from_millis(50));
        assert!(f.paster.restore_pending());
        assert!(!names(&f.events()).contains(&"restore"));

        f.finish_restore();

        let restores = names(&f.events())
            .iter()
            .filter(|what| **what == "restore")
            .count();
        assert_eq!(restores, 1);
        assert_eq!(f.clip.text(), "original");
    }

    #[test]
    fn paster_app_not_forward_copies_only() {
        let f = Fixture::new(true);
        let mut keys = f.keys();
        keys.forward_after = None;

        let result = f.paste("hello", &keys);

        assert_eq!(result, PasteResult::CopiedOnly(CopyReason::AppNotForward));
        // One question, then 20 more 25 ms apart: no shortcut into whatever else is in front after that.
        assert_eq!(keys.forward_questions(), 21);
        assert_eq!(f.clock.now(), Duration::from_millis(500));
        assert!(!names(&f.events()).iter().any(|w| w.starts_with("paste")));
        // The text stays on the clipboard for the user, and nothing is waiting to take it away.
        assert!(!f.paster.restore_pending());
        f.clock.release();
        assert_eq!(f.clip.text(), "hello");
    }

    #[test]
    fn paster_elevated_copies_only() {
        let f = Fixture::new(true);
        let mut keys = f.keys();
        keys.paste = Err(PasteError::Elevated);

        let result = f.paste("hello", &keys);

        assert_eq!(result, PasteResult::CopiedOnly(CopyReason::Elevated));
        assert!(!f.paster.restore_pending());
        f.clock.release();
        assert_eq!(f.clip.text(), "hello");
    }

    #[test]
    fn paster_waits_for_modifiers() {
        let f = Fixture::new(true);
        let mut keys = f.keys();
        keys.released_after = Some(3);

        assert_eq!(f.paste("hello", &keys), PasteResult::Pasted);

        // Three polls found a key held, 25 ms apart; the shortcut goes out on the fourth, which found none.
        let events = f.events();
        let sent = events.iter().find(|(_, w)| w.starts_with("paste")).unwrap();
        assert_eq!(sent.0, 75);
        f.finish_restore();
    }

    #[test]
    fn paster_pastes_after_a_second_of_held_modifiers() {
        let f = Fixture::new(true);
        let mut keys = f.keys();
        keys.released_after = None;

        assert_eq!(f.paste("hello", &keys), PasteResult::Pasted);

        let events = f.events();
        let sent = events.iter().find(|(_, w)| w.starts_with("paste")).unwrap();
        assert_eq!(sent.0, 1000);
        f.finish_restore();
    }

    #[test]
    fn paster_gives_an_app_that_was_asked_150ms_to_take_the_focus() {
        let f = Fixture::new(true);
        let mut keys = f.keys();
        keys.forward_after = Some(3);

        assert_eq!(f.paste("hello", &keys), PasteResult::Pasted);

        // Three polls found another app in front (75 ms), then 150 ms for the window to take the focus.
        let events = f.events();
        let sent = events.iter().find(|(_, w)| w.starts_with("paste")).unwrap();
        assert_eq!(sent.0, 225);
        f.finish_restore();
    }

    #[test]
    fn paster_passes_on_that_the_app_is_a_terminal() {
        let f = Fixture::new(true);
        let keys = f.keys();
        let clip: Arc<dyn Clipboard> = f.clip.clone();

        f.paster.paste("ls", &target(), true, &clip, &keys);

        assert!(names(&f.events()).contains(&"paste terminal=true"));
        f.finish_restore();
    }

    #[test]
    fn paster_reports_why_the_keys_could_not_be_sent() {
        for (error, reason) in [
            (PasteError::NoWaylandTool, CopyReason::NoWaylandTool),
            (
                PasteError::Failed("no display".into()),
                CopyReason::Failed("no display".into()),
            ),
        ] {
            let f = Fixture::new(true);
            let mut keys = f.keys();
            keys.paste = Err(error);
            assert_eq!(f.paste("hello", &keys), PasteResult::CopiedOnly(reason));
            assert!(!f.paster.restore_pending());
        }
    }

    #[test]
    fn paster_that_cannot_write_the_text_keeps_nothing_and_sends_nothing() {
        let mut f = Fixture::new(true);
        Arc::get_mut(&mut f.clip).unwrap().write_fails = Some("clipboard busy".into());
        let keys = f.keys();

        let result = f.paste("hello", &keys);

        assert_eq!(
            result,
            PasteResult::CopiedOnly(CopyReason::Failed("clipboard busy".into()))
        );
        assert!(!f.paster.restore_pending());
        assert!(!names(&f.events()).iter().any(|w| w.starts_with("paste")));
    }

    #[test]
    fn paster_that_cannot_write_the_text_gives_the_users_clipboard_back() {
        // A write that fails may have cleared the clipboard on the way.
        let mut f = Fixture::new(true);
        Arc::get_mut(&mut f.clip).unwrap().write_fails = Some("clipboard busy".into());
        let keys = f.keys();

        f.paste("hello", &keys);

        assert_eq!(names(&f.events()), ["snapshot", "restore"]);
        assert_eq!(f.clip.text(), "original");
    }

    #[test]
    fn paster_failing_a_paste_cancels_the_restore_of_the_paste_before() {
        let f = Fixture::new(true);
        let keys = f.keys();
        assert_eq!(f.paste("first", &keys), PasteResult::Pasted);
        let mut failing = f.keys();
        failing.paste = Err(PasteError::Elevated);

        // The second text is the one that the user is to paste by hand: the first restore must not take it away.
        assert_eq!(
            f.paste("second", &failing),
            PasteResult::CopiedOnly(CopyReason::Elevated)
        );
        f.clock.release();
        // Give the timer of the first paste time to run (it ends without a trace, so there is nothing to wait for).
        thread::sleep(Duration::from_millis(100));

        assert!(!names(&f.events()).contains(&"restore"));
        assert_eq!(f.clip.text(), "second");
    }

    #[test]
    fn paster_takes_a_new_snapshot_once_the_restore_is_over() {
        let f = Fixture::new(true);
        let keys = f.keys();
        assert_eq!(f.paste("first", &keys), PasteResult::Pasted);
        f.finish_restore();
        f.clip.user_copies("copied later");

        assert_eq!(f.paste("second", &keys), PasteResult::Pasted);
        f.finish_restore();

        assert_eq!(f.clip.snapshots(), 2);
        assert_eq!(f.clip.text(), "copied later");
    }

    #[test]
    fn landed_detects_change() {
        let before = FieldSnapshot {
            len: 10,
            sel_loc: 4,
            sel_len: 0,
        };
        let longer = FieldSnapshot { len: 15, ..before };
        let moved = FieldSnapshot {
            sel_loc: 9,
            ..before
        };
        let selected = FieldSnapshot {
            sel_len: 3,
            ..before
        };
        assert!(landed(before, Some(longer)));
        assert!(landed(before, Some(moved)));
        assert!(landed(before, Some(selected)));
        assert!(!landed(before, Some(before)));
    }

    #[test]
    fn landed_unknown_counts_as_landed() {
        let before = FieldSnapshot {
            len: 10,
            sel_loc: 4,
            sel_len: 0,
        };
        assert!(landed(before, None));
    }

    #[test]
    fn wtype_is_the_first_choice() {
        let wtype = PathBuf::from("/usr/bin/wtype");
        let ydotool = PathBuf::from("/usr/bin/ydotool");
        assert_eq!(
            pick_wayland_tool(Some(wtype.clone()), Some(ydotool), true),
            Some(WaylandTool::Wtype(wtype))
        );
    }

    #[test]
    fn ydotool_is_used_only_when_its_daemon_listens() {
        let ydotool = PathBuf::from("/usr/bin/ydotool");
        assert_eq!(
            pick_wayland_tool(None, Some(ydotool.clone()), true),
            Some(WaylandTool::Ydotool(ydotool.clone()))
        );
        assert_eq!(pick_wayland_tool(None, Some(ydotool), false), None);
    }

    #[test]
    fn no_tool_on_the_path_means_no_paste_on_wayland() {
        assert_eq!(pick_wayland_tool(None, None, true), None);
    }

    #[test]
    fn wtype_presses_ctrl_v_and_ctrl_shift_v_in_a_terminal() {
        let tool = WaylandTool::Wtype(PathBuf::from("wtype"));
        assert_eq!(
            wayland_args(&tool, false),
            ["-M", "ctrl", "v", "-m", "ctrl"]
        );
        assert_eq!(
            wayland_args(&tool, true),
            ["-M", "ctrl", "-M", "shift", "v", "-m", "shift", "-m", "ctrl"]
        );
    }

    #[test]
    fn ydotool_presses_the_key_codes_of_ctrl_v() {
        let tool = WaylandTool::Ydotool(PathBuf::from("ydotool"));
        // 29 is the left Ctrl, 42 the left Shift, 47 the V.
        assert_eq!(
            wayland_args(&tool, false),
            ["key", "29:1", "47:1", "47:0", "29:0"]
        );
        assert_eq!(
            wayland_args(&tool, true),
            ["key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"]
        );
    }

    #[test]
    fn the_ydotool_daemon_is_looked_for_where_it_says_and_where_it_lives() {
        assert_eq!(
            ydotool_sockets(Some("/tmp/mine.sock"), Some("/run/user/1000"), Some(1000)),
            [
                PathBuf::from("/tmp/mine.sock"),
                PathBuf::from("/run/user/1000/.ydotool_socket"),
            ]
        );
        assert_eq!(
            ydotool_sockets(None, None, Some(501)),
            [PathBuf::from("/run/user/501/.ydotool_socket")]
        );
        // An empty variable is no variable.
        assert_eq!(
            ydotool_sockets(Some(""), Some(""), None),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn a_tool_is_found_in_the_first_directory_that_has_it() {
        let path = std::ffi::OsString::from("/opt/bin:/usr/bin:/bin");
        let has = |p: &std::path::Path| p == std::path::Path::new("/usr/bin/wtype");
        assert_eq!(
            find_on_path("wtype", &path, has),
            Some(PathBuf::from("/usr/bin/wtype"))
        );
        assert_eq!(find_on_path("ydotool", &path, has), None);
        assert_eq!(find_on_path("wtype", std::ffi::OsStr::new(""), has), None);
    }

    #[test]
    fn windows_drops_keys_for_an_app_with_more_rights() {
        // Not elevated ourselves: a normal app takes the keys, an elevated one does not, and one that
        // can't be asked is treated like an elevated one.
        assert!(!blocked_by_uipi(false, Some(false)));
        assert!(blocked_by_uipi(false, Some(true)));
        assert!(blocked_by_uipi(false, None));
        // Elevated ourselves: every app takes them.
        assert!(!blocked_by_uipi(true, Some(true)));
        assert!(!blocked_by_uipi(true, None));
    }

    fn contents() -> Contents {
        Contents::default()
    }

    fn picture() -> Image {
        Image {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        }
    }

    #[test]
    fn files_come_back_as_files() {
        let files = Contents {
            files: vec![PathBuf::from("/tmp/a.txt")],
            text: Some("a.txt".into()),
            ..contents()
        };
        assert_eq!(choose_restore(&files), Restore::Files);
    }

    #[test]
    fn plain_text_comes_back_as_text() {
        let text = Contents {
            text: Some("hello".into()),
            ..contents()
        };
        assert_eq!(choose_restore(&text), Restore::Text);
    }

    #[test]
    fn a_screenshot_comes_back_as_an_image() {
        let screenshot = Contents {
            image: Some(picture()),
            ..contents()
        };
        assert_eq!(choose_restore(&screenshot), Restore::Image);
    }

    #[test]
    fn copied_cells_come_back_as_text_not_as_the_picture_of_them() {
        let cells = Contents {
            text: Some("1\t2".into()),
            html: Some("<table></table>".into()),
            image: Some(picture()),
            ..contents()
        };
        assert_eq!(choose_restore(&cells), Restore::Text);
    }

    #[test]
    fn a_copied_picture_with_only_its_html_comes_back_as_the_image() {
        let copied = Contents {
            html: Some("<img src=\"a.png\">".into()),
            image: Some(picture()),
            ..contents()
        };
        assert_eq!(choose_restore(&copied), Restore::Image);
    }

    #[test]
    fn html_alone_and_nothing_at_all() {
        let html = Contents {
            html: Some("<b>x</b>".into()),
            ..contents()
        };
        assert_eq!(choose_restore(&html), Restore::Html);
        assert_eq!(choose_restore(&contents()), Restore::Nothing);
        // An empty text is no text.
        let empty = Contents {
            text: Some(String::new()),
            ..contents()
        };
        assert_eq!(choose_restore(&empty), Restore::Nothing);
    }

    #[test]
    fn the_app_in_front_is_told_by_its_process_else_by_its_name() {
        let app = |id: &str, pid: Option<u32>| AppId { id: id.into(), pid };
        assert!(is_app(
            &app("chrome.exe", Some(7)),
            &app("Chrome.EXE", Some(7))
        ));
        // The same process under another name is the same app, and another process under the same name is not.
        assert!(is_app(&app("a", Some(7)), &app("b", Some(7))));
        assert!(!is_app(
            &app("chrome.exe", Some(7)),
            &app("chrome.exe", Some(8))
        ));
        // With no process on one side, the name decides.
        assert!(is_app(&app("kitty", None), &app("Kitty", Some(3))));
        assert!(!is_app(&app("kitty", Some(3)), &app("xterm", None)));
    }
}
