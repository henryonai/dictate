//! What the recorder tells the windows, and what it answers them with: the state of the recording, the choices of the
//! picker, and the errors as keys of the texts (`recording.error.*`) that the window translates.

use dictate_services::media::tools::{install_command, Tool};
use dictate_services::record::capture::SourceInfo;
use dictate_services::record::session::{Levels, TrackNote};
use dictate_services::record::{EndReason, Mode, RecordOptions, StartError};
use dictate_services::settings::Settings;
use serde::Serialize;

use crate::meetings::views::ErrorView;

/// Where a recording is.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    /// The sources are being started (a desktop may be asking the person what to share).
    Starting,
    Recording,
    Paused,
    /// The recording is being put together, or thrown away.
    Finishing,
}

/// The state of the recording, as the windows show it. The `recording-state` event carries one each time it changes, and at
/// most every 50 ms while it only goes on.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordingView {
    pub phase: Phase,
    /// How much of the recording there is, without the time it was paused.
    pub elapsed_ms: u64,
    pub levels: Levels,
    pub mode: Option<Mode>,
    /// The id of the meeting that the recording is becoming: its folder is in the library already.
    pub meeting_id: Option<String>,
    /// What the user should be told of the tracks, though the recording goes on (a microphone that hears only silence, a
    /// device that was lost): the latest note of each track.
    pub notes: Vec<TrackNote>,
}

impl RecordingView {
    pub fn idle() -> RecordingView {
        RecordingView {
            phase: Phase::Idle,
            elapsed_ms: 0,
            levels: Levels {
                mic: None,
                system: None,
            },
            mode: None,
            meeting_id: None,
            notes: Vec::new(),
        }
    }
}

/// Whether the screen can be recorded here, and if not, why: the picker disables the screen modes and says what to do.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ScreenAvailability {
    Ready,
    /// macOS: Screen Recording is not turned on for the app.
    NeedsPermission,
    NeedsFfmpeg,
    NoVideoEncoder,
    PortalUnavailable,
}

/// What the picker offers, and what it had the last time.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourcesView {
    pub displays: Vec<SourceInfo>,
    pub windows: Vec<SourceInfo>,
    /// The system asks the person what to share (the portal of Linux): the picker lists nothing.
    pub picker_is_the_system: bool,
    pub screen: ScreenAvailability,
    /// What installs ffmpeg with the usual package manager of this system.
    pub ffmpeg_command: String,
    pub last: RecordOptions,
}

/// The choices that settings keep, as options. The display and the window are not kept: a window comes back as no window.
pub fn options_from(settings: &Settings) -> RecordOptions {
    RecordOptions {
        mode: match settings.record_mode.as_str() {
            "window" => Mode::Window { id: String::new() },
            "audio" => Mode::Audio,
            _ => Mode::Screen { display: None },
        },
        quality: settings.record_quality,
        mic: settings.record_mic,
        system_audio: settings.record_system_audio,
    }
}

/// Puts the choices in `settings`: the kind of the mode, the quality and the two sounds.
pub fn remember(settings: &mut Settings, options: &RecordOptions) {
    settings.record_mode = match options.mode {
        Mode::Screen { .. } => "screen",
        Mode::Window { .. } => "window",
        Mode::Audio => "audio",
    }
    .to_owned();
    settings.record_quality = options.quality;
    settings.record_mic = options.mic;
    settings.record_system_audio = options.system_audio;
}

/// An error whose words are the text of `key`; what is behind it is `detail`, and `command` what the person can run.
pub fn error(kind: &str, key: &str, detail: Option<String>, command: Option<String>) -> ErrorView {
    ErrorView {
        kind: kind.to_owned(),
        key: key.to_owned(),
        detail,
        command,
    }
}

/// What installs `parec`, which Linux records the system's audio with: it is in the package `pulseaudio-utils`, which the
/// PulseAudio server of PipeWire comes with too. No other system needs it, so this is not one of the tools that
/// `install_command` is asked for.
const PAREC_INSTALL_COMMAND: &str = "sudo apt install pulseaudio-utils";

/// The key of the text for a recording that could not begin, and what goes in it.
pub fn start_error(error: StartError) -> ErrorView {
    let ffmpeg = || install_command(Tool::Ffmpeg).to_owned();
    match error {
        StartError::NothingToRecord => {
            self::error("Other", "recording.error.nothing_to_record", None, None)
        }
        // The window opens the pane of System Settings with this.
        StartError::NeedsScreenPermission => self::error(
            "Other",
            "recording.error.needs_screen_permission",
            None,
            Some("open_settings".to_owned()),
        ),
        StartError::NeedsFfmpeg => self::error(
            "MissingTool",
            "recording.error.needs_ffmpeg",
            None,
            Some(ffmpeg()),
        ),
        StartError::NeedsParec => self::error(
            "MissingTool",
            "recording.error.needs_parec",
            None,
            Some(PAREC_INSTALL_COMMAND.to_owned()),
        ),
        StartError::NoVideoEncoder => self::error(
            "MissingTool",
            "recording.error.no_video_encoder",
            None,
            Some(ffmpeg()),
        ),
        StartError::PortalUnavailable => {
            self::error("Other", "recording.error.portal_unavailable", None, None)
        }
        StartError::PortalCancelled => {
            self::error("Other", "recording.error.portal_cancelled", None, None)
        }
        StartError::WindowGone => self::error("Other", "recording.error.window_gone", None, None),
        StartError::Source(text) => {
            self::error("Other", "recording.error.source", Some(text), None)
        }
        StartError::Io(text) => self::error("Other", "recording.error.io", Some(text), None),
    }
}

/// The key of the text for a recording that stopped by itself, and what goes in it.
pub fn ended(reason: &EndReason) -> (&'static str, String) {
    match reason {
        EndReason::WindowClosed => ("recording.ended.window_closed", String::new()),
        EndReason::DisplayGone => ("recording.ended.display_gone", String::new()),
        EndReason::DeviceLost(why) => ("recording.ended.device_lost", why.clone()),
        EndReason::PermissionRevoked => ("recording.ended.permission_revoked", String::new()),
        EndReason::Failed(why) => ("recording.ended.failed", why.clone()),
    }
}

/// The title of a recording until its summary names it: "Screen recording 14:30 10/11" for a screen or a window, "Audio
/// recording 14:30 10/11" for sound alone, in the language of the interface. `when` is what `HH:mm dd/MM` shows.
pub fn title(lang: crate::tray::Lang, mode: &Mode, when: &str) -> String {
    use crate::tray::Lang;
    let audio = matches!(mode, Mode::Audio);
    let what = match (lang, audio) {
        (Lang::En, false) => "Screen recording",
        (Lang::En, true) => "Audio recording",
        (Lang::Vi, false) => "Ghi màn hình",
        (Lang::Vi, true) => "Ghi âm",
    };
    format!("{what} {when}")
}

/// The file a recording of `mode` will have, as far as is known when it begins: the real one is found out when it ends.
pub fn expected_media(mode: &Mode) -> &'static str {
    if matches!(mode, Mode::Audio) {
        "audio.m4a"
    } else {
        "recording.mp4"
    }
}

#[cfg(test)]
mod tests {
    use dictate_services::record::quality::Quality;
    use dictate_services::record::session::TrackNote;
    use dictate_services::record::{Note, Track};
    use serde_json::json;

    use super::*;
    use crate::tray::Lang;

    #[test]
    fn the_state_is_sent_in_the_words_the_window_reads() {
        let view = RecordingView {
            phase: Phase::Paused,
            elapsed_ms: 1_500,
            levels: Levels {
                mic: Some(0.25),
                system: None,
            },
            mode: Some(Mode::Window { id: "42".into() }),
            meeting_id: Some("2026-11-10 10-00-00".into()),
            notes: vec![TrackNote {
                track: Track::Mic,
                note: Note::Silent,
            }],
        };

        assert_eq!(
            serde_json::to_value(view).unwrap(),
            json!({
                "phase": "paused",
                "elapsedMs": 1_500,
                "levels": { "mic": 0.25, "system": null },
                "mode": { "kind": "window", "id": "42" },
                "meetingId": "2026-11-10 10-00-00",
                "notes": [{ "track": "mic", "note": { "kind": "silent" } }],
            })
        );
        assert_eq!(
            serde_json::to_value(RecordingView::idle()).unwrap(),
            json!({
                "phase": "idle",
                "elapsedMs": 0,
                "levels": { "mic": null, "system": null },
                "mode": null,
                "meetingId": null,
                "notes": [],
            })
        );
    }

    #[test]
    fn each_phase_and_availability_is_a_word_in_snake_case() {
        for (phase, word) in [
            (Phase::Idle, "idle"),
            (Phase::Starting, "starting"),
            (Phase::Recording, "recording"),
            (Phase::Paused, "paused"),
            (Phase::Finishing, "finishing"),
        ] {
            assert_eq!(serde_json::to_value(phase).unwrap(), word);
        }
        for (screen, word) in [
            (ScreenAvailability::Ready, "ready"),
            (ScreenAvailability::NeedsPermission, "needs_permission"),
            (ScreenAvailability::NeedsFfmpeg, "needs_ffmpeg"),
            (ScreenAvailability::NoVideoEncoder, "no_video_encoder"),
            (ScreenAvailability::PortalUnavailable, "portal_unavailable"),
        ] {
            assert_eq!(serde_json::to_value(screen).unwrap(), word);
        }
    }

    #[test]
    fn the_sources_are_sent_with_what_the_picker_had_last() {
        let view = SourcesView {
            displays: vec![SourceInfo {
                id: "1".into(),
                name: "Built-in".into(),
                app: None,
            }],
            windows: Vec::new(),
            picker_is_the_system: true,
            screen: ScreenAvailability::Ready,
            ffmpeg_command: "brew install ffmpeg".into(),
            last: RecordOptions {
                mode: Mode::Screen { display: None },
                quality: Quality::Standard,
                mic: true,
                system_audio: false,
            },
        };

        let sent = serde_json::to_value(view).unwrap();

        assert_eq!(sent["pickerIsTheSystem"], true);
        assert_eq!(sent["ffmpegCommand"], "brew install ffmpeg");
        assert_eq!(sent["screen"], "ready");
        assert_eq!(sent["displays"][0]["name"], "Built-in");
        assert_eq!(
            sent["last"]["mode"],
            json!({ "kind": "screen", "display": null })
        );
        assert_eq!(sent["last"]["quality"], "standard");
    }

    #[test]
    fn the_choices_come_back_from_the_settings_without_the_window() {
        let mut settings = Settings::default();
        remember(
            &mut settings,
            &RecordOptions {
                mode: Mode::Window { id: "42".into() },
                quality: Quality::Sharp,
                mic: false,
                system_audio: true,
            },
        );

        assert_eq!(settings.record_mode, "window");
        // What is kept is the kind of the mode: a window of the last time may be gone.
        assert_eq!(
            options_from(&settings),
            RecordOptions {
                mode: Mode::Window { id: String::new() },
                quality: Quality::Sharp,
                mic: false,
                system_audio: true,
            }
        );
        remember(
            &mut settings,
            &RecordOptions {
                mode: Mode::Screen {
                    display: Some("2".into()),
                },
                quality: Quality::Small,
                mic: true,
                system_audio: true,
            },
        );
        assert_eq!(options_from(&settings).mode, Mode::Screen { display: None });
        remember(
            &mut settings,
            &RecordOptions {
                mode: Mode::Audio,
                quality: Quality::Small,
                mic: true,
                system_audio: true,
            },
        );
        assert_eq!(options_from(&settings).mode, Mode::Audio);
        // A word that is none of the three (a file from another version) is the screen.
        settings.record_mode = "hologram".into();
        assert_eq!(options_from(&settings).mode, Mode::Screen { display: None });
    }

    #[test]
    fn a_refused_start_is_told_by_the_key_of_its_kind_with_what_is_behind_it() {
        let key = |error: StartError| start_error(error).key;
        assert_eq!(
            key(StartError::NothingToRecord),
            "recording.error.nothing_to_record"
        );
        assert_eq!(
            key(StartError::NeedsScreenPermission),
            "recording.error.needs_screen_permission"
        );
        assert_eq!(key(StartError::NeedsFfmpeg), "recording.error.needs_ffmpeg");
        assert_eq!(key(StartError::NeedsParec), "recording.error.needs_parec");
        assert_eq!(
            key(StartError::NoVideoEncoder),
            "recording.error.no_video_encoder"
        );
        assert_eq!(
            key(StartError::PortalUnavailable),
            "recording.error.portal_unavailable"
        );
        assert_eq!(
            key(StartError::PortalCancelled),
            "recording.error.portal_cancelled"
        );
        assert_eq!(key(StartError::WindowGone), "recording.error.window_gone");
        assert_eq!(
            key(StartError::Source("x".into())),
            "recording.error.source"
        );
        assert_eq!(key(StartError::Io("x".into())), "recording.error.io");

        // The page opens System Settings for a missing permission, and offers to copy the command for a missing tool.
        let permission = start_error(StartError::NeedsScreenPermission);
        assert_eq!(permission.command.as_deref(), Some("open_settings"));
        let ffmpeg = start_error(StartError::NeedsFfmpeg);
        assert_eq!(
            ffmpeg.command.as_deref(),
            Some(install_command(Tool::Ffmpeg))
        );
        assert_eq!(ffmpeg.kind, "MissingTool");
        // `parec` is what Linux records the system's audio with: the package that has it is named, like the one of ffmpeg.
        let parec = start_error(StartError::NeedsParec);
        assert_eq!(parec.kind, "MissingTool");
        assert_eq!(
            parec.command.as_deref(),
            Some("sudo apt install pulseaudio-utils")
        );
        assert_eq!(parec.detail, None);
        let source = start_error(StartError::Source("no display found".into()));
        assert_eq!(source.detail.as_deref(), Some("no display found"));
        assert_eq!(source.command, None);
    }

    #[test]
    fn a_recording_that_stopped_by_itself_is_told_by_the_key_of_the_reason() {
        assert_eq!(
            ended(&EndReason::WindowClosed),
            ("recording.ended.window_closed", String::new())
        );
        assert_eq!(
            ended(&EndReason::DisplayGone),
            ("recording.ended.display_gone", String::new())
        );
        assert_eq!(
            ended(&EndReason::PermissionRevoked),
            ("recording.ended.permission_revoked", String::new())
        );
        assert_eq!(
            ended(&EndReason::DeviceLost("unplugged".into())),
            ("recording.ended.device_lost", "unplugged".to_owned())
        );
        assert_eq!(
            ended(&EndReason::Failed("ffmpeg stopped".into())),
            ("recording.ended.failed", "ffmpeg stopped".to_owned())
        );
    }

    #[test]
    fn a_recording_is_titled_by_what_it_records_and_when_in_the_language_of_the_interface() {
        let screen = Mode::Screen { display: None };
        let window = Mode::Window { id: "1".into() };
        assert_eq!(
            title(Lang::En, &screen, "14:30 10/11"),
            "Screen recording 14:30 10/11"
        );
        assert_eq!(
            title(Lang::En, &window, "14:30 10/11"),
            "Screen recording 14:30 10/11"
        );
        assert_eq!(
            title(Lang::En, &Mode::Audio, "14:30 10/11"),
            "Audio recording 14:30 10/11"
        );
        assert_eq!(
            title(Lang::Vi, &screen, "14:30 10/11"),
            "Ghi màn hình 14:30 10/11"
        );
        assert_eq!(
            title(Lang::Vi, &Mode::Audio, "14:30 10/11"),
            "Ghi âm 14:30 10/11"
        );
    }

    #[test]
    fn the_file_a_recording_is_expected_to_have_depends_on_whether_it_has_video() {
        assert_eq!(expected_media(&Mode::Audio), "audio.m4a");
        assert_eq!(
            expected_media(&Mode::Screen { display: None }),
            "recording.mp4"
        );
        assert_eq!(
            expected_media(&Mode::Window { id: "1".into() }),
            "recording.mp4"
        );
    }
}
