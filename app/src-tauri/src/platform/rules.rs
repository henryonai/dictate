//! The decisions of the platform layer that need no operating system: which apps are terminals, which focused
//! elements take text, which caret rectangles to believe, which session a Linux desktop runs and which screen
//! a point is on. The OS files read the facts and ask these functions what they mean, so that the rules are
//! tested on every system. A helper that only one system uses is compiled for that system and for the tests,
//! which run on every system: it is tested everywhere and is dead code nowhere.

use super::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    Linux,
}

impl Os {
    /// The system this build runs on.
    pub const fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

// Terminals by the id the OS file reads for an app. Ids are compared without regard to case: bundle ids and file
// names have none, and the window class of one X11 terminal is written both "xterm" and "XTerm".
const MAC_TERMINALS: &[&str] = &[
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "dev.warp.Warp-Stable",
    "net.kovidgoyal.kitty",
    "org.alacritty",
    "io.alacritty",
    "com.mitchellh.ghostty",
    "com.github.wez.wezterm",
    "co.zeit.hyper",
];
const WINDOWS_TERMINALS: &[&str] = &[
    "WindowsTerminal.exe",
    "conhost.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "wezterm-gui.exe",
    "alacritty.exe",
    "mintty.exe",
];
const LINUX_TERMINALS: &[&str] = &[
    "gnome-terminal",
    "konsole",
    "kitty",
    "Alacritty",
    "foot",
    "wezterm",
    "xterm",
    "tilix",
    "ghostty",
    // The classes that WezTerm and Ghostty announce on X11 are their reverse-DNS app ids.
    "org.wezfurlong.wezterm",
    "com.mitchellh.ghostty",
];

/// Whether the app is a terminal. Its prompt is not a text element that can be read, so it always counts as a
/// field, takes no landing check and (outside macOS) takes Ctrl+Shift+V.
pub fn is_terminal(os: Os, app_id: &str) -> bool {
    let list = match os {
        Os::Mac => MAC_TERMINALS,
        Os::Windows => WINDOWS_TERMINALS,
        Os::Linux => LINUX_TERMINALS,
    };
    list.iter().any(|t| t.eq_ignore_ascii_case(app_id))
}

/// Whether the focused element takes text.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editable {
    Yes,
    No,
    /// A password field: never written into.
    Secure,
}

/// The AX subrole of a password field.
#[cfg(any(test, target_os = "macos"))]
const AX_SECURE_FIELD: &str = "AXSecureTextField";
/// The AX roles of the elements that take text.
#[cfg(any(test, target_os = "macos"))]
const AX_TEXT_ROLES: &[&str] = &["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"];

/// Whether an element of the macOS Accessibility API takes text. A password field never does, whatever its role.
/// A text role does. Any other element (Chromium and Electron text, a custom editor) does when its value can be
/// set and a selection says there is a caret.
#[cfg(any(test, target_os = "macos"))]
pub fn classify_ax(
    role: &str,
    subrole: &str,
    value_settable: bool,
    has_selection: bool,
) -> Editable {
    if subrole == AX_SECURE_FIELD {
        Editable::Secure
    } else if AX_TEXT_ROLES.contains(&role) || (value_settable && has_selection) {
        Editable::Yes
    } else {
        Editable::No
    }
}

/// Whether the element that an app says has the focus is the app itself, which Electron apps answer while they have
/// no accessibility tree to look into: the app doesn't say where the focus is, so there is nothing to classify.
#[cfg(any(test, target_os = "macos"))]
pub fn ax_focus_is_the_app(role: &str) -> bool {
    role == "AXApplication"
}

/// How the read of the fact "this is a password field" (the AX subrole, the UIA `IsPassword` property) went. Each
/// system builds the variants that its own read can give.
#[allow(dead_code)]
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureRead {
    /// The system answered.
    Answered,
    /// The element has no such fact (AX says no value or no such attribute): it is not marked as a password field.
    Unsupported,
    /// The read failed for another reason: whether this is a password field can't be known.
    Failed,
}

/// What to do after reading the password fact.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureOutcome {
    /// Go on and ask the other rules.
    Classify,
    /// Stop: there is no field to write into.
    NoField,
}

/// The rule that keeps a password field from ever being written into: a read that failed is not "no", it is "don't
/// know", and text is never written when it isn't known that the field is no password field.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub fn secure_read_outcome(read: SecureRead) -> SecureOutcome {
    match read {
        SecureRead::Answered | SecureRead::Unsupported => SecureOutcome::Classify,
        SecureRead::Failed => SecureOutcome::NoField,
    }
}

/// The length of the text of a field that was read up to `cap` units: the real length when the read ended before
/// the cap, nothing when it may have been cut off. A long text isn't read whole to tell whether a paste landed.
#[cfg(any(test, target_os = "windows"))]
pub fn bounded_text_len(len: usize, cap: usize) -> Option<usize> {
    (len < cap).then_some(len)
}

/// UI Automation control type ids (`UIA_EditControlTypeId` and `UIA_DocumentControlTypeId`).
#[cfg(any(test, target_os = "windows"))]
pub const UIA_EDIT: i32 = 50004;
#[cfg(any(test, target_os = "windows"))]
pub const UIA_DOCUMENT: i32 = 50030;

/// Whether an element of Windows UI Automation takes text. `has_value` and `has_text` say whether it offers the
/// value and the text pattern, and `read_only` is what the value pattern says (false without one). A password
/// field never takes text, a read-only one neither. An edit control does, a document when it has text to read,
/// and any other control when its value can be written and it has text.
#[cfg(any(test, target_os = "windows"))]
pub fn classify_uia(
    control_type: i32,
    is_password: bool,
    has_value: bool,
    read_only: bool,
    has_text: bool,
) -> Editable {
    if is_password {
        return Editable::Secure;
    }
    let takes_text = match control_type {
        _ if read_only => false,
        UIA_EDIT => true,
        UIA_DOCUMENT => has_text,
        _ => has_value && has_text,
    };
    if takes_text {
        Editable::Yes
    } else {
        Editable::No
    }
}

/// Whether a rectangle of a caret is believable: a line of text is neither a dot nor a whole page.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub fn accept_caret(r: &Rect) -> bool {
    (2.0..=120.0).contains(&r.h)
}

#[cfg(any(test, target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    MacOS,
    Windows,
    X11,
    Wayland,
}

/// The session this process runs in, read through `env` (the process environment, or a fake of it).
#[cfg(any(test, target_os = "linux"))]
pub fn session(env: &dyn Fn(&str) -> Option<String>) -> Session {
    session_for(Os::current(), env)
}

/// The session on `os`. On Linux it is Wayland when `XDG_SESSION_TYPE` says so or a compositor socket is set
/// (`WAYLAND_DISPLAY`), else X11. A variable that is set to nothing counts as not set.
#[cfg(any(test, target_os = "linux"))]
pub fn session_for(os: Os, env: &dyn Fn(&str) -> Option<String>) -> Session {
    match os {
        Os::Mac => Session::MacOS,
        Os::Windows => Session::Windows,
        Os::Linux => {
            let set = |name: &str| env(name).filter(|v| !v.is_empty());
            let by_type =
                set("XDG_SESSION_TYPE").is_some_and(|v| v.eq_ignore_ascii_case("wayland"));
            if by_type || set("WAYLAND_DISPLAY").is_some() {
                Session::Wayland
            } else {
                Session::X11
            }
        }
    }
}

/// One screen, with the part of it that no menu bar or taskbar covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub frame: Rect,
    pub visible: Rect,
}

/// A rectangle given from the bottom left of the primary screen (as AppKit does), from the top left.
#[cfg(any(test, target_os = "macos"))]
pub fn flip_y(r: Rect, primary_height: f64) -> Rect {
    Rect {
        y: primary_height - r.y - r.h,
        ..r
    }
}

/// The visible frame of the screen the point is on, of the nearest one when it is on none, and of the first
/// screen (the main one) when there is no point. `None` when there are no screens.
pub fn pick_visible_frame(screens: &[Screen], near: Option<(f64, f64)>) -> Option<Rect> {
    let Some((x, y)) = near else {
        return screens.first().map(|s| s.visible);
    };
    // The right and bottom edges are outside a screen: the point on the line between two screens is on the next.
    let on = screens.iter().find(|s| {
        let f = &s.frame;
        x >= f.x && x < f.x + f.w && y >= f.y && y < f.y + f.h
    });
    let nearest = || {
        screens
            .iter()
            .min_by(|a, b| distance_to(&a.frame, x, y).total_cmp(&distance_to(&b.frame, x, y)))
    };
    on.or_else(nearest).map(|s| s.visible)
}

/// How far the point is from the rectangle, 0 when it touches it.
fn distance_to(r: &Rect, x: f64, y: f64) -> f64 {
    let dx = (r.x - x).max(x - (r.x + r.w)).max(0.0);
    let dy = (r.y - y).max(y - (r.y + r.h)).max(0.0);
    dx.hypot(dy)
}

/// Whether any part of the rectangle is on a screen: a caret that scrolled out of sight has no place to show.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub fn on_any_screen(screens: &[Screen], r: &Rect) -> bool {
    screens.iter().any(|s| {
        r.x < s.frame.x + s.frame.w
            && r.x + r.w > s.frame.x
            && r.y < s.frame.y + s.frame.h
            && r.y + r.h > s.frame.y
    })
}

/// Whether the macOS app is a browser built on Chromium, which builds its accessibility tree for a client only
/// when told that one is there.
#[cfg(any(test, target_os = "macos"))]
pub fn is_chromium_browser(bundle_id: &str) -> bool {
    const CHROMIUM_BROWSERS: &[&str] = &[
        "com.google.Chrome",
        "com.google.Chrome.beta",
        "com.brave.Browser",
        "com.microsoft.edgemac",
        "company.thebrowser.Browser",
        "com.vivaldi.Vivaldi",
        "com.operasoftware.Opera",
        "org.chromium.Chromium",
    ];
    CHROMIUM_BROWSERS
        .iter()
        .any(|b| b.eq_ignore_ascii_case(bundle_id))
}

/// The anchor of a terminal, which has no caret to ask for: near the bottom left of its window, where the prompt
/// usually is.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub fn terminal_anchor(window: &Rect) -> Rect {
    Rect {
        x: window.x + 24.0,
        y: window.y + window.h - 56.0,
        w: 1.0,
        h: 20.0,
    }
}

/// The field itself as the anchor, when it is small enough (a chat box) for that to say where the caret is.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub fn field_anchor(field: &Rect) -> Option<Rect> {
    (field.h > 0.0 && field.h <= 160.0).then_some(Rect {
        w: field.w.max(1.0),
        ..*field
    })
}

/// The caret as the right edge of the character before it, for the apps that give no rectangle for an empty
/// range.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub fn caret_after(character: &Rect) -> Rect {
    Rect {
        x: character.x + character.w,
        w: 1.0,
        ..*character
    }
}

/// The first rectangle of what UI Automation gives for a text range: groups of four numbers, left, top, width
/// and height, one for each line the range covers.
#[cfg(any(test, target_os = "windows"))]
pub fn first_rect(values: &[f64]) -> Option<Rect> {
    match values.first_chunk::<4>()? {
        &[x, y, w, h] if [x, y, w, h].iter().all(|v| v.is_finite()) => Some(Rect { x, y, w, h }),
        _ => None,
    }
}

/// A rectangle in physical pixels as logical points on a screen that scales by `scale`.
#[cfg(any(test, target_os = "windows", target_os = "linux"))]
pub fn to_logical(r: Rect, scale: f64) -> Rect {
    if !(scale.is_finite() && scale > 0.0) {
        return r;
    }
    Rect {
        x: r.x / scale,
        y: r.y / scale,
        w: r.w / scale,
        h: r.h / scale,
    }
}

/// The window class of an X11 window from its `WM_CLASS` property: the instance name and the class name, each
/// ended by a 0 byte. One name alone is both.
#[cfg(any(test, target_os = "linux"))]
pub fn parse_wm_class(raw: &[u8]) -> Option<(String, String)> {
    let mut names = raw
        .split(|byte| *byte == 0)
        .map(|name| String::from_utf8_lossy(name).into_owned());
    let instance = names.next().unwrap_or_default();
    let class = names.next().filter(|class| !class.is_empty());
    match (instance.is_empty(), class) {
        (true, None) => None,
        (_, Some(class)) => Some((instance, class)),
        (false, None) => Some((instance.clone(), instance)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn terminals_per_os() {
        for id in [
            "com.apple.Terminal",
            "com.googlecode.iterm2",
            "dev.warp.Warp-Stable",
            "net.kovidgoyal.kitty",
            "org.alacritty",
            "io.alacritty",
            "com.mitchellh.ghostty",
            "com.github.wez.wezterm",
            "co.zeit.hyper",
        ] {
            assert!(is_terminal(Os::Mac, id), "{id} on macOS");
        }
        for id in [
            "WindowsTerminal.exe",
            "conhost.exe",
            "cmd.exe",
            "powershell.exe",
            "pwsh.exe",
            "wezterm-gui.exe",
            "alacritty.exe",
            "mintty.exe",
        ] {
            assert!(is_terminal(Os::Windows, id), "{id} on Windows");
        }
        for id in [
            "gnome-terminal",
            "konsole",
            "kitty",
            "Alacritty",
            "foot",
            "wezterm",
            "xterm",
            "tilix",
            "ghostty",
        ] {
            assert!(is_terminal(Os::Linux, id), "{id} on Linux");
        }
        // A terminal on one system is not one on another, and a browser is none anywhere.
        assert!(!is_terminal(Os::Mac, "WindowsTerminal.exe"));
        assert!(!is_terminal(Os::Windows, "com.apple.Terminal"));
        assert!(!is_terminal(Os::Linux, "cmd.exe"));
        for os in [Os::Mac, Os::Windows, Os::Linux] {
            assert!(!is_terminal(os, "com.google.Chrome"));
            assert!(!is_terminal(os, "chrome.exe"));
            assert!(!is_terminal(os, ""));
        }
    }

    #[test]
    fn windows_and_linux_names_ignore_case() {
        // File names on Windows have no case, and X11 classes are written both ways ("XTerm" and "xterm").
        assert!(is_terminal(Os::Windows, "windowsterminal.EXE"));
        assert!(is_terminal(Os::Linux, "XTerm"));
        assert!(is_terminal(Os::Linux, "Gnome-terminal"));
    }

    #[test]
    fn secure_fields_are_never_editable() {
        for role in [
            "AXTextField",
            "AXTextArea",
            "AXComboBox",
            "AXSearchField",
            "AXGroup",
            "AXWebArea",
            "",
        ] {
            for settable in [false, true] {
                for selection in [false, true] {
                    assert_eq!(
                        classify_ax(role, "AXSecureTextField", settable, selection),
                        Editable::Secure,
                        "{role} settable={settable} selection={selection}"
                    );
                }
            }
        }
        // The password flag wins over everything else UI Automation says about the control.
        for control_type in [UIA_EDIT, UIA_DOCUMENT, 50003, 50000] {
            for (has_value, read_only, has_text) in [
                (true, false, true),
                (false, false, false),
                (true, true, true),
            ] {
                assert_eq!(
                    classify_uia(control_type, true, has_value, read_only, has_text),
                    Editable::Secure,
                    "type {control_type}"
                );
            }
        }
    }

    #[test]
    fn ax_roles_editable() {
        for role in ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"] {
            // A text role takes text even when the app doesn't say it can set the value or has no selection.
            assert_eq!(classify_ax(role, "", false, false), Editable::Yes, "{role}");
            assert_eq!(
                classify_ax(role, "AXSearchField", false, false),
                Editable::Yes
            );
        }
        // Other roles (Chromium and Electron text, custom editors) count when the value can be set and there is a
        // caret to move.
        assert_eq!(classify_ax("AXGroup", "", true, true), Editable::Yes);
        assert_eq!(classify_ax("AXWebArea", "", true, true), Editable::Yes);
        assert_eq!(classify_ax("AXGroup", "", true, false), Editable::No);
        assert_eq!(classify_ax("AXGroup", "", false, true), Editable::No);
        assert_eq!(classify_ax("AXButton", "", false, false), Editable::No);
        assert_eq!(classify_ax("", "", false, false), Editable::No);
    }

    #[test]
    fn uia_edit_and_document_editable() {
        assert_eq!(
            classify_uia(UIA_EDIT, false, true, false, true),
            Editable::Yes
        );
        // An edit control that offers neither pattern is still an edit control.
        assert_eq!(
            classify_uia(UIA_EDIT, false, false, false, false),
            Editable::Yes
        );
        assert_eq!(
            classify_uia(UIA_DOCUMENT, false, false, false, true),
            Editable::Yes
        );
        // A document with no text to read (a viewer) takes nothing.
        assert_eq!(
            classify_uia(UIA_DOCUMENT, false, false, false, false),
            Editable::No
        );
        // Any other control counts when its value can be written and it has text.
        assert_eq!(classify_uia(50003, false, true, false, true), Editable::Yes);
        assert_eq!(classify_uia(50003, false, true, false, false), Editable::No);
        assert_eq!(
            classify_uia(50000, false, false, false, false),
            Editable::No
        );
    }

    #[test]
    fn read_only_controls_are_not_editable() {
        assert_eq!(
            classify_uia(UIA_EDIT, false, true, true, true),
            Editable::No
        );
        assert_eq!(
            classify_uia(UIA_DOCUMENT, false, true, true, true),
            Editable::No
        );
        assert_eq!(classify_uia(50003, false, true, true, true), Editable::No);
    }

    #[test]
    fn caret_height_bounds() {
        assert!(!accept_caret(&rect(10.0, 10.0, 1.0, 1.9)));
        assert!(accept_caret(&rect(10.0, 10.0, 1.0, 2.0)));
        assert!(accept_caret(&rect(10.0, 10.0, 1.0, 18.0)));
        assert!(accept_caret(&rect(10.0, 10.0, 1.0, 120.0)));
        assert!(!accept_caret(&rect(10.0, 10.0, 1.0, 120.1)));
        assert!(!accept_caret(&rect(10.0, 10.0, 1.0, 0.0)));
        assert!(!accept_caret(&rect(10.0, 10.0, 1.0, f64::NAN)));
    }

    #[test]
    fn wayland_detected_from_env() {
        let wayland = env(&[("XDG_SESSION_TYPE", "wayland")]);
        assert_eq!(session_for(Os::Linux, &wayland), Session::Wayland);
        // Some sessions leave XDG_SESSION_TYPE out, or say "tty": the compositor's socket gives it away.
        let socket = env(&[("WAYLAND_DISPLAY", "wayland-0")]);
        assert_eq!(session_for(Os::Linux, &socket), Session::Wayland);
        let tty = env(&[
            ("XDG_SESSION_TYPE", "tty"),
            ("WAYLAND_DISPLAY", "wayland-1"),
        ]);
        assert_eq!(session_for(Os::Linux, &tty), Session::Wayland);

        let x11 = env(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0")]);
        assert_eq!(session_for(Os::Linux, &x11), Session::X11);
        assert_eq!(session_for(Os::Linux, &env(&[])), Session::X11);
        // A variable that is set to nothing is not set.
        let empty = env(&[("WAYLAND_DISPLAY", "")]);
        assert_eq!(session_for(Os::Linux, &empty), Session::X11);

        // The environment is for Linux only.
        assert_eq!(session_for(Os::Mac, &wayland), Session::MacOS);
        assert_eq!(session_for(Os::Windows, &wayland), Session::Windows);
    }

    #[test]
    fn session_follows_the_system_this_build_runs_on() {
        let expected = match Os::current() {
            Os::Mac => Session::MacOS,
            Os::Windows => Session::Windows,
            Os::Linux => Session::X11,
        };
        assert_eq!(session(&env(&[])), expected);
        assert_eq!(Os::current() == Os::Mac, cfg!(target_os = "macos"));
        assert_eq!(Os::current() == Os::Windows, cfg!(target_os = "windows"));
    }

    #[test]
    fn appkit_rectangles_are_flipped_to_the_top_left() {
        // A 1000 pt high primary screen: a rectangle 200 pt above its bottom edge and 50 pt high has its top edge
        // 750 pt below the top of the screen.
        let flipped = flip_y(rect(100.0, 200.0, 300.0, 50.0), 1000.0);
        assert_eq!(flipped, rect(100.0, 750.0, 300.0, 50.0));
        // The frame of the primary screen itself stays where it is.
        let primary = rect(0.0, 0.0, 1440.0, 900.0);
        assert_eq!(flip_y(primary, 900.0), primary);
        // A screen above the primary one (AppKit y > 0) is at a negative y from the top.
        let above = flip_y(rect(0.0, 900.0, 1920.0, 1080.0), 900.0);
        assert_eq!(above, rect(0.0, -1080.0, 1920.0, 1080.0));
    }

    fn two_screens() -> [Screen; 2] {
        [
            Screen {
                frame: rect(0.0, 0.0, 1440.0, 900.0),
                visible: rect(0.0, 25.0, 1440.0, 800.0),
            },
            Screen {
                frame: rect(1440.0, 0.0, 1920.0, 1080.0),
                visible: rect(1440.0, 0.0, 1920.0, 1040.0),
            },
        ]
    }

    #[test]
    fn the_screen_of_a_point_gives_its_visible_frame() {
        let screens = two_screens();
        assert_eq!(
            pick_visible_frame(&screens, Some((100.0, 100.0))),
            Some(screens[0].visible)
        );
        assert_eq!(
            pick_visible_frame(&screens, Some((1500.0, 500.0))),
            Some(screens[1].visible)
        );
        // The right and bottom edges belong to the next screen.
        assert_eq!(
            pick_visible_frame(&screens, Some((1440.0, 10.0))),
            Some(screens[1].visible)
        );
    }

    #[test]
    fn a_point_on_no_screen_gets_the_nearest_one() {
        let screens = two_screens();
        assert_eq!(
            pick_visible_frame(&screens, Some((-300.0, 400.0))),
            Some(screens[0].visible)
        );
        assert_eq!(
            pick_visible_frame(&screens, Some((5000.0, 400.0))),
            Some(screens[1].visible)
        );
        assert_eq!(
            pick_visible_frame(&screens, Some((100.0, 1500.0))),
            Some(screens[0].visible)
        );
    }

    #[test]
    fn no_point_gives_the_main_screen() {
        let screens = two_screens();
        assert_eq!(pick_visible_frame(&screens, None), Some(screens[0].visible));
        assert_eq!(pick_visible_frame(&[], None), None);
        assert_eq!(pick_visible_frame(&[], Some((1.0, 1.0))), None);
    }

    #[test]
    fn a_rectangle_off_every_screen_has_no_place() {
        let screens = two_screens();
        assert!(on_any_screen(&screens, &rect(100.0, 100.0, 1.0, 18.0)));
        // Half on the second screen is on it.
        assert!(on_any_screen(&screens, &rect(3350.0, 500.0, 50.0, 18.0)));
        assert!(!on_any_screen(&screens, &rect(0.0, -30000.0, 1.0, 18.0)));
        assert!(!on_any_screen(&screens, &rect(9000.0, 100.0, 1.0, 18.0)));
        assert!(!on_any_screen(&[], &rect(100.0, 100.0, 1.0, 18.0)));
    }

    #[test]
    fn a_terminal_gets_an_anchor_near_the_bottom_left_of_its_window() {
        let window = rect(100.0, 50.0, 800.0, 600.0);
        // 24 pt from the left edge, 56 pt above the bottom edge, one point wide and a line high.
        assert_eq!(terminal_anchor(&window), rect(124.0, 594.0, 1.0, 20.0));
    }

    #[test]
    fn a_small_field_is_its_own_anchor_and_a_big_one_is_not() {
        let chat_box = rect(10.0, 700.0, 600.0, 40.0);
        assert_eq!(field_anchor(&chat_box), Some(chat_box));
        assert!(field_anchor(&rect(10.0, 10.0, 600.0, 160.0)).is_some());
        // A field with no width (some apps report none) still has a place: one point wide.
        assert_eq!(
            field_anchor(&rect(10.0, 700.0, 0.0, 21.0)),
            Some(rect(10.0, 700.0, 1.0, 21.0))
        );
        assert_eq!(field_anchor(&rect(10.0, 10.0, 600.0, 160.5)), None);
        assert_eq!(field_anchor(&rect(10.0, 10.0, 600.0, 0.0)), None);
    }

    #[test]
    fn the_caret_after_a_character_is_its_right_edge() {
        let character = rect(200.0, 300.0, 9.0, 18.0);
        assert_eq!(caret_after(&character), rect(209.0, 300.0, 1.0, 18.0));
    }

    #[test]
    fn the_first_line_of_a_text_range_is_its_rectangle() {
        let two_lines = [10.0, 20.0, 0.0, 18.0, 30.0, 40.0, 50.0, 18.0];
        assert_eq!(first_rect(&two_lines), Some(rect(10.0, 20.0, 0.0, 18.0)));
        assert_eq!(first_rect(&[]), None);
        // A group that is not complete says nothing.
        assert_eq!(first_rect(&[10.0, 20.0, 5.0]), None);
        assert_eq!(first_rect(&[f64::NAN, 20.0, 5.0, 18.0]), None);
    }

    #[test]
    fn physical_pixels_divide_by_the_scale_of_the_screen() {
        let physical = rect(300.0, 450.0, 3.0, 36.0);
        assert_eq!(to_logical(physical, 1.5), rect(200.0, 300.0, 2.0, 24.0));
        assert_eq!(to_logical(physical, 1.0), physical);
        // A scale that makes no sense changes nothing.
        assert_eq!(to_logical(physical, 0.0), physical);
        assert_eq!(to_logical(physical, f64::NAN), physical);
    }

    #[test]
    fn the_window_class_is_the_second_name_of_the_property() {
        assert_eq!(
            parse_wm_class(b"gnome-terminal-server\0Gnome-terminal\0"),
            Some(("gnome-terminal-server".into(), "Gnome-terminal".into()))
        );
        // The last 0 byte may be missing.
        assert_eq!(
            parse_wm_class(b"xterm\0XTerm"),
            Some(("xterm".into(), "XTerm".into()))
        );
        assert_eq!(
            parse_wm_class(b"kitty\0"),
            Some(("kitty".into(), "kitty".into()))
        );
        assert_eq!(parse_wm_class(b""), None);
        assert_eq!(parse_wm_class(b"\0\0"), None);
    }

    #[test]
    fn chromium_browsers_are_told_that_a_client_is_there() {
        for id in [
            "com.google.Chrome",
            "com.google.Chrome.beta",
            "com.brave.Browser",
            "com.microsoft.edgemac",
            "company.thebrowser.Browser",
            "com.vivaldi.Vivaldi",
            "com.operasoftware.Opera",
            "org.chromium.Chromium",
        ] {
            assert!(is_chromium_browser(id), "{id}");
        }
        assert!(!is_chromium_browser("com.apple.Safari"));
        assert!(!is_chromium_browser("org.mozilla.firefox"));
        assert!(!is_chromium_browser(""));
    }

    #[test]
    fn an_app_that_names_itself_as_the_focus_says_nothing() {
        assert!(ax_focus_is_the_app("AXApplication"));
        assert!(!ax_focus_is_the_app("AXTextField"));
        assert!(!ax_focus_is_the_app("AXWindow"));
        assert!(!ax_focus_is_the_app(""));
    }

    #[test]
    fn a_failed_read_of_the_password_flag_is_no_field() {
        // The password rule fails closed: an error is not an answer.
        assert_eq!(
            secure_read_outcome(SecureRead::Failed),
            SecureOutcome::NoField
        );
    }

    #[test]
    fn an_element_without_the_password_flag_is_classified_normally() {
        assert_eq!(
            secure_read_outcome(SecureRead::Unsupported),
            SecureOutcome::Classify
        );
        assert_eq!(
            secure_read_outcome(SecureRead::Answered),
            SecureOutcome::Classify
        );
    }

    #[test]
    fn a_text_read_up_to_its_cap_has_no_known_length() {
        assert_eq!(bounded_text_len(0, 50_000), Some(0));
        assert_eq!(bounded_text_len(49_999, 50_000), Some(49_999));
        // The read stopped at the cap: the text may go on.
        assert_eq!(bounded_text_len(50_000, 50_000), None);
        assert_eq!(bounded_text_len(70_000, 50_000), None);
    }
}
