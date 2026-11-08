//! Renaming the speakers of a recording ("Speaker 2" -> "chị Lan") in its transcript and summary, without AI.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::LazyLock;

use fancy_regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

use super::transcript::{grapheme_count, normalize_newlines, TranscriptLine, MAX_SPEAKER_LENGTH};

/// The longest name `TranscriptLine::parse` still reads as a speaker.
pub const MAX_LENGTH: usize = MAX_SPEAKER_LENGTH;

/// A line of at least this many characters is long enough to recognize a voice by.
const SAMPLE_LENGTH: usize = 40;

/// A character that can be part of a word.
const WORD: &str = r"[\p{L}\p{M}\p{N}_]";

/// A label speaker separation gives ("Speaker 2"), not a name someone typed.
static GENERIC_LABEL: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^Speaker \d+$").ok());

#[derive(Debug, Clone, PartialEq)]
pub struct Speaker {
    pub name: String,
    pub lines: usize,
    /// A line long enough to recognize the voice by, to play from.
    pub sample: TranscriptLine,
}

/// The transcript's speakers in the order they first talk.
pub fn speakers_in(transcript: &str) -> Vec<Speaker> {
    let mut groups: Vec<(String, Vec<TranscriptLine>)> = Vec::new();
    let mut position: HashMap<String, usize> = HashMap::new();
    for line in TranscriptLine::parse(transcript) {
        let Some(name) = line.speaker.clone() else {
            continue;
        };
        match position.get(&name) {
            Some(&i) => groups[i].1.push(line),
            None => {
                position.insert(name.clone(), groups.len());
                groups.push((name, vec![line]));
            }
        }
    }
    groups
        .into_iter()
        .filter_map(|(name, lines)| {
            let sample = lines
                .iter()
                .find(|line| grapheme_count(&line.text) >= SAMPLE_LENGTH)
                .or_else(|| longest(&lines))?
                .clone();
            Some(Speaker {
                name,
                lines: lines.len(),
                sample,
            })
        })
        .collect()
}

/// The line with the most characters, the first one of them when several have as many.
fn longest(lines: &[TranscriptLine]) -> Option<&TranscriptLine> {
    lines
        .iter()
        .enumerate()
        .max_by_key(|(i, line)| (grapheme_count(&line.text), Reverse(*i)))
        .map(|(_, line)| line)
}

/// A name as it can stand before ": " in a transcript line: one line, without ":" or "." (which would end it)
/// or "*" (which would break the summary's "**Owner**"), at most `MAX_LENGTH` characters, cut between words.
/// `None` when nothing is left.
pub fn clean(name: &str) -> Option<String> {
    let name: String = name
        .chars()
        .filter(|c| !matches!(c, ':' | '.' | '*'))
        .collect();
    let mut cleaned = String::new();
    for word in name.split_whitespace() {
        let next = if cleaned.is_empty() {
            word.graphemes(true).take(MAX_LENGTH).collect()
        } else {
            format!("{cleaned} {word}")
        };
        if grapheme_count(&next) > MAX_LENGTH {
            break;
        }
        cleaned = next;
    }
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The transcript with each speaker in `names` (old -> new) relabelled, all at once, so two speakers can swap
/// names or be merged under one. Only the labels change; other paragraphs are kept as they are.
pub fn rename_transcript(transcript: &str, names: &HashMap<String, String>) -> String {
    let transcript = normalize_newlines(transcript);
    transcript
        .split("\n\n")
        .map(|para| {
            let Some(line) = TranscriptLine::parse(para).into_iter().next() else {
                return para.to_string();
            };
            let new = line
                .speaker
                .as_ref()
                .and_then(|speaker| names.get(speaker))
                .filter(|new| !new.is_empty());
            match new {
                Some(new) => TranscriptLine {
                    speaker: Some(new.clone()),
                    ..line
                }
                .formatted(),
                None => para.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The summary (or any Markdown) with the speakers renamed, all at once. A generic label ("Speaker 2") is
/// replaced wherever it stands as a whole, in the same case ("Speaker 20" is another one). Any other name ("Lan",
/// "Bạn") can also be an ordinary word ("Hà Nội", "Việt Nam"), so it only changes as an action item owner
/// ("**Lan**"). A mention already inside its new name is left alone ("Speaker 1" in "Speaker 1 (Lan)").
pub fn rename_summary(summary: &str, names: &HashMap<String, String>) -> String {
    let names: HashMap<&str, &str> = names
        .iter()
        .filter(|(old, new)| !old.is_empty() && !new.is_empty() && old != new)
        .map(|(old, new)| (old.as_str(), new.as_str()))
        .collect();
    if names.is_empty() {
        return summary.to_string();
    }
    rename_mentions(summary, &names).unwrap_or_else(|| summary.to_string())
}

/// `None` when the mentions cannot be searched for: the summary is then better left as it is than half renamed.
fn rename_mentions(summary: &str, names: &HashMap<&str, &str>) -> Option<String> {
    let (generic, named): (Vec<&str>, Vec<&str>) =
        names.keys().copied().partition(|label| is_generic(label));
    let mut alternatives = Vec::new();
    if !generic.is_empty() {
        alternatives.push(format!("(?<!{WORD})(?:{})(?!{WORD})", either(&generic)));
    }
    if !named.is_empty() {
        alternatives.push(format!(r"(?<=\*\*)(?:{})(?=\*\*)", either(&named)));
    }
    let mut in_new_name: HashMap<&str, Vec<Range<usize>>> = HashMap::new();
    for new in names.values().copied().collect::<HashSet<_>>() {
        let pattern = format!("(?<!{WORD}){}(?!{WORD})", fancy_regex::escape(new));
        in_new_name.insert(new, mentions(&pattern, summary)?);
    }
    let mut out = String::with_capacity(summary.len());
    let mut at = 0;
    for mention in mentions(&alternatives.join("|"), summary)? {
        let Some(new) = summary.get(mention.clone()).and_then(|old| names.get(old)) else {
            continue;
        };
        let already_there = in_new_name.get(new).is_some_and(|ranges| {
            ranges
                .iter()
                .any(|r| r.start <= mention.start && mention.end <= r.end)
        });
        if already_there {
            continue;
        }
        out.push_str(summary.get(at..mention.start)?);
        out.push_str(new);
        at = mention.end;
    }
    out.push_str(summary.get(at..)?);
    Some(out)
}

/// The words as one regex alternative, the longest first, so a word is never cut short by another it starts.
fn either(words: &[&str]) -> String {
    let mut words = words.to_vec();
    words.sort_by(|a, b| {
        b.chars()
            .count()
            .cmp(&a.chars().count())
            .then_with(|| a.cmp(b))
    });
    words
        .iter()
        .map(|word| fancy_regex::escape(word))
        .collect::<Vec<_>>()
        .join("|")
}

/// Where `pattern` matches in `text`; `None` when it cannot be compiled or the search gives up.
fn mentions(pattern: &str, text: &str) -> Option<Vec<Range<usize>>> {
    let re = Regex::new(pattern).ok()?;
    re.find_iter(text)
        .map(|found| found.ok().map(|m| m.range()))
        .collect()
}

fn is_generic(label: &str) -> bool {
    GENERIC_LABEL
        .as_ref()
        .is_some_and(|re| re.is_match(label).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meetings::summary::SummaryDoc;

    const TRANSCRIPT: &str = r#"[00:00] Speaker 1: Chào mọi người.

[00:04] Speaker 2: Mình đề xuất tách repo test ra riêng, Speaker 2 trong file cũ là mình.

[00:20] Speaker 20: Mình đồng ý.

[00:31] Speaker 1: Vậy Henry tạo repo trước thứ Sáu nhé, mình sẽ gửi lại danh sách việc sau buổi họp.

Ghi chú không có mốc thời gian."#;

    fn names(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(old, new)| (old.to_string(), new.to_string()))
            .collect()
    }

    fn speaker_labels(transcript: &str) -> Vec<Option<String>> {
        TranscriptLine::parse(transcript)
            .into_iter()
            .map(|line| line.speaker)
            .collect()
    }

    fn labels(expected: &[Option<&str>]) -> Vec<Option<String>> {
        expected.iter().map(|s| s.map(str::to_string)).collect()
    }

    #[test]
    fn speakers_are_listed_in_order() {
        let speakers = speakers_in(TRANSCRIPT);
        assert_eq!(
            speakers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["Speaker 1", "Speaker 2", "Speaker 20"]
        );
        assert_eq!(
            speakers.iter().map(|s| s.lines).collect::<Vec<_>>(),
            [2, 1, 1]
        );
        // The first line long enough to recognize the voice by.
        assert_eq!(speakers[0].sample.stamp, "00:31");
        assert_eq!(speakers[2].sample.stamp, "00:20");
    }

    #[test]
    fn without_a_line_of_40_characters_the_sample_is_the_first_longest() {
        // "một hai ba" and "bốn năm ba" are both 10 characters; "bảy tám" is shorter.
        let speakers = speakers_in(
            "[00:01] Lan: bảy tám\n\n[00:02] Lan: một hai ba\n\n[00:03] Lan: bốn năm ba",
        );
        assert_eq!(speakers[0].lines, 3);
        assert_eq!(speakers[0].sample.stamp, "00:02");
    }

    #[test]
    fn a_long_line_is_measured_in_characters_not_bytes() {
        // 14 letters of 3 bytes each: 42 bytes, 14 characters.
        let speakers = speakers_in(&format!(
            "[00:01] Lan: {}\n\n[00:02] Lan: dài hơn một chút nhưng vẫn ngắn",
            "ệ".repeat(14)
        ));
        assert_eq!(speakers[0].sample.stamp, "00:02");
    }

    #[test]
    fn a_transcript_without_speakers_has_none() {
        assert!(speakers_in("[00:01] Không ai nói gì.\n\nGhi chú").is_empty());
        assert!(speakers_in("").is_empty());
    }

    #[test]
    fn transcript_labels_are_renamed() {
        let renamed = rename_transcript(
            TRANSCRIPT,
            &names(&[("Speaker 2", "chị Lan"), ("Speaker 1", "Henry")]),
        );
        assert_eq!(
            speaker_labels(&renamed),
            labels(&[
                Some("Henry"),
                Some("chị Lan"),
                Some("Speaker 20"),
                Some("Henry"),
                None
            ])
        );
        // Only labels change: the words people said stay, and so does a paragraph without a time.
        assert!(renamed.contains("Speaker 2 trong file cũ là mình."));
        assert!(renamed.ends_with("\n\nGhi chú không có mốc thời gian."));
        // Swapped and merged in one go.
        let swapped = rename_transcript(
            TRANSCRIPT,
            &names(&[
                ("Speaker 1", "Speaker 2"),
                ("Speaker 2", "Speaker 1"),
                ("Speaker 20", "Speaker 1"),
            ]),
        );
        assert_eq!(
            speaker_labels(&swapped),
            labels(&[
                Some("Speaker 2"),
                Some("Speaker 1"),
                Some("Speaker 1"),
                Some("Speaker 2"),
                None
            ])
        );
    }

    #[test]
    fn an_empty_new_name_leaves_the_label_alone() {
        let renamed = rename_transcript(TRANSCRIPT, &names(&[("Speaker 1", "")]));
        assert_eq!(renamed, TRANSCRIPT);
    }

    #[test]
    fn renaming_a_crlf_transcript_keeps_every_paragraph() {
        let crlf = TRANSCRIPT.replace('\n', "\r\n");
        let renamed = rename_transcript(&crlf, &names(&[("Speaker 1", "Henry")]));
        assert_eq!(
            speaker_labels(&renamed),
            labels(&[
                Some("Henry"),
                Some("Speaker 2"),
                Some("Speaker 20"),
                Some("Henry"),
                None
            ])
        );
        assert!(renamed.ends_with("\n\nGhi chú không có mốc thời gian."));
    }

    const SUMMARY: &str = r#"## Tóm tắt
Speaker 2 đề xuất tách repo; Speaker 20 đồng ý, speaker 2 viết thường giữ nguyên.
Lan sẽ gửi bản nháp cho Bạn trước khi ra Hà Nội.

## Việc cần làm
- [ ] **Speaker 1** — Tạo repo test (thứ Sáu)
- [ ] **Lan** — Gửi danh sách việc, chị Lan nhắc lại
- [ ] **Bạn** — Đặt phòng họp ở Việt Nam"#;

    #[test]
    fn summary_mentions_are_renamed() {
        let renamed = rename_summary(
            SUMMARY,
            &names(&[
                ("Speaker 1", "Henry"),
                ("Speaker 2", "Speaker 1"),
                ("Lan", "chị Lan"),
                ("Bạn", "Nam"),
                ("Nam", "anh Nam"),
            ]),
        );
        // Speaker labels change everywhere; names only as owners, so "Việt Nam" and "Bạn" in a sentence stay.
        assert_eq!(
            renamed,
            r#"## Tóm tắt
Speaker 1 đề xuất tách repo; Speaker 20 đồng ý, speaker 2 viết thường giữ nguyên.
Lan sẽ gửi bản nháp cho Bạn trước khi ra Hà Nội.

## Việc cần làm
- [ ] **Henry** — Tạo repo test (thứ Sáu)
- [ ] **chị Lan** — Gửi danh sách việc, chị Lan nhắc lại
- [ ] **Nam** — Đặt phòng họp ở Việt Nam"#
        );
        let owners: Vec<_> = SummaryDoc::parse(&renamed)
            .action_items()
            .into_iter()
            .map(|item| item.owner)
            .collect();
        assert_eq!(
            owners,
            [
                Some("Henry".to_string()),
                Some("chị Lan".to_string()),
                Some("Nam".to_string())
            ]
        );
        // A label already inside its new name isn't renamed twice.
        assert_eq!(
            rename_summary(
                "Speaker 1 (Lan) và Speaker 1",
                &names(&[("Speaker 1", "Speaker 1 (Lan)")])
            ),
            "Speaker 1 (Lan) và Speaker 1 (Lan)"
        );
    }

    #[test]
    fn a_label_is_only_renamed_as_a_whole_word() {
        let renamed = rename_summary(
            "Speaker 1, (Speaker 1) và Speaker 10, xSpeaker 1, Speaker 1x, Speaker 1.",
            &names(&[("Speaker 1", "Henry")]),
        );
        assert_eq!(
            renamed,
            "Henry, (Henry) và Speaker 10, xSpeaker 1, Speaker 1x, Henry."
        );
    }

    #[test]
    fn a_name_with_regex_characters_is_taken_literally() {
        let renamed = rename_summary(
            "Speaker 1 nói.\n- **Lan (PM)** — việc",
            &names(&[("Speaker 1", "A+B (C)"), ("Lan (PM)", "Lan [PM] $1")]),
        );
        assert_eq!(renamed, "A+B (C) nói.\n- **Lan [PM] $1** — việc");
    }

    #[test]
    fn summary_line_ends_are_kept() {
        let renamed = rename_summary(
            "Speaker 1 nói.\r\n- **Lan** — việc\r\n",
            &names(&[("Speaker 1", "Henry"), ("Lan", "chị Lan")]),
        );
        assert_eq!(renamed, "Henry nói.\r\n- **chị Lan** — việc\r\n");
    }

    #[test]
    fn nothing_to_rename_returns_the_summary_as_it_is() {
        assert_eq!(rename_summary(SUMMARY, &HashMap::new()), SUMMARY);
        assert_eq!(
            rename_summary(
                SUMMARY,
                &names(&[("Speaker 1", ""), ("", "Henry"), ("Lan", "Lan")])
            ),
            SUMMARY
        );
    }

    #[test]
    fn speaker_names_are_cleaned() {
        assert_eq!(
            clean("  TS. Nguyễn   Lan\n").as_deref(),
            Some("TS Nguyễn Lan")
        );
        assert_eq!(clean("Lan: **PM**").as_deref(), Some("Lan PM"));
        assert_eq!(clean(" .: "), None);
        // Long names are cut between words.
        let long = clean("Nguyễn Thị Phương Thảo Nguyên Anh").unwrap();
        assert_eq!(long, "Nguyễn Thị Phương Thảo");
        // Whatever is left still reads back as the speaker.
        let line = rename_transcript(
            "[01:02] Speaker 1: Xin chào.",
            &names(&[("Speaker 1", &long)]),
        );
        assert_eq!(
            TranscriptLine::parse(&line)[0].speaker.as_deref(),
            Some(long.as_str())
        );
    }

    #[test]
    fn the_longest_name_is_24_characters() {
        assert_eq!(MAX_LENGTH, 24);
        let name = "ab ".repeat(8) + "abcdefgh";
        assert_eq!(clean(&name).unwrap().chars().count(), 23);
        let exact = "abcdefghijklmnopqrstuvwx";
        assert_eq!(clean(exact).as_deref(), Some(exact));
    }

    #[test]
    fn one_word_longer_than_the_limit_is_cut() {
        assert_eq!(
            clean("abcdefghijklmnopqrstuvwxyz").as_deref(),
            Some("abcdefghijklmnopqrstuvwx")
        );
        assert_eq!(
            clean("abcdefghijklmnopqrstuvwxyz Lan").as_deref(),
            Some("abcdefghijklmnopqrstuvwx")
        );
    }

    #[test]
    fn a_name_is_measured_in_characters_a_reader_sees() {
        // 24 letters of "ế" written as "e" + two combining marks.
        let name = "e\u{302}\u{301}".repeat(24);
        assert_eq!(clean(&name).as_deref(), Some(name.as_str()));
    }

    #[test]
    fn any_kind_of_space_separates_the_words() {
        assert_eq!(
            clean("Lan\u{a0}Anh\tNguyễn\nPM").as_deref(),
            Some("Lan Anh Nguyễn PM")
        );
    }
}
