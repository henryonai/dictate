//! A way to look at the windows without the controller: in a debug build, `DICTATE_PANEL_DEMO=1` cycles fixture states
//! through [`emit_state`]. Debug builds only: a release build has none of it.
//!
//! - `DICTATE_PANEL_DEMO_ANCHOR=x,y` puts the panels next to a caret there (logical points from the top left of the
//!   main screen), where without it they stand at the bottom centre of the screen.
//! - `DICTATE_PANEL_DEMO_STEP_MS` is how long each state stays (3000).
//! - `DICTATE_PANEL_DEMO_HOLD=n` stays on the n-th state (from 0) and doesn't cycle.

use std::time::Duration;

use dictate_services::dictation::machine::Phase;
use tauri::AppHandle;

use crate::panel::{emit_state, set_anchor, PanelState, View, WorkKind, Working};
use crate::platform::Rect;

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
        text: "This is the part that is final, and this is what was heard.".into(),
        ..base()
    };
    vec![
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
        PanelState::hidden(),
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
    log::info!("Panel demo: cycling the fixture states (anchor {anchor:?})");
    let spawned = std::thread::Builder::new()
        .name("panel-demo".into())
        .spawn(move || run(&app, anchor, step, hold));
    if let Err(e) = spawned {
        log::warn!("Could not start the panel demo: {e}");
    }
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
        assert!(has(View::Bubble) && has(View::Pill));
        assert_eq!(fixtures.last().map(|f| f.view), Some(View::Hidden));
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
