//! Dictation in the app: the controller that connects the hotkey, the microphone, local Whisper, the stop pipeline,
//! the paste into the focused field and the panels, and the parts it is made of.

pub mod backend;
pub mod chip;
pub mod controller;
pub mod live;
pub mod save;
pub mod stt;
pub mod system;
pub mod target;

use dictate_core::text::segments::Segment;
use dictate_services::dictation::live::join;

/// The words of the segments of a transcript, one after the other, joined as the language joins words: with a space, or with
/// none for Japanese and Chinese.
pub fn join_segments(segments: &[Segment], language: &str) -> String {
    segments.iter().fold(String::new(), |text, segment| {
        join(&text, &segment.text, language)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments(texts: &[&str]) -> Vec<Segment> {
        texts
            .iter()
            .map(|text| Segment {
                start: 0.0,
                speaker: None,
                text: (*text).into(),
            })
            .collect()
    }

    #[test]
    fn the_segments_of_a_transcript_are_one_text() {
        assert_eq!(
            join_segments(&segments(&["hello", "there world"]), "en-US"),
            "hello there world"
        );
        assert_eq!(
            join_segments(&segments(&["こんにちは", "世界"]), "ja-JP"),
            "こんにちは世界"
        );
        assert_eq!(join_segments(&[], "en-US"), "");
    }
}
