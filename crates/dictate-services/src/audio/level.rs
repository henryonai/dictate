//! The level shown by the recording meter.

/// The quietest level the meter shows as anything: RMS at -55 dB below full scale.
const FLOOR_DB: f32 = -55.0;
/// How many dB the meter spans above [`FLOOR_DB`]: it is full at -10 dB.
const SPAN_DB: f32 = 45.0;
/// The RMS used for pure silence, so the logarithm stays finite.
const MIN_RMS: f32 = 1e-7;

/// A meter level from 0 to 1 for one buffer of samples: its RMS in dB, with -55 dB at 0 and -10 dB at 1.
pub fn level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    // Summed as f64: a long buffer of small squares would otherwise lose the small ones.
    let sum_of_squares: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (sum_of_squares / samples.len() as f64).sqrt() as f32;
    let db = 20.0 * rms.max(MIN_RMS).log10();
    ((db - FLOOR_DB) / SPAN_DB).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A constant signal whose RMS is exactly `db` decibels below full scale.
    fn constant_at(db: f32) -> Vec<f32> {
        vec![10.0_f32.powf(db / 20.0); 1024]
    }

    #[test]
    fn level_of_silence_is_zero() {
        assert_eq!(level(&[0.0; 1024]), 0.0);
    }

    #[test]
    fn level_of_nothing_is_zero() {
        assert_eq!(level(&[]), 0.0);
    }

    #[test]
    fn level_of_full_scale_sine_is_one() {
        let sine: Vec<f32> = (0..1024)
            .map(|i| (std::f32::consts::TAU * i as f32 / 64.0).sin())
            .collect();
        assert_eq!(level(&sine), 1.0);
    }

    #[test]
    fn level_at_minus_55_db_is_zero() {
        assert!(level(&constant_at(-55.0)).abs() < 1e-3);
        // Quieter than that stays at zero: the meter does not go negative.
        assert_eq!(level(&constant_at(-70.0)), 0.0);
    }

    #[test]
    fn level_at_minus_10_db_is_one() {
        assert!((level(&constant_at(-10.0)) - 1.0).abs() < 1e-3);
        // Louder than that stays at one.
        assert_eq!(level(&constant_at(-3.0)), 1.0);
    }

    #[test]
    fn level_halfway_between_minus_55_and_minus_10_db_is_half() {
        assert!((level(&constant_at(-32.5)) - 0.5).abs() < 1e-3);
    }
}
