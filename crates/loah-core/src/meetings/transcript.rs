//! A transcript as the app stores it: paragraphs of "[mm:ss] Speaker: text".

use std::borrow::Cow;
use std::sync::LazyLock;

use fancy_regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

use crate::text::timestamp;

/// The longest label a paragraph can start with and still be read as a speaker, in characters.
pub(crate) const MAX_SPEAKER_LENGTH: usize = 24;

/// The "[mm:ss]" or "[h:mm:ss]" a paragraph starts with, and the spaces after it.
static STAMP: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^\[((?:\d+:)?\d{1,2}:\d{2})\]\s*").ok());

/// One paragraph of a transcript: "[mm:ss] Speaker: text" (speaker and time are optional).
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptLine {
    pub id: usize,
    pub start: f64,
    pub stamp: String,
    pub speaker: Option<String>,
    pub text: String,
}

impl TranscriptLine {
    /// The paragraphs of `transcript` (separated by a blank line), without the empty ones. `id` is the
    /// paragraph's position among all of them. A paragraph without a time starts when the one before it did.
    pub fn parse(transcript: &str) -> Vec<TranscriptLine> {
        let transcript = normalize_newlines(transcript);
        let mut out: Vec<TranscriptLine> = Vec::new();
        for (id, para) in transcript.split("\n\n").enumerate() {
            let para = para.trim();
            if para.is_empty() {
                continue;
            }
            let Some((stamp, rest)) = split_stamp(para) else {
                let start = out.last().map_or(0.0, |line| line.start);
                out.push(TranscriptLine {
                    id,
                    start,
                    stamp: String::new(),
                    speaker: None,
                    text: para.to_string(),
                });
                continue;
            };
            let start = timestamp::parse(stamp).unwrap_or(0.0);
            let (speaker, text) = match rest.split_once(": ") {
                Some((label, text))
                    if grapheme_count(label) <= MAX_SPEAKER_LENGTH && !label.contains('.') =>
                {
                    (Some(label.to_string()), text)
                }
                _ => (None, rest),
            };
            out.push(TranscriptLine {
                id,
                start,
                stamp: stamp.to_string(),
                speaker,
                text: text.to_string(),
            });
        }
        out
    }

    /// The paragraph as it appears in the transcript file.
    pub fn formatted(&self) -> String {
        let stamp = if self.stamp.is_empty() {
            String::new()
        } else {
            format!("[{}] ", self.stamp)
        };
        let speaker = self
            .speaker
            .as_ref()
            .map_or(String::new(), |name| format!("{name}: "));
        format!("{stamp}{speaker}{}", self.text)
    }
}

/// The same text with "\r\n" line ends as "\n", which is what the parsers split on.
pub(crate) fn normalize_newlines(text: &str) -> Cow<'_, str> {
    if text.contains("\r\n") {
        Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(text)
    }
}

/// `"[01:05] text"` -> `("01:05", "text")`; `None` when `text` does not start with a time mark.
pub(crate) fn split_stamp(text: &str) -> Option<(&str, &str)> {
    let caps = STAMP.as_ref()?.captures(text).ok()??;
    let stamp = caps.get(1)?.as_str();
    let rest = text.get(caps.get(0)?.end()..)?;
    Some((stamp, rest))
}

/// How many characters a reader sees in `text`: a letter with combining marks counts once.
pub(crate) fn grapheme_count(text: &str) -> usize {
    text.graphemes(true).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixture::TRANSCRIPT;

    #[test]
    fn transcript_paragraphs() {
        let lines = TranscriptLine::parse(&format!(
            "{TRANSCRIPT}\n\nKhông có mốc thời gian.\n\n[1:00:00] Hết."
        ));
        assert_eq!(lines.len(), 5);
        assert!(lines[1].start == 65.0 && lines[1].stamp == "01:05");
        assert_eq!(lines[1].speaker.as_deref(), Some("Speaker 2"));
        assert_eq!(lines[1].text, "Mình đề xuất tách repo test ra riêng.");
        // inherits the time before it
        assert!(lines[3].stamp.is_empty() && lines[3].start == 130.0);
        assert!(lines[4].start == 3600.0 && lines[4].speaker.is_none());
        assert_eq!(
            lines[1].formatted(),
            "[01:05] Speaker 2: Mình đề xuất tách repo test ra riêng."
        );
    }

    #[test]
    fn transcript_crlf() {
        assert_eq!(
            TranscriptLine::parse(&TRANSCRIPT.replace('\n', "\r\n")).len(),
            3
        );
    }

    #[test]
    fn a_paragraph_before_any_time_starts_at_zero() {
        let lines = TranscriptLine::parse("Mở đầu.\n\n[00:09] Speaker 1: Chào.");
        assert_eq!(lines[0].start, 0.0);
        assert!(lines[0].stamp.is_empty() && lines[0].speaker.is_none());
        assert_eq!(lines[0].text, "Mở đầu.");
    }

    #[test]
    fn ids_count_the_blank_paragraphs_too() {
        let lines = TranscriptLine::parse("[00:01] a\n\n\n\n[00:02] b\n\n   \n\nc");
        assert_eq!(lines.iter().map(|l| l.id).collect::<Vec<_>>(), [0, 2, 4]);
    }

    #[test]
    fn whitespace_after_the_stamp_is_not_part_of_it() {
        let line = &TranscriptLine::parse("[01:05]\tSpeaker 2: Chào.")[0];
        assert_eq!(line.stamp, "01:05");
        assert_eq!(line.start, 65.0);
        assert_eq!(line.formatted(), "[01:05] Speaker 2: Chào.");
    }

    #[test]
    fn a_speaker_label_with_a_dot_is_part_of_the_text() {
        let line = &TranscriptLine::parse("[00:01] TS. Lan: Chào.")[0];
        assert_eq!(line.speaker, None);
        assert_eq!(line.text, "TS. Lan: Chào.");
    }

    #[test]
    fn a_speaker_label_is_at_most_24_characters() {
        let at_limit = "a".repeat(24);
        let line = &TranscriptLine::parse(&format!("[00:01] {at_limit}: Chào."))[0];
        assert_eq!(line.speaker.as_deref(), Some(at_limit.as_str()));
        let over = "a".repeat(25);
        let line = &TranscriptLine::parse(&format!("[00:01] {over}: Chào."))[0];
        assert_eq!(line.speaker, None);
    }

    #[test]
    fn a_speaker_label_counts_what_a_reader_sees_as_one_character() {
        // 24 letters, each "ế" written as "e" + two combining marks (72 code points).
        let name = "e\u{302}\u{301}".repeat(24);
        let line = &TranscriptLine::parse(&format!("[00:01] {name}: Chào."))[0];
        assert_eq!(line.speaker.as_deref(), Some(name.as_str()));
    }

    #[test]
    fn only_the_first_colon_can_end_the_speaker() {
        // The first ": " is too far in for a name, and the second one is not looked at.
        let text = "Họp xong rồi, ghi chú cho buổi sau: chốt lịch: Lan";
        let line = &TranscriptLine::parse(&format!("[00:01] {text}"))[0];
        assert_eq!(line.speaker, None);
        assert_eq!(line.text, text);
        let line = &TranscriptLine::parse("[00:01] Lan: chốt lịch: thứ Sáu")[0];
        assert_eq!(line.speaker.as_deref(), Some("Lan"));
        assert_eq!(line.text, "chốt lịch: thứ Sáu");
    }

    #[test]
    fn formatted_leaves_out_what_the_line_does_not_have() {
        let line = TranscriptLine {
            id: 0,
            start: 0.0,
            stamp: String::new(),
            speaker: None,
            text: "Ghi chú".into(),
        };
        assert_eq!(line.formatted(), "Ghi chú");
        let line = TranscriptLine {
            speaker: Some("Lan".into()),
            ..line
        };
        assert_eq!(line.formatted(), "Lan: Ghi chú");
    }
}
