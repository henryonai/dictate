//! Where a dictation goes: into the text field that has the focus, or into the review card. Decided at the start from what
//! the platform layer finds, and once more right before the text is pasted. Only the rules are here; asking the system
//! is the caller's, so that the rules run on every system with nothing but numbers and names.

use crate::platform::rules::Os;
use crate::platform::{AppId, FieldLookup, Permission, Rect};

/// Why a dictation has no field to write into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardReason {
    /// The setting that writes into the field is off.
    AutoWriteOff,
    /// macOS only lets an app that has the Accessibility permission look into other apps' fields.
    AccessibilityMissing,
    /// The app in front is Dictate: its own window is no place to paste into.
    OwnWindow,
    /// The focus is on something that doesn't take text: a button, the desktop, a password field.
    NoField,
    /// The system doesn't say (a hung app, a failed read): not knowing that it is no password field means no paste.
    Unknown,
}

/// Where the text goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Pasted into this app, with the bubble by `anchor`.
    Inline {
        app: AppId,
        terminal: bool,
        anchor: Option<Rect>,
    },
    /// Shown in the card, to edit and copy.
    Card(CardReason),
}

impl Target {
    pub fn is_inline(&self) -> bool {
        matches!(self, Target::Inline { .. })
    }
}

/// What the rules need to know besides the answers of the system.
#[derive(Debug, Clone, Copy)]
pub struct Facts {
    pub os: Os,
    pub auto_write: bool,
    pub accessibility: Permission,
    pub own_pid: u32,
    /// One of this app's own windows has the focus (Settings, the card). Where the system names the app in front this
    /// says what `own_pid` says; Wayland names none, and the focus of our own window is all that is known of it.
    pub own_window_focused: bool,
}

/// Where the text goes. `front` is the app that has the focus; `lookup` and `mouse` ask the system, and are only asked
/// when the answer can still change the result.
///
/// Only a field that the system says is one gets a paste. The exception is X11 and Wayland, which can't tell a password
/// field from another field, or a field from the desktop, and have no other way to write than to try: an answer of
/// "don't know" there gets a bubble by the mouse and a paste.
pub fn decide(
    facts: Facts,
    front: Option<&AppId>,
    lookup: impl FnOnce() -> FieldLookup,
    mouse: impl FnOnce() -> Option<(f64, f64)>,
) -> Target {
    if !facts.auto_write {
        return Target::Card(CardReason::AutoWriteOff);
    }
    if facts.os == Os::Mac && facts.accessibility != Permission::Granted {
        return Target::Card(CardReason::AccessibilityMissing);
    }
    if facts.own_window_focused || front.is_some_and(|front| front.pid == Some(facts.own_pid)) {
        return Target::Card(CardReason::OwnWindow);
    }
    match lookup() {
        FieldLookup::Field(field) => Target::Inline {
            app: field.app,
            terminal: field.terminal,
            anchor: field.anchor,
        },
        FieldLookup::NoField => Target::Card(CardReason::NoField),
        FieldLookup::Unknown if facts.os == Os::Linux => Target::Inline {
            // Wayland has no question for the app in front: an empty id is "whoever is in front".
            app: front.cloned().unwrap_or(AppId {
                id: String::new(),
                pid: None,
            }),
            terminal: false,
            anchor: mouse().map(|(x, y)| Rect {
                x,
                y,
                w: 1.0,
                h: 1.0,
            }),
        },
        FieldLookup::Unknown => Target::Card(CardReason::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::platform::FieldInfo;

    const OWN: u32 = 4242;

    fn facts(os: Os) -> Facts {
        Facts {
            os,
            auto_write: true,
            accessibility: if os == Os::Mac {
                Permission::Granted
            } else {
                Permission::NotNeeded
            },
            own_pid: OWN,
            own_window_focused: false,
        }
    }

    fn app(id: &str, pid: u32) -> AppId {
        AppId {
            id: id.into(),
            pid: Some(pid),
        }
    }

    fn field(anchor: Option<Rect>) -> FieldLookup {
        FieldLookup::Field(FieldInfo {
            app: app("com.apple.TextEdit", 10),
            terminal: false,
            anchor,
        })
    }

    fn caret() -> Rect {
        Rect {
            x: 100.0,
            y: 200.0,
            w: 2.0,
            h: 18.0,
        }
    }

    /// Decides with the answers given, and says whether the system was asked for the field.
    fn decided(
        facts: Facts,
        front: Option<AppId>,
        lookup: FieldLookup,
        mouse: Option<(f64, f64)>,
    ) -> (Target, bool) {
        let asked = Cell::new(false);
        let target = decide(
            facts,
            front.as_ref(),
            || {
                asked.set(true);
                lookup
            },
            || mouse,
        );
        (target, asked.get())
    }

    #[test]
    fn a_field_is_written_into_with_the_bubble_at_its_caret() {
        for os in [Os::Mac, Os::Windows, Os::Linux] {
            let (target, _) = decided(facts(os), Some(app("x", 10)), field(Some(caret())), None);
            assert_eq!(
                target,
                Target::Inline {
                    app: app("com.apple.TextEdit", 10),
                    terminal: false,
                    anchor: Some(caret())
                },
                "{os:?}"
            );
        }
    }

    #[test]
    fn a_terminal_is_a_field_that_says_so() {
        let lookup = FieldLookup::Field(FieldInfo {
            app: app("com.apple.Terminal", 11),
            terminal: true,
            anchor: None,
        });
        let (target, _) = decided(facts(Os::Mac), Some(app("x", 11)), lookup, None);
        assert!(
            matches!(target, Target::Inline { terminal: true, .. }),
            "{target:?}"
        );
    }

    #[test]
    fn no_field_means_the_card_and_never_a_paste() {
        // This is how a password field is kept out: the platform layer answers `NoField` for it.
        for os in [Os::Mac, Os::Windows, Os::Linux] {
            let (target, _) = decided(facts(os), Some(app("x", 10)), FieldLookup::NoField, None);
            assert_eq!(target, Target::Card(CardReason::NoField), "{os:?}");
        }
    }

    #[test]
    fn an_answer_of_unknown_is_a_card_on_macos_and_windows() {
        // A hung app or a failed read: it isn't known that the field is no password field.
        for os in [Os::Mac, Os::Windows] {
            let (target, _) = decided(
                facts(os),
                Some(app("x", 10)),
                FieldLookup::Unknown,
                Some((5.0, 6.0)),
            );
            assert_eq!(target, Target::Card(CardReason::Unknown), "{os:?}");
        }
    }

    #[test]
    fn an_answer_of_unknown_on_linux_is_a_paste_with_the_bubble_at_the_mouse() {
        let (target, _) = decided(
            facts(Os::Linux),
            Some(app("firefox", 77)),
            FieldLookup::Unknown,
            Some((300.0, 400.0)),
        );
        assert_eq!(
            target,
            Target::Inline {
                app: app("firefox", 77),
                terminal: false,
                anchor: Some(Rect {
                    x: 300.0,
                    y: 400.0,
                    w: 1.0,
                    h: 1.0
                })
            }
        );
    }

    #[test]
    fn wayland_names_no_app_in_front_and_the_paste_goes_to_whoever_is() {
        let (target, _) = decided(facts(Os::Linux), None, FieldLookup::Unknown, None);
        assert_eq!(
            target,
            Target::Inline {
                app: AppId {
                    id: String::new(),
                    pid: None
                },
                terminal: false,
                anchor: None
            }
        );
    }

    #[test]
    fn nothing_is_looked_up_when_the_setting_is_off_and_the_card_is_the_answer() {
        for os in [Os::Mac, Os::Windows, Os::Linux] {
            let off = Facts {
                auto_write: false,
                ..facts(os)
            };
            let (target, asked) = decided(off, Some(app("x", 10)), field(None), None);
            assert_eq!(target, Target::Card(CardReason::AutoWriteOff), "{os:?}");
            assert!(!asked, "{os:?} looked at another app's field for nothing");
        }
    }

    #[test]
    fn macos_without_accessibility_has_the_card_and_looks_at_nothing() {
        for permission in [Permission::Denied, Permission::NotDetermined] {
            let missing = Facts {
                accessibility: permission,
                ..facts(Os::Mac)
            };
            let (target, asked) = decided(missing, Some(app("x", 10)), field(None), None);
            assert_eq!(target, Target::Card(CardReason::AccessibilityMissing));
            assert!(!asked);
        }
    }

    #[test]
    fn the_accessibility_permission_is_a_matter_for_macos_only() {
        let denied = Facts {
            accessibility: Permission::Denied,
            ..facts(Os::Windows)
        };
        let (target, _) = decided(denied, Some(app("x", 10)), field(None), None);
        assert!(target.is_inline());
    }

    #[test]
    fn dictate_itself_in_front_has_the_card() {
        for os in [Os::Mac, Os::Windows, Os::Linux] {
            let (target, asked) =
                decided(facts(os), Some(app("io.dictate", OWN)), field(None), None);
            assert_eq!(target, Target::Card(CardReason::OwnWindow), "{os:?}");
            assert!(!asked, "{os:?}");
        }
    }

    #[test]
    fn one_of_our_own_windows_with_the_focus_has_the_card_on_every_system() {
        // Wayland names no app in front, so that only the focus of our own window says that Settings is where the person is.
        for os in [Os::Mac, Os::Windows, Os::Linux] {
            let own = Facts {
                own_window_focused: true,
                ..facts(os)
            };
            let (target, asked) = decided(own, None, FieldLookup::Unknown, Some((5.0, 6.0)));
            assert_eq!(target, Target::Card(CardReason::OwnWindow), "{os:?}");
            assert!(!asked, "{os:?}");
        }
    }

    #[test]
    fn another_process_with_the_same_name_is_not_dictate() {
        let (target, _) = decided(
            facts(Os::Windows),
            Some(app("io.dictate", OWN + 1)),
            field(None),
            None,
        );
        assert!(target.is_inline());
    }
}
