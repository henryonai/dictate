//! The audio window and the text of a dictation while it is recording.
//!
//! About once a second the recording is transcribed again so that the text on screen follows the speech. The job
//! covers all the audio while it is short. Once it is 20 s long, its first 20 s are committed (their text is
//! final) and the next job starts after them, so that no job grows without bound on a long take. The text is
//! what is committed followed by the latest guess for the rest, joined with a space, or with none for languages
//! written without spaces between words (Japanese, Chinese, Cantonese, Thai).

use std::ops::Range;

/// The least audio worth transcribing: 0.5 s at 16 kHz.
pub const LIVE_MIN_SAMPLES: usize = 8_000;
/// The most audio one live transcription takes: 20 s at 16 kHz.
pub const LIVE_WINDOW_SAMPLES: usize = 320_000;

/// Languages written without spaces between words, by their primary language subtag.
const NO_SPACE_LANGUAGES: [&str; 4] = ["ja", "zh", "yue", "th"];

/// Joins two pieces of live text with a space, or with none for languages written without spaces. An empty side
/// leaves the other as it is, and whitespace at the seam is dropped so that it never doubles.
pub fn join(a: &str, b: &str, lang: &str) -> String {
    let (a, b) = (a.trim_end(), b.trim_start());
    if a.is_empty() {
        return b.to_owned();
    }
    if b.is_empty() {
        return a.to_owned();
    }
    let primary = lang.split(['-', '_']).next().unwrap_or_default();
    if NO_SPACE_LANGUAGES
        .iter()
        .any(|l| primary.eq_ignore_ascii_case(l))
    {
        format!("{a}{b}")
    } else {
        format!("{a} {b}")
    }
}

/// One live transcription to run: the samples (16 kHz) to give it, and whether its text is final.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveJob {
    pub range: Range<usize>,
    pub commit: bool,
}

/// The part of the recording that live transcription looks at. It starts at the beginning, grows with the
/// recording up to [`LIVE_WINDOW_SAMPLES`], and then is cut off as a commit job so that no job grows without
/// bound on a long take. The caller ticks about once a second and says how much audio there is.
#[derive(Debug, Default)]
pub struct LiveWindow {
    /// Where the window starts: the end of the audio whose text is committed.
    start: usize,
    /// The end of the commit job offered last, until [`LiveWindow::commit_done`] moves the start there.
    commit_end: Option<usize>,
}

impl LiveWindow {
    pub fn new() -> Self {
        Self::default()
    }

    /// The next live transcription, if one is due. `total_16k` is all the 16 kHz samples recorded so far and
    /// `busy` is whether the last job is still running. Nothing is offered while busy or before the window holds
    /// [`LIVE_MIN_SAMPLES`]. A window of [`LIVE_WINDOW_SAMPLES`] or more gives the commit job for its first
    /// 20 s, offered again until [`LiveWindow::commit_done`]. A shorter one gives a partial job for all of it.
    pub fn next_job(&mut self, total_16k: usize, busy: bool) -> Option<LiveJob> {
        let held = total_16k.saturating_sub(self.start);
        if busy || held < LIVE_MIN_SAMPLES {
            return None;
        }
        if held >= LIVE_WINDOW_SAMPLES {
            let end = self.start + LIVE_WINDOW_SAMPLES;
            self.commit_end = Some(end);
            Some(LiveJob {
                range: self.start..end,
                commit: true,
            })
        } else {
            Some(LiveJob {
                range: self.start..total_16k,
                commit: false,
            })
        }
    }

    /// The commit job offered last is over: the window now starts where that job ended. A caller calls it when
    /// the text is committed, and also to give up on a commit job that failed, whose audio is then left out;
    /// without it the same job is offered again on every tick. It does nothing when no commit job is waiting.
    pub fn commit_done(&mut self) {
        if let Some(end) = self.commit_end.take() {
            self.start = end;
        }
    }
}

/// The text so far: what is final, then the latest guess for the audio after it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LiveTranscript {
    pub committed: String,
    pub partial: String,
}

impl LiveTranscript {
    /// Makes `text` final, after what is already, and drops the partial it replaces.
    pub fn commit(&mut self, text: &str, lang: &str) {
        self.committed = join(&self.committed, text, lang);
        self.partial.clear();
    }

    /// Replaces the guess for the audio after the committed text.
    pub fn set_partial(&mut self, text: &str) {
        self.partial = text.to_owned();
    }

    /// All the text, committed then partial.
    pub fn full(&self, lang: &str) -> String {
        join(&self.committed, &self.partial, lang)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: usize = 16_000;

    #[test]
    fn join_uses_no_space_for_cjk_and_thai() {
        for lang in ["ja-JP", "zh-CN", "zh-TW", "yue-HK", "th-TH", "ja", "ZH-cn"] {
            assert_eq!(join("你好", "世界", lang), "你好世界", "{lang}");
        }
    }

    #[test]
    fn join_uses_a_space_for_other_languages() {
        for lang in ["en-US", "vi-VN", "ko-KR", "fr-FR", "tha-XX", ""] {
            assert_eq!(join("hello", "world", lang), "hello world", "{lang}");
        }
    }

    #[test]
    fn join_keeps_the_other_side_when_one_is_empty() {
        assert_eq!(join("", "world", "en-US"), "world");
        assert_eq!(join("hello", "", "en-US"), "hello");
        assert_eq!(join("", "", "en-US"), "");
    }

    #[test]
    fn join_does_not_double_the_space_at_the_seam() {
        assert_eq!(join("hello ", " world", "en-US"), "hello world");
        assert_eq!(join("hello", "  ", "en-US"), "hello");
    }

    #[test]
    fn live_window_waits_for_half_a_second() {
        let mut window = LiveWindow::new();
        assert_eq!(window.next_job(0, false), None);
        assert_eq!(window.next_job(LIVE_MIN_SAMPLES - 1, false), None);
        assert_eq!(
            window.next_job(LIVE_MIN_SAMPLES, false),
            Some(LiveJob {
                range: 0..LIVE_MIN_SAMPLES,
                commit: false
            })
        );
    }

    #[test]
    fn live_window_skips_when_busy() {
        let mut window = LiveWindow::new();
        assert_eq!(window.next_job(3 * SECOND, true), None);
        // A busy tick must not use up the commit job either.
        assert_eq!(window.next_job(25 * SECOND, true), None);
        assert_eq!(
            window.next_job(25 * SECOND, false),
            Some(LiveJob {
                range: 0..LIVE_WINDOW_SAMPLES,
                commit: true
            })
        );
    }

    #[test]
    fn live_window_partial_job_covers_everything_since_the_start() {
        let mut window = LiveWindow::new();
        assert_eq!(
            window.next_job(3 * SECOND, false),
            Some(LiveJob {
                range: 0..3 * SECOND,
                commit: false
            })
        );
        assert_eq!(
            window.next_job(4 * SECOND, false),
            Some(LiveJob {
                range: 0..4 * SECOND,
                commit: false
            })
        );
    }

    #[test]
    fn live_window_commits_every_20_seconds() {
        let mut window = LiveWindow::new();
        let total = 25 * SECOND;
        assert_eq!(
            window.next_job(total, false),
            Some(LiveJob {
                range: 0..320_000,
                commit: true
            })
        );
        window.commit_done();
        assert_eq!(
            window.next_job(total, false),
            Some(LiveJob {
                range: 320_000..400_000,
                commit: false
            })
        );
    }

    #[test]
    fn live_window_commits_at_exactly_20_seconds() {
        let mut window = LiveWindow::new();
        assert_eq!(
            window.next_job(LIVE_WINDOW_SAMPLES - 1, false),
            Some(LiveJob {
                range: 0..LIVE_WINDOW_SAMPLES - 1,
                commit: false
            })
        );
        assert_eq!(
            window.next_job(LIVE_WINDOW_SAMPLES, false),
            Some(LiveJob {
                range: 0..LIVE_WINDOW_SAMPLES,
                commit: true
            })
        );
    }

    #[test]
    fn live_window_offers_the_same_commit_job_until_commit_done() {
        let mut window = LiveWindow::new();
        let first = window.next_job(25 * SECOND, false);
        let again = window.next_job(26 * SECOND, false);
        assert_eq!(first, again);
        assert_eq!(
            first,
            Some(LiveJob {
                range: 0..LIVE_WINDOW_SAMPLES,
                commit: true
            })
        );
    }

    #[test]
    fn live_window_commit_done_without_a_commit_job_moves_nothing() {
        let mut window = LiveWindow::new();
        window.commit_done();
        assert_eq!(
            window.next_job(3 * SECOND, false),
            Some(LiveJob {
                range: 0..3 * SECOND,
                commit: false
            })
        );
        // A partial job is not a commit either.
        window.commit_done();
        assert_eq!(
            window.next_job(3 * SECOND, false),
            Some(LiveJob {
                range: 0..3 * SECOND,
                commit: false
            })
        );
    }

    #[test]
    fn live_window_long_take_is_bounded() {
        let mut window = LiveWindow::new();
        let total = 600 * SECOND;
        let mut commits = 0;
        let mut covered = 0;
        for _ in 0..100 {
            let Some(job) = window.next_job(total, false) else {
                break;
            };
            assert!(job.range.len() <= LIVE_WINDOW_SAMPLES, "{job:?}");
            assert_eq!(
                job.range.start, covered,
                "jobs leave no gap and no overlap: {job:?}"
            );
            if job.commit {
                commits += 1;
                covered = job.range.end;
                window.commit_done();
            } else {
                break;
            }
        }
        // 600 s is exactly 30 windows of 20 s, and nothing is left over for a partial job.
        assert_eq!(commits, 30);
        assert_eq!(window.next_job(total, false), None);
    }

    #[test]
    fn transcript_shows_committed_then_partial() {
        let mut text = LiveTranscript::default();
        text.set_partial("hello");
        assert_eq!(text.full("en-US"), "hello");
        text.commit("hello there", "en-US");
        text.set_partial("how are");
        assert_eq!(text.full("en-US"), "hello there how are");
    }

    #[test]
    fn transcript_commit_clears_the_partial() {
        let mut text = LiveTranscript::default();
        text.set_partial("hel");
        text.commit("hello", "en-US");
        assert_eq!(text.committed, "hello");
        assert_eq!(text.partial, "");
    }

    #[test]
    fn transcript_partial_replaces_the_last_partial() {
        let mut text = LiveTranscript::default();
        text.set_partial("hel");
        text.set_partial("hello");
        assert_eq!(text.partial, "hello");
    }

    #[test]
    fn transcript_joins_without_a_space_in_japanese() {
        let mut text = LiveTranscript::default();
        text.commit("こんにちは", "ja-JP");
        text.set_partial("世界");
        assert_eq!(text.full("ja-JP"), "こんにちは世界");
    }
}
