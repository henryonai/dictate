//! The microphone of a recording: the system's default input, as dictation uses it.

use std::sync::mpsc::Sender;

use super::{reporter, start_error, CaptureSource};
use crate::audio::capture::{default_input, Capture};
use crate::record::{RunningSource, SourceEvent, StartError, Track};

/// Starts the microphone as [`Track::Mic`]: its buffers are sent to `tx` as they are captured, in the microphone's own
/// rate and with its channels averaged to one. A stream that breaks (it is unplugged) ends the track with
/// `DeviceLost`.
pub fn start(tx: Sender<SourceEvent>) -> Result<Box<dyn RunningSource>, StartError> {
    let (device, supported) = default_input().map_err(|e| start_error(e, "microphone"))?;
    let rate = supported.sample_rate();
    let capture = Capture::start_device(&device, supported, reporter(Track::Mic, rate, tx))
        .map_err(|e| start_error(e, "microphone"))?;
    Ok(Box::new(CaptureSource(capture)))
}

#[cfg(test)]
mod tests {
    use super::super::reporter;
    use super::start;
    use crate::audio::capture::Tap;
    use crate::record::{EndReason, Note, SourceEvent, Track};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// What `rx` has, as the stamps of its buffers.
    fn audio_stamps(rx: &mpsc::Receiver<SourceEvent>) -> Vec<Instant> {
        rx.try_iter()
            .map(|event| match event {
                SourceEvent::Audio {
                    track: Track::Mic,
                    at,
                    rate: 48_000,
                    mono,
                } => {
                    assert_eq!(mono.len(), 960);
                    at
                }
                _ => panic!("not a buffer of the microphone at 48 kHz"),
            })
            .collect()
    }

    #[test]
    fn mic_events_carry_increasing_times() {
        let (tx, rx) = mpsc::channel();
        let mut tap = Tap::new(2, 48_000, reporter(Track::Mic, 48_000, tx));
        let start = Instant::now();
        // A stereo microphone hands over buffers of 960 frames, 20 ms, each when its last frame has been taken.
        for n in 1..=5 {
            tap.push_at(&[0.25_f32; 1_920], start + Duration::from_millis(20) * n);
        }

        let stamps = audio_stamps(&rx);
        assert_eq!(
            stamps,
            (0..5)
                .map(|n| start + Duration::from_millis(20) * n)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_stream_that_breaks_ends_the_track_as_a_lost_device() {
        let (tx, rx) = mpsc::channel();
        let mut report = reporter(Track::Mic, 48_000, tx);
        report(crate::audio::capture::Report::Error(
            crate::audio::capture::CaptureError::Stream("unplugged".into()),
        ));

        let Some(SourceEvent::Ended { track, reason }) = rx.try_recv().ok() else {
            panic!("the track did not end");
        };
        assert_eq!(track, Track::Mic);
        assert_eq!(reason, EndReason::DeviceLost("unplugged".into()));
    }

    #[test]
    fn a_microphone_that_hears_only_silence_is_a_note_and_not_an_end() {
        let (tx, rx) = mpsc::channel();
        let mut tap = Tap::new(1, 48_000, reporter(Track::Mic, 48_000, tx));
        let start = Instant::now();
        // 2 s of exact zeros: the audio goes on, once, and the track is not over.
        for n in 1..=100 {
            tap.push_at(&[0.0_f32; 960], start + Duration::from_millis(20) * n);
        }

        let events: Vec<SourceEvent> = rx.try_iter().collect();
        let notes: Vec<&SourceEvent> = events
            .iter()
            .filter(|event| matches!(event, SourceEvent::Note { .. }))
            .collect();
        assert_eq!(notes.len(), 1, "one note");
        assert!(matches!(
            notes[0],
            SourceEvent::Note {
                track: Track::Mic,
                note: Note::Silent
            }
        ));
        let audio = events
            .iter()
            .filter(|event| matches!(event, SourceEvent::Audio { .. }))
            .count();
        assert_eq!(audio, 100, "every buffer is kept");
        assert!(!events
            .iter()
            .any(|event| matches!(event, SourceEvent::Ended { .. })));
    }

    /// Records the real default microphone for a second. Needs a microphone, and on macOS the permission of the app that
    /// runs the test (the terminal).
    #[test]
    #[ignore = "needs a microphone"]
    fn the_real_microphone_delivers_stamped_audio() {
        let (tx, rx) = mpsc::channel();
        let began = Instant::now();
        let source = start(tx).expect("the default microphone opens");
        std::thread::sleep(Duration::from_secs(1));
        source.stop();

        let mut rates = Vec::new();
        let mut stamps = Vec::new();
        let mut samples = 0;
        for event in rx.try_iter() {
            match event {
                SourceEvent::Audio {
                    track: Track::Mic,
                    at,
                    rate,
                    mono,
                } => {
                    rates.push(rate);
                    stamps.push(at);
                    samples += mono.len();
                }
                SourceEvent::Ended { reason, .. } => panic!("the microphone ended: {reason:?}"),
                _ => panic!("not the microphone's audio"),
            }
        }
        eprintln!(
            "{} buffers, {samples} samples at {:?} Hz",
            stamps.len(),
            rates.first()
        );
        assert!(!stamps.is_empty(), "no audio in a second");
        assert!(
            rates.windows(2).all(|pair| pair[0] == pair[1]),
            "the rate changed"
        );
        assert!(
            stamps.windows(2).all(|pair| pair[0] < pair[1]),
            "the stamps do not increase"
        );
        assert!(
            stamps[0] >= began - Duration::from_millis(100),
            "the first buffer is from before the start"
        );
        assert!(
            samples as u32 > rates[0] / 2,
            "{samples} samples in a second"
        );
    }
}
