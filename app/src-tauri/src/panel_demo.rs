//! A way to look at the windows without the controller: in a debug build, `DICTATE_PANEL_DEMO=1` cycles fixture states
//! through [`emit_state`]. Debug builds only: a release build has none of it.
//!
//! - `DICTATE_PANEL_DEMO_ANCHOR=x,y` puts the panels next to a caret there (logical points from the top left of the
//!   main screen), where without it they stand at the bottom centre of the screen.
//! - `DICTATE_PANEL_DEMO_STEP_MS` is how long each state stays (3000).
//! - `DICTATE_PANEL_DEMO_HOLD=n` stays on the n-th state (from 0) and doesn't cycle.
//! - `DICTATE_PANEL_DEMO_MONITOR=1` also starts the monitor of the keyboard and the mouse, which only logs that it heard
//!   a key or a click (never which).

use std::sync::Arc;
use std::time::Duration;

use dictate_services::dictation::machine::Phase;
use dictate_services::dictation::pipeline::{Notice, NoticeKey};
use tauri::AppHandle;

use crate::panel::{emit_state, set_anchor, PanelState, View, WorkKind, Working};
use crate::platform::monitor::InputMonitor;
use crate::platform::{self, Rect};

/// How often the level and the timer of a state that listens are updated.
const TICK: Duration = Duration::from_millis(100);

fn base() -> PanelState {
    PanelState {
        hotkey: "\u{2303}\u{2325}Space".into(),
        ..PanelState::hidden()
    }
}

fn working(kind: WorkKind, style: Option<&str>, model: Option<&str>, editing: bool) -> Working {
    Working {
        kind,
        style: style.map(str::to_owned),
        model: model.map(str::to_owned),
        editing,
    }
}

/// One state of each shape the page draws now, in the order a dictation goes through them.
pub fn fixtures() -> Vec<PanelState> {
    let listening = PanelState {
        view: View::Bubble,
        phase: Phase::Recording,
        ..base()
    };
    let words = PanelState {
        committed: "This is the part that is final, and".into(),
        partial: "this is what is still a guess".into(),
        ..listening.clone()
    };
    let pill = PanelState {
        view: View::Pill,
        phase: Phase::Recording,
        duration_s: 65.0,
        ..base()
    };
    let edited = PanelState {
        view: View::Bubble,
        phase: Phase::Working,
        text: WRITTEN.into(),
        ..base()
    };
    let mut states = vec![
        listening,
        words.clone(),
        PanelState {
            push_to_talk: true,
            ..words.clone()
        },
        PanelState {
            phase: Phase::Finishing,
            ..words
        },
        PanelState {
            working: Some(working(WorkKind::Editing, Some("concise"), None, false)),
            ..edited.clone()
        },
        PanelState {
            working: Some(working(WorkKind::Writing, None, None, false)),
            ..edited.clone()
        },
        PanelState {
            working: Some(working(
                WorkKind::Retranscribing,
                Some("clean"),
                Some("whisper-1"),
                true,
            )),
            ..edited
        },
        pill.clone(),
        PanelState {
            committed: "The pill shows the words above it,".into(),
            partial: "and a guess for the rest".into(),
            ..pill.clone()
        },
        PanelState {
            phase: Phase::Finishing,
            ..pill
        },
    ];
    states.extend(chips());
    states.extend(cards());
    states.push(PanelState::hidden());
    states
}

const WRITTEN: &str = "This is the part that is final, and this is what was heard.";

/// The result chip as each of its titles is made: nothing was done to the text, an AI edit, the original, a notice
/// with text (and so a ⋯), and a notice alone (which has none).
fn chips() -> Vec<PanelState> {
    let chip = PanelState {
        view: View::Chip,
        phase: Phase::Written,
        text: WRITTEN.into(),
        ..base()
    };
    let notice = |key, detail: Option<&str>| {
        Some(Notice {
            key,
            detail: detail.map(str::to_owned),
        })
    };
    vec![
        chip.clone(),
        PanelState {
            original: Some("this is the part that is final and this is what was heard".into()),
            style: Some("concise".into()),
            ..chip.clone()
        },
        PanelState {
            original: Some(WRITTEN.into()),
            ..chip.clone()
        },
        PanelState {
            notice: notice(NoticeKey::NotEditedNoKey, None),
            ..chip.clone()
        },
        PanelState {
            notice: notice(
                NoticeKey::EditFailed,
                Some("The AI service answered with an error"),
            ),
            ..chip.clone()
        },
        PanelState {
            text: String::new(),
            notice: notice(NoticeKey::NothingHeard, None),
            ..chip
        },
    ]
}

/// The review card: from the dictation, with an AI edit under way, with the permission missing, with an error, and
/// opened from the chip (with the box for a custom request too).
fn cards() -> Vec<PanelState> {
    let card = PanelState {
        view: View::Card,
        phase: Phase::Review,
        text: WRITTEN.into(),
        duration_s: 65.0,
        can_write: true,
        ..base()
    };
    vec![
        card.clone(),
        PanelState {
            phase: Phase::Refining,
            working: Some(working(WorkKind::Editing, Some("concise"), None, false)),
            ..card.clone()
        },
        PanelState {
            style: Some("concise".into()),
            original: Some("this is the part that is final and this is what was heard".into()),
            ..card.clone()
        },
        PanelState {
            accessibility_missing: true,
            notice: Some(Notice {
                key: NoticeKey::EditFailed,
                detail: Some("The AI service answered with an error".into()),
            }),
            ..card.clone()
        },
        PanelState {
            from_chip: true,
            duration_s: 0.0,
            original: Some(WRITTEN.into()),
            ..card.clone()
        },
        PanelState {
            from_chip: true,
            open_custom: true,
            duration_s: 0.0,
            ..card
        },
    ]
}

/// Starts cycling when the environment asks for it.
pub fn start(app: &AppHandle) {
    if std::env::var_os("DICTATE_PANEL_DEMO").is_none() {
        return;
    }
    let anchor = std::env::var("DICTATE_PANEL_DEMO_ANCHOR")
        .ok()
        .and_then(|value| parse_anchor(&value));
    let step = std::env::var("DICTATE_PANEL_DEMO_STEP_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(Duration::from_secs(3), Duration::from_millis);
    let hold = std::env::var("DICTATE_PANEL_DEMO_HOLD")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let app = app.clone();
    let monitor = std::env::var_os("DICTATE_PANEL_DEMO_MONITOR")
        .is_some()
        .then(|| start_monitor(&app));
    log::info!("Panel demo: cycling the fixture states (anchor {anchor:?})");
    let spawned = std::thread::Builder::new()
        .name("panel-demo".into())
        .spawn(move || {
            // The monitor listens for as long as the demo runs, which is as long as the app does.
            let _monitor = monitor;
            run(&app, anchor, step, hold);
        });
    if let Err(e) = spawned {
        log::warn!("Could not start the panel demo: {e}");
    }
}

/// Starts the monitor of the keyboard and the mouse, to see that it hears. It logs that something was pressed, and
/// nothing about what.
fn start_monitor(app: &AppHandle) -> Arc<dyn InputMonitor> {
    let monitor = platform::current_monitor(app);
    monitor.start(Box::new(|| {
        log::info!("Panel demo: the monitor heard a key or a click");
    }));
    monitor
}

/// "x,y": a caret at that point, 2 wide and a line of text high.
pub fn parse_anchor(value: &str) -> Option<Rect> {
    let (x, y) = value.split_once(',')?;
    Some(Rect {
        x: x.trim().parse().ok()?,
        y: y.trim().parse().ok()?,
        w: 2.0,
        h: 18.0,
    })
}

fn run(app: &AppHandle, anchor: Option<Rect>, step: Duration, hold: Option<usize>) {
    // The pages need a moment to load before they hear a state.
    std::thread::sleep(Duration::from_secs(2));
    set_anchor(app, anchor);
    let fixtures = fixtures();
    loop {
        let shown: Vec<&PanelState> = match hold {
            Some(n) => fixtures.get(n).into_iter().collect(),
            None => fixtures.iter().collect(),
        };
        for fixture in shown {
            log::info!("Panel demo: {:?} {:?}", fixture.view, fixture.phase);
            let started = std::time::Instant::now();
            while started.elapsed() < step {
                let mut state = fixture.clone();
                let t = started.elapsed().as_secs_f32();
                // Something like speech: a level that comes and goes.
                state.level = ((t * 6.0).sin() * 0.5 + 0.5) * ((t * 2.3).sin().abs());
                state.duration_s += f64::from(t);
                emit_state(app, &state);
                std::thread::sleep(TICK);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_shows_every_shape_the_page_draws_and_ends_hidden() {
        let fixtures = fixtures();
        let has = |view: View| fixtures.iter().any(|f| f.view == view);
        assert!(has(View::Bubble) && has(View::Pill) && has(View::Chip) && has(View::Card));
        assert_eq!(fixtures.last().map(|f| f.view), Some(View::Hidden));
    }

    #[test]
    fn the_demo_shows_the_chips_that_the_titles_come_from() {
        let fixtures = fixtures();
        let chips: Vec<&PanelState> = fixtures.iter().filter(|f| f.view == View::Chip).collect();
        // What was written with nothing done to it, an edited text, the text that is its original, a notice with text
        // and a notice alone (which has no ⋯).
        assert!(chips
            .iter()
            .any(|c| c.notice.is_none() && c.style.is_none() && c.original.is_none()));
        assert!(chips
            .iter()
            .any(|c| c.style.is_some() && c.original.is_some()));
        assert!(chips
            .iter()
            .any(|c| c.original.as_deref() == Some(c.text.as_str()) && !c.text.is_empty()));
        assert!(chips
            .iter()
            .any(|c| c.notice.is_some() && !c.text.is_empty()));
        assert!(chips
            .iter()
            .any(|c| c.notice.is_some() && c.text.is_empty()));
    }

    #[test]
    fn the_demo_shows_the_card_as_it_is_opened_in_each_way() {
        let fixtures = fixtures();
        let cards: Vec<&PanelState> = fixtures.iter().filter(|f| f.view == View::Card).collect();
        // From the dictation and from the chip, with the box for a custom request, while the AI edits, with the
        // permission missing, and with an original to undo to.
        assert!(cards.iter().any(|c| !c.from_chip));
        assert!(cards.iter().any(|c| c.from_chip));
        assert!(cards.iter().any(|c| c.open_custom));
        assert!(cards.iter().any(|c| c.phase == Phase::Refining));
        assert!(cards.iter().any(|c| c.accessibility_missing));
        assert!(cards.iter().any(|c| c.original.is_some()));
    }

    #[test]
    fn the_demo_shows_each_title_of_the_bubble_and_both_hints() {
        let fixtures = fixtures();
        let kinds: Vec<WorkKind> = fixtures
            .iter()
            .filter_map(|f| f.working.as_ref().map(|w| w.kind))
            .collect();
        for kind in [
            WorkKind::Writing,
            WorkKind::Editing,
            WorkKind::Retranscribing,
        ] {
            assert!(kinds.contains(&kind), "{kind:?}");
        }
        assert!(fixtures.iter().any(|f| f.push_to_talk));
        assert!(fixtures.iter().any(|f| !f.push_to_talk));
        assert!(fixtures.iter().any(|f| f.phase == Phase::Finishing));
    }

    #[test]
    fn an_anchor_is_x_comma_y() {
        assert_eq!(
            parse_anchor("300, 200"),
            Some(Rect {
                x: 300.0,
                y: 200.0,
                w: 2.0,
                h: 18.0
            })
        );
        assert_eq!(parse_anchor("300"), None);
        assert_eq!(parse_anchor("a,b"), None);
    }
}
