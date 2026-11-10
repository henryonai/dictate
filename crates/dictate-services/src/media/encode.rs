//! Writing `audio.m4a`: AAC-LC, mono, 48 kHz, 96 kbps, one block at a time.
//!
//! The pipeline gives each block of 48 kHz audio to `M4aSink::write` as it comes out of the decoder and the resampler,
//! and the encoder takes it at once: nothing holds the recording, so a recording of two hours costs the same memory as one
//! of a minute. The system's own encoder does the work where there is one (AudioToolbox on macOS, the Media Foundation
//! Sink Writer on Windows) and ffmpeg everywhere else, or when Media Foundation is not there (the "N" editions of
//! Windows leave it out unless the Media Feature Pack is installed).
//!
//! Every sink writes to `<out>.<nonce>.part` (a name of its own) and renames it to `<out>` when it is finished, so that a
//! quit or a crash never leaves a half-written `audio.m4a`; a sink that is dropped without being finished removes its
//! `.part` file.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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

/// How often a file that Windows will not let go of is tried again, and how long that goes on: 40 tries 50 ms apart, about two
/// seconds.
const RETRIES: u32 = 40;
const PAUSE: Duration = Duration::from_millis(50);

/// Whether a failure on a file is Windows saying "not now" rather than "no": a refusal (a virus scanner has the new file open), or
/// a sharing or lock violation (error 32, 33: a program that was just ended still has it open). Only Windows is asked.
fn is_busy(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::PermissionDenied || matches!(error.raw_os_error(), Some(32 | 33))
}

/// Whether Windows is saying "not now", on Windows; no other system has such a thing to wait out.
fn is_busy_on_windows(error: &io::Error) -> bool {
    cfg!(windows) && is_busy(error)
}

/// Does `attempt`, and again up to `retries` times, `pause` apart, for as long as it fails in a way that `busy` says is for a
/// moment. The result of the last try is the result.
fn retrying<T>(
    mut attempt: impl FnMut() -> io::Result<T>,
    busy: impl Fn(&io::Error) -> bool,
    retries: u32,
    pause: Duration,
) -> io::Result<T> {
    let mut tried_again = 0;
    loop {
        match attempt() {
            Err(e) if tried_again < retries && busy(&e) => {
                tried_again += 1;
                std::thread::sleep(pause);
            }
            result => return result,
        }
    }
}

/// Puts the finished `.part` file in the place of `out`, which it replaces if there is one. Windows can refuse for a
/// moment (a virus scanner has the new file open, or the program that wrote it is still being cleaned up): it is tried again for
/// about two seconds.
fn publish(part: &Path, out: &Path) -> io::Result<()> {
    retrying(
        || std::fs::rename(part, out),
        is_busy_on_windows,
        RETRIES,
        PAUSE,
    )
}

/// Removes the `.part` file of a sink that was not finished (it may not be there). Windows can refuse for a moment, while
/// the program that wrote it is still being cleaned up or a virus scanner has the file open: it is tried again for about
/// two seconds.
fn remove_part(part: &Path) {
    let _ = retrying(
        || std::fs::remove_file(part),
        is_busy_on_windows,
        RETRIES,
        PAUSE,
    );
}

/// The file a sink writes to until it is finished: `audio.m4a` becomes `audio.m4a.<nonce>.part`, a name that no other sink
/// has. A job that is cancelled while its encoder is still going ends late (the blocking work looks at the cancel between
/// blocks), possibly after the job that replaced it has started on the same `audio.m4a`: each removes only its own file.
/// The sweep at the start of the app removes every `*.part`, whatever the nonce.
fn part_path(out: &Path) -> PathBuf {
    static SINKS: AtomicU64 = AtomicU64::new(0);
    let nonce = format!(
        "{:x}-{:x}",
        std::process::id(),
        SINKS.fetch_add(1, Ordering::Relaxed)
    );
    let mut part = OsString::from(out);
    part.push(format!(".{nonce}.part"));
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

    /// Waits up to `seconds` for `condition`, which becomes true on its own (the encoder is another process or thread),
    /// and says whether it did.
    pub fn becomes_true_within(seconds: u64, condition: impl Fn() -> bool) -> bool {
        let give_up = Instant::now() + Duration::from_secs(seconds);
        while !condition() {
            if Instant::now() >= give_up {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    /// Waits for `condition` as long as it takes (10 s at most), and fails when it never becomes true.
    pub fn wait_for(what: &str, condition: impl Fn() -> bool) {
        assert!(becomes_true_within(10, condition), "waited 10 s for {what}");
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
        assert!(
            part_files(&out).is_empty(),
            "the .part file was left behind"
        );
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
        let mut sink = start(&out);

        for block in 0..600 {
            sink.write(&tone_from(block * BLOCK, BLOCK)).unwrap();
            if block == 299 {
                // 30 s of audio is about 360 KB of AAC: far more than an encoder's own buffers hold.
                wait_for("the encoder to write to the file as it goes", || {
                    part_files(&out)
                        .iter()
                        .any(|part| fs::metadata(part).is_ok_and(|m| m.len() > 100_000))
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
        let mut sink = start(&out);
        sink.write(&tone(M4A_RATE as usize)).unwrap();
        // Given a moment, so that there is a file to remove when the encoder makes one early. It is not waited for:
        // an encoder that is another process makes its file when it likes (ffmpeg 6 only once it has read enough of
        // its input to start), and the drop has to leave nothing behind whether the file is there yet or not.
        becomes_true_within(2, || !part_files(&out).is_empty());

        drop(sink);

        assert!(!out.exists());
        assert!(part_files(&out).is_empty());
    }

    /// The `.part` files of `out` that are in its folder now (whatever the nonce in their names).
    pub fn part_files(out: &Path) -> Vec<PathBuf> {
        let name = out.file_name().unwrap().to_string_lossy().into_owned();
        let Ok(entries) = fs::read_dir(out.parent().unwrap()) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                let found = entry.file_name().to_string_lossy().into_owned();
                found.starts_with(&format!("{name}.")) && found.ends_with(".part")
            })
            .map(|entry| entry.path())
            .collect()
    }

    /// A job that was cancelled while its encoder was still going ends late, after the job that replaced it has started
    /// writing the same `audio.m4a`: what the old one removes is its own `.part` file, and the new one still finishes and
    /// publishes its own audio.
    pub fn an_old_sink_that_ends_late_leaves_the_new_one_alone(start: Start) {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("audio.m4a");
        let mut old = start(&out);
        old.write(&tone(M4A_RATE as usize)).unwrap();
        let mut new = start(&out);
        new.write(&tone(2 * M4A_RATE as usize)).unwrap();
        // An encoder that is another process makes its file when it likes: both have theirs before the old one ends.
        wait_for("the two sinks to have a file each", || {
            part_files(&out).len() == 2
        });

        drop(old);

        assert_eq!(
            part_files(&out).len(),
            1,
            "the old sink took the new one's file"
        );
        new.finish().unwrap();
        assert!(part_files(&out).is_empty(), "a .part file is left");
        let mut decoder = MonoDecoder::open(&out, None).unwrap();
        let samples = read_all(&mut decoder).len();
        // The new sink's two seconds, not the old one's one.
        assert!(
            samples.abs_diff(2 * M4A_RATE as usize) <= 2 * M4A_RATE as usize * 5 / 100,
            "{samples} samples"
        );
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
    fn an_old_encode_that_ends_late_leaves_the_new_one_alone() {
        let Some(start) = system_writer() else { return };

        an_old_sink_that_ends_late_leaves_the_new_one_alone(&start);
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
    fn a_sharing_violation_is_busy_as_a_refusal_is_and_nothing_else_is() {
        // 32 is ERROR_SHARING_VIOLATION and 33 ERROR_LOCK_VIOLATION: what Windows says while a program that is being ended still has
        // the file open.
        for code in [32, 33] {
            assert!(is_busy(&io::Error::from_raw_os_error(code)), "{code}");
        }
        assert!(is_busy(&io::Error::from(io::ErrorKind::PermissionDenied)));
        assert!(!is_busy(&io::Error::from(io::ErrorKind::NotFound)));
        assert!(!is_busy(&io::Error::from_raw_os_error(2)));
    }

    #[test]
    fn a_busy_attempt_is_tried_again_until_it_works() {
        let mut busy_for = 5;
        let mut tries = 0;

        let result = retrying(
            || {
                tries += 1;
                if busy_for > 0 {
                    busy_for -= 1;
                    Err(io::Error::from_raw_os_error(32))
                } else {
                    Ok("done")
                }
            },
            is_busy,
            RETRIES,
            Duration::ZERO,
        );

        assert_eq!(result.unwrap(), "done");
        assert_eq!(tries, 6);
    }

    #[test]
    fn an_attempt_that_stays_busy_is_given_up_after_the_retries_and_says_why() {
        let mut tries = 0;

        let result: io::Result<()> = retrying(
            || {
                tries += 1;
                Err(io::Error::from_raw_os_error(33))
            },
            is_busy,
            3,
            Duration::ZERO,
        );

        assert_eq!(result.unwrap_err().raw_os_error(), Some(33));
        assert_eq!(tries, 4, "the first try and three more");
    }

    #[test]
    fn an_attempt_that_fails_for_another_reason_is_not_tried_again() {
        let mut tries = 0;

        let result: io::Result<()> = retrying(
            || {
                tries += 1;
                Err(io::Error::from(io::ErrorKind::NotFound))
            },
            is_busy,
            RETRIES,
            Duration::ZERO,
        );

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(tries, 1);
    }

    #[test]
    fn windows_is_given_about_two_seconds_to_let_go_of_a_file() {
        let total = PAUSE * RETRIES;

        assert!(total >= Duration::from_millis(1_900), "{total:?}");
        assert!(total <= Duration::from_millis(2_500), "{total:?}");
    }

    #[test]
    fn each_sink_has_a_part_file_of_its_own_next_to_the_audio() {
        let out = Path::new("/a b/audio.m4a");

        let first = part_path(out);
        let second = part_path(out);

        assert_ne!(first, second);
        for part in [&first, &second] {
            assert_eq!(part.parent(), Some(Path::new("/a b")));
            let name = part.file_name().unwrap().to_string_lossy();
            assert!(
                name.starts_with("audio.m4a.") && name.ends_with(".part"),
                "{name}"
            );
        }
    }
}
