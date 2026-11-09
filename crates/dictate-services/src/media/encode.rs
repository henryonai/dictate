//! Writing `audio.m4a`: AAC-LC, mono, 48 kHz, 96 kbps, one block at a time.
//!
//! The pipeline gives each block of 48 kHz audio to `M4aSink::write` as it comes out of the decoder and the resampler,
//! and the encoder takes it at once: nothing holds the recording, so a recording of two hours costs the same memory as one
//! of a minute. The system's own encoder does the work where there is one (AudioToolbox on macOS, the Media Foundation
//! Sink Writer on Windows) and ffmpeg everywhere else, or when Media Foundation is not there (the "N" editions of
//! Windows leave it out unless the Media Feature Pack is installed).
//!
//! Every sink writes to `<out>.part` and renames it to `<out>` when it is finished, so that a quit or a crash never
//! leaves a half-written `audio.m4a`; a sink that is dropped without being finished removes its `.part` file.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

mod ffmpeg;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// The sample rate of what `M4aSink::write` takes, and of the file.
pub const M4A_RATE: u32 = 48_000;
/// The bit rate of the file. Media Foundation's AAC encoder takes only 96, 128, 160 and 192 kbps.
pub const M4A_BITRATE: u32 = 96_000;

/// Where an `audio.m4a` is written, block by block.
pub trait M4aSink: Send {
    /// Encodes the next block of mono audio at `M4A_RATE` (samples from -1.0 to 1.0). The block goes to the encoder
    /// as it is: the sink keeps none of it.
    fn write(&mut self, mono_48k: &[f32]) -> io::Result<()>;

    /// Ends the audio and puts the file in its place. When this fails there is no file.
    fn finish(self: Box<Self>) -> io::Result<()>;
}

/// Why no `audio.m4a` could be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// This system has no AAC encoder to use (Linux without ffmpeg, Windows without Media Foundation or ffmpeg).
    Unavailable,
    /// There is an encoder and it could not be started; the text is the detail.
    Failed(String),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::Unavailable => {
                f.write_str("There is no AAC encoder to save the audio with.")
            }
            EncodeError::Failed(m) => write!(f, "Could not start the audio encoder: {m}"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Starts writing `out` (an `audio.m4a`). The encoder is the first of these that works: AudioToolbox on macOS, Media
/// Foundation on Windows, then `ffmpeg` (when it is the path of one). Without any of them, or with one that could not
/// be started, the error is that of the system's own encoder, and ffmpeg's when that is the one that was tried last.
pub fn m4a_writer(out: &Path, ffmpeg: Option<&Path>) -> Result<Box<dyn M4aSink>, EncodeError> {
    // Media Foundation can be missing (an "N" edition of Windows), and the others can fail: ffmpeg is the next.
    let system = match system_encoder(out) {
        Ok(sink) => return Ok(sink),
        Err(error) => error,
    };
    match ffmpeg {
        Some(ffmpeg) => ffmpeg::create(out, ffmpeg),
        None => Err(system),
    }
}

/// The system's own encoder: AudioToolbox on macOS, Media Foundation on Windows, none on Linux.
fn system_encoder(out: &Path) -> Result<Box<dyn M4aSink>, EncodeError> {
    #[cfg(target_os = "macos")]
    return macos::create(out);
    #[cfg(windows)]
    return windows::create(out);
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = out;
        Err(EncodeError::Unavailable)
    }
}

/// Puts the finished `.part` file in the place of `out`, which it replaces if there is one. Windows can refuse for a
/// moment (a virus scanner has the new file open): it is tried again for a second.
fn publish(part: &Path, out: &Path) -> io::Result<()> {
    let mut retries = 0;
    loop {
        match std::fs::rename(part, out) {
            Err(e)
                if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied && retries < 20 =>
            {
                retries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

/// Removes the `.part` file of a sink that was not finished (it may not be there). Windows can refuse for a moment, while
/// the program that wrote it is still being cleaned up or a virus scanner has the file open: it is tried again for a
/// second.
fn remove_part(part: &Path) {
    for _ in 0..20 {
        match std::fs::remove_file(part) {
            Err(e) if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => return,
        }
    }
}

/// The file a sink writes to until it is finished: `audio.m4a` becomes `audio.m4a.part`.
fn part_path(out: &Path) -> PathBuf {
    let mut part = OsString::from(out);
    part.push(".part");
    PathBuf::from(part)
}

#[cfg(test)]
pub(crate) mod checks {
    //! What every encoder has to do, as functions that take the way to start one, so that each backend runs the same
    //! tests.

    use super::*;
    use crate::media::decode::MonoDecoder;
    use crate::media::test_support::{read_all, zero_crossings};
    use std::fs;
    use std::time::{Duration, Instant};

    /// Starts a sink that writes `out`.
    pub type Start<'a> = &'a dyn Fn(&Path) -> Box<dyn M4aSink>;

    /// The tone every test writes: 440 Hz at half scale.
    pub fn tone(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / M4A_RATE as f32).sin())
            .collect()
    }

    /// Waits for `condition`, which becomes true on its own (the encoder is another process or thread).
    pub fn wait_for(what: &str, condition: impl Fn() -> bool) {
        let give_up = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < give_up, "waited 10 s for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Three seconds of tone in blocks that do not divide it evenly come out as a playable file: an `ftyp` box with a
    /// brand an M4A player knows, 48 kHz, 3 s long, and the tone.
    pub fn writes_a_playable_m4a(start: Start) {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("audio.m4a");
        let mut sink = start(&out);

        sink.write(&[]).unwrap();
        for block in tone(3 * M4A_RATE as usize).chunks(4_801) {
            sink.write(block).unwrap();
        }
        sink.finish().unwrap();

        assert!(out.exists());
        assert!(!part_path(&out).exists(), "the .part file was left behind");
        let bytes = fs::read(&out).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp", "not an MP4 file");
        let brand = &bytes[8..12];
        assert!(
            [b"M4A ", b"mp42", b"isom"].contains(&<&[u8; 4]>::try_from(brand).unwrap()),
            "brand {:?}",
            String::from_utf8_lossy(brand)
        );
        let mut decoder = MonoDecoder::open(&out, None).unwrap();
        assert_eq!(decoder.rate(), M4A_RATE);
        let samples = read_all(&mut decoder);
        assert!(
            samples.len().abs_diff(3 * M4A_RATE as usize) <= 3 * M4A_RATE as usize * 3 / 100,
            "{} samples",
            samples.len()
        );
        let crossings = zero_crossings(&samples);
        assert!(
            crossings.abs_diff(2_640) <= 2_640 * 3 / 100,
            "{crossings} zero crossings"
        );
    }

    /// A tone that goes on from where the block before it stopped.
    fn tone_from(start: usize, len: usize) -> Vec<f32> {
        (start..start + len)
            .map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / M4A_RATE as f32).sin())
            .collect()
    }

    /// 600 blocks of 0.1 s, one minute, go through the encoder as they come: halfway, what has been written is
    /// already in the file, so that the encoder does not wait for the end to write, or hold the recording in memory
    /// (a two-hour recording is 1.4 GB at 48 kHz). The file is a minute long, at the bit rate that was asked for.
    pub fn streams_without_buffering(start: Start) {
        const BLOCK: usize = 4_800;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("audio.m4a");
        let part = part_path(&out);
        let mut sink = start(&out);

        for block in 0..600 {
            sink.write(&tone_from(block * BLOCK, BLOCK)).unwrap();
            if block == 299 {
                // 30 s of audio is about 360 KB of AAC: far more than an encoder's own buffers hold.
                wait_for("the encoder to write to the file as it goes", || {
                    fs::metadata(&part).is_ok_and(|m| m.len() > 100_000)
                });
            }
        }
        sink.finish().unwrap();

        let size = fs::metadata(&out).unwrap().len();
        let wanted = u64::from(M4A_BITRATE) / 8 * 60;
        assert!(
            size.abs_diff(wanted) <= wanted * 15 / 100,
            "{size} bytes for a minute at {M4A_BITRATE} bit/s"
        );
        let mut decoder = MonoDecoder::open(&out, None).unwrap();
        let samples = read_all(&mut decoder).len();
        assert!(
            samples.abs_diff(60 * M4A_RATE as usize) <= 60 * M4A_RATE as usize * 2 / 100,
            "{samples} samples"
        );
    }

    /// A sink that is dropped without `finish` leaves neither the file nor its `.part`.
    pub fn leaves_no_file_when_dropped(start: Start) {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("audio.m4a");
        let part = part_path(&out);
        let mut sink = start(&out);
        sink.write(&tone(M4A_RATE as usize)).unwrap();
        // The file is there to be removed (an encoder that is another process makes it a moment after it starts).
        wait_for("the .part file", || part.exists());

        drop(sink);

        assert!(!out.exists());
        assert!(!part.exists());
    }

    /// A folder named in Vietnamese, with spaces and (where a file can have one) a colon in the name of the file.
    pub fn writes_into_a_vietnamese_folder(start: Start) {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Cuộc họp tháng 11");
        fs::create_dir_all(&folder).unwrap();
        let name = if cfg!(windows) {
            "bản ghi 1.m4a"
        } else {
            "bản ghi: 1.m4a"
        };
        let out = folder.join(name);
        let mut sink = start(&out);

        sink.write(&tone(M4A_RATE as usize)).unwrap();
        sink.finish().unwrap();

        let mut decoder = MonoDecoder::open(&out, None).unwrap();
        let samples = read_all(&mut decoder).len();
        // An AAC file runs a little over: the encoder's warm-up and the last packet, which symphonia does not trim.
        assert!(samples.abs_diff(48_000) <= 4_800, "{samples} samples");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1, "a .part is left");
    }
}

#[cfg(test)]
mod tests {
    use super::checks::*;
    use super::*;
    use crate::media::test_support::real_ffmpeg;

    /// Starts the encoder that `m4a_writer` uses on this system: the system's own on macOS and Windows (no ffmpeg is
    /// passed, so that a failure of Media Foundation is not hidden by ffmpeg taking over), ffmpeg on Linux. `None`
    /// when that is ffmpeg and there is none on this machine.
    fn system_writer() -> Option<impl Fn(&Path) -> Box<dyn M4aSink>> {
        let ffmpeg = if cfg!(any(target_os = "macos", windows)) {
            None
        } else {
            Some(real_ffmpeg()?)
        };
        Some(move |out: &Path| m4a_writer(out, ffmpeg.as_deref()).unwrap())
    }

    #[test]
    fn encode_writes_playable_m4a() {
        let Some(start) = system_writer() else { return };

        writes_a_playable_m4a(&start);
    }

    #[test]
    fn encode_streams_without_buffering() {
        let Some(start) = system_writer() else { return };

        streams_without_buffering(&start);
    }

    #[test]
    fn unfinished_sink_leaves_no_file() {
        let Some(start) = system_writer() else { return };

        leaves_no_file_when_dropped(&start);
    }

    #[test]
    fn encode_writes_into_a_folder_named_in_vietnamese() {
        let Some(start) = system_writer() else { return };

        writes_into_a_vietnamese_folder(&start);
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn no_encoder_is_unavailable() {
        let dir = tempfile::tempdir().unwrap();

        let error = m4a_writer(&dir.path().join("audio.m4a"), None).err();

        assert_eq!(error, Some(EncodeError::Unavailable));
    }

    #[test]
    fn the_part_file_is_the_name_with_part_added() {
        assert_eq!(
            part_path(Path::new("/a b/audio.m4a")),
            Path::new("/a b/audio.m4a.part")
        );
    }
}
