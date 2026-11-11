//! One source of the recording's sound (the microphone, the system's audio) as a WAV file of its own: mono, 48 kHz,
//! 16 bits, laid out on the recording's clock. A buffer that arrives late leaves silence behind it, and the stems
//! are put together only when the recording ends (`record::mix`), so that nothing a stem holds is lost if the program
//! is.

use crate::audio::resample::StreamResampler;
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;
use std::time::Duration;

/// The rate of every stem.
pub const STEM_RATE: u32 = 48_000;
/// The longest silence that a gap in the audio is filled with. A source that says its next buffer is further away
/// than this (no recording lasts so long) has a clock that is wrong, and it is the audio that is believed, not the
/// clock: the audio goes on where the last of it ended.
pub const MAX_GAP_FILL: Duration = Duration::from_secs(6 * 3600);
/// How far (20 ms, in samples at `STEM_RATE`) the time of a buffer may be from where the audio so far ends, before
/// `StemWriter::push` puts the buffer back in its place. Buffers are stamped when they are taken from the device,
/// and that jitters by milliseconds.
const TOLERANCE: u64 = 20 * STEM_RATE as u64 / 1000;
/// The audio between two flushes of the WAV header: a second of it.
const FLUSH_EVERY: u64 = STEM_RATE as u64;

pub(super) fn spec() -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: STEM_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

/// Writes one stem as the audio of its source comes in.
pub struct StemWriter {
    writer: hound::WavWriter<BufWriter<File>>,
    source_rate: u32,
    resampler: StreamResampler,
    /// Samples of the source given to `resampler` since it was made.
    fed: u64,
    /// Where in the stem (in samples at `STEM_RATE`) the first sample that `resampler` puts out goes.
    base: u64,
    /// Samples in the file.
    written: u64,
    /// `written` at the last flush.
    flushed_at: u64,
}

impl StemWriter {
    /// Starts `path` for a source that delivers mono audio at `source_rate` Hz.
    pub fn create(path: &Path, source_rate: u32) -> io::Result<StemWriter> {
        if source_rate == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the source of a stem needs a sample rate",
            ));
        }
        let mut writer = hound::WavWriter::create(path, spec()).map_err(into_io_error)?;
        // The header is written to the file now, and not with the first second of sound: a source that says nothing for a
        // while (a system that plays no sound) must not leave a file that is shorter than a header for a crash to find.
        writer.flush().map_err(into_io_error)?;
        Ok(StemWriter {
            writer,
            source_rate,
            resampler: StreamResampler::new(source_rate, STEM_RATE),
            fed: 0,
            base: 0,
            written: 0,
            flushed_at: 0,
        })
    }

    /// Adds `mono` (at the source's rate), whose first sample is from `at` on the recording's active clock. Silence
    /// is written when `at` is more than 20 ms past the end of the audio so far (the source had nothing to say for
    /// a while: a system that plays no sound delivers no buffers), and the start of `mono` is dropped when `at` is
    /// more than 20 ms before it (a stamp that is late must not make the stem longer than the recording). Within
    /// 20 ms the buffer follows on from the last as it is. The WAV header is flushed for every second of audio
    /// written, so that a stem that was never finished is still a file that can be read, up to the last flush.
    pub fn push(&mut self, at: Duration, mono: &[f32]) -> io::Result<()> {
        if mono.is_empty() {
            return Ok(());
        }
        let want = samples_at(at);
        let end = self.end();
        let mut mono = mono;
        if want > end + TOLERANCE {
            if want - end <= samples_at(MAX_GAP_FILL) {
                self.fill_gap_to(want)?;
            }
        } else if want + TOLERANCE < end {
            let overlap = (end - want) * u64::from(self.source_rate) / u64::from(STEM_RATE);
            mono = &mono[(overlap as usize).min(mono.len())..];
        }
        self.fed += mono.len() as u64;
        let out = self.resampler.push(mono);
        self.write(&out)?;
        self.flush_when_due()
    }

    /// Ends the stem and returns its length in seconds.
    pub fn finish(mut self) -> io::Result<f64> {
        let tail = self.take_resampler().finish();
        self.write(&tail)?;
        let seconds = self.written as f64 / f64::from(STEM_RATE);
        self.writer.finalize().map_err(into_io_error)?;
        Ok(seconds)
    }

    /// Where the audio that has been pushed ends in the stem, in samples at `STEM_RATE`: the file, and what the
    /// resampler still holds.
    fn end(&self) -> u64 {
        let rate = u64::from(self.source_rate);
        self.base + (self.fed * u64::from(STEM_RATE) + rate / 2) / rate
    }

    /// The source goes on at `rate` Hz from the next `push`, and not at the rate it had: a track that was started again
    /// (on the device that the system has now, after the one it had was lost) may have another. What the resampler
    /// holds of the audio before is written, and what comes after is resampled from `rate`. Where it goes in the stem
    /// is still given by the time of the next `push`: what is between is silence. Nothing happens when `rate` is the
    /// rate the source has.
    pub fn set_source_rate(&mut self, rate: u32) -> io::Result<()> {
        if rate == self.source_rate {
            return Ok(());
        }
        if rate == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the source of a stem needs a sample rate",
            ));
        }
        self.restart_resampler(rate)?;
        self.base = self.written;
        Ok(())
    }

    /// Lets the resampler bring out what it holds and goes on with a new one, for a source at `rate` (the old one holds
    /// the end of the audio before: it must not leak into what comes after). Nothing is fed to the new one yet.
    fn restart_resampler(&mut self, rate: u32) -> io::Result<()> {
        let fresh = StreamResampler::new(rate, STEM_RATE);
        let tail = std::mem::replace(&mut self.resampler, fresh).finish();
        self.write(&tail)?;
        self.source_rate = rate;
        self.fed = 0;
        Ok(())
    }

    /// Writes silence up to `want`, after what the resampler holds, and goes on with a new resampler.
    fn fill_gap_to(&mut self, want: u64) -> io::Result<()> {
        self.restart_resampler(self.source_rate)?;
        for _ in self.written..want {
            self.writer.write_sample(0_i16).map_err(into_io_error)?;
            self.written += 1;
        }
        self.base = self.written;
        Ok(())
    }

    /// The resampler, with one that does nothing in its place.
    fn take_resampler(&mut self) -> StreamResampler {
        std::mem::replace(
            &mut self.resampler,
            StreamResampler::new(STEM_RATE, STEM_RATE),
        )
    }

    fn write(&mut self, samples: &[f32]) -> io::Result<()> {
        for &sample in samples {
            self.writer
                .write_sample(to_i16(sample))
                .map_err(into_io_error)?;
        }
        self.written += samples.len() as u64;
        Ok(())
    }

    fn flush_when_due(&mut self) -> io::Result<()> {
        if self.written - self.flushed_at >= FLUSH_EVERY {
            self.writer.flush().map_err(into_io_error)?;
            self.flushed_at = self.written;
        }
        Ok(())
    }
}

/// Makes the stem at `path`, a finished one, `seconds` long with silence at its end. A stem that is that long
/// already is left alone.
pub fn pad_to(path: &Path, seconds: f64) -> io::Result<()> {
    let mut writer = hound::WavWriter::append(path).map_err(into_io_error)?;
    if writer.spec() != spec() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a stem: it is not mono, 16-bit audio at 48 kHz",
        ));
    }
    // `as` saturates: a length that is negative, or not a number, is 0.
    let want = (seconds * f64::from(STEM_RATE)).round() as u64;
    for _ in u64::from(writer.duration())..want {
        writer.write_sample(0_i16).map_err(into_io_error)?;
    }
    writer.finalize().map_err(into_io_error)
}

/// The sample of the stem that `at` is, rounded.
fn samples_at(at: Duration) -> u64 {
    ((at.as_nanos() * u128::from(STEM_RATE) + 500_000_000) / 1_000_000_000) as u64
}

/// A float from -1 to 1 as 16-bit PCM: the inverse of the `/ 32 768` that reading does.
pub(super) fn to_i16(sample: f32) -> i16 {
    // `as` saturates: values beyond the range (and NaN, as 0) land on the ends.
    (sample * 32_768.0).round() as i16
}

pub(super) fn into_io_error(error: hound::Error) -> io::Error {
    match error {
        hound::Error::IoError(error) => error,
        other => io::Error::new(io::ErrorKind::InvalidData, other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// `seconds` of a sine wave at `hz`, sampled at `rate`.
    fn sine(hz: f32, rate: u32, seconds: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn stem_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("mic.wav")
    }

    fn read(path: &Path) -> (hound::WavSpec, Vec<i16>) {
        let mut reader = hound::WavReader::open(path).unwrap();
        let samples = reader.samples::<i16>().map(Result::unwrap).collect();
        (reader.spec(), samples)
    }

    fn seconds(samples: &[i16]) -> f64 {
        samples.len() as f64 / f64::from(STEM_RATE)
    }

    fn at(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    #[test]
    fn a_stem_holds_what_was_pushed_at_48k() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let mut stem = StemWriter::create(&path, 44_100).unwrap();
        stem.push(at(0.0), &sine(440.0, 44_100, 1.0)).unwrap();
        let written = stem.finish().unwrap();

        assert!((written - 1.0).abs() < 0.01, "{written} s");
        let (spec, samples) = read(&path);
        assert_eq!(
            (spec.sample_rate, spec.channels, spec.bits_per_sample),
            (48_000, 1, 16)
        );
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        assert!((seconds(&samples) - 1.0).abs() < 0.01);
        // The pitch survives the resampling: 440 Hz crosses zero 880 times a second.
        let crossings = samples
            .windows(2)
            .filter(|w| (w[0] < 0) != (w[1] < 0))
            .count();
        assert!(crossings.abs_diff(880) <= 10, "{crossings} crossings");
    }

    #[test]
    fn a_gap_in_the_audio_becomes_silence() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let tone = sine(440.0, 44_100, 1.0);
        let mut stem = StemWriter::create(&path, 44_100).unwrap();
        stem.push(at(0.0), &tone).unwrap();
        stem.push(at(3.0), &tone).unwrap();
        let written = stem.finish().unwrap();

        assert!((written - 4.0).abs() < 0.01, "{written} s");
        let (_, samples) = read(&path);
        assert!((seconds(&samples) - 4.0).abs() < 0.01);
        assert!(samples[..48_000].iter().any(|&s| s.abs() > 1000));
        assert!(samples[48_000..144_000].iter().all(|&s| s == 0));
        assert!(samples[144_000..].iter().any(|&s| s.abs() > 1000));
    }

    #[test]
    fn audio_that_starts_late_is_preceded_by_silence() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        stem.push(at(1.0), &sine(440.0, 48_000, 0.5)).unwrap();
        let written = stem.finish().unwrap();

        assert!((written - 1.5).abs() < 0.01, "{written} s");
        let (_, samples) = read(&path);
        assert!(samples[..48_000].iter().all(|&s| s == 0));
        assert!(samples[48_000..].iter().any(|&s| s.abs() > 1000));
    }

    #[test]
    fn an_overlap_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let tone = sine(440.0, 44_100, 1.0);
        let mut stem = StemWriter::create(&path, 44_100).unwrap();
        stem.push(at(0.0), &tone).unwrap();
        stem.push(at(0.5), &tone).unwrap();
        let written = stem.finish().unwrap();

        assert!((written - 1.5).abs() < 0.01, "{written} s");
        assert!((seconds(&read(&path).1) - 1.5).abs() < 0.01);
    }

    #[test]
    fn a_buffer_that_is_a_few_milliseconds_off_is_taken_as_it_comes() {
        // Callback times jitter: a stem that is cut and padded for every 5 ms would be full of clicks.
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let block = vec![0.25_f32; 4_800];
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        stem.push(at(0.0), &block).unwrap();
        stem.push(at(0.095), &block).unwrap();
        stem.push(at(0.205), &block).unwrap();
        stem.finish().unwrap();

        assert_eq!(read(&path).1.len(), 14_400);
    }

    #[test]
    fn a_gap_past_twenty_milliseconds_is_filled() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let block = vec![0.25_f32; 4_800];
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        stem.push(at(0.0), &block).unwrap();
        stem.push(at(0.1 + 0.03), &block).unwrap();
        stem.finish().unwrap();

        let (_, samples) = read(&path);
        assert_eq!(samples.len(), 4_800 + 1_440 + 4_800);
        assert!(samples[4_800..6_240].iter().all(|&s| s == 0));
    }

    #[test]
    fn a_time_that_cannot_be_right_is_not_believed() {
        // A timestamp a day ahead must not make a stem of 17 GB.
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let block = vec![0.25_f32; 4_800];
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        stem.push(at(0.0), &block).unwrap();
        stem.push(MAX_GAP_FILL + Duration::from_secs(60), &block)
            .unwrap();
        stem.finish().unwrap();

        assert_eq!(read(&path).1.len(), 9_600);
    }

    #[test]
    fn a_stem_cut_off_mid_recording_is_still_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let copy = dir.path().join("copy.wav");
        let tone = sine(440.0, 48_000, 2.5);
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        for (i, block) in tone.chunks(4_800).enumerate() {
            stem.push(at(i as f64 * 0.1), block).unwrap();
        }
        // The program is gone before `finish`: what is on the disk now is what a later run finds.
        std::fs::copy(&path, &copy).unwrap();

        let (_, samples) = read(&copy);
        assert!(seconds(&samples) >= 1.0, "{} s", seconds(&samples));
        drop(stem);
    }

    #[test]
    fn a_stem_of_nothing_is_a_valid_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let written = StemWriter::create(&path, 44_100).unwrap().finish().unwrap();
        assert_eq!(written, 0.0);
        assert!(read(&path).1.is_empty());
    }

    #[test]
    fn a_stem_that_was_just_made_is_a_file_that_can_be_read() {
        // A source that has not delivered a second of sound (a system that plays none, on Windows) leaves a stem that a
        // crash finds as it is: it must be a WAV with no samples, and not a file with a header that was never written.
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let stem = StemWriter::create(&path, 48_000).unwrap();

        assert!(std::fs::metadata(&path).unwrap().len() >= 44);
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.duration(), 0);
        drop(reader);
        drop(stem);
    }

    #[test]
    fn a_source_without_a_rate_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let error = StemWriter::create(&stem_in(&dir), 0).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_stem_in_a_missing_folder_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("mic.wav");
        assert!(StemWriter::create(&path, 48_000).is_err());
    }

    #[test]
    fn samples_beyond_full_scale_are_clipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        stem.push(at(0.0), &[2.0, -2.0, 0.5]).unwrap();
        stem.finish().unwrap();
        assert_eq!(read(&path).1, [i16::MAX, i16::MIN, 16_384]);
    }

    /// How often the signal crosses zero in `samples[from..to]`.
    fn crossings(samples: &[i16], from: usize, to: usize) -> usize {
        samples[from..to]
            .windows(2)
            .filter(|w| (w[0] < 0) != (w[1] < 0))
            .count()
    }

    #[test]
    fn a_source_that_goes_on_at_another_rate_is_resampled_from_that_rate() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        stem.push(at(0.0), &sine(440.0, 48_000, 1.0)).unwrap();
        // The device is lost, and the track is started again on one that has 44.1 kHz, 2 s later.
        stem.set_source_rate(44_100).unwrap();
        stem.push(at(3.0), &sine(880.0, 44_100, 1.0)).unwrap();
        let written = stem.finish().unwrap();

        assert!((written - 4.0).abs() < 0.01, "{written} s");
        let (_, samples) = read(&path);
        assert!((seconds(&samples) - 4.0).abs() < 0.01);
        assert!(crossings(&samples, 0, 48_000).abs_diff(880) <= 10);
        assert!(samples[48_100..143_900].iter().all(|&s| s == 0));
        // 880 Hz crosses zero 1 760 times a second: samples taken as 48 kHz would be 1 900.
        let later = crossings(&samples, 144_100, 191_900);
        assert!(later.abs_diff(1_760) <= 20, "{later} crossings");
    }

    #[test]
    fn a_source_that_goes_on_at_another_rate_without_a_gap_does_so_seamlessly() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        stem.push(at(0.0), &sine(440.0, 48_000, 1.0)).unwrap();
        stem.set_source_rate(16_000).unwrap();
        stem.push(at(1.0), &sine(440.0, 16_000, 1.0)).unwrap();
        stem.finish().unwrap();

        let (_, samples) = read(&path);
        assert!(
            (seconds(&samples) - 2.0).abs() < 0.01,
            "{} s",
            seconds(&samples)
        );
        // No silence was put between them.
        assert!(samples[40_000..56_000].iter().any(|&s| s.abs() > 1000));
        assert!(crossings(&samples, 52_000, 92_000).abs_diff(733) <= 10);
    }

    #[test]
    fn the_rate_it_already_has_is_left_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let mut stem = StemWriter::create(&path, 44_100).unwrap();
        stem.push(at(0.0), &sine(440.0, 44_100, 1.0)).unwrap();
        stem.set_source_rate(44_100).unwrap();
        stem.push(at(1.0), &sine(440.0, 44_100, 1.0)).unwrap();
        stem.finish().unwrap();

        let (_, samples) = read(&path);
        assert!((seconds(&samples) - 2.0).abs() < 0.01);
        assert!(crossings(&samples, 0, 96_000).abs_diff(1_760) <= 20);
    }

    #[test]
    fn a_source_cannot_go_on_without_a_rate() {
        let dir = tempfile::tempdir().unwrap();
        let mut stem = StemWriter::create(&stem_in(&dir), 48_000).unwrap();
        let error = stem.set_source_rate(0).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn pad_to_lengthens_a_stem() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        stem.push(at(0.0), &sine(440.0, 48_000, 1.0)).unwrap();
        stem.finish().unwrap();

        pad_to(&path, 2.5).unwrap();

        let (spec, samples) = read(&path);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(samples.len(), 120_000);
        assert!(samples[..48_000].iter().any(|&s| s.abs() > 1000));
        assert!(samples[48_000..].iter().all(|&s| s == 0));
    }

    #[test]
    fn pad_to_leaves_a_longer_stem_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = stem_in(&dir);
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        stem.push(at(0.0), &sine(440.0, 48_000, 1.0)).unwrap();
        stem.finish().unwrap();

        pad_to(&path, 0.5).unwrap();

        assert_eq!(read(&path).1.len(), 48_000);
    }

    #[test]
    fn pad_to_refuses_a_file_that_is_not_a_stem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("other.wav");
        crate::audio::wav::write_16k_mono(&path, &[0.0; 160]).unwrap();
        let error = pad_to(&path, 2.0).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
