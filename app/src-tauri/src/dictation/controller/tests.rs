//! The controller, end to end, on fakes: the hotkey in, the paste, the chip and the card out.

use std::sync::atomic::Ordering;
use std::time::Duration;

use dictate_services::audio::capture::{CaptureError, CaptureEvent};
use dictate_services::dictation::machine::Phase;
use dictate_services::dictation::pipeline::{Notice, NoticeKey};
use dictate_services::presets::{find_edit_preset, TranscribePreset};
use dictate_services::settings::Engine;

use super::fakes::*;
use crate::hotkey::HotkeyEvent;
use crate::panel::{ErrorAction, PanelState, View, WorkKind};
use crate::platform::paste::PasteError;
use crate::platform::rules::Os;
use crate::platform::{FieldInfo, FieldLookup, FieldSnapshot, Permission};

fn snapshot(len: usize) -> Option<FieldSnapshot> {
    Some(FieldSnapshot {
        len,
        sel_loc: len,
        sel_len: 0,
    })
}

impl Fixture {
    /// One short dictation, from the press that starts it to the chip.
    fn dictate_to_chip(&self) {
        self.start_listening();
        self.talk(1);
        self.press();
        self.run_until("the chip", || self.chip_is_up());
    }

    /// One short dictation with no field to write into, from the press that starts it to the card.
    fn dictate_to_card(&self) {
        self.platform.lookups.set([FieldLookup::NoField]);
        self.start_listening();
        self.talk(1);
        self.press();
        self.wait_for_view(View::Card);
        self.wait_for("the card to be done", || {
            self.ui.last().phase == Phase::Review
        });
    }

    fn saved_files(&self) -> Vec<String> {
        self.files_in(&self.saved_dir())
    }

    fn uses_the_api(&self, preset: TranscribePreset) {
        self.change_settings(|s| {
            s.dictation_engine = Engine::Api;
            s.set_transcribe_preset(preset);
        });
        self.flags
            .set(dictate_services::secrets::Account::Transcribe, true);
    }
}

fn clean() -> &'static str {
    find_edit_preset("clean").instruction
}

fn notice(key: NoticeKey) -> Option<Notice> {
    Some(Notice { key, detail: None })
}

fn error_key(state: &PanelState) -> Option<&str> {
    state.error.as_ref().map(|e| e.key.as_str())
}

// ---- the flow ---------------------------------------------------------------------------------------------------

#[test]
fn controller_toggle_flow_pastes_and_shows_chip() {
    let fx = Fixture::new();
    fx.stt.say("hello world");

    fx.start_listening();
    let listening = fx.ui.last();
    assert_eq!(listening.view, View::Bubble);
    assert_eq!(listening.language, "en-US", "the language is the setting's");
    assert_eq!(listening.hotkey, "\u{2303}\u{2325}Space");
    assert_eq!(
        fx.ui.anchors.lock().unwrap().last().copied().flatten(),
        Some(caret()),
        "the bubble is by the caret"
    );

    // Live text follows the speech, a second behind.
    fx.talk(2);
    fx.wait_for("the live text", || fx.ui.last().partial == "hello world");
    assert!(fx.ui.last().level > 0.0, "the meter moves");
    assert!(fx.ui.last().duration_s >= 2.0);

    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());

    // Written into the field the person was in.
    assert_eq!(*fx.clipboard.writes.lock().unwrap(), ["Hello, world."]);
    // Asked to come forward by the controller, which looks at its field, and again by the paste.
    let forwarded = fx.keys.forwarded.lock().unwrap().clone();
    assert!(!forwarded.is_empty() && forwarded.iter().all(|app| *app == text_edit()));
    assert_eq!(
        *fx.keys.pastes.lock().unwrap(),
        [(false, false)],
        "a paste for a field that is no terminal, while nothing watched the keyboard"
    );
    let chip = fx.ui.last();
    assert_eq!(chip.phase, Phase::Written);
    assert_eq!(chip.text, "Hello, world.");
    assert_eq!(chip.original.as_deref(), Some("hello world"));
    assert_eq!(chip.style.as_deref(), Some("clean"));
    assert_eq!((chip.notice.clone(), chip.error.clone()), (None, None));
    assert!(chip.can_write);
    assert!(
        fx.monitor.running.load(Ordering::SeqCst),
        "the chip watches the keyboard"
    );
    assert_eq!(
        fx.backend.refines(),
        [("hello world".to_string(), clean().to_string())]
    );

    // The bubble said what was going on, between the words and the chip.
    let history = fx.ui.history();
    assert!(history.iter().any(|s| s.phase == Phase::Finishing));
    assert!(history.iter().any(|s| {
        s.phase == Phase::Working
            && s.working
                .as_ref()
                .is_some_and(|w| w.kind == WorkKind::Editing && w.style.as_deref() == Some("clean"))
            && s.text == "hello world"
    }));
    assert!(
        history
            .iter()
            .any(|s| s.view == View::Hidden && s.phase == Phase::Working),
        "our panels are out of the way of the paste"
    );

    // The recording is kept with what was heard, not what the AI made of it.
    let saved = fx.saved_files();
    assert_eq!(saved.len(), 2, "{saved:?}");
    let txt = saved.iter().find(|n| n.ends_with(".txt")).unwrap();
    assert_eq!(
        std::fs::read_to_string(fx.saved_dir().join(txt)).unwrap(),
        "hello world"
    );
    assert!(saved
        .iter()
        .any(|n| n == "20261109-101530.wav" || n.ends_with(".wav")));
    assert_eq!(fx.files_in(&fx.temp_dir()), Vec::<String>::new());

    // The shortcut was told that the dictation is over (not by the press that stopped it, see the test of that), and nothing
    // is left running.
    assert_eq!(fx.resets.load(Ordering::SeqCst), 1);
    assert!(fx.jobs.cancel_all_and_wait(Duration::from_secs(2)));
}

#[test]
fn controller_no_field_shows_card() {
    let fx = Fixture::new();
    fx.platform.lookups.set([FieldLookup::NoField]);

    fx.start_listening();
    assert_eq!(
        fx.ui.last().view,
        View::Pill,
        "no field: the pill and not the bubble"
    );
    fx.talk(1);
    fx.press();
    fx.wait_for_view(View::Card);
    fx.wait_for("the card", || fx.ui.last().phase == Phase::Review);

    let card = fx.ui.last();
    assert_eq!(card.text, "Hello, world.");
    assert_eq!(card.original.as_deref(), Some("hello world"));
    assert_eq!(card.style.as_deref(), Some("clean"));
    assert!(!card.from_chip && !card.can_write);
    assert_eq!((card.notice, card.error), (None, None));
    assert!(fx.keys.pastes.lock().unwrap().is_empty());
    assert!(fx.clipboard.writes.lock().unwrap().is_empty());
    // The card is the answer: nothing else is on screen, and no chip watches the keyboard.
    assert!(!fx.monitor.running.load(Ordering::SeqCst));
    // The transcript is kept as it was heard.
    assert!(fx.saved_files().iter().any(|n| n.ends_with(".txt")));
}

#[test]
fn controller_card_shows_the_text_as_heard_while_the_ai_and_the_api_work_on_it() {
    // The AI edits: the card is up with what was heard, which it can't be typed in yet, and the result replaces it.
    let fx = Fixture::new();
    fx.platform.lookups.set([FieldLookup::NoField]);
    let gate = Gate::closed();
    *fx.backend.refine_gate.lock().unwrap() = Some(gate.clone());
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.wait_for("the card to wait for the AI", || {
        let card = fx.ui.last();
        card.view == View::Card && card.phase == Phase::Refining
    });
    let waiting = fx.ui.last();
    assert_eq!(waiting.text, "hello world");
    assert_eq!(
        waiting.working.as_ref().map(|w| w.kind),
        Some(WorkKind::Editing)
    );
    gate.open();
    fx.wait_for("the result", || fx.ui.last().text == "Hello, world.");
    assert_eq!(fx.ui.last().phase, Phase::Review);

    // The API hears the recording again: the same, with the local text in the card meanwhile.
    let fx = Fixture::new();
    fx.platform.lookups.set([FieldLookup::NoField]);
    fx.uses_the_api(TranscribePreset::OpenAi);
    *fx.backend.transcribe_gate.lock().unwrap() = Some(Gate::closed());
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.wait_for("the card to wait for the API", || {
        let card = fx.ui.last();
        card.view == View::Card
            && card.working.as_ref().map(|w| w.kind) == Some(WorkKind::Retranscribing)
    });
    assert_eq!(fx.ui.last().text, "hello world");
}

#[test]
fn controller_password_field_never_pastes() {
    let fx = Fixture::new();
    // The platform layer answers `NoField` for a password field.
    fx.platform.lookups.set([FieldLookup::NoField]);
    fx.dictate_to_card();

    assert_eq!(fx.ui.last().view, View::Card);
    assert!(
        fx.keys.pastes.lock().unwrap().is_empty(),
        "send_paste was called"
    );
    assert!(fx.clipboard.writes.lock().unwrap().is_empty());
}

#[test]
fn controller_the_field_is_looked_at_and_read_after_the_app_is_in_front_and_before_the_paste() {
    let fx = Fixture::new();
    fx.dictate_to_chip();
    // The first thing that is asked of the system after the recording (the field was looked at when it began) is the app, to
    // come forward; then the look at the field that is in front, the reading of its text, and the paste, whose own wait for
    // the app finds it in front.
    let calls = fx.log.all();
    let forward = calls.iter().position(|c| *c == "forward").unwrap();
    let delivery = &calls[forward..];
    assert_eq!(delivery[..3], ["forward", "look", "snapshot"], "{calls:?}");
    let paste = delivery.iter().position(|c| *c == "paste").unwrap();
    assert!(paste > 2, "{calls:?}");
}

#[test]
fn controller_a_password_field_is_found_in_the_target_app_after_the_person_switched_apps() {
    let fx = Fixture::new();
    // A field when the dictation begins; then the person goes to another app while the text is made, and the field that the
    // target app has in front when it comes forward again is a password field.
    fx.platform.lookups.set([a_field(), FieldLookup::NoField]);
    fx.start_listening();
    fx.talk(1);
    fx.platform.set_front(Some(crate::platform::AppId {
        id: "com.apple.Safari".into(),
        pid: Some(77),
    }));
    fx.press();
    fx.wait_for_view(View::Card);
    fx.wait_for("the card", || fx.ui.last().phase == Phase::Review);

    assert!(fx.keys.pastes.lock().unwrap().is_empty());
    assert!(fx.clipboard.writes.lock().unwrap().is_empty());
    assert_eq!(error_key(&fx.ui.last()), Some("dictation.not_written"));
    assert_eq!(fx.ui.last().text, "Hello, world.");
}

#[test]
fn controller_the_text_of_a_card_is_not_pasted_into_a_password_field_either() {
    let fx = Fixture::new();
    fx.dictate_to_chip();
    fx.ctl.open_card(false);
    fx.wait_for_view(View::Card);
    // This app is in front while its card is up, and the field of the app behind it has become a password field.
    fx.platform.set_front(Some(crate::platform::AppId {
        id: "io.dictate".into(),
        pid: Some(OWN_PID),
    }));
    fx.platform.lookups.set([FieldLookup::NoField]);

    fx.ctl.card_text("Final words.");
    fx.ctl.paste_card("Final words.");
    fx.wait_for("the card again", || {
        fx.ui.last().view == View::Card && error_key(&fx.ui.last()) == Some("dictation.not_written")
    });
    assert_eq!(
        fx.keys.pastes.lock().unwrap().len(),
        1,
        "only the first paste was made"
    );
    assert_eq!(fx.ui.last().text, "Final words.");
}

#[test]
fn controller_the_text_of_a_card_is_read_after_the_app_is_in_front_not_while_the_card_is() {
    let fx = Fixture::new();
    fx.dictate_to_chip();
    fx.ctl.open_card(false);
    fx.wait_for_view(View::Card);
    fx.platform.set_front(Some(crate::platform::AppId {
        id: "io.dictate".into(),
        pid: Some(OWN_PID),
    }));
    let before = fx.log.all().len();

    fx.ctl.paste_card("Final words.");
    fx.run_until("the chip again", || {
        fx.chip_is_up() && fx.ui.last().text == "Final words."
    });
    let calls = fx.log.all()[before..].to_vec();
    assert_eq!(calls[..3], ["forward", "look", "snapshot"], "{calls:?}");
    assert!(calls.contains(&"paste"), "{calls:?}");
}

#[test]
fn controller_a_field_that_became_a_password_field_is_not_pasted_into() {
    let fx = Fixture::new();
    // A text field when the dictation starts, a password field when its text is ready.
    fx.platform.lookups.set([a_field(), FieldLookup::NoField]);
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.wait_for_view(View::Card);
    fx.wait_for("the card", || fx.ui.last().phase == Phase::Review);

    assert!(fx.keys.pastes.lock().unwrap().is_empty());
    assert!(fx.clipboard.writes.lock().unwrap().is_empty());
    assert_eq!(fx.ui.last().text, "Hello, world.");
}

#[test]
fn controller_quick_hold_waits_for_lookup() {
    let fx = Fixture::new();
    fx.change_settings(|s| s.push_to_talk = true);
    let gate = Gate::closed();
    *fx.platform.lookup_gate.lock().unwrap() = Some(gate.clone());

    // A tap of the key: pressed and let go before the field was found.
    fx.press();
    fx.wait_for("the microphone", || {
        fx.mic.starts.load(Ordering::SeqCst) > 0
    });
    fx.say(1.0);
    fx.release();
    fx.settle();
    assert!(
        fx.keys.pastes.lock().unwrap().is_empty(),
        "wrote before it knew where"
    );
    assert_eq!(fx.ui.last().phase, Phase::Finishing);

    gate.open();
    fx.run_until("the chip", || fx.chip_is_up());
    assert_eq!(*fx.clipboard.writes.lock().unwrap(), ["Hello, world."]);
}

#[test]
fn controller_hold_mode_ignores_the_press_while_recording_and_stops_on_release() {
    let fx = Fixture::new();
    fx.change_settings(|s| s.push_to_talk = true);
    fx.start_listening();
    fx.press();
    fx.settle();
    assert_eq!(fx.ui.last().phase, Phase::Recording);
    assert!(fx.ui.last().push_to_talk);
    fx.release();
    fx.wait_for("the end of the recording", || {
        fx.ui.last().phase != Phase::Recording
    });
}

#[test]
fn controller_the_press_that_stops_a_toggle_dictation_does_not_reset_the_shortcut() {
    // The key is still down at that moment: the gate knows it, and a reset would make the repeats of a held key presses.
    let fx = Fixture::new();
    // The AI never answers, so that nothing else ends the dictation (and resets) meanwhile.
    *fx.backend.refine_gate.lock().unwrap() = Some(Gate::closed());
    fx.start_listening();
    fx.talk(1);
    assert_eq!(fx.resets.load(Ordering::SeqCst), 0);
    fx.press();
    fx.wait_for("the AI to be editing", || fx.ui.last().working.is_some());
    assert_eq!(
        fx.resets.load(Ordering::SeqCst),
        0,
        "the press that stops reset the shortcut"
    );

    // Every other way of ending a recording does: the Stop button and Return of the pill, a release in hold mode.
    let fx = Fixture::new();
    *fx.backend.refine_gate.lock().unwrap() = Some(Gate::closed());
    fx.start_listening();
    fx.talk(1);
    fx.ctl.stop();
    assert_eq!(fx.resets.load(Ordering::SeqCst), 1);

    let fx = Fixture::new();
    fx.change_settings(|s| s.push_to_talk = true);
    *fx.backend.refine_gate.lock().unwrap() = Some(Gate::closed());
    fx.start_listening();
    fx.talk(1);
    fx.release();
    assert_eq!(fx.resets.load(Ordering::SeqCst), 1);
}

#[test]
fn controller_press_during_ai_writes_raw() {
    let fx = Fixture::new();
    let gate = Gate::closed();
    *fx.backend.refine_gate.lock().unwrap() = Some(gate);
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.wait_for("the AI to be editing", || {
        fx.ui
            .last()
            .working
            .is_some_and(|w| w.kind == WorkKind::Editing)
    });
    assert!(fx.keys.pastes.lock().unwrap().is_empty());

    // The shortcut again: write what was heard, as it is.
    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());

    assert_eq!(*fx.clipboard.writes.lock().unwrap(), ["hello world"]);
    let chip = fx.ui.last();
    assert_eq!((chip.original, chip.style, chip.notice), (None, None, None));
    assert_eq!(
        fx.backend.refines().len(),
        1,
        "the edit that was dropped was the only one"
    );
}

#[test]
fn controller_press_during_api_retranscription_writes_the_local_text_at_once() {
    let fx = Fixture::new();
    fx.uses_the_api(TranscribePreset::OpenAi);
    // The API never answers.
    *fx.backend.transcribe_gate.lock().unwrap() = Some(Gate::closed());
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.wait_for("the transcription to be sent again", || {
        fx.ui.last().working.is_some_and(|w| {
            w.kind == WorkKind::Retranscribing && w.model.as_deref() == Some("whisper-1")
        })
    });

    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());

    assert_eq!(*fx.clipboard.writes.lock().unwrap(), ["hello world"]);
    let chip = fx.ui.last();
    assert_eq!(
        chip.notice, None,
        "a call that was dropped on purpose is no failure"
    );
    assert!(
        fx.backend.refines().is_empty(),
        "no edit of a text that was asked to be written now"
    );
}

#[test]
fn controller_cancel_keeps_saved_audio_without_text() {
    let fx = Fixture::new();
    *fx.backend.refine_gate.lock().unwrap() = Some(Gate::closed());
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.wait_for("the AI to be editing", || fx.ui.last().working.is_some());

    fx.ctl.cancel();
    fx.wait_for("the panels to go", || fx.ui.last().view == View::Hidden);
    fx.wait_for("the work to end", || {
        fx.jobs.cancel_all_and_wait(Duration::ZERO)
    });

    let saved = fx.saved_files();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert!(
        saved[0].ends_with(".wav"),
        "the audio stays, the text is dropped: {saved:?}"
    );
    assert!(fx.clipboard.writes.lock().unwrap().is_empty());
    assert!(fx.keys.pastes.lock().unwrap().is_empty());
    assert_eq!(fx.ui.last().phase, Phase::Hidden);
    assert_eq!(fx.files_in(&fx.temp_dir()), Vec::<String>::new());
    assert!(fx.resets.load(Ordering::SeqCst) >= 1);
}

#[test]
fn controller_cancel_while_listening_keeps_the_audio_and_drops_the_text() {
    let fx = Fixture::new();
    fx.start_listening();
    fx.talk(1);

    fx.ctl.cancel();
    fx.wait_for("the panels to go", || fx.ui.last().view == View::Hidden);
    fx.wait_for("the microphone to stop", || {
        fx.mic.stopped.load(Ordering::SeqCst)
    });
    fx.wait_for("the audio to be kept", || fx.saved_files().len() == 1);

    assert!(fx.saved_files()[0].ends_with(".wav"));
    assert!(fx.clipboard.writes.lock().unwrap().is_empty());
}

#[test]
fn controller_a_local_dictation_that_is_not_saved_writes_no_audio_at_all() {
    // Local speech recognition works on what is in memory: a file would only be a recording left in the shared temporary
    // folder (until the next launch if the app is killed) for no one to need.
    let fx = Fixture::new();
    fx.change_settings(|s| s.save_recordings = false);
    *fx.backend.refine_gate.lock().unwrap() = Some(Gate::closed());
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.wait_for("the AI to be editing", || fx.ui.last().working.is_some());

    assert_eq!(fx.files_in(&fx.temp_dir()), Vec::<String>::new());
    assert_eq!(fx.saved_files(), Vec::<String>::new());

    fx.ctl.cancel();
    fx.wait_for("the panels to go", || fx.ui.last().view == View::Hidden);
}

#[test]
fn controller_the_speech_api_hears_a_temporary_file_when_the_recording_is_not_saved() {
    let fx = Fixture::new();
    fx.uses_the_api(TranscribePreset::OpenAi);
    fx.change_settings(|s| s.save_recordings = false);
    fx.dictate_to_chip();

    // The API was given a file to upload, and nothing of it is left once the text is written.
    let uploads = fx.backend.calls();
    let uploaded = uploads
        .iter()
        .find_map(|call| match call {
            BackendCall::Transcribe { wav } | BackendCall::TranscribeAndEdit { wav, .. } => {
                Some(wav.clone())
            }
            BackendCall::Refine { .. } => None,
        })
        .expect("the audio was uploaded");
    assert!(uploaded.starts_with(fx.temp_dir()), "{uploaded:?}");
    assert_eq!(fx.files_in(&fx.temp_dir()), Vec::<String>::new());
    assert_eq!(fx.saved_files(), Vec::<String>::new());
}

#[test]
fn controller_cancel_without_saving_leaves_no_audio_behind() {
    let fx = Fixture::new();
    fx.uses_the_api(TranscribePreset::OpenAi);
    fx.change_settings(|s| s.save_recordings = false);
    *fx.backend.refine_gate.lock().unwrap() = Some(Gate::closed());
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.wait_for("the AI to be editing", || fx.ui.last().working.is_some());
    // The recording is in a temporary file now, for the speech API.
    assert_eq!(fx.files_in(&fx.temp_dir()).len(), 1);

    fx.ctl.cancel();
    fx.wait_for("the temporary audio to go", || {
        fx.files_in(&fx.temp_dir()).is_empty()
    });
    assert!(fx.saved_files().is_empty());
}

#[test]
fn controller_a_dictation_that_is_not_saved_leaves_nothing_behind_not_even_its_text() {
    let fx = Fixture::new();
    fx.change_settings(|s| s.save_recordings = false);
    fx.dictate_to_chip();
    assert_eq!(fx.ui.last().text, "Hello, world.");

    // The audio is a temporary file that is deleted, and the transcript is written nowhere.
    assert_eq!(fx.files_in(&fx.temp_dir()), Vec::<String>::new());
    assert_eq!(fx.saved_files(), Vec::<String>::new());
}

#[test]
fn controller_empty_take_shows_4s_notice() {
    let fx = Fixture::new();
    fx.stt.say("");
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());

    let chip = fx.ui.last();
    assert_eq!(chip.notice, notice(NoticeKey::NothingHeard));
    assert_eq!(chip.text, "", "nothing to write, nothing to rewrite");
    assert!(fx.clipboard.writes.lock().unwrap().is_empty());
    assert!(fx.keys.pastes.lock().unwrap().is_empty());
    // The audio is there, and no text file for the nothing that was said.
    assert_eq!(fx.saved_files().len(), 1);

    // It stays for 4 seconds, not 10.
    fx.clock.advance(Duration::from_millis(3_900));
    fx.settle();
    assert!(fx.chip_is_up(), "gone before its time");
    fx.run_until("the chip to go", || fx.ui.last().view == View::Hidden);
    assert!(!fx.monitor.running.load(Ordering::SeqCst));
}

#[test]
fn controller_landing_failure_opens_card() {
    let fx = Fixture::new();
    // The field is the same after the paste as it was before, then and 400 ms later.
    fx.platform.script_snapshots([snapshot(5)]);
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.run_until("the card", || {
        fx.ui.last().view == View::Card && fx.ui.last().phase == Phase::Review
    });

    let card = fx.ui.last();
    assert_eq!(error_key(&card), Some("dictation.not_landed"));
    assert_eq!(card.text, "Hello, world.");
    assert_eq!(card.original.as_deref(), Some("hello world"));
    assert_eq!(*fx.keys.pastes.lock().unwrap(), [(false, false)]);
    let sleeps = fx.clock.sleeps();
    assert!(sleeps.contains(&Duration::from_millis(350)), "{sleeps:?}");
    assert!(sleeps.contains(&Duration::from_millis(400)), "{sleeps:?}");
    assert!(!fx.monitor.running.load(Ordering::SeqCst));
}

#[test]
fn controller_a_paste_that_lands_late_is_a_paste_that_landed() {
    let fx = Fixture::new();
    // Nothing after 350 ms, the text after 750.
    fx.platform
        .script_snapshots([snapshot(5), snapshot(5), snapshot(18)]);
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());
    assert_eq!(error_key(&fx.ui.last()), None);
}

#[test]
fn controller_landing_is_not_checked_where_it_cannot_be() {
    // Linux can't read the field, a terminal has no text to read, and Windows doesn't read a document of 50 000
    // characters: a paste there is taken as it is.
    let linux = Fixture::on(Os::Linux);
    linux.platform.script_snapshots([snapshot(5)]);
    linux.platform.lookups.set([FieldLookup::Unknown]);
    linux.start_listening();
    linux.talk(1);
    linux.press();
    linux.run_until("the chip on Linux", || linux.chip_is_up());

    let terminal = Fixture::new();
    terminal.platform.script_snapshots([snapshot(5)]);
    terminal
        .platform
        .lookups
        .set([FieldLookup::Field(FieldInfo {
            app: text_edit(),
            terminal: true,
            anchor: None,
        })]);
    terminal.start_listening();
    terminal.talk(1);
    terminal.press();
    terminal.run_until("the chip in a terminal", || terminal.chip_is_up());
    assert_eq!(*terminal.keys.pastes.lock().unwrap(), [(true, false)]);

    let big = Fixture::on(Os::Windows);
    big.platform.script_snapshots([None]);
    big.start_listening();
    big.talk(1);
    big.press();
    big.run_until("the chip on Windows", || big.chip_is_up());
}

#[test]
fn controller_a_failed_paste_opens_the_card_with_the_text_and_says_why() {
    let cases = [
        (PasteError::Elevated, "dictation.copied_elevated"),
        (
            PasteError::NoWaylandTool,
            "dictation.copied_no_wayland_tool",
        ),
        // The platform's own English words are not shown, and they are not the text of the card.
        (
            PasteError::Failed("CGEventPost failed".into()),
            "dictation.copied_only",
        ),
    ];
    for (error, key) in cases {
        let fx = Fixture::new();
        *fx.keys.paste_error.lock().unwrap() = Some(error.clone());
        fx.start_listening();
        fx.talk(1);
        fx.press();
        fx.wait_for_view(View::Card);
        fx.wait_for("the card", || fx.ui.last().phase == Phase::Review);
        let card = fx.ui.last();
        assert_eq!(error_key(&card), Some(key), "{error:?}");
        assert_eq!(card.error.as_ref().unwrap().detail, None, "{error:?}");
        assert_eq!(card.text, "Hello, world.");
    }

    // The app would not come forward: the same words as for any paste that didn't go through.
    let fx = Fixture::new();
    fx.keys.forward_ok.store(false, Ordering::SeqCst);
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.run_until("the card", || {
        fx.ui.last().view == View::Card && fx.ui.last().phase == Phase::Review
    });
    assert_eq!(error_key(&fx.ui.last()), Some("dictation.copied_only"));

    // The text could not even be put on the clipboard: "already copied" would be a lie.
    let fx = Fixture::new();
    fx.clipboard.fail_writes.store(true, Ordering::SeqCst);
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.wait_for_view(View::Card);
    fx.wait_for("the card", || fx.ui.last().phase == Phase::Review);
    assert_eq!(error_key(&fx.ui.last()), Some("dictation.not_copied"));
}

#[test]
fn controller_capture_error_keeps_audio() {
    let fx = Fixture::new();
    fx.start_listening();
    fx.talk(1);

    // The microphone is unplugged: the dictation ends as if the person had stopped it.
    fx.mic.send(CaptureEvent::Error(CaptureError::Stream(
        "device gone".into(),
    )));
    fx.run_until("the chip", || fx.chip_is_up());

    let chip = fx.ui.last();
    assert_eq!(error_key(&chip), Some("audio.stream"));
    assert_eq!(
        chip.error.as_ref().unwrap().detail.as_deref(),
        Some("device gone")
    );
    assert_eq!(
        chip.text, "Hello, world.",
        "what was said before is written"
    );
    assert!(fx.saved_files().iter().any(|n| n.ends_with(".wav")));
    assert!(fx.mic.stopped.load(Ordering::SeqCst));
}

#[test]
fn controller_silence_is_a_notice_and_the_recording_goes_on() {
    let fx = Fixture::new();
    fx.start_listening();
    fx.mic.send(CaptureEvent::Error(CaptureError::Silent));
    fx.wait_for("the notice", || {
        error_key(&fx.ui.last()) == Some("audio.silent")
    });

    // Still recording; the person decides.
    assert_eq!(fx.ui.last().phase, Phase::Recording);
    assert!(!fx.mic.stopped.load(Ordering::SeqCst));
    fx.talk(1);
    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());
    assert_eq!(fx.ui.last().text, "Hello, world.");
}

#[test]
fn controller_the_silence_notice_goes_when_the_microphone_is_heard_again() {
    let fx = Fixture::new();
    fx.stt.say("");
    fx.start_listening();
    fx.mic.send(CaptureEvent::Error(CaptureError::Silent));
    fx.wait_for("the notice", || {
        error_key(&fx.ui.last()) == Some("audio.silent")
    });

    // The person allowed the microphone, or unmuted it: sound comes in.
    fx.say(0.5);
    fx.wait_for("the notice to go", || fx.ui.last().error.is_none());
    assert_eq!(fx.ui.last().phase, Phase::Recording);

    // A take that was silent only at the start is not "silent" when it ends with nothing heard.
    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());
    assert_eq!(fx.ui.last().notice, notice(NoticeKey::NothingHeard));
    assert_eq!(fx.ui.last().error, None);
}

#[test]
fn controller_no_microphone_shows_card_and_a_microphone_that_will_not_start_says_so() {
    let fx = Fixture::new();
    *fx.mic.start_error.lock().unwrap() = Some(CaptureError::NoDevice);
    fx.press();
    fx.wait_for_view(View::Card);
    let card = fx.ui.last();
    assert_eq!(error_key(&card), Some("audio.no_device"));
    assert_eq!(card.phase, Phase::Review);
    assert_eq!(card.text, "");
    assert!(fx.keys.pastes.lock().unwrap().is_empty());

    let fx = Fixture::new();
    *fx.mic.start_error.lock().unwrap() = Some(CaptureError::Stream("busy".into()));
    fx.press();
    fx.wait_for_view(View::Card);
    assert_eq!(error_key(&fx.ui.last()), Some("audio.start_failed"));
    assert_eq!(fx.ui.last().error.unwrap().detail.as_deref(), Some("busy"));
}

#[test]
fn controller_microphone_permission_is_asked_for_and_respected() {
    // Not allowed: no recording, a card that says so.
    let fx = Fixture::new();
    *fx.platform.microphone.lock().unwrap() = Permission::Denied;
    fx.press();
    fx.wait_for_view(View::Card);
    assert_eq!(error_key(&fx.ui.last()), Some("audio.denied"));
    assert_eq!(fx.mic.starts.load(Ordering::SeqCst), 0);

    // Not asked yet, and the person says yes: the recording starts.
    let fx = Fixture::new();
    *fx.platform.microphone.lock().unwrap() = Permission::NotDetermined;
    fx.start_listening();
    assert_eq!(fx.platform.mic_requests.load(Ordering::SeqCst), 1);

    // ...and no: the card.
    let fx = Fixture::new();
    *fx.platform.microphone.lock().unwrap() = Permission::NotDetermined;
    fx.platform.mic_grant.store(false, Ordering::SeqCst);
    fx.press();
    fx.wait_for_view(View::Card);
    assert_eq!(error_key(&fx.ui.last()), Some("audio.denied"));
    assert_eq!(fx.mic.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn controller_long_take_live_ticks_skip_when_busy() {
    let fx = Fixture::new();
    let gate = Gate::closed();
    *fx.stt.gate.lock().unwrap() = Some(gate.clone());
    fx.start_listening();

    // The first live transcription takes longer than the ticks that follow it.
    fx.talk(3);
    fx.wait_for("the first job", || fx.stt.call_count() == 1);
    fx.settle();
    assert_eq!(
        fx.stt.call_count(),
        1,
        "a tick started a job while one was running"
    );

    gate.open();
    fx.talk_until("the next job", || fx.stt.call_count() == 2);
    // The later job has all the audio so far.
    let calls = fx.stt.calls.lock().unwrap().clone();
    assert!(calls[1].samples > calls[0].samples, "{calls:?}");
}

#[test]
fn controller_api_engine_never_uploads_when_engine_is_local() {
    let fx = Fixture::new();
    // Everything for the API is set up, but the engine is the local one.
    fx.flags
        .set(dictate_services::secrets::Account::Transcribe, true);
    assert_eq!(fx.settings.get().dictation_engine, Engine::Whisper);
    fx.dictate_to_chip();

    assert_eq!(fx.backend.uploads(), 0, "{:?}", fx.backend.calls());
    assert_eq!(fx.ui.last().text, "Hello, world.");
}

#[test]
fn controller_api_engine_hears_the_recording_again() {
    let fx = Fixture::new();
    fx.uses_the_api(TranscribePreset::OpenAi);
    fx.dictate_to_chip();

    let calls = fx.backend.calls();
    assert_eq!(fx.backend.uploads(), 1, "{calls:?}");
    let BackendCall::Transcribe { wav } = &calls[0] else {
        panic!("{calls:?}")
    };
    assert!(wav.is_file() || !wav.exists());
    // The API text is the one that is edited and written; the saved text is what the API heard.
    assert_eq!(
        fx.backend.refines(),
        [("hello from the api".to_string(), clean().to_string())]
    );
    assert_eq!(fx.ui.last().text, "Hello, world.");
    let txt = fx
        .saved_files()
        .into_iter()
        .find(|n| n.ends_with(".txt"))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(fx.saved_dir().join(txt)).unwrap(),
        "hello from the api"
    );
}

#[test]
fn controller_a_chat_model_transcribes_and_edits_in_one_call() {
    let fx = Fixture::new();
    fx.uses_the_api(TranscribePreset::OpenRouter);
    fx.dictate_to_chip();

    let calls = fx.backend.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(
        matches!(&calls[0], BackendCall::TranscribeAndEdit { instruction, .. } if instruction == clean())
    );
    let chip = fx.ui.last();
    assert_eq!(chip.text, "Hello from the API.");
    assert_eq!(chip.original.as_deref(), Some("hello world"));
    assert_eq!(chip.style.as_deref(), Some("clean"));
}

#[test]
fn controller_api_engine_without_a_key_stays_on_the_local_model() {
    let fx = Fixture::new();
    fx.change_settings(|s| s.dictation_engine = Engine::Api);
    fx.dictate_to_chip();
    assert_eq!(fx.backend.uploads(), 0);
}

#[test]
fn controller_api_engine_without_a_local_model_records_with_no_live_text() {
    let fx = Fixture::new();
    fx.uses_the_api(TranscribePreset::OpenAi);
    fx.stt.available.store(false, Ordering::SeqCst);
    fx.start_listening();
    fx.talk(2);
    assert_eq!(fx.stt.call_count(), 0, "no model to listen with");
    assert_eq!(fx.ui.last().partial, "");

    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());
    assert_eq!(fx.backend.uploads(), 1);
    assert_eq!(fx.ui.last().text, "Hello, world.");
    assert_eq!(fx.stt.call_count(), 0);
}

#[test]
fn controller_no_model_error() {
    // The local engine, and no model downloaded yet.
    let fx = Fixture::new();
    fx.stt.available.store(false, Ordering::SeqCst);
    fx.press();
    fx.wait_for_view(View::Card);

    let card = fx.ui.last();
    assert_eq!(error_key(&card), Some("dictation.no_model"));
    assert_eq!(
        card.error.as_ref().unwrap().action,
        Some(ErrorAction::OpenSettings)
    );
    assert_eq!(card.phase, Phase::Review);
    assert_eq!(
        fx.mic.starts.load(Ordering::SeqCst),
        0,
        "nothing is recorded for no one to hear"
    );

    fx.ctl.open_settings();
    assert_eq!(fx.ui.settings_opened.load(Ordering::SeqCst), 1);

    // The API as the engine, and it isn't set up either: the same.
    let fx = Fixture::new();
    fx.change_settings(|s| s.dictation_engine = Engine::Api);
    fx.stt.available.store(false, Ordering::SeqCst);
    fx.press();
    fx.wait_for_view(View::Card);
    assert_eq!(error_key(&fx.ui.last()), Some("dictation.no_model"));
}

// ---- what the AI does --------------------------------------------------------------------------------------------

#[test]
fn controller_a_failed_edit_writes_the_text_as_it_is_and_says_so() {
    let fx = Fixture::new();
    *fx.backend.refine.lock().unwrap() = Err("HTTP 500".into());
    fx.dictate_to_chip();
    let chip = fx.ui.last();
    assert_eq!(chip.text, "hello world");
    assert_eq!(
        chip.notice,
        Some(Notice {
            key: NoticeKey::EditFailed,
            detail: Some("HTTP 500".into())
        })
    );
}

#[test]
fn controller_no_ai_key_is_a_notice_on_the_chip_and_nothing_on_the_card() {
    let fx = Fixture::new();
    fx.flags.set(dictate_services::secrets::Account::Ai, false);
    fx.dictate_to_chip();
    assert_eq!(fx.ui.last().notice, notice(NoticeKey::NotEditedNoKey));
    assert_eq!(fx.ui.last().text, "hello world");
    assert!(fx.backend.refines().is_empty());

    // The card shows the text as it is: the card is where it can be edited by hand or by a style.
    let fx = Fixture::new();
    fx.flags.set(dictate_services::secrets::Account::Ai, false);
    fx.dictate_to_card();
    assert_eq!(fx.ui.last().notice, None);
    assert_eq!(fx.ui.last().text, "hello world");
}

#[test]
fn controller_card_with_nothing_heard_says_so() {
    let fx = Fixture::new();
    fx.stt.say("");
    fx.dictate_to_card();
    let card = fx.ui.last();
    assert_eq!(card.notice, notice(NoticeKey::NothingHeard));
    assert_eq!(card.text, "");
}

// ---- the chip ------------------------------------------------------------------------------------------------------

#[test]
fn controller_rewrite_pastes_fresh_text() {
    let fx = Fixture::new();
    fx.dictate_to_chip();
    *fx.backend.refine.lock().unwrap() = Ok("Hi, world.".into());

    fx.ctl.rewrite("concise").unwrap();
    fx.run_until("the new chip", || {
        fx.chip_is_up() && fx.ui.last().text == "Hi, world."
    });

    assert_eq!(
        fx.backend.refines().last().unwrap(),
        &(
            "Hello, world.".to_string(),
            find_edit_preset("concise").instruction.to_string()
        )
    );
    assert_eq!(
        *fx.clipboard.writes.lock().unwrap(),
        ["Hello, world.", "Hi, world."]
    );
    let chip = fx.ui.last();
    assert_eq!(chip.style.as_deref(), Some("concise"));
    assert_eq!(
        chip.original.as_deref(),
        Some("hello world"),
        "the first version stays the original"
    );
    // The monitor of the keyboard was off while our own keys went down.
    assert_eq!(
        *fx.keys.pastes.lock().unwrap(),
        [(false, false), (false, false)]
    );
    assert!(fx.monitor.running.load(Ordering::SeqCst));
    // The app of the field was brought forward again.
    assert!(fx
        .keys
        .forwarded
        .lock()
        .unwrap()
        .iter()
        .all(|app| *app == text_edit()));
    assert!(fx.keys.forwarded.lock().unwrap().len() >= 2);
    // While the new text was made, the bubble said what was going on.
    assert!(fx.ui.history().iter().any(|s| {
        s.phase == Phase::Working
            && s.working.as_ref().is_some_and(|w| {
                w.kind == WorkKind::Rewriting && w.style.as_deref() == Some("concise")
            })
    }));

    // A key of the person's goes down: the chip goes.
    fx.monitor.press_a_key();
    fx.wait_for("the chip to go", || fx.ui.last().view == View::Hidden);
}

#[test]
fn controller_the_chip_ignores_the_echo_of_our_own_paste() {
    // A terminal has no landing check, so that the chip is up as soon as the paste is sent, and the system may still be
    // reporting our own Ctrl+V to whoever listens.
    let fx = Fixture::new();
    fx.platform.lookups.set([FieldLookup::Field(FieldInfo {
        app: text_edit(),
        terminal: true,
        anchor: None,
    })]);
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());

    fx.monitor.press_a_key();
    fx.settle();
    assert!(fx.chip_is_up(), "our own paste dismissed the chip");

    // A key of the person's, a little later, is theirs.
    fx.clock.advance(Duration::from_secs(1));
    fx.monitor.press_a_key();
    fx.wait_for("the chip to go", || fx.ui.last().view == View::Hidden);
}

#[test]
fn controller_rewrite_that_fails_keeps_the_chip_and_says_so() {
    let fx = Fixture::new();
    fx.dictate_to_chip();
    *fx.backend.refine.lock().unwrap() = Err("quota".into());

    fx.ctl.rewrite("email").unwrap();
    fx.wait_for("the chip to come back", || {
        fx.chip_is_up() && fx.ui.last().notice.is_some()
    });
    let chip = fx.ui.last();
    assert_eq!(
        chip.text, "Hello, world.",
        "what is in the field is still what the chip says"
    );
    assert_eq!(chip.notice.unwrap().detail.as_deref(), Some("quota"));
    assert_eq!(fx.clipboard.writes.lock().unwrap().len(), 1);
}

#[test]
fn controller_rewrite_asks_for_a_style_that_exists() {
    let fx = Fixture::new();
    fx.dictate_to_chip();
    assert!(fx.ctl.rewrite("sarcastic").is_err());
    assert!(fx.chip_is_up());
}

#[test]
fn controller_press_during_rewrite_starts_a_new_dictation() {
    let fx = Fixture::new();
    fx.dictate_to_chip();
    *fx.backend.refine_gate.lock().unwrap() = Some(Gate::closed());
    fx.ctl.rewrite("formal").unwrap();
    fx.wait_for("the rewrite", || {
        fx.ui
            .last()
            .working
            .is_some_and(|w| w.kind == WorkKind::Rewriting)
    });

    fx.press();
    fx.wait_for("the new recording", || {
        fx.ui.last().phase == Phase::Recording && fx.mic.starts.load(Ordering::SeqCst) == 2
    });
    assert_eq!(
        fx.clipboard.writes.lock().unwrap().len(),
        1,
        "the rewrite was dropped"
    );
}

#[test]
fn controller_chip_goes_when_a_key_goes_down_elsewhere_or_another_app_comes_forward() {
    let fx = Fixture::new();
    fx.dictate_to_chip();
    fx.monitor.press_a_key();
    fx.wait_for("the chip to go", || fx.ui.last().view == View::Hidden);
    assert!(!fx.monitor.running.load(Ordering::SeqCst));
    assert_eq!(fx.ui.last().phase, Phase::Hidden);

    let fx = Fixture::new();
    fx.dictate_to_chip();
    fx.platform.set_front(Some(crate::platform::AppId {
        id: "com.apple.Safari".into(),
        pid: Some(77),
    }));
    fx.run_until("the chip to go", || fx.ui.last().view == View::Hidden);

    // This app in front (its own Settings window) is no reason.
    let fx = Fixture::new();
    fx.dictate_to_chip();
    fx.platform.set_front(Some(crate::platform::AppId {
        id: "io.dictate".into(),
        pid: Some(OWN_PID),
    }));
    fx.clock.advance(Duration::from_secs(1));
    fx.settle();
    assert!(fx.chip_is_up());
}

#[test]
fn controller_chip_stays_while_the_mouse_is_over_it_or_its_menu_is_open() {
    let fx = Fixture::new();
    fx.dictate_to_chip();

    fx.ctl.chip_hover(true);
    fx.clock.advance(Duration::from_secs(60));
    fx.settle();
    assert!(fx.chip_is_up(), "gone while the mouse was over it");

    // Open the menu, let the mouse go: the menu still holds it.
    fx.ctl.menu_open(true);
    fx.ctl.chip_hover(false);
    fx.clock.advance(Duration::from_secs(60));
    fx.settle();
    assert!(fx.chip_is_up(), "gone while its menu was open");

    fx.ctl.menu_open(false);
    fx.run_until("the chip to go", || fx.ui.last().view == View::Hidden);
}

#[test]
fn controller_a_mouse_over_the_bubble_is_not_a_mouse_over_the_chip() {
    let fx = Fixture::new();
    fx.start_listening();
    // The bubble and the chip are the same window: the mouse over the bubble must not hold the chip that comes later.
    fx.ctl.chip_hover(true);
    fx.talk(1);
    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());
    fx.clock.advance(Duration::from_secs(11));
    fx.run_until("the chip to go", || fx.ui.last().view == View::Hidden);
}

#[test]
fn controller_a_new_dictation_replaces_the_chip() {
    let fx = Fixture::new();
    fx.dictate_to_chip();
    fx.press();
    fx.wait_for("the new recording", || {
        fx.ui.last().phase == Phase::Recording
    });
    assert!(
        !fx.monitor.running.load(Ordering::SeqCst),
        "the chip is gone"
    );
}

// ---- the card ------------------------------------------------------------------------------------------------------

#[test]
fn controller_card_rewrites_the_text_in_its_box_and_undoes() {
    let fx = Fixture::new();
    fx.dictate_to_card();
    *fx.backend.refine.lock().unwrap() = Ok("Short.".into());

    // The person typed in the box first.
    fx.ctl.card_text("my own words");
    fx.ctl.rewrite("concise").unwrap();
    fx.wait_for("the new text", || fx.ui.last().text == "Short.");
    let card = fx.ui.last();
    assert_eq!(card.phase, Phase::Review);
    assert_eq!(card.style.as_deref(), Some("concise"));
    assert_eq!(
        card.original.as_deref(),
        Some("hello world"),
        "the first original stays"
    );
    assert_eq!(
        fx.backend.refines().last().unwrap(),
        &(
            "my own words".to_string(),
            find_edit_preset("concise").instruction.to_string()
        )
    );
    assert!(
        fx.keys.pastes.lock().unwrap().is_empty(),
        "the card writes into its own box"
    );
    // The AI took a moment, and the card said so.
    assert!(fx.ui.history().iter().any(|s| s.phase == Phase::Refining
        && s.working
            .as_ref()
            .is_some_and(|w| w.style.as_deref() == Some("concise"))));

    fx.ctl.undo();
    fx.wait_for("the original", || fx.ui.last().text == "hello world");
    assert_eq!(fx.ui.last().style, None);
    assert_eq!(fx.ui.last().original, None, "nothing left to undo");
}

#[test]
fn controller_card_custom_request_uses_its_words_as_the_title() {
    let fx = Fixture::new();
    fx.dictate_to_card();
    *fx.backend.refine.lock().unwrap() = Ok("Dear boss, ...".into());

    fx.ctl.card_text("hello world");
    fx.ctl.custom("rewrite for my boss");
    fx.wait_for("the new text", || fx.ui.last().text == "Dear boss, ...");
    assert_eq!(fx.ui.last().style.as_deref(), Some("rewrite for my boss"));
    assert_eq!(
        fx.backend.refines().last().unwrap(),
        &("hello world".to_string(), "rewrite for my boss".to_string())
    );
}

#[test]
fn controller_card_edit_that_fails_leaves_the_text_and_says_so() {
    let fx = Fixture::new();
    fx.dictate_to_card();
    *fx.backend.refine.lock().unwrap() = Err("quota".into());
    let before = fx.ui.last().text;
    fx.ctl.card_text("hello world");
    fx.ctl.rewrite("formal").unwrap();
    fx.wait_for("the failure", || {
        fx.ui.last().phase == Phase::Review && fx.ui.last().notice.is_some()
    });
    assert_eq!(
        fx.ui.last().text,
        before,
        "the page keeps what is in the box"
    );
    assert_eq!(fx.ui.last().notice.unwrap().key, NoticeKey::EditFailed);
}

#[test]
fn controller_card_copy_copies_closes_and_gives_the_focus_back() {
    let fx = Fixture::new();
    fx.dictate_to_card();
    fx.ctl.copy("the final words");

    assert_eq!(*fx.clipboard.copies.lock().unwrap(), ["the final words"]);
    fx.wait_for("the card to close", || fx.ui.last().view == View::Hidden);
    // The card made this app the active one: the app that was in front is asked to come forward again.
    fx.wait_for("the app to come forward", || {
        fx.keys.forwarded.lock().unwrap().contains(&text_edit())
    });
}

#[test]
fn controller_card_discard_gives_the_focus_back_and_drops_the_text() {
    let fx = Fixture::new();
    fx.dictate_to_card();
    fx.ctl.cancel();
    fx.wait_for("the card to close", || fx.ui.last().view == View::Hidden);
    fx.wait_for("the app to come forward", || {
        fx.keys.forwarded.lock().unwrap().contains(&text_edit())
    });
    assert!(fx.clipboard.copies.lock().unwrap().is_empty());
}

#[test]
fn controller_chip_edit_opens_the_card_and_pastes_from_it() {
    let fx = Fixture::new();
    fx.dictate_to_chip();

    fx.ctl.open_card(true);
    fx.wait_for_view(View::Card);
    let card = fx.ui.last();
    assert!(card.from_chip && card.can_write && card.open_custom);
    assert_eq!(card.text, "Hello, world.");
    assert_eq!(card.original.as_deref(), Some("hello world"));
    assert_eq!(card.phase, Phase::Review);
    assert!(
        !fx.monitor.running.load(Ordering::SeqCst),
        "no chip while the card is up"
    );

    fx.ctl.card_text("Final words.");
    fx.ctl.paste_card("Final words.");
    fx.run_until("the chip again", || {
        fx.chip_is_up() && fx.ui.last().text == "Final words."
    });
    assert_eq!(
        *fx.clipboard.writes.lock().unwrap(),
        ["Hello, world.", "Final words."]
    );
}

#[test]
fn controller_the_custom_box_of_the_card_is_asked_to_open_once() {
    let fx = Fixture::new();
    fx.dictate_to_chip();
    fx.ctl.open_card(true);
    fx.wait_for_view(View::Card);
    assert!(fx.ui.last().open_custom, "the card is told to open the box");

    // The next state is not another request to open it: the person may have closed it by then.
    *fx.backend.refine.lock().unwrap() = Ok("Short.".into());
    fx.ctl.card_text("Hello, world.");
    fx.ctl.rewrite("concise").unwrap();
    fx.wait_for("the new text", || fx.ui.last().text == "Short.");
    assert!(fx.ui.history().iter().rev().take(2).all(|s| !s.open_custom));
    assert!(!fx.ctl.state().open_custom);
}

#[test]
fn controller_chip_copy_copies_what_was_written() {
    let fx = Fixture::new();
    fx.dictate_to_chip();
    fx.ctl.copy("Hello, world.");
    assert_eq!(*fx.clipboard.copies.lock().unwrap(), ["Hello, world."]);
    fx.wait_for("the chip to go", || fx.ui.last().view == View::Hidden);
}

#[test]
fn controller_macos_without_accessibility_uses_the_card_and_offers_to_ask() {
    let fx = Fixture::new();
    *fx.platform.accessibility.lock().unwrap() = Permission::Denied;
    fx.start_listening();
    assert_eq!(fx.ui.last().view, View::Pill);
    assert_eq!(
        fx.platform.field_lookups.load(Ordering::SeqCst),
        0,
        "no look into another app without the permission"
    );
    fx.talk(1);
    fx.press();
    fx.wait_for_view(View::Card);
    fx.wait_for("the card", || fx.ui.last().phase == Phase::Review);
    assert!(fx.ui.last().accessibility_missing);

    fx.ctl.grant_accessibility();
    assert_eq!(fx.platform.accessibility_requests.load(Ordering::SeqCst), 1);
}

#[test]
fn controller_auto_write_off_uses_the_card() {
    let fx = Fixture::new();
    fx.change_settings(|s| s.auto_write = false);
    fx.start_listening();
    assert_eq!(fx.ui.last().view, View::Pill);
    assert_eq!(fx.platform.field_lookups.load(Ordering::SeqCst), 0);
    assert!(
        !fx.ui.last().accessibility_missing,
        "the setting is off, and that is not the permission's fault"
    );
}

#[test]
fn controller_dictate_in_front_uses_the_card() {
    let fx = Fixture::new();
    fx.platform.set_front(Some(crate::platform::AppId {
        id: "io.dictate".into(),
        pid: Some(OWN_PID),
    }));
    fx.start_listening();
    assert_eq!(fx.ui.last().view, View::Pill);
    assert_eq!(fx.platform.field_lookups.load(Ordering::SeqCst), 0);
}

#[test]
fn controller_unknown_field_is_a_card_on_mac_and_a_paste_on_linux() {
    let mac = Fixture::new();
    mac.platform.lookups.set([FieldLookup::Unknown]);
    mac.start_listening();
    assert_eq!(mac.ui.last().view, View::Pill);

    let linux = Fixture::on(Os::Linux);
    linux.platform.lookups.set([FieldLookup::Unknown]);
    linux.start_listening();
    assert_eq!(linux.ui.last().view, View::Bubble);
    // At the mouse.
    let anchor = linux
        .ui
        .anchors
        .lock()
        .unwrap()
        .last()
        .copied()
        .flatten()
        .unwrap();
    assert_eq!((anchor.x, anchor.y), (50.0, 60.0));
}

#[test]
fn controller_a_dictation_started_with_our_own_window_focused_goes_to_the_card_where_no_app_is_in_front(
) {
    // Wayland names no app in front: the focus of our own window (Settings) is all that says it is not a field to paste into.
    let fx = Fixture::on(Os::Linux);
    fx.platform.set_front(None);
    fx.platform.lookups.set([FieldLookup::Unknown]);
    fx.ui.own_focus.store(true, Ordering::SeqCst);
    fx.start_listening();
    assert_eq!(
        fx.ui.last().view,
        View::Pill,
        "no bubble at a field that is not one"
    );
    fx.talk(1);
    fx.press();
    fx.wait_for_view(View::Card);
    fx.wait_for("the card to be done", || {
        fx.ui.last().phase == Phase::Review
    });

    assert!(fx.keys.pastes.lock().unwrap().is_empty());
    assert_eq!(fx.ui.last().text, "Hello, world.");

    // The same dictation with no window of ours in focus is pasted, as before.
    let other = Fixture::on(Os::Linux);
    other.platform.set_front(None);
    other.platform.lookups.set([FieldLookup::Unknown]);
    other.start_listening();
    other.talk(1);
    other.press();
    other.run_until("the chip", || other.chip_is_up());
    assert_eq!(other.keys.pastes.lock().unwrap().len(), 1);
}

// ---- language, state, quitting -------------------------------------------------------------------------------------

#[test]
fn controller_language_change_is_saved_and_applies_to_the_text_that_follows() {
    let fx = Fixture::new();
    fx.change_settings(|s| s.dictation_language = "ja-JP".into());
    fx.start_listening();
    assert_eq!(fx.ui.last().language, "ja-JP");

    fx.ctl.set_language("vi-VN").unwrap();
    assert_eq!(fx.settings.get().dictation_language, "vi-VN");
    fx.wait_for("the new language", || fx.ui.last().language == "vi-VN");

    fx.talk(1);
    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());
    let calls = fx.stt.calls.lock().unwrap().clone();
    assert_eq!(
        calls.last().unwrap().language,
        "vi",
        "the final transcript is in the new language"
    );
}

#[test]
fn controller_language_is_a_language_code_and_nothing_else() {
    let fx = Fixture::new();
    for bad in [
        "",
        "english please",
        "../../etc",
        "en-US\n",
        &"x".repeat(40),
    ] {
        assert!(fx.ctl.set_language(bad).is_err(), "{bad:?}");
    }
    assert_eq!(fx.settings.get().dictation_language, "en-US");
    assert!(fx.ctl.set_language("zh-CN").is_ok());
    assert!(fx.ctl.set_language("yue").is_ok());
}

#[test]
fn controller_language_change_is_told_to_the_settings_window_after_it_was_saved() {
    // The Settings window keeps its own copy of the settings and would send the old language back with its next save.
    let fx = Fixture::new();
    assert!(fx.ctl.set_language("../x").is_err());
    assert!(fx.ui.languages.lock().unwrap().is_empty());

    fx.ctl.set_language("vi-VN").unwrap();

    assert_eq!(*fx.ui.languages.lock().unwrap(), ["vi-VN"]);
    assert_eq!(fx.settings.get().dictation_language, "vi-VN");
}

#[test]
fn controller_vocabulary_goes_to_local_speech_recognition() {
    let fx = Fixture::new();
    fx.change_settings(|s| s.vocabulary = "Dictate, Whisper".into());
    fx.start_listening();
    fx.talk(1);
    assert_eq!(fx.stt.calls.lock().unwrap()[0].prompt, "Dictate, Whisper");
    assert_eq!(fx.stt.calls.lock().unwrap()[0].language, "en");
}

#[test]
fn controller_state_is_there_for_a_page_that_loads_late() {
    let fx = Fixture::new();
    assert_eq!(fx.ctl.state().view, View::Hidden);
    fx.start_listening();
    assert_eq!(fx.ctl.state(), {
        let mut shown = fx.ui.last();
        shown.language = fx.ctl.state().language;
        shown
    });
    assert_eq!(fx.ctl.state().view, View::Bubble);
}

#[test]
fn controller_quitting_stops_the_dictation() {
    let fx = Fixture::new();
    *fx.backend.refine_gate.lock().unwrap() = Some(Gate::closed());
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.wait_for("the AI to be editing", || fx.ui.last().working.is_some());

    // What the app does when it quits.
    assert!(
        fx.jobs.cancel_all_and_wait(Duration::from_secs(3)),
        "the dictation did not stop"
    );
    assert!(fx.clipboard.writes.lock().unwrap().is_empty());
    assert!(fx.keys.pastes.lock().unwrap().is_empty());
}

#[test]
fn controller_toggle_from_the_command_line_is_a_press() {
    let fx = Fixture::new();
    fx.ctl.on_hotkey(HotkeyEvent::Toggle);
    fx.wait_for("the recording", || fx.ui.last().phase == Phase::Recording);
    fx.ctl.on_hotkey(HotkeyEvent::Toggle);
    fx.wait_for("the end of the recording", || {
        fx.ui.last().phase != Phase::Recording
    });
}

// ---- what goes wrong around it -------------------------------------------------------------------------------------

#[test]
fn controller_a_bug_in_the_work_does_not_leave_the_panels_up() {
    let fx = Fixture::new();
    // The final transcription panics, as a bug in the engine might.
    *crate::state::lock(&fx.stt.answer) = Box::new(|_| panic!("a bug in the engine"));
    fx.start_listening();
    fx.say(1.0);
    fx.press();
    fx.wait_for("the panels to go", || fx.ui.last().view == View::Hidden);
    assert_eq!(fx.ui.last().phase, Phase::Hidden);
    assert!(fx.keys.pastes.lock().unwrap().is_empty());
    // Nothing is stuck: the next dictation starts.
    *crate::state::lock(&fx.stt.answer) = Box::new(|_| Ok("hello world".to_string()));
    fx.press();
    fx.wait_for("the next recording", || {
        fx.mic.starts.load(Ordering::SeqCst) == 2
    });
}

#[test]
fn controller_launch_deletes_the_temporary_audio_an_earlier_run_left() {
    let fx = Fixture::with_leftovers(|temp| {
        std::fs::write(temp.join("dictate-1234-1.wav"), b"x").unwrap();
        std::fs::write(temp.join("notes.txt"), b"x").unwrap();
    });
    assert_eq!(fx.files_in(&fx.temp_dir()), ["notes.txt"]);
}

#[test]
fn controller_the_shortcut_while_the_microphone_prompt_is_up_ends_the_dictation_quietly() {
    let fx = Fixture::new();
    *fx.platform.microphone.lock().unwrap() = Permission::NotDetermined;
    // The system's prompt is open: the answer doesn't come.
    fx.platform.mic_answers.store(false, Ordering::SeqCst);
    fx.press();
    fx.wait_for("the prompt", || {
        fx.platform.mic_requests.load(Ordering::SeqCst) == 1
    });

    fx.press();
    fx.wait_for("the panels to go", || {
        fx.ui.last().view == View::Hidden && fx.ui.last().phase == Phase::Hidden
    });
    assert_eq!(fx.mic.starts.load(Ordering::SeqCst), 0);
    assert!(
        fx.jobs.cancel_all_and_wait(Duration::from_secs(2)),
        "the dictation is still waiting"
    );
}

#[test]
fn controller_a_new_dictation_from_the_card_gives_the_focus_back_and_writes_into_that_app() {
    let fx = Fixture::new();
    fx.dictate_to_card();
    // The card has made this app the one in front, as it does on macOS.
    fx.platform.set_front(Some(crate::platform::AppId {
        id: "io.dictate".into(),
        pid: Some(OWN_PID),
    }));
    fx.platform.lookups.set([a_field()]);

    fx.press();
    fx.wait_for("the new recording", || {
        fx.ui.last().phase == Phase::Recording && fx.ui.last().view == View::Bubble
    });
    assert!(
        fx.keys.forwarded.lock().unwrap().contains(&text_edit()),
        "the app that had the focus was not asked to take it back"
    );
}

#[test]
fn controller_a_tap_too_short_to_hear_is_nothing_heard_and_not_an_error() {
    let fx = Fixture::new();
    fx.start_listening();
    // 50 ms of sound: Whisper takes no such thing.
    fx.mic.send(CaptureEvent::Samples(vec![0.1; 800]));
    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());

    assert_eq!(fx.ui.last().notice, notice(NoticeKey::NothingHeard));
    assert_eq!(fx.stt.call_count(), 0, "nothing to transcribe");
    assert_eq!(fx.backend.uploads(), 0);
}

#[test]
fn controller_local_speech_recognition_that_fails_says_so_instead_of_nothing_heard() {
    let fx = Fixture::new();
    *crate::state::lock(&fx.stt.answer) =
        Box::new(|_| Err("Whisper does not know the language".into()));
    fx.start_listening();
    fx.say(1.0);
    fx.press();
    fx.wait_for_view(View::Card);
    fx.wait_for("the card", || fx.ui.last().phase == Phase::Review);

    let card = fx.ui.last();
    assert_eq!(error_key(&card), Some("dictation.local_failed"));
    assert_eq!(
        card.error.unwrap().detail.as_deref(),
        Some("Whisper does not know the language")
    );
    assert!(fx.keys.pastes.lock().unwrap().is_empty());
    // What was said is still kept.
    assert!(fx.saved_files().iter().any(|n| n.ends_with(".wav")));
}

#[test]
fn controller_a_bug_while_the_shortcut_is_handled_ends_that_dictation_and_the_shortcut_goes_on_working(
) {
    let fx = Fixture::new();
    // Showing the panels fails once, in the middle of the handling of the press that starts a dictation.
    fx.ui.panic_next.store(true, Ordering::SeqCst);
    fx.press();

    assert_eq!(fx.ui.last().view, View::Hidden);
    assert_eq!(fx.ui.last().phase, Phase::Hidden);
    assert!(
        fx.jobs.cancel_all_and_wait(Duration::from_secs(2)),
        "a dictation was left running"
    );
    assert_eq!(fx.mic.starts.load(Ordering::SeqCst), 0);

    // The next press starts a dictation as if nothing had happened.
    fx.start_listening();
    fx.talk(1);
    fx.press();
    fx.run_until("the chip", || fx.chip_is_up());
    assert_eq!(fx.ui.last().text, "Hello, world.");
}
