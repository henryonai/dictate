//! The live text of a dictation that is being recorded: the audio as it comes in, brought to 16 kHz a piece at a time, the
//! window that [`LiveWindow`] cuts from it, and the text that the transcriptions of that window give. It does no
//! transcribing itself and has no thread: the controller asks it for the next job, runs the job where it likes and
//! tells it the result, so that all of it is tested with plain numbers.

use dictate_services::audio::resample;
use dictate_services::dictation::live::{LiveJob, LiveTranscript, LiveWindow};

/// How many times a commit job that fails is tried again before it is given up on. The next one then starts after it, so
/// that a job that always fails (a language that Whisper does not know) cannot hold the live text still for the rest of
/// a long take.
pub const COMMIT_RETRIES: u32 = 2;

/// The live text, and the audio it is made from.
#[derive(Debug, Default)]
pub struct LiveText {
    window: LiveWindow,
    transcript: LiveTranscript,
    /// All the audio so far at 16 kHz, brought there piece by piece as it came.
    audio: Vec<f32>,
    /// How many of the samples of the microphone are in `audio` already.
    converted: usize,
    /// A job is running: no other is started until it has ended.
    busy: bool,
    /// How many times the commit job of the window has failed in a row.
    commit_failures: u32,
}

impl LiveText {
    pub fn new() -> Self {
        Self::default()
    }

    /// Brings the samples of the microphone (`raw`, all of them since the start, at `rate` Hz) that aren't in yet to 16 kHz.
    ///
    /// Each piece is resampled by itself: at its edges the filter doesn't see the audio next to it, which a ear can't hear and
    /// Whisper doesn't mind. The recording that goes to the speech API and to the file is resampled whole, at the end.
    pub fn feed(&mut self, raw: &[f32], rate: u32) {
        if raw.len() > self.converted {
            self.audio
                .extend(resample::to_16k(&raw[self.converted..], rate));
            self.converted = raw.len();
        }
    }

    /// The next transcription to run, with the audio it takes, and the job is now running: none is offered until
    /// [`LiveText::finished`] says it is over (a tick that comes while a long one runs is skipped).
    pub fn next_job(&mut self) -> Option<(LiveJob, Vec<f32>)> {
        let job = self.window.next_job(self.audio.len(), self.busy)?;
        self.busy = true;
        let samples = self.audio[job.range.clone()].to_vec();
        Some((job, samples))
    }

    /// A job is over. A commit job's text is final and the window moves past it; a failed one is offered again, up to
    /// [`COMMIT_RETRIES`] times, and then given up on. The text of any other job is the latest guess. A failed one changes
    /// no text. `lang` is the BCP-47 code the words are joined by.
    pub fn finished(&mut self, job: &LiveJob, result: Result<String, String>, lang: &str) {
        self.busy = false;
        match (job.commit, result) {
            (true, Ok(text)) => {
                self.transcript.commit(&text, lang);
                self.window.commit_done();
                self.commit_failures = 0;
            }
            (true, Err(_)) => {
                self.commit_failures += 1;
                if self.commit_failures > COMMIT_RETRIES {
                    self.window.commit_done();
                    self.commit_failures = 0;
                }
            }
            (false, Ok(text)) => self.transcript.set_partial(&text),
            (false, Err(_)) => {}
        }
    }

    /// A job is running.
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// What is final, and the latest guess for the rest.
    pub fn text(&self) -> (&str, &str) {
        (&self.transcript.committed, &self.transcript.partial)
    }

    /// All the words, joined as the language joins them.
    pub fn full(&self, lang: &str) -> String {
        self.transcript.full(lang)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dictate_services::dictation::live::{LIVE_MIN_SAMPLES, LIVE_WINDOW_SAMPLES};

    const SECOND: usize = 16_000;

    fn seconds(n: usize) -> Vec<f32> {
        vec![0.25; n * SECOND]
    }

    #[test]
    fn live_text_needs_half_a_second_before_the_first_job() {
        let mut live = LiveText::new();
        live.feed(&vec![0.1; LIVE_MIN_SAMPLES - 1], 16_000);
        assert!(live.next_job().is_none());
        live.feed(&vec![0.1; LIVE_MIN_SAMPLES], 16_000);
        let (job, samples) = live.next_job().unwrap();
        assert_eq!(job.range, 0..LIVE_MIN_SAMPLES);
        assert_eq!(samples.len(), LIVE_MIN_SAMPLES);
    }

    #[test]
    fn live_text_resamples_what_came_in_since_the_last_tick() {
        let mut live = LiveText::new();
        let mut raw = vec![0.1_f32; 48_000];
        live.feed(&raw, 48_000);
        raw.extend(vec![0.1_f32; 48_000]);
        live.feed(&raw, 48_000);
        // Feeding what was fed already adds nothing.
        live.feed(&raw, 48_000);
        let (job, _) = live.next_job().unwrap();
        // Two seconds at 48 kHz are two seconds at 16 kHz, give or take the edge of a resampled piece.
        assert!(
            job.range.end.abs_diff(2 * SECOND) <= 320,
            "{} samples",
            job.range.end
        );
    }

    #[test]
    fn live_text_shows_the_latest_guess_after_what_is_final() {
        let mut live = LiveText::new();
        live.feed(&seconds(3), 16_000);
        let (job, _) = live.next_job().unwrap();
        live.finished(&job, Ok("hello".into()), "en-US");
        assert_eq!(live.text(), ("", "hello"));

        live.feed(&seconds(25), 16_000);
        // The window is 20 s long by now: the first 20 s are a commit job.
        let (job, _) = live.next_job().unwrap();
        assert!(job.commit);
        assert_eq!(job.range.len(), LIVE_WINDOW_SAMPLES);
        live.finished(&job, Ok("hello there".into()), "en-US");
        assert_eq!(live.text(), ("hello there", ""));

        let (job, _) = live.next_job().unwrap();
        live.finished(&job, Ok("how are".into()), "en-US");
        assert_eq!(live.full("en-US"), "hello there how are");
    }

    #[test]
    fn live_ticks_skip_while_a_job_runs() {
        let mut live = LiveText::new();
        live.feed(&seconds(3), 16_000);
        let (job, _) = live.next_job().unwrap();
        assert!(live.is_busy());

        live.feed(&seconds(4), 16_000);
        assert!(
            live.next_job().is_none(),
            "a second job while the first runs"
        );
        live.feed(&seconds(5), 16_000);
        assert!(live.next_job().is_none());

        live.finished(&job, Ok("hello".into()), "en-US");
        assert!(!live.is_busy());
        assert!(live.next_job().is_some());
    }

    #[test]
    fn a_failed_partial_job_changes_no_text_and_is_not_repeated() {
        let mut live = LiveText::new();
        live.feed(&seconds(3), 16_000);
        let (job, _) = live.next_job().unwrap();
        live.finished(&job, Ok("hello".into()), "en-US");
        let (job, _) = live.next_job().unwrap();
        live.finished(&job, Err("whisper failed".into()), "en-US");
        assert_eq!(live.text(), ("", "hello"), "the guess before it stays");
        assert!(!live.is_busy());
    }

    #[test]
    fn live_failing_commit_is_retried_then_released() {
        let mut live = LiveText::new();
        live.feed(&seconds(25), 16_000);

        // The commit job fails and is offered again, as often as `COMMIT_RETRIES` allows.
        let first_range = {
            let (job, _) = live.next_job().unwrap();
            assert!(job.commit);
            live.finished(&job, Err("no memory".into()), "en-US");
            job.range
        };
        for _ in 0..COMMIT_RETRIES {
            let (job, _) = live.next_job().unwrap();
            assert!(job.commit);
            assert_eq!(job.range, first_range, "the same job again");
            live.finished(&job, Err("no memory".into()), "en-US");
        }

        // Then it is given up on, and the text goes on after it: live text is not held still for the rest of the take.
        let (job, _) = live.next_job().unwrap();
        assert!(!job.commit, "{job:?}");
        assert_eq!(job.range.start, first_range.end);
        live.finished(&job, Ok("still here".into()), "en-US");
        assert_eq!(live.text(), ("", "still here"));
    }

    #[test]
    fn a_commit_that_works_after_a_failure_starts_the_count_again() {
        let mut live = LiveText::new();
        live.feed(&seconds(65), 16_000);
        let (job, _) = live.next_job().unwrap();
        live.finished(&job, Err("hiccup".into()), "en-US");
        let (job, _) = live.next_job().unwrap();
        live.finished(&job, Ok("first twenty".into()), "en-US");

        // The next window fails twice, and is tried the full number of times from there.
        for _ in 0..=COMMIT_RETRIES {
            let (job, _) = live.next_job().unwrap();
            assert!(job.commit);
            assert_eq!(job.range.start, LIVE_WINDOW_SAMPLES);
            live.finished(&job, Err("hiccup".into()), "en-US");
        }
        let (job, _) = live.next_job().unwrap();
        assert_eq!(job.range.start, 2 * LIVE_WINDOW_SAMPLES);
    }
}
