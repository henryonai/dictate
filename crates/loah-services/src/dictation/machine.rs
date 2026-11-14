//! What a press or a release of the dictation hotkey does, by phase.
//!
//! In toggle mode a press stops a recording and starts one from every other phase. In hold mode (push to talk) a
//! press starts the recording and the release stops it. A press while the recording is being finished, or while
//! a paste is in progress, is ignored. A press while the AI is still editing and nothing is written yet writes
//! the transcript as it is, without the edit.

use serde::Serialize;

/// Where a dictation is. It travels to the UI inside the panel state, as a lowercase name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Hidden,
    Recording,
    Finishing,
    Review,
    Refining,
    Working,
    Written,
}

/// What the app does in answer to the hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    WriteNow,
    Ignore,
}

/// What a press of the hotkey does. `push_to_talk` is the hold mode (a press starts, the release stops); without
/// it a press toggles. `writing` is a paste in progress and `written` that the text is already in the field: only
/// the working phase looks at them. The `--toggle` command line flag is a press with `push_to_talk` off.
pub fn on_press(phase: Phase, push_to_talk: bool, writing: bool, written: bool) -> Action {
    match phase {
        Phase::Recording if push_to_talk => Action::Ignore,
        Phase::Recording => Action::Stop,
        Phase::Finishing => Action::Ignore,
        // A paste cannot be taken back, so wait for it.
        Phase::Working if writing => Action::Ignore,
        // The AI is still editing: write what was heard as it is. Once the text is in the field, a press is
        // for the next dictation while a rewrite runs.
        Phase::Working if !written => Action::WriteNow,
        Phase::Working | Phase::Hidden | Phase::Review | Phase::Refining | Phase::Written => {
            Action::Start
        }
    }
}

/// What a release of the hotkey does: it stops a recording in hold mode, and nothing else.
pub fn on_release(phase: Phase, push_to_talk: bool) -> Action {
    if push_to_talk && phase == Phase::Recording {
        Action::Stop
    } else {
        Action::Ignore
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_PHASES: [Phase; 7] = [
        Phase::Hidden,
        Phase::Recording,
        Phase::Finishing,
        Phase::Review,
        Phase::Refining,
        Phase::Working,
        Phase::Written,
    ];

    #[test]
    fn machine_toggle_press_starts_and_stops() {
        assert_eq!(on_press(Phase::Hidden, false, false, false), Action::Start);
        assert_eq!(
            on_press(Phase::Recording, false, false, false),
            Action::Stop
        );
    }

    #[test]
    fn machine_hold_press_while_recording_is_ignored() {
        assert_eq!(
            on_press(Phase::Recording, true, false, false),
            Action::Ignore
        );
    }

    #[test]
    fn machine_hold_release_stops() {
        assert_eq!(on_release(Phase::Recording, true), Action::Stop);
    }

    #[test]
    fn machine_hold_release_outside_recording_is_ignored() {
        for phase in ALL_PHASES.into_iter().filter(|p| *p != Phase::Recording) {
            assert_eq!(on_release(phase, true), Action::Ignore, "{phase:?}");
        }
    }

    #[test]
    fn machine_release_in_toggle_mode_is_ignored() {
        for phase in ALL_PHASES {
            assert_eq!(on_release(phase, false), Action::Ignore, "{phase:?}");
        }
    }

    #[test]
    fn machine_press_while_finishing_is_ignored() {
        for push_to_talk in [false, true] {
            assert_eq!(
                on_press(Phase::Finishing, push_to_talk, false, false),
                Action::Ignore
            );
        }
    }

    #[test]
    fn machine_press_while_writing_is_ignored() {
        for push_to_talk in [false, true] {
            for written in [false, true] {
                assert_eq!(
                    on_press(Phase::Working, push_to_talk, true, written),
                    Action::Ignore
                );
            }
        }
    }

    #[test]
    fn machine_press_during_ai_writes_now() {
        for push_to_talk in [false, true] {
            assert_eq!(
                on_press(Phase::Working, push_to_talk, false, false),
                Action::WriteNow
            );
        }
    }

    #[test]
    fn machine_press_during_rewrite_starts_new() {
        for push_to_talk in [false, true] {
            assert_eq!(
                on_press(Phase::Working, push_to_talk, false, true),
                Action::Start
            );
        }
    }

    #[test]
    fn machine_press_from_review_chip_or_hidden_starts() {
        for phase in [
            Phase::Hidden,
            Phase::Review,
            Phase::Refining,
            Phase::Written,
        ] {
            for push_to_talk in [false, true] {
                // Only a Working phase looks at `writing` and `written`.
                for (writing, written) in [(false, false), (true, true)] {
                    assert_eq!(
                        on_press(phase, push_to_talk, writing, written),
                        Action::Start,
                        "{phase:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn phase_serializes_as_a_lowercase_name() {
        let names: Vec<String> = ALL_PHASES
            .iter()
            .map(|p| serde_json::to_string(p).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "\"hidden\"",
                "\"recording\"",
                "\"finishing\"",
                "\"review\"",
                "\"refining\"",
                "\"working\"",
                "\"written\""
            ]
        );
    }
}
