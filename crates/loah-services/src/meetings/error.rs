//! Why a job failed, as the app shows it: a key for the translated text plus the detail behind it.

use std::fmt;

use crate::media::decode::DecodeError;
use crate::media::tools::{install_command, Tool};
use crate::media::ytdlp::LinkError;
use crate::transcribe_api::{LongApiError, TranscribeError};

/// The keys of the translated texts, which the UI looks up as they are.
pub mod key {
    pub const NO_MEDIA: &str = "meetings.error.no_media";
    pub const NO_AUDIO: &str = "meetings.error.no_audio";
    pub const NEEDS_FFMPEG: &str = "meetings.error.needs_ffmpeg";
    pub const YTDLP_MISSING: &str = "meetings.error.ytdlp_missing";
    pub const YTDLP_FAILED: &str = "meetings.error.ytdlp_failed";
    pub const EMPTY_TRANSCRIPT: &str = "meetings.error.empty_transcript";
    pub const NO_MODEL: &str = "meetings.error.no_model";
    pub const TRANSCRIPT_API: &str = "meetings.error.transcript_api";
    pub const PART_FAILED: &str = "meetings.error.part_failed";
    pub const SUMMARY_FAILED: &str = "meetings.error.summary_failed";
    pub const UNFINISHED: &str = "meetings.error.unfinished";
    pub const CANCELLED: &str = "meetings.error.cancelled";
    pub const OTHER: &str = "meetings.error.other";
    /// Not a failure of a run: the link that was pasted is not one. The app answers with it before it starts a job.
    pub const INVALID_LINK: &str = "meetings.error.invalid_link";
}

/// What kind of failure it is, which decides what the error card offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// An API (the AI model or the transcription API) failed: open the API settings, try the summary again.
    Api,
    /// A program is not installed: `command` installs it, to copy.
    MissingTool {
        command: String,
    },
    /// No Whisper model and no transcription API to transcribe with.
    NoModel,
    /// The file has no audio.
    NoAudio,
    /// There is no media file (never one, or it was moved or deleted).
    NoMedia,
    /// The file is in a format that needs ffmpeg, which is not installed: `command` installs it.
    Unsupported {
        command: String,
    },
    /// Nothing was heard.
    Empty,
    /// The app was closed (or crashed) before the recording was processed.
    Unfinished,
    Cancelled,
    Other,
}

/// A failure: `key` is the translated text, `detail` what is behind it, in English as the program that failed
/// said it. Both are shown; the English message of the log is `message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingError {
    pub kind: ErrorKind,
    pub key: &'static str,
    pub detail: Option<String>,
}

impl fmt::Display for MeetingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for MeetingError {}

/// How much of what a program or a server said goes into a detail, in characters. A provider's error page can run to
/// kilobytes, and one line of that size is more than `append_log` keeps (it drops the whole history).
pub const DETAIL_LIMIT: usize = 500;

/// `text` cut to at most `limit` characters (never in the middle of one), with `…` after what is left when
/// something was cut.
pub fn cap(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        None => text.to_owned(),
        Some((end, _)) => format!("{}…", &text[..end]),
    }
}

impl MeetingError {
    pub fn new(kind: ErrorKind, key: &'static str, detail: Option<String>) -> Self {
        MeetingError {
            kind,
            key,
            detail: detail.map(|text| cap(&text, DETAIL_LIMIT)),
        }
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, key::CANCELLED, None)
    }

    pub fn no_media() -> Self {
        Self::new(ErrorKind::NoMedia, key::NO_MEDIA, None)
    }

    pub fn no_audio() -> Self {
        Self::new(ErrorKind::NoAudio, key::NO_AUDIO, None)
    }

    pub fn no_model() -> Self {
        Self::new(ErrorKind::NoModel, key::NO_MODEL, None)
    }

    pub fn empty_transcript() -> Self {
        Self::new(ErrorKind::Empty, key::EMPTY_TRANSCRIPT, None)
    }

    pub fn unfinished() -> Self {
        Self::new(ErrorKind::Unfinished, key::UNFINISHED, None)
    }

    pub fn invalid_link() -> Self {
        Self::new(ErrorKind::Other, key::INVALID_LINK, None)
    }

    pub fn other(detail: impl Into<String>) -> Self {
        Self::new(ErrorKind::Other, key::OTHER, Some(detail.into()))
    }

    /// The AI model could not write the summary; `detail` is what it said (already without the key).
    pub fn summary_failed(detail: &str) -> Self {
        Self::new(ErrorKind::Api, key::SUMMARY_FAILED, Some(detail.to_owned()))
    }

    /// Why a link did not download. yt-dlp that cannot be started is taken for yt-dlp that is not installed.
    pub fn from_link(error: LinkError) -> Self {
        match error {
            LinkError::Cancelled => Self::cancelled(),
            LinkError::Spawn(detail) => Self::new(
                ErrorKind::MissingTool {
                    command: install_command(Tool::YtDlp).to_owned(),
                },
                key::YTDLP_MISSING,
                Some(detail),
            ),
            failed @ LinkError::Failed { .. } => Self::new(
                ErrorKind::Other,
                key::YTDLP_FAILED,
                Some(failed.to_string()),
            ),
        }
    }

    /// Why a media file could not be decoded.
    pub fn from_decode(error: DecodeError) -> Self {
        match error {
            DecodeError::Missing => Self::no_media(),
            DecodeError::NoAudio => Self::no_audio(),
            DecodeError::Unsupported { what, ffmpeg_hint } => Self::new(
                ErrorKind::Unsupported {
                    command: ffmpeg_hint.to_owned(),
                },
                key::NEEDS_FFMPEG,
                Some(what),
            ),
            other => Self::other(other.to_string()),
        }
    }

    /// Why the transcription API gave no transcript. The detail has the part it failed at, what it said, and how many
    /// finished parts a second run continues from: all in English, from the program that failed.
    pub fn from_api(error: LongApiError) -> Self {
        match error {
            LongApiError::Cancelled => Self::cancelled(),
            LongApiError::Io(_) => {
                Self::new(ErrorKind::Api, key::TRANSCRIPT_API, Some(error.to_string()))
            }
            LongApiError::PartFailed {
                index,
                count,
                kept,
                error,
            } => {
                let blocked = matches!(error, TranscribeError::Blocked { .. });
                let mut detail = format!(
                    "Part {index}/{count}: {}",
                    cap(&error.to_string(), DETAIL_LIMIT)
                );
                match kept {
                    0 => {}
                    1 => detail.push_str(&format!(
                        "\n1 finished part was kept; running it again continues from part {index}."
                    )),
                    _ => detail.push_str(&format!(
                        "\n{kept} finished parts were kept; running it again continues from part {index}."
                    )),
                }
                // The provider's own filter stopped the reply: only another model helps, so it has a text of its own.
                let key = if blocked {
                    key::TRANSCRIPT_API
                } else {
                    key::PART_FAILED
                };
                MeetingError {
                    kind: ErrorKind::Api,
                    key,
                    detail: Some(detail),
                }
            }
        }
    }

    /// The English message of the failure, as `log.txt` has it after "ERROR: ".
    pub fn message(&self) -> String {
        let detail = self.detail.as_deref();
        match (self.key, &self.kind) {
            (key::NO_MEDIA, _) => "No media file.".into(),
            (key::NO_AUDIO, _) => "The file has no audio.".into(),
            (key::NEEDS_FFMPEG, ErrorKind::Unsupported { command }) => {
                format!("This format needs ffmpeg: {command}")
            }
            (key::YTDLP_MISSING, ErrorKind::MissingTool { command }) => {
                format!("yt-dlp isn't installed. Run: {command}")
            }
            (key::EMPTY_TRANSCRIPT, _) => "Empty transcript — no speech was heard.".into(),
            (key::NO_MODEL, _) => {
                "Download a Whisper model in Settings → Speech, or set up a transcription API."
                    .into()
            }
            (key::TRANSCRIPT_API, _) => match detail {
                Some(detail) => format!("Transcription API failed: {detail}"),
                None => "Transcription API failed.".into(),
            },
            (key::SUMMARY_FAILED, _) => match detail {
                Some(detail) => format!("Summary failed: {detail}"),
                None => "Summary failed.".into(),
            },
            (key::UNFINISHED, _) => "Not finished processing".into(),
            (key::CANCELLED, _) => "Cancelled".into(),
            (key::INVALID_LINK, _) => "Invalid link.".into(),
            // The detail is the whole message: "Part 2/5: ..." and "yt-dlp failed (1): ...", or what the decoder said.
            _ => detail.unwrap_or("Something went wrong.").to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_cuts_on_a_character_boundary_and_says_so() {
        assert_eq!(cap("short", 500), "short");
        assert_eq!(cap("abc", 3), "abc");
        assert_eq!(cap("abcd", 3), "abc…");
        // Two and three bytes to a character: a cut by bytes would land inside one.
        assert_eq!(cap("éééé", 3), "ééé…");
        assert_eq!(cap("ệệệệ", 2), "ệệ…");
        assert_eq!(cap("", 0), "");
        assert_eq!(cap("é", 0), "…");
    }

    #[test]
    fn the_detail_of_an_error_is_capped() {
        let error = MeetingError::other("é".repeat(10_000));
        assert_eq!(error.detail.unwrap().chars().count(), DETAIL_LIMIT + 1);
    }

    #[test]
    fn messages_are_english() {
        let message = |error: MeetingError| error.message();
        assert_eq!(message(MeetingError::no_media()), "No media file.");
        assert_eq!(message(MeetingError::no_audio()), "The file has no audio.");
        assert_eq!(message(MeetingError::cancelled()), "Cancelled");
        assert_eq!(
            message(MeetingError::unfinished()),
            "Not finished processing"
        );
        assert_eq!(message(MeetingError::invalid_link()), "Invalid link.");
        assert_eq!(
            message(MeetingError::empty_transcript()),
            "Empty transcript — no speech was heard."
        );
        assert_eq!(
            message(MeetingError::no_model()),
            "Download a Whisper model in Settings → Speech, or set up a transcription API."
        );
        assert_eq!(
            message(MeetingError::summary_failed("API error 401: bad key")),
            "Summary failed: API error 401: bad key"
        );
        assert_eq!(message(MeetingError::other("It broke")), "It broke");
        assert_eq!(
            message(MeetingError::new(ErrorKind::Other, key::OTHER, None)),
            "Something went wrong."
        );
        assert_eq!(
            message(MeetingError::from_api(LongApiError::Io("disk full".into()))),
            "Transcription API failed: Could not prepare the audio for the API: disk full"
        );
    }

    #[test]
    fn every_key_is_under_meetings_error() {
        for key in [
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
        ] {
            assert!(key.starts_with("meetings.error."), "{key}");
        }
    }

    #[test]
    fn a_part_that_failed_names_the_part_the_error_and_what_was_kept() {
        let part = |kept| {
            MeetingError::from_api(LongApiError::PartFailed {
                index: 2,
                count: 5,
                kept,
                error: TranscribeError::Io("gone".into()),
            })
        };

        assert_eq!(
            part(0).detail.as_deref(),
            Some("Part 2/5: Could not read the audio file: gone")
        );
        assert_eq!(
            part(1).detail.as_deref(),
            Some("Part 2/5: Could not read the audio file: gone\n1 finished part was kept; running it again continues from part 2.")
        );
        assert_eq!(part(1).key, key::PART_FAILED);
        assert_eq!(part(1).message(), part(1).detail.unwrap());
    }
}
