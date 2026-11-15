//! Writing a 16 kHz mono recording to a WAV file.

use std::io;
use std::path::Path;

/// Writes `samples_16k` (mono, 16 kHz, -1 to 1) to `path` as a 16-bit PCM WAV file. Samples beyond full scale
/// are clipped.
pub fn write_16k_mono(path: &Path, samples_16k: &[f32]) -> io::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(into_io_error)?;
    for &sample in samples_16k {
        writer.write_sample(to_i16(sample)).map_err(into_io_error)?;
    }
    writer.finalize().map_err(into_io_error)
}

/// A float from -1 to 1 as 16-bit PCM: the inverse of the `/ 32 768` the WAV reader does.
fn to_i16(sample: f32) -> i16 {
    // `as` saturates: values beyond the range (and NaN, as 0) land on the ends.
    (sample * 32_768.0).round() as i16
}

fn into_io_error(e: hound::Error) -> io::Error {
    match e {
        hound::Error::IoError(e) => e,
        other => io::Error::other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::whisper::local::read_wav_16k_mono;

    #[test]
    fn wav_round_trip() {
        let samples: Vec<f32> = (0..16_000)
            .map(|i| 0.8 * (std::f32::consts::TAU * 440.0 * i as f32 / 16_000.0).sin())
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");

        write_16k_mono(&path, &samples).unwrap();
        let back = read_wav_16k_mono(&path).unwrap();

        assert_eq!(back.len(), samples.len());
        let worst = samples
            .iter()
            .zip(&back)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(worst < 1e-3, "worst error {worst}");
    }

    #[test]
    fn wav_is_16_bit_mono_at_16_khz() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        write_16k_mono(&path, &[0.0; 160]).unwrap();

        let spec = hound::WavReader::open(&path).unwrap().spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
    }

    #[test]
    fn wav_clips_samples_outside_full_scale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loud.wav");
        write_16k_mono(&path, &[2.0, -2.0, 1.0, -1.0]).unwrap();

        let ints: Vec<i16> = hound::WavReader::open(&path)
            .unwrap()
            .into_samples::<i16>()
            .map(Result::unwrap)
            .collect();
        assert_eq!(ints, [i16::MAX, i16::MIN, i16::MAX, i16::MIN]);
    }

    #[test]
    fn wav_of_nothing_is_a_valid_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.wav");
        write_16k_mono(&path, &[]).unwrap();
        assert!(read_wav_16k_mono(&path).unwrap().is_empty());
    }

    #[test]
    fn wav_in_a_missing_folder_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("take.wav");
        assert!(write_16k_mono(&path, &[0.0; 16]).is_err());
    }
}
