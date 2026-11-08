//! The transcript an audio chat model (Gemini via OpenRouter) writes as JSON:
//! `{"segments":[{"start":12.5,"speaker":"Speaker 1","text":"..."}]}`.

use std::sync::LazyLock;

use fancy_regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::timestamp;

type Object = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub text: String,
}

/// A character `fold` trims from both ends: punctuation (Unicode `P`), a space (`Zs`) or a tab.
static EDGE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^[\p{P}\p{Zs}\t]$").ok());

/// The longest phrase `collapsing_repeats` looks for, in words.
const LONGEST_PHRASE: usize = 10;

/// The segments of a reply and whether it was complete. The second item is `false` when the JSON was cut off
/// (the reply ran out of tokens): the segments are then only the ones written in full before the cut.
/// `None` when the reply isn't segments JSON at all.
pub fn parse(content: &str, duration: Option<f64>) -> Option<(Vec<Segment>, bool)> {
    // Some replies escape the quotes as if the JSON sat inside a string: {\"segments\":[...]}.
    if !content.contains(r#""segments""#) && content.contains(r#"\"segments\""#) {
        let unescaped = content.replace(r#"\""#, "\"").replace(r"\\", r"\");
        return parse(&unescaped, duration);
    }
    let (objects, complete) = if let Some(objects) = whole_objects(content) {
        (objects, true)
    } else if let Some((_, after_key)) = content.split_once(r#""segments""#) {
        (complete_objects(after_key), false)
    } else {
        return None;
    };
    let mut starts: Vec<f64> = objects.iter().map(start_of).collect();
    // Some replies give milliseconds: rescale when the times run far past the audio's length.
    if let (Some(duration), Some(latest)) = (duration, greatest(&starts)) {
        if latest > duration * 3.0 {
            starts.iter_mut().for_each(|start| *start /= 1000.0);
        }
    }
    let segments = objects
        .iter()
        .zip(starts)
        .filter_map(|(object, start)| {
            let text = object.get("text")?.as_str()?.trim();
            (!text.is_empty()).then(|| Segment {
                start,
                speaker: object
                    .get("speaker")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                text: text.to_string(),
            })
        })
        .collect();
    Some((segments, complete))
}

/// The objects of the `segments` array of a reply that is whole JSON, between its first `{` and last `}`.
fn whole_objects(content: &str) -> Option<Vec<Object>> {
    let first = content.find('{')?;
    let last = content.rfind('}')?;
    if first >= last {
        return None;
    }
    // Both ends are ASCII, so these are character boundaries.
    let Value::Object(mut reply) = json_value(content.get(first..=last)?)? else {
        return None;
    };
    let Value::Array(items) = reply.remove("segments")? else {
        return None;
    };
    items
        .into_iter()
        .map(|item| match item {
            Value::Object(object) => Some(object),
            _ => None,
        })
        .collect()
}

/// The `{...}` objects of a JSON array that were closed before the text ends.
fn complete_objects(text: &str) -> Vec<Object> {
    let mut found = Vec::new();
    let mut depth: i64 = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut start: Option<usize> = None;
    for (at, c) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(at);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    // `}` is ASCII, so `at..=at` is one character.
                    let object = start
                        .and_then(|from| text.get(from..=at))
                        .and_then(json_value);
                    if let Some(Value::Object(object)) = object {
                        found.push(object);
                    }
                }
            }
            ']' if depth == 0 => return found,
            _ => {}
        }
    }
    found
}

/// JSON as the app reads it: a comma left before a closing bracket is forgiven.
fn json_value(json: &str) -> Option<Value> {
    serde_json::from_str(&without_dangling_commas(json)).ok()
}

/// `json` without a comma that sits just before a `]` or `}` with a value before it (`[1,2,]`).
/// Swift's `JSONSerialization` accepts that, a stricter parser does not.
fn without_dangling_commas(json: &str) -> String {
    let chars: Vec<char> = json.chars().collect();
    let is_space = |c: &char| matches!(c, ' ' | '\t' | '\n' | '\r');
    let mut out = String::with_capacity(json.len());
    let mut in_string = false;
    let mut escaped = false;
    // The last character that was not a space, outside of strings.
    let mut previous = ' ';
    for (at, &c) in chars.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else {
            if c == ',' {
                let next = chars.iter().skip(at + 1).find(|c| !is_space(c));
                if matches!(next, Some(']' | '}')) && !matches!(previous, '[' | '{' | ',') {
                    continue;
                }
            }
            if c == '"' {
                in_string = true;
            }
            if !is_space(&c) {
                previous = c;
            }
        }
        out.push(c);
    }
    out
}

/// A segment's `start`: seconds as a number or as a time ("01:05"), 0 when it is neither.
fn start_of(object: &Object) -> f64 {
    match object.get("start") {
        Some(Value::Number(seconds)) => seconds.as_f64().unwrap_or(0.0),
        Some(Value::String(time)) => timestamp::parse(time).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// The greatest of `values`, `None` when there are none.
fn greatest(values: &[f64]) -> Option<f64> {
    let (&first, rest) = values.split_first()?;
    Some(rest.iter().fold(first, |m, &v| if m < v { v } else { m }))
}

/// A model stuck in a loop repeats itself, often over silence: the same segment over and over, or the
/// same words within one. Three identical segments in a row (same speaker) and four repeats of a phrase
/// of up to 10 words are kept once.
pub fn dropping_loops(segments: Vec<Segment>) -> Vec<Segment> {
    let cleaned: Vec<Segment> = segments
        .into_iter()
        .map(|segment| Segment {
            text: collapsing_repeats(&segment.text),
            ..segment
        })
        .collect();
    let keys: Vec<String> = cleaned.iter().map(|s| fold(&s.text)).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cleaned.len() {
        let mut j = i + 1;
        while j < cleaned.len() && cleaned[j].speaker == cleaned[i].speaker && keys[j] == keys[i] {
            j += 1;
        }
        // A pair stays ("Vâng." "Vâng."); three or more in a row are kept once.
        if j - i >= 3 {
            out.push(cleaned[i].clone());
        } else {
            out.extend_from_slice(&cleaned[i..j]);
        }
        i = j;
    }
    out
}

/// "cảm ơn cảm ơn cảm ơn cảm ơn" -> "cảm ơn": a phrase of up to 10 words repeated 4 or more times in a row.
/// Line by line, so the line breaks of an email or a list stay.
pub fn collapsing_repeats(text: &str) -> String {
    text.split('\n')
        .map(collapsing_repeats_in_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn collapsing_repeats_in_line(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let keys: Vec<String> = words.iter().map(|word| fold(word)).collect();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    'words: while i < words.len() {
        for n in 1..=LONGEST_PHRASE {
            if i + n * 4 > words.len() {
                break;
            }
            let mut reps = 1;
            while i + (reps + 1) * n <= words.len()
                && keys[i + reps * n..i + (reps + 1) * n] == keys[i..i + n]
            {
                reps += 1;
            }
            if reps >= 4 {
                out.extend_from_slice(&words[i..i + n]);
                i += reps * n;
                continue 'words;
            }
        }
        out.push(words[i]);
        i += 1;
    }
    if out.len() == words.len() {
        text.to_string()
    } else {
        out.join(" ")
    }
}

/// Lower case, without the punctuation and spaces around it: "Cảm ơn." and "cảm ơn" are the same words.
fn fold(text: &str) -> String {
    text.to_lowercase().trim_matches(is_edge).to_string()
}

fn is_edge(c: char) -> bool {
    let mut buffer = [0; 4];
    EDGE.as_ref()
        .is_some_and(|edge| edge.is_match(c.encode_utf8(&mut buffer)).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(start: f64, speaker: Option<&str>, text: &str) -> Segment {
        Segment {
            start,
            speaker: speaker.map(str::to_string),
            text: text.to_string(),
        }
    }

    fn starts(reply: Option<(Vec<Segment>, bool)>) -> Option<Vec<f64>> {
        reply.map(|(segments, _)| segments.iter().map(|s| s.start).collect())
    }

    #[test]
    fn reads_json_around_stray_text() {
        let content = r#"```json
{"segments":[{"start":0,"speaker":null,"text":"Xin chào."},{"start":"01:05","speaker":"Speaker 2","text":" Bắt đầu nhé. "},{"start":70,"text":""}]}
```"#;
        let (segments, complete) = parse(content, Some(120.0)).unwrap();
        assert!(complete);
        assert_eq!(
            segments,
            [
                segment(0.0, None, "Xin chào."),
                segment(65.0, Some("Speaker 2"), "Bắt đầu nhé.")
            ]
        );
    }

    #[test]
    fn milliseconds_are_rescaled_when_they_run_far_past_the_audio() {
        let content = r#"{"segments":[{"start":1500,"text":"a"},{"start":90000,"text":"b"}]}"#;
        assert_eq!(starts(parse(content, Some(100.0))), Some(vec![1.5, 90.0]));
    }

    #[test]
    fn times_within_three_times_the_audio_are_left_alone() {
        let content = r#"{"segments":[{"start":1,"text":"a"},{"start":300,"text":"b"}]}"#;
        assert_eq!(starts(parse(content, Some(100.0))), Some(vec![1.0, 300.0]));
        // Without a duration there is nothing to measure against.
        let content = r#"{"segments":[{"start":90000,"text":"b"}]}"#;
        assert_eq!(starts(parse(content, None)), Some(vec![90000.0]));
    }

    #[test]
    fn an_empty_segments_array_is_an_empty_reply() {
        let (segments, complete) = parse(r#"{"segments":[]}"#, None).unwrap();
        assert!(segments.is_empty());
        assert!(complete);
    }

    #[test]
    fn a_reply_with_escaped_quotes_is_read_as_if_it_had_none() {
        // Gemini 3.8 Flash sometimes escapes the quotes of the whole reply.
        let escaped = r#"{\"segments\":[{\"start\":0.0,\"speaker\":null,\"text\":\"Ta đã chuyển sang Gemini.\"}]}"#;
        let (segments, complete) = parse(escaped, Some(40.0)).unwrap();
        assert!(complete);
        assert_eq!(segments, [segment(0.0, None, "Ta đã chuyển sang Gemini.")]);
    }

    #[test]
    fn keeps_what_was_written_before_a_cut() {
        let content = r#"{"segments":[{"start":0,"speaker":"Speaker 1","text":"Mở ngoặc { và \"trích dẫn\" }"},{"start":4.5,"speaker":"Speaker 2","text":"Câu thứ hai"},{"start":9,"speaker":"Speaker 1","text":"Câu bị cắt gi"#;
        let (segments, complete) = parse(content, Some(300.0)).unwrap();
        assert!(!complete);
        assert_eq!(
            segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            [r#"Mở ngoặc { và "trích dẫn" }"#, "Câu thứ hai"]
        );
    }

    #[test]
    fn a_reply_cut_inside_the_first_segment_is_empty_and_incomplete() {
        let (segments, complete) = parse(r#"{"segments":[{"sta"#, None).unwrap();
        assert!(segments.is_empty());
        assert!(!complete);
    }

    #[test]
    fn text_that_is_not_segments_json_is_none() {
        assert_eq!(parse("Chỉ là chữ thường.", None), None);
    }

    #[test]
    fn segments_that_are_not_all_objects_are_recovered_one_by_one() {
        let content = r#"{"segments":[1,{"start":2,"text":"x"}]}"#;
        let (segments, complete) = parse(content, None).unwrap();
        assert_eq!(segments, [segment(2.0, None, "x")]);
        assert!(!complete);
    }

    #[test]
    fn recovery_stops_at_the_end_of_the_segments_array() {
        let content = r#"{"segments":[{"start":1,"text":"a"}],"other":{"start":2,"text":"b"}"#;
        let (segments, complete) = parse(content, None).unwrap();
        assert_eq!(segments, [segment(1.0, None, "a")]);
        assert!(!complete);
    }

    #[test]
    fn a_comma_left_before_a_closing_bracket_is_forgiven() {
        // JSONSerialization, which the app reads replies with, accepts this.
        let content = r#"{"segments":[{"start":0,"text":"a",},{"start":1,"text":"b"},]}"#;
        let (segments, complete) = parse(content, None).unwrap();
        assert_eq!(segments.len(), 2);
        assert!(complete);
        // ...but not a comma with nothing before it.
        let (segments, complete) = parse(r#"{"segments":[,]}"#, None).unwrap();
        assert!(segments.is_empty());
        assert!(!complete);
        // A comma inside a string is text.
        let content = r#"{"segments":[{"start":0,"text":"a,}"}]}"#;
        assert_eq!(parse(content, None).unwrap().0[0].text, "a,}");
    }

    #[test]
    fn a_start_that_is_neither_a_number_nor_a_time_is_zero() {
        let content =
            r#"{"segments":[{"start":null,"text":"a"},{"start":"soon","text":"b"},{"text":"c"}]}"#;
        assert_eq!(starts(parse(content, None)), Some(vec![0.0, 0.0, 0.0]));
    }

    #[test]
    fn a_speaker_that_is_not_a_string_is_none() {
        let content = r#"{"segments":[{"start":0,"speaker":3,"text":"a"}]}"#;
        assert_eq!(parse(content, None).unwrap().0[0].speaker, None);
    }

    #[test]
    fn a_segment_without_a_speaker_is_written_and_read_without_the_key() {
        let a = segment(1.5, None, "a");
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, r#"{"start":1.5,"text":"a"}"#);
        assert_eq!(serde_json::from_str::<Segment>(&json).unwrap(), a);
        let b = segment(2.0, Some("Speaker 1"), "b");
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(json, r#"{"start":2.0,"speaker":"Speaker 1","text":"b"}"#);
        assert_eq!(serde_json::from_str::<Segment>(&json).unwrap(), b);
    }

    #[test]
    fn loops_are_kept_once() {
        let a = segment(0.0, Some("Speaker 1"), "Cảm ơn.");
        let looped: Vec<Segment> = (0..5)
            .map(|i| {
                let text = if i == 4 { "cảm ơn" } else { "Cảm ơn." };
                segment(f64::from(i), Some("Speaker 1"), text)
            })
            .collect();
        assert_eq!(dropping_loops(looped), [a]);
    }

    #[test]
    fn a_pair_or_the_same_words_from_different_people_is_real_speech() {
        let a = segment(0.0, Some("Speaker 1"), "Cảm ơn.");
        let pair = vec![a.clone(), segment(1.0, Some("Speaker 1"), "Cảm ơn.")];
        assert_eq!(dropping_loops(pair.clone()), pair);
        let others = vec![
            a,
            segment(1.0, Some("Speaker 2"), "Cảm ơn."),
            segment(2.0, Some("Speaker 1"), "Cảm ơn."),
        ];
        assert_eq!(dropping_loops(others.clone()), others);
    }

    #[test]
    fn segments_match_across_unicode_punctuation_but_not_symbols() {
        let same: Vec<Segment> = ["Cảm ơn…", "「cảm ơn」", "  CẢM ƠN。 "]
            .iter()
            .map(|t| segment(0.0, None, t))
            .collect();
        assert_eq!(dropping_loops(same).len(), 1);
        // "+" is a symbol, not punctuation: "a+" is not "a".
        let different: Vec<Segment> = ["a+", "a+", "a"]
            .iter()
            .map(|t| segment(0.0, None, t))
            .collect();
        assert_eq!(dropping_loops(different).len(), 3);
    }

    #[test]
    fn a_loop_inside_one_segment_collapses() {
        let looped = vec![segment(0.0, None, "ừ ừ ừ ừ ừ ừ rồi")];
        assert_eq!(dropping_loops(looped), [segment(0.0, None, "ừ rồi")]);
    }

    #[test]
    fn repeated_phrases_collapse() {
        assert_eq!(
            collapsing_repeats("Cảm ơn. Cảm ơn. Cảm ơn. Cảm ơn. Cảm ơn. Hẹn gặp lại"),
            "Cảm ơn. Hẹn gặp lại"
        );
        assert_eq!(collapsing_repeats("a b a b a b a b c"), "a b c");
    }

    #[test]
    fn three_repeats_is_still_speech() {
        assert_eq!(collapsing_repeats("rất rất rất tốt"), "rất rất rất tốt");
    }

    #[test]
    fn line_breaks_survive_collapsing() {
        assert_eq!(
            collapsing_repeats("Chào anh,\n\nem gửi em gửi em gửi em gửi báo cáo."),
            "Chào anh,\n\nem gửi báo cáo."
        );
    }

    #[test]
    fn text_without_a_loop_is_returned_as_it_was() {
        assert_eq!(collapsing_repeats("Không   đổi gì"), "Không   đổi gì");
    }

    #[test]
    fn a_phrase_of_eleven_words_is_not_collapsed() {
        let phrase = "a b c d e f g h i j k";
        let text = [phrase; 4].join(" ");
        assert_eq!(collapsing_repeats(&text), text);
        let phrase = "a b c d e f g h i j";
        let text = [phrase; 4].join(" ");
        assert_eq!(collapsing_repeats(&text), phrase);
    }
}
