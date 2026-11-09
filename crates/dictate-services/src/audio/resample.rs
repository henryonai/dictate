//! Resampling a finished recording to 16 kHz.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

/// The rate Whisper listens at.
const TARGET_RATE: u32 = 16_000;
/// Frames the resampler takes in one go. A longer recording is fed through it chunk by chunk.
const CHUNK_FRAMES: usize = 1024;
/// How many seconds of silence may follow the clip to bring the last of it out. The resampler works on blocks of
/// `rate / gcd(rate, 16000)` frames, which is a whole second at the oddest rates, and puts nothing out until it has one:
/// the first calls (and the last, at the end of the clip) may come back empty.
const FLUSH_SECONDS: usize = 2;

/// `samples` (mono, at `rate` Hz) resampled to 16 kHz, with a low-pass filter so that nothing above 8 kHz folds
/// back into the speech. A recording that is already at 16 kHz is returned as it is.
///
/// Empty when there is nothing to resample, when `rate` is 0, or when no resampler can be built for the rate.
pub fn to_16k(samples: &[f32], rate: u32) -> Vec<f32> {
    if rate == TARGET_RATE {
        return samples.to_vec();
    }
    if samples.is_empty() || rate == 0 {
        return Vec::new();
    }
    resample(samples, rate).unwrap_or_default()
}

/// Runs the whole clip through rubato's FFT resampler one chunk at a time.
///
/// rubato's own `process_all` is not used: it cuts the filter's delay off the start in a way that leaves the first
/// tens of milliseconds wrong. Here the delay is cut by hand: the clip is followed by silence until the resampler
/// has put out its delay plus every sample that was asked for, and then the delay is dropped from the front.
fn resample(samples: &[f32], rate: u32) -> Option<Vec<f32>> {
    let mut resampler = Fft::<f32>::new(
        rate as usize,
        TARGET_RATE as usize,
        CHUNK_FRAMES,
        1,
        FixedSync::Input,
    )
    .ok()?;
    // Whole samples of 16 kHz audio that the clip makes, rounded up.
    let wanted = (samples.len() as u64 * u64::from(TARGET_RATE)).div_ceil(u64::from(rate)) as usize;
    let delay = resampler.output_delay();

    let mut out: Vec<f32> = Vec::with_capacity(wanted + delay + resampler.output_frames_max());
    let mut chunk = vec![0.0_f32; resampler.input_frames_max()];
    let mut consumed = 0;
    // The most rounds that are worth it: the clip, and then the silence that flushes the filter. Past that the resampler
    // is not going to put out more, and a loop that never ends is worse than the audio that did come out.
    let budget = (samples.len() + FLUSH_SECONDS * rate as usize).div_ceil(CHUNK_FRAMES) + 1;
    for _ in 0..budget {
        if out.len() >= wanted + delay {
            break;
        }
        let frames = resampler.input_frames_next();
        let from = consumed.min(samples.len());
        let to = (consumed + frames).min(samples.len());
        chunk[..to - from].copy_from_slice(&samples[from..to]);
        chunk[to - from..frames].fill(0.0);
        consumed += frames;

        let input = InterleavedSlice::new(&chunk[..frames], 1, frames).ok()?;
        // An empty block is not the end: the resampler is collecting the input of its first block.
        let block = resampler.process(&input, None).ok()?.take_data();
        out.extend_from_slice(&block);
    }
    if out.len() <= delay {
        return None;
    }
    out.drain(..delay);
    out.truncate(wanted);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `seconds` of a sine wave at `hz`, sampled at `rate`.
    fn sine(hz: f32, rate: u32, seconds: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn zero_crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    #[test]
    fn resample_48k_sine_keeps_length_and_pitch() {
        let out = to_16k(&sine(440.0, 48_000, 1.0), 48_000);
        assert!(
            out.len().abs_diff(16_000) <= 160,
            "got {} samples",
            out.len()
        );
        let crossings = zero_crossings(&out);
        assert!(
            crossings.abs_diff(880) <= 10,
            "got {crossings} zero crossings"
        );
    }

    #[test]
    fn resample_16k_is_identity() {
        let input = sine(300.0, 16_000, 0.25);
        assert_eq!(to_16k(&input, 16_000), input);
    }

    #[test]
    fn resample_44100_length() {
        let out = to_16k(&sine(440.0, 44_100, 1.0), 44_100);
        assert!(
            out.len().abs_diff(16_000) <= 160,
            "got {} samples",
            out.len()
        );
    }

    #[test]
    fn resample_keeps_the_loudness_of_a_tone() {
        let out = to_16k(&sine(440.0, 48_000, 1.0), 48_000);
        // The whole output but the last 100 samples, where the filter runs out of input.
        let peak = out[..out.len() - 100]
            .iter()
            .fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.02, "peak {peak}");
    }

    #[test]
    fn resample_removes_what_16_khz_cannot_hold() {
        // 12 kHz is above the 8 kHz limit of 16 kHz audio: plain decimation would fold it down to a 4 kHz tone.
        let out = to_16k(&sine(12_000.0, 48_000, 1.0), 48_000);
        // Not the first 20 and the last 100 samples: the tone starts and stops abruptly there, and a filter cannot
        // tell that from a click (the very first sample reaches 0.08, the one after it 0.014).
        let peak = out[20..out.len() - 100]
            .iter()
            .fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak < 0.02, "peak {peak}");
    }

    /// `seconds` of a sine wave at `hz` sampled at `rate`, each sample worked out exactly (in f64).
    fn exact_sine(hz: f64, rate: u32, seconds: f64) -> Vec<f32> {
        let n = (f64::from(rate) * seconds).round() as usize;
        (0..n)
            .map(|i| (0.5 * (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin()) as f32)
            .collect()
    }

    #[test]
    fn resample_matches_an_ideal_16_khz_sine_from_the_first_sample() {
        // The whole output, head included, must be the sine that was recorded: the filter's delay is cut off the
        // start and not left in as a few milliseconds of the wrong waveform.
        for rate in [48_000, 44_100, 96_000, 22_050, 8_000] {
            let out = to_16k(&exact_sine(440.0, rate, 1.0), rate);
            let ideal = exact_sine(440.0, 16_000, 1.0);
            assert_eq!(out.len(), ideal.len(), "{rate} Hz: length");
            // Not the last 100 samples: the end of the clip is where the filter runs out of input. A tone that is one
            // sample late is off by 0.086 at worst, so the 0.06 allows the small error of the filter, 0.044 at most
            // (at the very first samples, where the tone starts abruptly), but not a shift.
            let checked = out.len() - 100;
            let (at, worst) = out[..checked]
                .iter()
                .zip(&ideal)
                .map(|(a, b)| (a - b).abs())
                .enumerate()
                .fold((0, 0.0_f32), |w, (i, e)| if e > w.1 { (i, e) } else { w });
            assert!(
                worst < 0.06,
                "{rate} Hz: off by {worst} at output sample {at}"
            );
        }
    }

    #[test]
    fn resample_of_nothing_is_nothing() {
        assert!(to_16k(&[], 48_000).is_empty());
    }

    #[test]
    fn resample_with_a_rate_of_zero_is_nothing() {
        assert!(to_16k(&[0.1; 100], 0).is_empty());
    }

    #[test]
    fn resample_a_clip_shorter_than_one_chunk_stays_aligned() {
        // 800 frames at 44.1 kHz (18 ms) is less than the 1024 frames the resampler takes at a time. An impulse in
        // the middle must come out where it belongs: sample 441 of 44 100 is sample 160 of 16 000.
        let mut clip = vec![0.0_f32; 800];
        clip[441] = 1.0;
        let out = to_16k(&clip, 44_100);

        assert_eq!(out.len(), 291, "800 * 16000 / 44100 rounded up");
        let loudest = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(i, _)| i)
            .unwrap();
        assert!(
            loudest.abs_diff(160) <= 2,
            "the impulse landed at {loudest}"
        );
    }

    #[test]
    fn resample_unusual_rates_still_gives_audio_of_the_right_length_and_pitch() {
        // Rates that are a little off the usual ones (a device that runs at 47.952 kHz, or a sample clock at 44.101 kHz) have no
        // small ratio to 16 kHz: the first blocks that the resampler puts out may be empty, which is not the end of the audio.
        for rate in [
            47_952, 44_101, 22_051, 11_025, 12_345, 32_001, 96_001, 192_000,
        ] {
            let input = exact_sine(440.0, rate, 1.0);
            let out = to_16k(&input, rate);
            assert!(!out.is_empty(), "{rate} Hz: no audio came out");
            assert_eq!(out.len(), 16_000, "{rate} Hz: length");
            let crossings = zero_crossings(&out);
            assert!(
                crossings.abs_diff(880) <= 10,
                "{rate} Hz: {crossings} zero crossings"
            );
        }
    }

    #[test]
    fn resample_a_short_clip_at_an_unusual_rate_still_comes_out_whole() {
        // The shortest take that is kept is a fifth of a second: far less than the block of input that the resampler waits for.
        let input = exact_sine(440.0, 47_952, 0.2);
        let out = to_16k(&input, 47_952);
        assert_eq!(out.len(), 3_200);
        assert!(
            out.iter().any(|s| s.abs() > 0.3),
            "the clip came out silent"
        );
    }
}
