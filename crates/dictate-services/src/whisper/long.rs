//! Whisper on long recordings: where to cut them, and how the lines Whisper writes become the transcript.
//!
//! The chunk loop itself is `LocalWhisper::transcribe_timed`; the parts here need no model.

use std::ops::Range;

use dictate_core::text::segments::{collapsing_repeats, dropping_loops, Segment};
use dictate_core::text::whisper::{
    clean, dropping_padding_lines, splitting_by_speaker, Timed, Turn,
};

/// How long the quiet stretch is that a cut looks for, in seconds.
const QUIET_WINDOW_S: f64 = 0.5;

/// The chunks to give Whisper one at a time, as ranges of `samples` (recorded at `rate` samples a second): they
/// follow one another with no gap and no overlap, and together they are all of `samples`.
///
/// A chunk is about `target_s` long. Where it ends is looked for within `search_s` either side of that: the cut
/// goes at the start of the quietest half second there (the lowest RMS, the earliest when several are as quiet),
/// so that it falls between two words and not in the middle of one. The last chunk takes what is left, so a
/// recording shorter than `target_s + search_s` is one chunk. No samples, no chunks.
pub fn chunk_bounds(samples: &[f32], rate: u32, target_s: f64, search_s: f64) -> Vec<Range<usize>> {
    let len = samples.len();
    if len == 0 {
        return Vec::new();
    }
    let per_second = f64::from(rate);
    let target = in_samples(target_s, per_second);
    let search = in_samples(search_s, per_second);
    let window = in_samples(QUIET_WINDOW_S, per_second).max(1);
    // At least 2: a cut needs a sample before it and one after it.
    let enough_to_cut = target.saturating_add(search).max(2);
    let mut chunks = Vec::new();
    let mut start = 0;
    // With a target of nothing there is no cutting: all of it is one chunk.
    while target > 0 && len - start >= enough_to_cut {
        let cut = quietest_start(samples, start, start + target, search, window);
        chunks.push(start..cut);
        start = cut;
    }
    chunks.push(start..len);
    chunks
}

/// `seconds` as a number of samples; 0 for a time that is negative or not a number.
fn in_samples(seconds: f64, per_second: f64) -> usize {
    // A cast from float to integer saturates, and gives 0 for NaN.
    (seconds * per_second).round() as usize
}

/// Where to cut `samples` after `start`: the start of the quietest run of `window` samples that begins within
/// `search` samples of `center`. Always after `start` and before the end of `samples`, which are at least 2 apart.
fn quietest_start(
    samples: &[f32],
    start: usize,
    center: usize,
    search: usize,
    window: usize,
) -> usize {
    let len = samples.len();
    let first = center.saturating_sub(search).max(start + 1);
    // The run has to fit before the end, and a cut at the very end would leave an empty chunk.
    let last = center
        .saturating_add(search)
        .min(len.saturating_sub(window))
        .min(len - 1);
    if first > last {
        return first.min(len - 1);
    }
    // Running sums of squares: the energy of any run is two of them apart, so each run costs the same however
    // long it is.
    let region = &samples[first..last + window];
    let mut sums = Vec::with_capacity(region.len() + 1);
    let mut total = 0.0_f64;
    sums.push(total);
    for &sample in region {
        total += f64::from(sample) * f64::from(sample);
        sums.push(total);
    }
    let mut quietest = (0, f64::INFINITY);
    for at in 0..=last - first {
        let energy = sums[at + window] - sums[at];
        if energy < quietest.1 {
            quietest = (at, energy);
        }
    }
    first + quietest.0
}

/// The segments of a recording of `duration` seconds from the lines Whisper wrote (each with its words, when it
/// gave their times) and, when speaker separation ran, who spoke when.
///
/// In this order:
/// 1. lines Whisper made up in the silence at the end of a chunk are dropped;
/// 2. with `turns`, lines are split where the speaker changes and the speakers are numbered by who talks first
///    (fewer than two voices means no names at all);
/// 3. Whisper's tags (`<|en|>`, `[BLANK_AUDIO]`) are taken out of the text, and a segment left with no text goes;
/// 4. a phrase said over and over within a line is said once;
/// 5. each becomes a `Segment`, the speaker named "Speaker 1", "Speaker 2"... in English whatever the language
///    of the app;
/// 6. a segment repeated three or more times in a row is kept once.
pub fn timed_to_segments(
    lines: Vec<(Timed, Vec<Timed>)>,
    turns: Option<&[Turn]>,
    duration: f64,
) -> Vec<Segment> {
    let lines = dropping_padding_lines(lines, duration, |line| line.0.start, |line| line.0.end);
    let lines: Vec<Timed> = match turns {
        Some(turns) if !turns.is_empty() => splitting_by_speaker(&lines, turns),
        _ => lines.into_iter().map(|(line, _words)| line).collect(),
    };
    let segments = lines
        .into_iter()
        .filter_map(|line| {
            let text = clean(&line.text);
            if text.is_empty() {
                return None;
            }
            Some(Segment {
                start: line.start,
                speaker: line.speaker.map(|n| format!("Speaker {n}")),
                text: collapsing_repeats(&text),
            })
        })
        .collect();
    dropping_loops(segments)
}

/// A token Whisper wrote: its bytes (a character can be split over several tokens) and when it starts and ends,
/// in hundredths of a second from the start of its chunk. Whisper gives no time to a token it did not time:
/// that is -1.
pub(super) struct TokenSpan {
    pub bytes: Vec<u8>,
    pub start: i64,
    pub end: i64,
}

/// The words of a line from its tokens, with their times in seconds from the start of the recording (`offset` is
/// where the chunk starts). A word starts at a token with a leading space, as Whisper's words do, and keeps that
/// space: that is how `splitting_by_speaker` expects them.
pub(super) fn words_from_tokens(tokens: &[TokenSpan], offset: f64) -> Vec<Timed> {
    // The bytes of each word, and its first start and last end.
    let mut words: Vec<(Vec<u8>, i64, i64)> = Vec::new();
    for token in tokens.iter().filter(|token| !token.bytes.is_empty()) {
        match words.last_mut() {
            Some(word) if !token.bytes.starts_with(b" ") => {
                word.0.extend_from_slice(&token.bytes);
                word.2 = word.2.max(token.end);
            }
            _ => words.push((token.bytes.clone(), token.start, token.end)),
        }
    }
    words
        .into_iter()
        .map(|(bytes, start, end)| {
            let start = offset + seconds(start);
            Timed {
                start,
                end: (offset + seconds(end)).max(start),
                text: String::from_utf8_lossy(&bytes).into_owned(),
                speaker: None,
            }
        })
        .collect()
}

/// A time of Whisper's, in hundredths of a second, as seconds; 0 for one that was not given.
pub(super) fn seconds(hundredths: i64) -> f64 {
    hundredths.max(0) as f64 / 100.0
}

/// How much of what the chunk before said is shown to Whisper before the next one, in characters.
pub(super) const CONTEXT_CHARS: usize = 200;

/// What Whisper is shown before a chunk: the vocabulary, then the last `CONTEXT_CHARS` characters of what the
/// chunk before it said, so that a sentence cut at the end of one chunk goes on in the next in the same words and
/// spelling. The model keeps the end of what it is shown, which is the end of this.
pub(super) fn chunk_prompt(vocabulary: &str, before: &str) -> String {
    let skipped = before.chars().count().saturating_sub(CONTEXT_CHARS);
    let end: String = before.chars().skip(skipped).collect();
    let end = end.trim();
    if end.is_empty() {
        vocabulary.to_string()
    } else if vocabulary.trim().is_empty() {
        end.to_string()
    } else {
        format!("{} {end}", vocabulary.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// Noise between -0.5 and 0.5, the same every time.
    fn noise(seconds: f64) -> Vec<f32> {
        let mut seed: u32 = 12_345;
        (0..(seconds * f64::from(RATE)) as usize)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / 16_777_216.0 - 0.5
            })
            .collect()
    }

    fn silence(samples: &mut [f32], from_s: f64, to_s: f64) {
        let rate = f64::from(RATE);
        samples[(from_s * rate) as usize..(to_s * rate) as usize].fill(0.0);
    }

    fn seconds(samples: usize) -> f64 {
        samples as f64 / f64::from(RATE)
    }

    fn timed(start: f64, end: f64, text: &str) -> Timed {
        Timed {
            start,
            end,
            text: text.to_string(),
            speaker: None,
        }
    }

    fn turn(start: f64, end: f64, speaker: u32) -> Turn {
        Turn {
            start,
            end,
            speaker,
        }
    }

    fn segment(start: f64, speaker: Option<&str>, text: &str) -> Segment {
        Segment {
            start,
            speaker: speaker.map(str::to_string),
            text: text.to_string(),
        }
    }

    #[test]
    fn chunk_bounds_short_is_one_chunk() {
        // A minute, and up to just under target + search.
        for seconds in [60.0, 629.0] {
            let audio = noise(seconds);

            let chunks = chunk_bounds(&audio, RATE, 600.0, 30.0);

            assert_eq!(chunks.len(), 1, "{seconds} s: {chunks:?}");
            assert_eq!(chunks[0], 0..audio.len());
        }
    }

    #[test]
    fn chunk_bounds_of_nothing_is_no_chunks() {
        assert!(chunk_bounds(&[], RATE, 600.0, 30.0).is_empty());
    }

    #[test]
    fn chunk_bounds_cuts_at_quiet_window() {
        // 25 minutes of noise with a second of silence close to each 10-minute mark.
        let mut audio = noise(25.0 * 60.0);
        silence(&mut audio, 9.0 * 60.0 + 50.0, 9.0 * 60.0 + 51.0);
        silence(&mut audio, 19.0 * 60.0 + 40.0, 19.0 * 60.0 + 41.0);

        let chunks = chunk_bounds(&audio, RATE, 600.0, 30.0);

        assert_eq!(chunks.len(), 3, "{chunks:?}");
        let first_cut = seconds(chunks[0].end);
        let second_cut = seconds(chunks[1].end);
        assert!((first_cut - 590.0).abs() <= 0.5, "cut at {first_cut}");
        assert!((second_cut - 1180.0).abs() <= 0.5, "cut at {second_cut}");
    }

    #[test]
    fn chunk_bounds_cover_everything_without_overlap() {
        let audio = noise(2.0 * 60.0 * 60.0 + 17.0);

        let chunks = chunk_bounds(&audio, RATE, 600.0, 30.0);

        assert_eq!(chunks.first().map(|c| c.start), Some(0));
        assert_eq!(chunks.last().map(|c| c.end), Some(audio.len()));
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "{pair:?}");
        }
        // Each is about 10 minutes (the last takes what is left, which is less than target + search).
        for chunk in &chunks[..chunks.len() - 1] {
            let length = seconds(chunk.len());
            assert!((570.0..=630.0).contains(&length), "{length} s");
        }
        let last = seconds(chunks[chunks.len() - 1].len());
        assert!(last > 0.0 && last < 630.0, "{last} s");
    }

    #[test]
    fn chunk_bounds_never_make_an_empty_chunk_from_odd_settings() {
        let audio = noise(30.0);
        // A search window wider than the target, a target of nothing, a rate of nothing.
        for (rate, target, search) in [(RATE, 2.0, 5.0), (RATE, 0.0, 1.0), (0, 10.0, 1.0)] {
            let chunks = chunk_bounds(&audio, rate, target, search);
            assert!(chunks.iter().all(|c| !c.is_empty()), "{chunks:?}");
            assert_eq!(chunks.first().map(|c| c.start), Some(0));
            assert_eq!(chunks.last().map(|c| c.end), Some(audio.len()));
            for pair in chunks.windows(2) {
                assert_eq!(pair[0].end, pair[1].start);
            }
        }
    }

    #[test]
    fn timed_to_segments_names_speakers_and_cleans() {
        // Speaker 7 talks first, then 3: they become Speaker 1 and Speaker 2. The first line starts with
        // a language token.
        let lines = vec![
            (
                timed(0.0, 3.0, "<|en|> Chào mọi người."),
                vec![
                    timed(0.0, 0.0, " <|en|>"),
                    timed(0.4, 1.0, " Chào"),
                    timed(1.0, 1.6, " mọi"),
                    timed(1.6, 3.0, " người."),
                ],
            ),
            (
                timed(4.0, 6.0, " Dạ vâng."),
                vec![timed(4.0, 5.0, " Dạ"), timed(5.0, 6.0, " vâng.")],
            ),
        ];
        let turns = [turn(0.0, 3.5, 7), turn(3.5, 6.5, 3)];

        let segments = timed_to_segments(lines, Some(&turns), 6.5);

        assert_eq!(
            segments,
            [
                segment(0.0, Some("Speaker 1"), "Chào mọi người."),
                segment(4.0, Some("Speaker 2"), "Dạ vâng."),
            ]
        );
    }

    #[test]
    fn timed_to_segments_splits_a_line_where_the_speaker_changes() {
        let lines = vec![(
            timed(0.0, 4.0, " Xong chưa? Xong rồi."),
            vec![
                timed(0.0, 1.0, " Xong"),
                timed(1.0, 2.0, " chưa?"),
                timed(2.2, 3.0, " Xong"),
                timed(3.0, 4.0, " rồi."),
            ],
        )];
        let turns = [turn(0.0, 2.1, 0), turn(2.1, 4.0, 1)];

        let segments = timed_to_segments(lines, Some(&turns), 4.0);

        assert_eq!(
            segments,
            [
                segment(0.0, Some("Speaker 1"), "Xong chưa?"),
                segment(2.2, Some("Speaker 2"), "Xong rồi."),
            ]
        );
    }

    #[test]
    fn timed_to_segments_single_speaker_has_no_label() {
        let lines = || {
            vec![
                (timed(0.0, 3.0, " <|en|> Chào mọi người."), vec![]),
                (timed(3.5, 6.0, " Bắt đầu nhé."), vec![]),
            ]
        };
        let expected = [
            segment(0.0, None, "Chào mọi người."),
            segment(3.5, None, "Bắt đầu nhé."),
        ];

        // Speaker separation found one voice, found nobody, or did not run.
        for turns in [Some(vec![turn(0.0, 6.0, 5)]), Some(vec![]), None] {
            let segments = timed_to_segments(lines(), turns.as_deref(), 6.0);
            assert_eq!(segments, expected, "{turns:?}");
        }
    }

    #[test]
    fn timed_to_segments_drops_padding_lines_first() {
        // The second line runs on past the start of the third: Whisper heard it in the silence at the
        // end of a chunk.
        let lines = vec![
            (timed(0.0, 5.0, " Hello."), vec![]),
            (timed(5.0, 35.0, " Thank you for watching."), vec![]),
            (timed(10.0, 12.0, " Next."), vec![]),
        ];

        let segments = timed_to_segments(lines, None, 12.0);

        assert_eq!(
            segments,
            [segment(0.0, None, "Hello."), segment(10.0, None, "Next.")]
        );
    }

    #[test]
    fn timed_to_segments_drops_silence_tags_and_loops() {
        let lines = vec![
            (timed(0.0, 2.0, " [BLANK_AUDIO]"), vec![]),
            (
                timed(2.0, 6.0, " cảm ơn cảm ơn cảm ơn cảm ơn cảm ơn"),
                vec![],
            ),
            (timed(6.0, 7.0, " Vâng."), vec![]),
            (timed(7.0, 8.0, " Vâng."), vec![]),
            (timed(8.0, 9.0, " Vâng."), vec![]),
            (timed(9.0, 10.0, " Rồi."), vec![]),
        ];

        let segments = timed_to_segments(lines, None, 10.0);

        assert_eq!(
            segments,
            [
                segment(2.0, None, "cảm ơn"),
                segment(6.0, None, "Vâng."),
                segment(9.0, None, "Rồi."),
            ]
        );
    }

    fn token(text: &str, start: i64, end: i64) -> TokenSpan {
        TokenSpan {
            bytes: text.as_bytes().to_vec(),
            start,
            end,
        }
    }

    fn word(start: f64, end: f64, text: &str) -> Timed {
        timed(start, end, text)
    }

    #[test]
    fn words_start_at_a_leading_space_and_carry_it() {
        let tokens = [
            token(" Xin", 0, 30),
            token(" ch", 30, 50),
            token("ào", 50, 70),
            token(".", 70, 80),
        ];

        let words = words_from_tokens(&tokens, 0.0);

        assert_eq!(words, [word(0.0, 0.3, " Xin"), word(0.3, 0.8, " chào.")]);
    }

    #[test]
    fn word_times_count_from_the_start_of_the_chunk() {
        let words = words_from_tokens(&[token(" Xin", 100, 150)], 590.0);

        assert_eq!(words, [word(591.0, 591.5, " Xin")]);
    }

    #[test]
    fn a_character_split_over_two_tokens_is_put_back_together() {
        // "à" is the bytes C3 A0, which Whisper can write as two tokens.
        let tokens = [
            token(" ch", 0, 10),
            TokenSpan {
                bytes: vec![0xC3],
                start: 10,
                end: 15,
            },
            TokenSpan {
                bytes: vec![0xA0],
                start: 15,
                end: 20,
            },
            token("o", 20, 30),
        ];

        let words = words_from_tokens(&tokens, 0.0);

        assert_eq!(words, [word(0.0, 0.3, " chào")]);
    }

    #[test]
    fn the_first_token_starts_a_word_even_without_a_space() {
        let tokens = [token("Xin", 0, 20), token(" chào", 20, 40)];

        let words = words_from_tokens(&tokens, 0.0);

        assert_eq!(words, [word(0.0, 0.2, "Xin"), word(0.2, 0.4, " chào")]);
    }

    #[test]
    fn a_token_without_a_time_does_not_make_a_word_run_backwards() {
        let tokens = [token(" Xin", -1, -1), token(" chào", 20, 10)];

        let words = words_from_tokens(&tokens, 5.0);

        assert_eq!(words, [word(5.0, 5.0, " Xin"), word(5.2, 5.2, " chào")]);
    }

    #[test]
    fn no_tokens_no_words() {
        assert!(words_from_tokens(&[], 0.0).is_empty());
    }

    #[test]
    fn the_first_chunk_is_shown_the_vocabulary_only() {
        assert_eq!(chunk_prompt("Dictate, Whisper.", ""), "Dictate, Whisper.");
        assert_eq!(chunk_prompt("", ""), "");
    }

    #[test]
    fn a_later_chunk_is_shown_the_vocabulary_and_the_end_of_the_text_before() {
        let before = format!("{}{}", "a".repeat(100), "é".repeat(200));

        assert_eq!(
            chunk_prompt("Dictate, Whisper.", &before),
            format!("Dictate, Whisper. {}", "é".repeat(200))
        );
        assert_eq!(chunk_prompt("", &before), "é".repeat(200));
    }

    #[test]
    fn the_end_of_the_text_before_is_cut_clean() {
        assert_eq!(
            chunk_prompt("", "Hẹn gặp lại ngày mai.  \n"),
            "Hẹn gặp lại ngày mai."
        );
        // A text shorter than the context is all there is.
        assert_eq!(chunk_prompt("Tên", "Xin chào."), "Tên Xin chào.");
        assert_eq!(chunk_prompt("Tên", "   "), "Tên");
    }
}
