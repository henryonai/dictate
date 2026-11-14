//! Clean-up for what Whisper writes when it runs on this machine.

use std::borrow::Cow;
use std::sync::LazyLock;

use fancy_regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

/// Whisper's special tokens (`<|vi|>`, `<|0.00|>`...) and silence tags (`[BLANK_AUDIO]`, `[MUSIC]`).
static SPECIAL: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"<\|[^|]*\|>|\[[A-Z_ ]+\]").ok());

/// Two or more whitespace characters.
static SPACES: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\s{2,}").ok());

/// Without Whisper's special tokens (`<|vi|>`, `<|0.00|>`...) and its silence tags (`[BLANK_AUDIO]`, `[MUSIC]`).
pub fn clean(text: &str) -> String {
    let without_tags = replace_all(&SPECIAL, text, "");
    replace_all(&SPACES, &without_tags, " ").trim().to_string()
}

/// `text` unchanged when the pattern did not compile or could not run.
fn replace_all(pattern: &Option<Regex>, text: &str, with: &str) -> String {
    pattern
        .as_ref()
        .and_then(|re| re.try_replacen(text, 0, with).ok())
        .map_or_else(|| text.to_string(), Cow::into_owned)
}

/// Whisper pads the end of each chunk with silence and now and then "hears" a closing line there
/// ("Cảm ơn các bạn đã theo dõi", "Thank you."). Such a line runs on past where the next one starts, or past
/// the end of the audio, which real speech never does. `items` are in time order; times are in seconds.
pub fn dropping_padding_lines<T>(
    items: Vec<T>,
    duration: f64,
    start: impl Fn(&T) -> f64,
    end: impl Fn(&T) -> f64,
) -> Vec<T> {
    let keep: Vec<bool> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let next = items.get(i + 1).map_or(duration, &start);
            // Written as "overruns", not `<=`: a time that is not a number keeps the line.
            let overruns = end(item) > next + 1.0;
            !overruns
        })
        .collect();
    items
        .into_iter()
        .zip(keep)
        .filter_map(|(item, keep)| keep.then_some(item))
        .collect()
}

/// A line or a word of a transcript; `speaker` is a number from speaker separation.
#[derive(Debug, Clone, PartialEq)]
pub struct Timed {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub speaker: Option<u32>,
}

/// Who speaks when, from speaker separation.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: u32,
}

/// Gives each word the speaker whose turn overlaps it most (the nearest turn within a second when none does,
/// or the previous word's speaker), and splits lines where the speaker changes, since one Whisper line can hold
/// a quick reply. Speakers are numbered 1, 2... in the order they first talk; with fewer than two, none is named.
/// A line without word timings counts as one word.
pub fn splitting_by_speaker(lines: &[(Timed, Vec<Timed>)], turns: &[Turn]) -> Vec<Timed> {
    let mut out: Vec<Timed> = Vec::new();
    let mut previous: Option<u32> = None;
    for (line, words) in lines {
        let words = if words.is_empty() {
            std::slice::from_ref(line)
        } else {
            words.as_slice()
        };
        let mut current: Option<Timed> = None;
        for word in words {
            let who = speaker_of(word, turns).or(previous);
            previous = who;
            if let Some(c) = current.as_mut().filter(|c| c.speaker == who) {
                c.text = joined(&c.text, &word.text);
                c.end = word.end;
            } else {
                out.extend(current.take());
                current = Some(Timed {
                    start: word.start,
                    end: word.end,
                    text: word.text.clone(),
                    speaker: who,
                });
            }
        }
        out.extend(current);
    }
    let mut out: Vec<Timed> = out
        .into_iter()
        .filter_map(|mut line| {
            line.text = line.text.trim().to_string();
            (!line.text.is_empty()).then_some(line)
        })
        .collect();
    let mut order: Vec<u32> = Vec::new();
    for speaker in out.iter().filter_map(|line| line.speaker) {
        if !order.contains(&speaker) {
            order.push(speaker);
        }
    }
    for line in &mut out {
        line.speaker = if order.len() < 2 {
            None
        } else {
            line.speaker
                .and_then(|s| order.iter().position(|&o| o == s))
                .and_then(|i| u32::try_from(i + 1).ok())
        };
    }
    out
}

/// The speaker whose turn overlaps `word` most; in a pause between turns, or for a word with no length,
/// the nearest turn if it is within a second.
fn speaker_of(word: &Timed, turns: &[Turn]) -> Option<u32> {
    let mut best: Option<(u32, f64)> = None;
    for turn in turns {
        let overlap = word.end.min(turn.end) - word.start.max(turn.start);
        if overlap > best.map_or(0.0, |(_, o)| o) {
            best = Some((turn.speaker, overlap));
        }
    }
    if let Some((speaker, _)) = best {
        return Some(speaker);
    }
    let mut nearest: Option<(u32, f64)> = None;
    for turn in turns {
        let gap = (turn.start - word.end).max(word.start - turn.end).max(0.0);
        if nearest.is_none_or(|(_, g)| gap < g) {
            nearest = Some((turn.speaker, gap));
        }
    }
    nearest.and_then(|(speaker, gap)| (gap <= 1.0).then_some(speaker))
}

/// Whisper's words carry their leading space (" họp"); add one only where two words would run together.
/// Chinese, Japanese, Thai... don't put spaces between words, and their words come without one.
fn joined(a: &str, b: &str) -> String {
    let last = a.graphemes(true).next_back().and_then(|g| g.chars().next());
    let first = b.graphemes(true).next().and_then(|g| g.chars().next());
    match (last, first) {
        (Some(last), Some(first))
            if is_word(last)
                && is_word(first)
                && !written_without_spaces(last)
                && !written_without_spaces(first) =>
        {
            format!("{a} {b}")
        }
        _ => format!("{a}{b}"),
    }
}

/// The first code point of a character is a letter or a digit.
fn is_word(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric()
}

fn written_without_spaces(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0E00..=0x0EFF // Thai, Lao
        | 0x1000..=0x109F // Myanmar
        | 0x1780..=0x17FF // Khmer
        | 0x3040..=0x30FF // Hiragana, Katakana
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3134F // Chinese characters
        | 0xFF66..=0xFF9F // half-width Katakana
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed(start: f64, end: f64, text: &str) -> Timed {
        Timed {
            start,
            end,
            text: text.to_string(),
            speaker: None,
        }
    }

    fn named(start: f64, end: f64, text: &str, speaker: u32) -> Timed {
        Timed {
            speaker: Some(speaker),
            ..timed(start, end, text)
        }
    }

    fn turn(start: f64, end: f64, speaker: u32) -> Turn {
        Turn {
            start,
            end,
            speaker,
        }
    }

    #[test]
    fn clean_drops_tokens_and_silence_tags() {
        assert_eq!(
            clean("<|vi|><|0.00|> Chào mọi người. [BLANK_AUDIO] Bắt đầu nhé.<|7.00|>"),
            "Chào mọi người. Bắt đầu nhé."
        );
        assert_eq!(clean(" [MUSIC] "), "");
    }

    #[test]
    fn clean_keeps_brackets_in_real_speech() {
        assert_eq!(clean("Mở file [bản nháp] nhé"), "Mở file [bản nháp] nhé");
        // Only all-capital tags are silence tags.
        assert_eq!(clean("[Music] [MUSIC] ok"), "[Music] ok");
    }

    #[test]
    fn clean_collapses_runs_of_whitespace_but_not_a_lone_newline() {
        assert_eq!(clean("a \n b\u{a0}\u{a0}c"), "a b c");
        assert_eq!(clean("a\nb"), "a\nb");
        // Vertical tab and NEL count as whitespace too (Unicode `White_Space`, as `\s` is in ICU).
        assert_eq!(clean("a\u{b}\u{85}b"), "a b");
        assert_eq!(clean("a\u{2003}\u{b}b"), "a b");
    }

    #[test]
    fn padding_lines_are_dropped() {
        // From a 5-minute meeting: each chunk ended with a 30-second line Whisper heard in the padding.
        let segments = vec![
            (
                12.5,
                18.5,
                "Mục tiêu của buổi họp là chốt danh sách tính năng.",
            ),
            (18.5, 48.5, "Cảm ơn các bạn đã theo dõi và hẹn gặp lại."),
            (23.9, 31.9, "Đầu tiên là phần đọc chính tả."),
            (37.2, 47.2, "Chúng ta đã chuyển sang dùng model mới."),
            (47.2, 49.0, "Ngay sau đó."),
            (291.8, 321.8, "Cảm ơn các bạn đã theo dõi."),
            (297.3, 304.0, "Cuối cùng, mọi người nhớ cập nhật tiến độ."),
            (309.4, 313.1, "Cảm ơn mọi người, buổi họp kết thúc tại đây."),
        ];
        let kept: Vec<&str> = dropping_padding_lines(segments, 313.2, |s| s.0, |s| s.1)
            .into_iter()
            .map(|s| s.2)
            .collect();
        assert_eq!(
            kept,
            [
                "Mục tiêu của buổi họp là chốt danh sách tính năng.",
                "Đầu tiên là phần đọc chính tả.",
                "Chúng ta đã chuyển sang dùng model mới.",
                "Ngay sau đó.",
                "Cuối cùng, mọi người nhớ cập nhật tiến độ.",
                "Cảm ơn mọi người, buổi họp kết thúc tại đây.",
            ]
        );
    }

    #[test]
    fn a_line_running_past_the_end_of_the_audio_is_padding() {
        let kept = dropping_padding_lines(vec![(0.0, 5.0), (5.0, 35.0)], 9.0, |s| s.0, |s| s.1);
        assert_eq!(kept, [(0.0, 5.0)]);
    }

    #[test]
    fn a_line_overrunning_the_next_by_exactly_a_second_stays() {
        let kept = dropping_padding_lines(vec![(0.0, 6.0), (5.0, 7.0)], 7.0, |s| s.0, |s| s.1);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn whisper_lines_are_split_by_speaker() {
        let turns = [turn(0.0, 4.0, 7), turn(4.2, 6.0, 3), turn(6.5, 9.0, 7)];
        // The first line holds a question and its quick answer; the second has no word timings.
        let lines = [
            (
                timed(0.0, 6.0, "Xong chưa? Xong rồi."),
                vec![
                    timed(0.0, 1.5, " Xong"),
                    timed(1.5, 3.8, " chưa?"),
                    timed(4.3, 5.0, " Xong"),
                    timed(5.0, 5.9, " rồi."),
                ],
            ),
            (timed(6.6, 8.8, " Tốt, gửi tôi nhé."), vec![]),
        ];
        assert_eq!(
            splitting_by_speaker(&lines, &turns),
            [
                named(0.0, 3.8, "Xong chưa?", 1),
                named(4.3, 5.9, "Xong rồi.", 2),
                named(6.6, 8.8, "Tốt, gửi tôi nhé.", 1),
            ]
        );
    }

    #[test]
    fn one_speaker_is_not_named() {
        let lines = [
            (timed(0.0, 3.0, "Chào mọi người."), vec![]),
            (timed(3.5, 6.0, "Bắt đầu nhé."), vec![]),
        ];
        let split = splitting_by_speaker(&lines, &[turn(0.0, 6.0, 0)]);
        assert_eq!(
            split.iter().map(|l| l.speaker).collect::<Vec<_>>(),
            [None, None]
        );
    }

    #[test]
    fn words_that_lost_their_spaces_are_joined_with_one() {
        let glued = splitting_by_speaker(
            &[(
                timed(0.0, 2.0, ""),
                vec![timed(0.0, 1.0, "Xin"), timed(1.0, 2.0, "chào.")],
            )],
            &[],
        );
        assert_eq!(
            glued.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["Xin chào."]
        );
    }

    #[test]
    fn words_that_lost_their_spaces_are_joined_by_letter_not_by_code_point() {
        // "chà" typed as a + combining grave: the last code point is a mark, the last character a letter.
        let glued = splitting_by_speaker(
            &[(
                timed(0.0, 2.0, ""),
                vec![timed(0.0, 1.0, "cha\u{300}"), timed(1.0, 2.0, "o")],
            )],
            &[],
        );
        assert_eq!(glued[0].text, "cha\u{300} o");
    }

    #[test]
    fn chinese_japanese_and_thai_words_stay_without_spaces() {
        let samples: [&[&str]; 3] = [
            &["你好", "世界", "。"],
            &["今日", "は", "会議", "です"],
            &["สวัสดี", "ครับ"],
        ];
        for words in samples {
            let words_timed: Vec<Timed> = words.iter().map(|w| timed(0.0, 1.0, w)).collect();
            let line = splitting_by_speaker(&[(timed(0.0, 2.0, ""), words_timed)], &[]);
            assert_eq!(
                line.iter().map(|l| l.text.clone()).collect::<Vec<_>>(),
                [words.concat()]
            );
        }
    }

    #[test]
    fn a_word_in_a_pause_takes_the_nearest_turn_within_a_second_else_the_previous_speaker() {
        let turns = [turn(0.0, 2.0, 1), turn(10.0, 12.0, 2)];
        let words = vec![
            timed(0.0, 1.0, " một"),
            // Half a second after the first turn, 9 seconds before the second.
            timed(2.4, 2.6, " hai"),
            // Four seconds from either turn.
            timed(6.0, 6.2, " ba"),
            timed(10.0, 11.0, " bốn"),
        ];
        let split = splitting_by_speaker(&[(timed(0.0, 11.0, ""), words)], &turns);
        assert_eq!(
            split,
            [
                named(0.0, 6.2, "một hai ba", 1),
                named(10.0, 11.0, "bốn", 2)
            ]
        );
    }

    #[test]
    fn speakers_are_numbered_in_the_order_they_first_talk() {
        let turns = [turn(0.0, 1.0, 9), turn(1.0, 2.0, 4), turn(2.0, 3.0, 9)];
        let lines = [
            (timed(0.0, 1.0, "a"), vec![]),
            (timed(1.0, 2.0, "b"), vec![]),
            (timed(2.0, 3.0, "c"), vec![]),
        ];
        let split = splitting_by_speaker(&lines, &turns);
        assert_eq!(
            split.iter().map(|l| l.speaker).collect::<Vec<_>>(),
            [Some(1), Some(2), Some(1)]
        );
    }

    #[test]
    fn lines_with_only_blanks_are_dropped() {
        let lines = [
            (timed(0.0, 1.0, "  \n"), vec![]),
            (timed(1.0, 2.0, "ok"), vec![]),
        ];
        let split = splitting_by_speaker(&lines, &[]);
        assert_eq!(split.len(), 1);
        assert_eq!(split[0].text, "ok");
    }
}
