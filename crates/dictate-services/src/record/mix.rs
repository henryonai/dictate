//! The recording's stems put together into the one track of `audio.m4a`: what the rest of the app (the transcript,
//! the player) works from. The stems are read a second at a time, so the mix of a long recording never is in memory.

use super::stem::{self, into_io_error, to_i16, STEM_RATE};
use crate::media::encode::{m4a_writer, EncodeError, M4aSink, M4A_RATE};
use dictate_core::meetings::library::files;
use hound::{WavReader, WavWriter};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

/// The file of the mix when no encoder can write the m4a: the same sound, uncompressed.
pub const WAV: &str = "recording.wav";

// The mix goes to the encoder as it is read: the two must be the same rate.
const _: () = assert!(M4A_RATE == STEM_RATE);

/// The file the stems were mixed into.
#[derive(Debug, Clone, PartialEq)]
pub struct Mixed {
    /// `audio.m4a` in the recording's folder, or `recording.wav` when there was no encoder for it.
    pub path: PathBuf,
    /// Seconds: the longest of the stems.
    pub duration: f64,
}

/// Mixes `stems` (each a finished stem: 16-bit, mono, `STEM_RATE`) into `folder/audio.m4a`. They are added sample
/// by sample, a stem that ends early counting as silence from there on, and the sum is clamped to full scale.
/// When the m4a cannot be started (`EncodeError`: no AAC encoder here) the mix is written as `folder/recording.wav`
/// instead, so that the sound is not lost. No stems is `InvalidInput`; a stem that is not one is `InvalidData`.
/// Cancelling stops the mix with `Interrupted` and leaves no file.
pub fn mix_stems(
    stems: &[PathBuf],
    folder: &Path,
    ffmpeg: Option<&Path>,
    cancel: &CancellationToken,
) -> io::Result<Mixed> {
    mix_with(stems, folder, cancel, |out| m4a_writer(out, ffmpeg))
}

/// `mix_stems` with the way of starting the m4a left to the caller.
fn mix_with(
    stems: &[PathBuf],
    folder: &Path,
    cancel: &CancellationToken,
    start_m4a: impl Fn(&Path) -> Result<Box<dyn M4aSink>, EncodeError>,
) -> io::Result<Mixed> {
    if stems.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "there is no audio to mix",
        ));
    }
    let mut readers = stems
        .iter()
        .map(|path| open_stem(path))
        .collect::<io::Result<Vec<_>>>()?;

    let m4a = folder.join(files::AUDIO);
    let (path, mut sink) = match start_m4a(&m4a) {
        Ok(sink) => (m4a, sink),
        Err(_) => {
            let wav = folder.join(WAV);
            let sink: Box<dyn M4aSink> = Box::new(WavSink::create(&wav)?);
            (wav, sink)
        }
    };
    // A sink that is dropped on the way out (an error, the cancel) takes its unfinished file with it.
    let samples = mix_blocks(&mut readers, sink.as_mut(), cancel)?;
    sink.finish()?;
    Ok(Mixed {
        path,
        duration: samples as f64 / f64::from(STEM_RATE),
    })
}

fn open_stem(path: &Path) -> io::Result<WavReader<BufReader<File>>> {
    let reader = WavReader::open(path).map_err(into_io_error)?;
    if reader.spec() != stem::spec() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is not a stem: it is not mono, 16-bit audio at 48 kHz",
                path.display()
            ),
        ));
    }
    Ok(reader)
}

/// Adds the stems up a second at a time and gives each second to `sink`. Returns the samples that were mixed.
fn mix_blocks(
    readers: &mut [WavReader<BufReader<File>>],
    sink: &mut dyn M4aSink,
    cancel: &CancellationToken,
) -> io::Result<u64> {
    let mut block = vec![0.0_f32; STEM_RATE as usize];
    let mut total = 0_u64;
    loop {
        if cancel.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "the mix was cancelled",
            ));
        }
        block.fill(0.0);
        let mut longest = 0;
        for reader in readers.iter_mut() {
            let mut read = 0;
            for (slot, sample) in block.iter_mut().zip(reader.samples::<i16>()) {
                *slot += f32::from(sample.map_err(into_io_error)?) / 32_768.0;
                read += 1;
            }
            longest = longest.max(read);
        }
        if longest == 0 {
            return Ok(total);
        }
        let mixed = &mut block[..longest];
        for sample in mixed.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
        sink.write(mixed)?;
        total += longest as u64;
    }
}

/// `recording.wav`, written like an m4a: to a `.part` file that takes its name when the audio is over, and is
/// removed when it is dropped before that.
struct WavSink {
    writer: Option<WavWriter<BufWriter<File>>>,
    part: PathBuf,
    out: PathBuf,
}

impl WavSink {
    fn create(out: &Path) -> io::Result<WavSink> {
        let mut part = OsString::from(out.as_os_str());
        part.push(".part");
        let part = PathBuf::from(part);
        let writer = WavWriter::create(&part, stem::spec()).map_err(into_io_error)?;
        Ok(WavSink {
            writer: Some(writer),
            part,
            out: out.to_path_buf(),
        })
    }
}

impl M4aSink for WavSink {
    fn write(&mut self, mono_48k: &[f32]) -> io::Result<()> {
        let writer = self
            .writer
            .as_mut()
            .expect("the writer is there until finish");
        for &sample in mono_48k {
            writer.write_sample(to_i16(sample)).map_err(into_io_error)?;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        let writer = self
            .writer
            .take()
            .expect("the writer is there until finish");
        writer.finalize().map_err(into_io_error)?;
        fs::rename(&self.part, &self.out)
    }
}

impl Drop for WavSink {
    fn drop(&mut self) {
        // Only a sink that was not finished still has its writer (and, on Windows, its file open); or one whose
        // rename failed, which leaves the part where it is: either way the part is of no use.
        self.writer.take();
        let _ = fs::remove_file(&self.part);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::decode::MonoDecoder;
    use crate::media::test_support::{read_all, real_ffmpeg};
    use crate::record::stem::StemWriter;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A stem of `samples` (mono, at `STEM_RATE`) in `dir`.
    fn stem(dir: &Path, name: &str, samples: &[f32]) -> PathBuf {
        let path = dir.join(name);
        let mut writer = StemWriter::create(&path, STEM_RATE).unwrap();
        writer.push(Duration::ZERO, samples).unwrap();
        writer.finish().unwrap();
        path
    }

    fn tone(amplitude: f32, seconds: f32) -> Vec<f32> {
        (0..(STEM_RATE as f32 * seconds) as usize)
            .map(|i| {
                amplitude * (std::f32::consts::TAU * 440.0 * i as f32 / STEM_RATE as f32).sin()
            })
            .collect()
    }

    /// What the mix hands to the encoder, block by block, and whether it was told that the audio is over.
    #[derive(Clone, Default)]
    struct Heard {
        blocks: Arc<Mutex<Vec<Vec<f32>>>>,
        finished: Arc<Mutex<bool>>,
    }

    impl Heard {
        fn sink(&self) -> Result<Box<dyn M4aSink>, EncodeError> {
            Ok(Box::new(self.clone()))
        }

        fn samples(&self) -> Vec<f32> {
            self.blocks.lock().unwrap().concat()
        }

        fn block_lengths(&self) -> Vec<usize> {
            self.blocks.lock().unwrap().iter().map(Vec::len).collect()
        }
    }

    impl M4aSink for Heard {
        fn write(&mut self, mono_48k: &[f32]) -> io::Result<()> {
            self.blocks.lock().unwrap().push(mono_48k.to_vec());
            Ok(())
        }

        fn finish(self: Box<Self>) -> io::Result<()> {
            *self.finished.lock().unwrap() = true;
            Ok(())
        }
    }

    fn mix_into(heard: &Heard, stems: &[PathBuf], folder: &Path) -> io::Result<Mixed> {
        mix_with(stems, folder, &CancellationToken::new(), |_| heard.sink())
    }

    /// The ffmpeg to give the mix when this machine can write an m4a (`Some(None)`: with its own encoder), and
    /// `None` when it cannot, so that the test is skipped. Linux has no encoder of its own.
    fn encoder_or_skip() -> Option<Option<PathBuf>> {
        let ffmpeg = real_ffmpeg();
        (cfg!(any(target_os = "macos", windows)) || ffmpeg.is_some()).then_some(ffmpeg)
    }

    #[test]
    fn two_stems_become_one_m4a() {
        let Some(ffmpeg) = encoder_or_skip() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let stems = [
            stem(dir.path(), "mic.wav", &tone(0.3, 2.0)),
            stem(dir.path(), "system.wav", &tone(0.2, 3.0)),
        ];

        let mixed = mix_stems(
            &stems,
            dir.path(),
            ffmpeg.as_deref(),
            &CancellationToken::new(),
        )
        .unwrap();

        assert_eq!(mixed.path, dir.path().join("audio.m4a"));
        assert!((mixed.duration - 3.0).abs() < 0.05, "{} s", mixed.duration);
        let mut decoder = MonoDecoder::open(&mixed.path, ffmpeg.as_deref()).unwrap();
        let samples = read_all(&mut decoder);
        assert!(
            samples.len() as f64 / 48_000.0 >= 2.9,
            "{} samples",
            samples.len()
        );
        assert!(samples.iter().any(|s| s.abs() > 0.2));
    }

    #[test]
    fn a_loud_sum_is_clamped() {
        let Some(ffmpeg) = encoder_or_skip() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let stems = [
            stem(dir.path(), "mic.wav", &tone(1.0, 1.0)),
            stem(dir.path(), "system.wav", &tone(1.0, 1.0)),
        ];

        let mixed = mix_stems(
            &stems,
            dir.path(),
            ffmpeg.as_deref(),
            &CancellationToken::new(),
        )
        .unwrap();

        let mut decoder = MonoDecoder::open(&mixed.path, ffmpeg.as_deref()).unwrap();
        let samples = read_all(&mut decoder);
        let loudest = samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        // What the encoder got is exactly clamped (the test above); AAC may overshoot a signal that is flat at full
        // scale by a few percent when it is decoded, where the unclamped sum would come back at 2.0.
        assert!(loudest <= 1.1, "a sample of {loudest}");
        assert!(loudest > 0.9, "the sum is not silent: {loudest}");
    }

    #[test]
    fn the_encoder_is_given_the_sum_clamped_to_full_scale() {
        let dir = tempfile::tempdir().unwrap();
        let stems = [
            stem(dir.path(), "a.wav", &[0.75, -0.75, 0.25, 1.0]),
            stem(dir.path(), "b.wav", &[0.75, -0.75, -0.5, 1.0]),
        ];
        let heard = Heard::default();

        mix_into(&heard, &stems, dir.path()).unwrap();

        // 0.75, 0.25 and 0.5 are exact in 16 bits; 1.0 is stored as 32 767 / 32 768, and twice that is clamped.
        assert_eq!(heard.samples(), [1.0, -1.0, -0.25, 1.0]);
    }

    #[test]
    fn a_shorter_stem_counts_as_silence() {
        let dir = tempfile::tempdir().unwrap();
        let stems = [
            stem(dir.path(), "long.wav", &vec![0.25; 72_000]),
            stem(dir.path(), "short.wav", &vec![0.5; 24_000]),
        ];
        let heard = Heard::default();

        let mixed = mix_into(&heard, &stems, dir.path()).unwrap();

        let samples = heard.samples();
        assert_eq!(samples.len(), 72_000);
        assert!(samples[..24_000].iter().all(|&s| s == 0.75));
        assert!(samples[24_000..].iter().all(|&s| s == 0.25));
        assert_eq!(mixed.duration, 1.5);
        assert!(*heard.finished.lock().unwrap());
    }

    #[test]
    fn the_audio_goes_to_the_encoder_a_second_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let stems = [stem(dir.path(), "mic.wav", &vec![0.1; 120_000])];
        let heard = Heard::default();

        mix_into(&heard, &stems, dir.path()).unwrap();

        assert_eq!(heard.block_lengths(), [48_000, 48_000, 24_000]);
    }

    #[test]
    fn without_an_encoder_the_mix_is_a_wav() {
        let dir = tempfile::tempdir().unwrap();
        let stems = [
            stem(dir.path(), "mic.wav", &vec![0.25; 96_000]),
            stem(dir.path(), "system.wav", &vec![0.5; 48_000]),
        ];

        let mixed = mix_with(&stems, dir.path(), &CancellationToken::new(), |_| {
            Err(EncodeError::Unavailable)
        })
        .unwrap();

        assert_eq!(mixed.path, dir.path().join("recording.wav"));
        assert_eq!(mixed.duration, 2.0);
        assert!(!dir.path().join("audio.m4a").exists());
        let mut reader = hound::WavReader::open(&mixed.path).unwrap();
        let spec = reader.spec();
        assert_eq!(
            (spec.sample_rate, spec.channels, spec.bits_per_sample),
            (48_000, 1, 16)
        );
        let samples: Vec<i16> = reader.samples::<i16>().map(Result::unwrap).collect();
        assert_eq!(samples.len(), 96_000);
        assert!(samples[..48_000].iter().all(|&s| s == 24_576));
        assert!(samples[48_000..].iter().all(|&s| s == 8_192));
    }

    /// Linux has no AAC encoder of its own: without an ffmpeg `mix_stems` itself falls back to the wav.
    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn without_ffmpeg_on_this_system_mix_stems_writes_a_wav() {
        let dir = tempfile::tempdir().unwrap();
        let stems = [stem(dir.path(), "mic.wav", &vec![0.25; 4_800])];

        let mixed = mix_stems(&stems, dir.path(), None, &CancellationToken::new()).unwrap();

        assert_eq!(mixed.path, dir.path().join("recording.wav"));
        assert!(!dir.path().join("audio.m4a").exists());
    }

    #[test]
    fn an_encoder_that_failed_to_start_leaves_the_wav_too() {
        let dir = tempfile::tempdir().unwrap();
        let stems = [stem(dir.path(), "mic.wav", &vec![0.25; 4_800])];

        let mixed = mix_with(&stems, dir.path(), &CancellationToken::new(), |_| {
            Err(EncodeError::Failed("no space".into()))
        })
        .unwrap();

        assert_eq!(mixed.path, dir.path().join("recording.wav"));
        assert!(mixed.path.exists());
    }

    #[test]
    fn the_wav_of_the_mix_is_clamped_and_has_no_part_file_left() {
        let dir = tempfile::tempdir().unwrap();
        let stems = [
            stem(dir.path(), "a.wav", &[0.75, -0.75]),
            stem(dir.path(), "b.wav", &[0.75, -0.75]),
        ];

        let mixed = mix_with(&stems, dir.path(), &CancellationToken::new(), |_| {
            Err(EncodeError::Unavailable)
        })
        .unwrap();

        let samples: Vec<i16> = hound::WavReader::open(&mixed.path)
            .unwrap()
            .samples::<i16>()
            .map(Result::unwrap)
            .collect();
        assert_eq!(samples, [i16::MAX, i16::MIN]);
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn no_stems_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let error = mix_stems(&[], dir.path(), None, &CancellationToken::new()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_file_that_is_not_a_stem_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("other.wav");
        crate::audio::wav::write_16k_mono(&other, &[0.0; 160]).unwrap();
        let heard = Heard::default();

        let error = mix_into(&heard, &[other], dir.path()).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(heard.blocks.lock().unwrap().is_empty());
    }

    #[test]
    fn a_missing_stem_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let heard = Heard::default();
        let error = mix_into(&heard, &[dir.path().join("gone.wav")], dir.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_cancelled_mix_stops_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let stems = [stem(dir.path(), "mic.wav", &vec![0.1; 120_000])];
        let cancel = CancellationToken::new();
        cancel.cancel();

        let error = mix_with(&stems, dir.path(), &cancel, |_| {
            Err(EncodeError::Unavailable)
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(!dir.path().join("recording.wav").exists());
        assert!(!dir.path().join("recording.wav.part").exists());
    }

    #[test]
    fn a_cancelled_mix_does_not_finish_the_encoder() {
        let dir = tempfile::tempdir().unwrap();
        let stems = [stem(dir.path(), "mic.wav", &vec![0.1; 120_000])];
        let cancel = CancellationToken::new();
        cancel.cancel();
        let heard = Heard::default();

        let error = mix_with(&stems, dir.path(), &cancel, |_| heard.sink()).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(!*heard.finished.lock().unwrap());
    }
}
