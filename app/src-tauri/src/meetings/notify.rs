//! What the app says on its own about meetings: the banner that tells a recording is done, the titles of the dialogs it
//! opens from Rust, and the message of a failure as a banner shows it.
//!
//! The words are built here, in the language of the UI as the tray menu is (`tray::language_for`), and are the same as the
//! UI's: they are in `app/ui/src/locales/{en,vi}.json` too, and a test keeps the two in step. The English is the right
//! side of R8 and the Vietnamese the left (the Swift app's own words).

use std::sync::Arc;

use dictate_services::meetings::{ErrorKind, MeetingError};
use dictate_services::settings::SettingsStore;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::tray::{self, Lang};

/// A recording's job has ended in a way that is worth a banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// The recording's id.
    pub id: String,
    /// The recording's title.
    pub title: String,
    pub kind: NoticeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeKind {
    /// The summary is written; how many decisions and action items it has.
    SummaryReady {
        decisions: usize,
        actions: usize,
    },
    /// The transcript is written and there is no AI to write a summary with.
    TranscriptOnly,
    Failed(MeetingError),
    /// Something about a recording that is being made or has just been (`recording.ended.*` and `recording.error.*`): the
    /// text of `key` with `detail` in it, in English as the program that said it did.
    Recording {
        key: String,
        detail: String,
    },
}

/// Says a notice to the person: the title and the body of the banner.
pub trait Notifier: Send + Sync {
    fn notify(&self, notice: Notice);
}

/// The banners of the app: a notification of the system, in the language the settings say now.
///
/// The system's notification has no way to tell the app that it was clicked (the plugin has none on the desktop), so "click
/// to view" is the system's own: it brings the app forward.
pub struct AppNotifier {
    pub app: AppHandle,
    pub settings: Arc<SettingsStore>,
}

impl Notifier for AppNotifier {
    fn notify(&self, notice: Notice) {
        let lang = tray::language_for(&self.settings.get());
        let (title, body) = banner(lang, &notice);
        if let Err(e) = self
            .app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
        {
            log::warn!("Could not show a notification: {e}");
        }
    }
}

/// Every text this file knows: its key (the same as in the locale files), the English and the Vietnamese. `{name}` is
/// where a value goes.
pub const TEXTS: &[(&str, &str, &str)] = &[
    // The banners.
    (
        "meetings.notify.title",
        "Dictate — {title}",
        "Dictate — {title}",
    ),
    (
        "meetings.notify.decisions",
        "{n} decisions",
        "{n} quyết định",
    ),
    ("meetings.notify.decision", "1 decision", "1 quyết định"),
    (
        "meetings.notify.actions",
        "{n} action items",
        "{n} việc cần làm",
    ),
    ("meetings.notify.action", "1 action item", "1 việc cần làm"),
    (
        "meetings.notify.summary_ready",
        "Summary ready",
        "Tóm tắt đã sẵn sàng",
    ),
    (
        "meetings.notify.view",
        "{what} — click to view",
        "{what} — bấm để xem",
    ),
    (
        "meetings.notify.transcript_only",
        "Transcript ready (not summarized — no API).",
        "Đã có transcript (chưa tóm tắt — thiếu API).",
    ),
    (
        "meetings.notify.error",
        "Error: {message}",
        "Lỗi: {message}",
    ),
    // The dialogs that Rust opens.
    (
        "meetings.dialog.pick_title",
        "Choose video/audio to transcribe and summarize",
        "Chọn video/audio để transcript và tóm tắt",
    ),
    (
        "meetings.dialog.filter_media",
        "Audio and video",
        "Âm thanh và video",
    ),
    ("meetings.dialog.filter_all", "All files", "Tất cả file"),
    ("meetings.dialog.export_title", "Export", "Xuất"),
    ("meetings.dialog.filter_markdown", "Markdown", "Markdown"),
    // Why a job failed (`MeetingError::key`).
    (
        "meetings.error.no_media",
        "No media file.",
        "Không có file media.",
    ),
    (
        "meetings.error.no_audio",
        "The file has no audio.",
        "File không có âm thanh.",
    ),
    (
        "meetings.error.needs_ffmpeg",
        "This format needs ffmpeg: {command}",
        "Định dạng này cần ffmpeg: {command}",
    ),
    (
        "meetings.error.ytdlp_missing",
        "yt-dlp isn't installed. Run: {command}",
        "Chưa cài yt-dlp. Chạy: {command}",
    ),
    ("meetings.error.ytdlp_failed", "{detail}", "{detail}"),
    (
        "meetings.error.empty_transcript",
        "Empty transcript — no speech was heard.",
        "Transcript rỗng — không nghe được lời nói nào.",
    ),
    (
        "meetings.error.no_model",
        "Download a Whisper model in Settings → Speech, or set up a transcription API.",
        "Tải model Whisper ở Cài đặt → Nhận dạng giọng nói, hoặc thiết lập API transcript.",
    ),
    (
        "meetings.error.transcript_api",
        "Transcription API failed: {detail}",
        "API transcript lỗi: {detail}",
    ),
    ("meetings.error.part_failed", "{detail}", "{detail}"),
    (
        "meetings.error.summary_failed",
        "Summary failed: {detail}",
        "Tóm tắt lỗi: {detail}",
    ),
    (
        "meetings.error.unfinished",
        "Not finished processing",
        "Chưa xử lý xong",
    ),
    ("meetings.error.cancelled", "Cancelled", "Đã hủy"),
    ("meetings.error.other", "{detail}", "{detail}"),
    (
        "meetings.error.generic",
        "Something went wrong.",
        "Có lỗi khi xử lý.",
    ),
    (
        "meetings.error.invalid_link",
        "Invalid link.",
        "Link không hợp lệ.",
    ),
    // What a command answers when it can't do what was asked (`views::key`).
    (
        "meetings.error.busy",
        "This recording is being processed. Try again when it is done.",
        "Bản ghi này đang được xử lý. Hãy thử lại khi xong.",
    ),
    (
        "meetings.error.no_ai",
        "API not set up in Settings.",
        "Chưa cấu hình API trong Cài đặt.",
    ),
    (
        "meetings.error.empty_question",
        "The question is empty.",
        "Câu hỏi đang trống.",
    ),
    (
        "meetings.error.no_transcript",
        "No transcript yet.",
        "Chưa có transcript.",
    ),
    // A recording that stopped by itself (`RecordingEnded`): the recording is kept and goes on to be processed.
    (
        "recording.ended.window_closed",
        "The window you were recording was closed. What was recorded is kept and is being processed.",
        "Cửa sổ đang quay đã đóng. Phần đã quay được giữ lại và đang được xử lý.",
    ),
    (
        "recording.ended.display_gone",
        "The screen you were recording is no longer available. What was recorded is kept and is being processed.",
        "Màn hình đang quay không còn khả dụng. Phần đã quay được giữ lại và đang được xử lý.",
    ),
    (
        "recording.ended.device_lost",
        "A device was lost and the recording stopped: {detail}. What was recorded is kept and is being processed.",
        "Mất một thiết bị nên bản ghi đã dừng: {detail}. Phần đã quay được giữ lại và đang được xử lý.",
    ),
    (
        "recording.ended.permission_revoked",
        "Permission to record was turned off, so the recording stopped. What was recorded is kept and is being processed.",
        "Quyền quay đã bị tắt nên bản ghi đã dừng. Phần đã quay được giữ lại và đang được xử lý.",
    ),
    (
        "recording.ended.failed",
        "The recording stopped: {detail}. What was recorded is kept and is being processed.",
        "Bản ghi đã dừng: {detail}. Phần đã quay được giữ lại và đang được xử lý.",
    ),
    // Why a recording could not begin (one for each `StartError`), and why it could not be made into a meeting.
    (
        "recording.error.nothing_to_record",
        "Turn on at least system audio or the microphone.",
        "Bật ít nhất âm thanh hệ thống hoặc micro.",
    ),
    (
        "recording.error.needs_screen_permission",
        "No Screen Recording permission. Turn on Dictate in System Settings → Privacy & Security → Screen & System Audio Recording, then reopen the app.",
        "Chưa có quyền Screen Recording. Bật cho Dictate trong System Settings → Privacy & Security → Screen & System Audio Recording, rồi mở lại app.",
    ),
    (
        "recording.error.needs_ffmpeg",
        "Recording the screen needs ffmpeg: {command}",
        "Quay màn hình cần ffmpeg: {command}",
    ),
    (
        "recording.error.no_video_encoder",
        "This ffmpeg has no video encoder that works on this computer. Install a full build: {command}",
        "ffmpeg này không có bộ mã hóa video chạy được trên máy này. Hãy cài bản đầy đủ: {command}",
    ),
    (
        "recording.error.portal_unavailable",
        "This desktop can't share the screen with apps.",
        "Máy tính này không cho ứng dụng chia sẻ màn hình.",
    ),
    (
        "recording.error.portal_cancelled",
        "Nothing was chosen to share, so the recording did not start.",
        "Chưa chọn gì để chia sẻ nên bản ghi chưa bắt đầu.",
    ),
    (
        "recording.error.window_gone",
        "That window is gone. Choose another window.",
        "Cửa sổ đó không còn nữa. Hãy chọn cửa sổ khác.",
    ),
    (
        "recording.error.source",
        "Couldn't start recording: {detail}",
        "Không bắt đầu quay được: {detail}",
    ),
    (
        "recording.error.io",
        "Couldn't write the recording's folder: {detail}",
        "Không ghi được thư mục của bản ghi: {detail}",
    ),
    (
        "recording.error.nothing_recorded",
        "Nothing was recorded, so there is nothing to summarize.",
        "Chưa quay được gì nên không có gì để tóm tắt.",
    ),
    (
        "recording.error.stop_failed",
        "Couldn't finish the recording: {detail}. What was recorded is kept, and Dictate tries again the next time it starts.",
        "Không hoàn tất được bản ghi: {detail}. Phần đã quay được giữ lại, và Dictate sẽ thử lại ở lần mở sau.",
    ),
    (
        "recording.error.discard_failed",
        "The recording was stopped, but its folder could not be removed: {detail}",
        "Đã dừng bản ghi nhưng không xóa được thư mục của nó: {detail}",
    ),
];

/// The text for `key` in `lang`, with each `{name}` filled from `vars`. A key that is not here is the key itself.
pub fn text(lang: Lang, key: &str, vars: &[(&str, &str)]) -> String {
    let Some((_, english, vietnamese)) = TEXTS.iter().find(|(known, _, _)| *known == key) else {
        return key.to_owned();
    };
    let mut text = match lang {
        Lang::En => *english,
        Lang::Vi => *vietnamese,
    }
    .to_owned();
    for (name, value) in vars {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// The message of a failure as a person reads it: the text of its key, with the detail and the command in it.
pub fn error_message(lang: Lang, error: &MeetingError) -> String {
    let command = match &error.kind {
        ErrorKind::MissingTool { command } | ErrorKind::Unsupported { command } => command.as_str(),
        _ => "",
    };
    let detail = error.detail.as_deref().unwrap_or_default();
    let message = text(lang, error.key, &[("detail", detail), ("command", command)]);
    // A message that is only its detail has nothing to say without one; one that ends in ": " lost it.
    let message = message.trim_end_matches([':', ' ']);
    if message.is_empty() {
        text(lang, "meetings.error.generic", &[])
    } else {
        message.to_owned()
    }
}

/// The title and the body of the banner for `notice`.
pub fn banner(lang: Lang, notice: &Notice) -> (String, String) {
    // A recording that has not begun has no title to name it by.
    let title = if notice.title.is_empty() && matches!(notice.kind, NoticeKind::Recording { .. }) {
        "Dictate".to_owned()
    } else {
        text(lang, "meetings.notify.title", &[("title", &notice.title)])
    };
    let body = match &notice.kind {
        NoticeKind::SummaryReady { decisions, actions } => {
            let counts: Vec<String> = [
                count(
                    lang,
                    *decisions,
                    "meetings.notify.decision",
                    "meetings.notify.decisions",
                ),
                count(
                    lang,
                    *actions,
                    "meetings.notify.action",
                    "meetings.notify.actions",
                ),
            ]
            .into_iter()
            .flatten()
            .collect();
            let what = if counts.is_empty() {
                text(lang, "meetings.notify.summary_ready", &[])
            } else {
                counts.join(" · ")
            };
            text(lang, "meetings.notify.view", &[("what", &what)])
        }
        NoticeKind::TranscriptOnly => text(lang, "meetings.notify.transcript_only", &[]),
        NoticeKind::Recording { key, detail } => {
            text(lang, key, &[("detail", detail), ("command", detail)])
        }
        NoticeKind::Failed(error) => text(
            lang,
            "meetings.notify.error",
            &[("message", &error_message(lang, error))],
        ),
    };
    (title, body)
}

/// "1 decision" or "3 decisions"; nothing for none.
fn count(lang: Lang, n: usize, one: &str, many: &str) -> Option<String> {
    match n {
        0 => None,
        1 => Some(text(lang, one, &[])),
        n => Some(text(lang, many, &[("n", &n.to_string())])),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use dictate_services::meetings::error::key;
    use serde_json::Value;

    use super::*;

    fn notice(kind: NoticeKind) -> Notice {
        Notice {
            id: "2026-11-09 10-00-00".into(),
            title: "Họp kế hoạch".into(),
            kind,
        }
    }

    #[test]
    fn a_banner_for_a_summary_counts_decisions_and_action_items() {
        let ready = |decisions, actions| notice(NoticeKind::SummaryReady { decisions, actions });
        let en = |n: Notice| banner(Lang::En, &n);
        let vi = |n: Notice| banner(Lang::Vi, &n);

        assert_eq!(
            en(ready(3, 2)),
            (
                "Dictate — Họp kế hoạch".to_owned(),
                "3 decisions · 2 action items — click to view".to_owned()
            )
        );
        assert_eq!(
            en(ready(1, 1)).1,
            "1 decision · 1 action item — click to view"
        );
        // What there is none of is left out, and with none at all the summary is just ready.
        assert_eq!(en(ready(0, 4)).1, "4 action items — click to view");
        assert_eq!(en(ready(2, 0)).1, "2 decisions — click to view");
        assert_eq!(en(ready(0, 0)).1, "Summary ready — click to view");

        assert_eq!(
            vi(ready(3, 2)),
            (
                "Dictate — Họp kế hoạch".to_owned(),
                "3 quyết định · 2 việc cần làm — bấm để xem".to_owned()
            )
        );
        assert_eq!(vi(ready(0, 0)).1, "Tóm tắt đã sẵn sàng — bấm để xem");
    }

    #[test]
    fn a_banner_for_a_transcript_without_a_summary_says_there_is_no_api() {
        let n = notice(NoticeKind::TranscriptOnly);
        assert_eq!(
            banner(Lang::En, &n).1,
            "Transcript ready (not summarized — no API)."
        );
        assert_eq!(
            banner(Lang::Vi, &n).1,
            "Đã có transcript (chưa tóm tắt — thiếu API)."
        );
    }

    #[test]
    fn a_banner_for_a_failure_says_error_and_the_message_in_the_language_of_the_ui() {
        let failed = |error| notice(NoticeKind::Failed(error));
        assert_eq!(
            banner(Lang::En, &failed(MeetingError::no_audio())).1,
            "Error: The file has no audio."
        );
        assert_eq!(
            banner(Lang::Vi, &failed(MeetingError::no_audio())).1,
            "Lỗi: File không có âm thanh."
        );
        // The detail is in English whatever the language, and is in the message.
        assert_eq!(
            banner(
                Lang::Vi,
                &failed(MeetingError::summary_failed("API error 401: bad key"))
            )
            .1,
            "Lỗi: Tóm tắt lỗi: API error 401: bad key"
        );
        assert_eq!(
            banner(
                Lang::En,
                &failed(MeetingError::summary_failed("API error 401: bad key"))
            )
            .1,
            "Error: Summary failed: API error 401: bad key"
        );
    }

    #[test]
    fn a_banner_for_a_recording_that_stopped_by_itself_says_why_in_the_language_of_the_ui() {
        let ended = |key: &str, detail: &str| {
            notice(NoticeKind::Recording {
                key: key.to_owned(),
                detail: detail.to_owned(),
            })
        };

        let (title, body) = banner(Lang::En, &ended("recording.ended.display_gone", ""));
        assert_eq!(title, "Dictate — Họp kế hoạch");
        // Not more than is known: on Linux a stream that breaks ends in this too.
        assert_eq!(
            body,
            "The screen you were recording is no longer available. What was recorded is kept and is being processed."
        );
        assert_eq!(
            banner(Lang::Vi, &ended("recording.ended.window_closed", "")).1,
            "Cửa sổ đang quay đã đóng. Phần đã quay được giữ lại và đang được xử lý."
        );
        // What the source said is in the text that has a place for it, as it said it.
        assert_eq!(
            banner(Lang::En, &ended("recording.ended.failed", "ffmpeg stopped (exit 1)")).1,
            "The recording stopped: ffmpeg stopped (exit 1). What was recorded is kept and is being processed."
        );
    }

    #[test]
    fn a_banner_for_a_recording_that_has_no_meeting_is_titled_dictate() {
        // A start that fails from the shortcut: there is no recording to name.
        let (title, body) = banner(
            Lang::En,
            &Notice {
                id: String::new(),
                title: String::new(),
                kind: NoticeKind::Recording {
                    key: "recording.error.needs_ffmpeg".to_owned(),
                    detail: String::new(),
                },
            },
        );

        assert_eq!(title, "Dictate");
        assert!(
            body.starts_with("Recording the screen needs ffmpeg"),
            "{body}"
        );
    }

    #[test]
    fn every_text_of_the_recording_has_words_in_both_languages() {
        let keys = [
            "recording.ended.window_closed",
            "recording.ended.display_gone",
            "recording.ended.device_lost",
            "recording.ended.permission_revoked",
            "recording.ended.failed",
            "recording.error.nothing_to_record",
            "recording.error.needs_screen_permission",
            "recording.error.needs_ffmpeg",
            "recording.error.no_video_encoder",
            "recording.error.portal_unavailable",
            "recording.error.portal_cancelled",
            "recording.error.window_gone",
            "recording.error.source",
            "recording.error.io",
            "recording.error.nothing_recorded",
            "recording.error.stop_failed",
            "recording.error.discard_failed",
        ];
        for key in keys {
            let entry = TEXTS.iter().find(|(known, _, _)| *known == key);
            let (_, en, vi) = entry.unwrap_or_else(|| panic!("no words for {key}"));
            assert!(!en.is_empty() && !vi.is_empty(), "{key}");
        }
    }

    #[test]
    fn a_message_has_its_command_and_falls_back_when_it_has_nothing_to_say() {
        let missing = MeetingError::from_link(dictate_services::media::ytdlp::LinkError::Spawn(
            "not found".into(),
        ));
        let message = error_message(Lang::En, &missing);
        assert!(
            message.starts_with("yt-dlp isn't installed. Run: "),
            "{message}"
        );
        assert!(message.len() > "yt-dlp isn't installed. Run: ".len());
        assert_eq!(error_message(Lang::En, &missing), missing.message());

        // An error of no kind in particular with no detail.
        let nothing = MeetingError::new(ErrorKind::Other, key::OTHER, None);
        assert_eq!(error_message(Lang::En, &nothing), "Something went wrong.");
        assert_eq!(error_message(Lang::Vi, &nothing), "Có lỗi khi xử lý.");
        let no_detail = MeetingError::new(ErrorKind::Api, key::SUMMARY_FAILED, None);
        assert_eq!(error_message(Lang::En, &no_detail), "Summary failed");
    }

    #[test]
    fn the_message_of_each_failure_is_the_english_of_the_log_when_the_ui_is_in_english() {
        // `MeetingError::message` is what `log.txt` has after "ERROR: ".
        let errors = [
            MeetingError::no_media(),
            MeetingError::no_audio(),
            MeetingError::no_model(),
            MeetingError::empty_transcript(),
            MeetingError::unfinished(),
            MeetingError::cancelled(),
            MeetingError::invalid_link(),
            MeetingError::other("It broke"),
            MeetingError::summary_failed("API error 401"),
            MeetingError::from_decode(dictate_services::media::decode::DecodeError::Unsupported {
                what: "ac3".into(),
                ffmpeg_hint: "brew install ffmpeg",
            }),
        ];
        for error in errors {
            assert_eq!(
                error_message(Lang::En, &error),
                error.message(),
                "{}",
                error.key
            );
        }
    }

    #[test]
    fn every_key_of_a_failure_has_words_in_both_languages() {
        let keys = [
            key::NO_MEDIA,
            key::NO_AUDIO,
            key::NEEDS_FFMPEG,
            key::YTDLP_MISSING,
            key::YTDLP_FAILED,
            key::EMPTY_TRANSCRIPT,
            key::NO_MODEL,
            key::TRANSCRIPT_API,
            key::PART_FAILED,
            key::SUMMARY_FAILED,
            key::UNFINISHED,
            key::CANCELLED,
            key::OTHER,
            key::INVALID_LINK,
        ];
        for key in keys {
            let entry = TEXTS.iter().find(|(known, _, _)| *known == key);
            let (_, en, vi) = entry.unwrap_or_else(|| panic!("no words for {key}"));
            assert!(!en.is_empty() && !vi.is_empty(), "{key}");
        }
        let unique: BTreeSet<_> = TEXTS.iter().map(|(key, _, _)| key).collect();
        assert_eq!(unique.len(), TEXTS.len(), "a key is there twice");
    }

    #[test]
    fn every_key_the_app_answers_with_has_words_in_both_languages() {
        use crate::meetings::views::key as app_key;
        for key in [
            app_key::BUSY,
            app_key::NO_AI,
            app_key::EMPTY_QUESTION,
            app_key::NO_TRANSCRIPT,
        ] {
            let entry = TEXTS.iter().find(|(known, _, _)| *known == key);
            let (_, en, vi) = entry.unwrap_or_else(|| panic!("no words for {key}"));
            assert!(!en.is_empty() && !vi.is_empty(), "{key}");
            assert!(key.starts_with("meetings.error."), "{key}");
        }
        // R8's words for the one it has.
        assert_eq!(
            text(Lang::En, app_key::NO_AI, &[]),
            "API not set up in Settings."
        );
        assert_eq!(
            text(Lang::Vi, app_key::NO_AI, &[]),
            "Chưa cấu hình API trong Cài đặt."
        );
        assert_eq!(
            text(Lang::En, app_key::NO_TRANSCRIPT, &[]),
            "No transcript yet."
        );
        assert_eq!(
            text(Lang::Vi, app_key::NO_TRANSCRIPT, &[]),
            "Chưa có transcript."
        );
    }

    /// The locale file of the UI as a map of keys to texts.
    fn locale(json: &str) -> serde_json::Map<String, Value> {
        match serde_json::from_str(json).expect("a locale file") {
            Value::Object(map) => map,
            other => panic!("{other}"),
        }
    }

    #[test]
    fn the_words_here_are_the_words_of_the_ui() {
        let en = locale(include_str!("../../../ui/src/locales/en.json"));
        let vi = locale(include_str!("../../../ui/src/locales/vi.json"));
        for (key, english, vietnamese) in TEXTS {
            assert_eq!(
                en.get(*key).and_then(Value::as_str),
                Some(*english),
                "en {key}"
            );
            assert_eq!(
                vi.get(*key).and_then(Value::as_str),
                Some(*vietnamese),
                "vi {key}"
            );
        }
    }
}
