//! Resampling a finished recording to 16 kHz.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

/// The rate Whisper listens at.
const TARGET_RATE: u32 = 16_000;
/// Frames the resampler takes in one go. A longer recording is fed through it chunk by chunk.
const CHUNK_FRAMES: usize = 1024;

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

fn resample(samples: &[f32], rate: u32) -> Option<Vec<f32>> {
    let mut resampler = Fft::<f32>::new(
        rate as usize,
        TARGET_RATE as usize,
        CHUNK_FRAMES,
        1,
        FixedSync::Input,
    )
    .ok()?;
    let input = InterleavedSlice::new(samples, 1, samples.len()).ok()?;
    let output = resampler.process_all(&input, samples.len(), None).ok()?;
    Some(output.take_data())
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
        // Skip the edges, where the filter is still settling.
        let middle = &out[1_000..out.len() - 1_000];
        let peak = middle.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.02, "peak {peak}");
    }

    #[test]
    fn resample_removes_what_16_khz_cannot_hold() {
        // 12 kHz is above the 8 kHz limit of 16 kHz audio: plain decimation would fold it down to a 4 kHz tone.
        let out = to_16k(&sine(12_000.0, 48_000, 1.0), 48_000);
        let middle = &out[1_000..out.len() - 1_000];
        let peak = middle.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak < 0.02, "peak {peak}");
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
    fn resample_a_clip_shorter_than_one_chunk() {
        // 100 ms at 44.1 kHz is less than the resampler's chunk size.
        let out = to_16k(&sine(440.0, 44_100, 0.1), 44_100);
        assert!(out.len().abs_diff(1_600) <= 16, "got {} samples", out.len());
    }
}
