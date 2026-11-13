//! The sources of a recording that belong to the system: the microphone, the sound that the system plays, and the screen
//! (on macOS the screen and the system's sound come from one stream; on Windows the screen is the Graphics Capture API; on Linux it
//! is the ScreenCast portal and PipeWire).
//!
//! What a source does for the session, whichever it is:
//! - It is started on the session's thread by [`Capturer::start`], and stopped on that same thread: a microphone of some
//!   systems cannot be moved to another. `stop` comes back promptly (it does not wait for anything of the system's to
//!   happen) with the device or the child process released, and dropping a source stops it as well.
//! - The rate of a track is the same for as long as a start of it lives. A device that changes its format ends the
//!   track with [`EndReason::DeviceLost`], and the track is not carried on at another rate: the session starts it again,
//!   on the device that the system has then, and the stem goes on at the rate of the new one.
//! - Each buffer is stamped with when its first sample was captured: the time the system's callback ran, less the length
//!   of the buffer.
//! - Nothing that runs on the audio thread of the system blocks or logs: it sends on the channel and returns.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use serde::Serialize;

use crate::audio::capture::{Capture, CaptureError, Report};

use super::{
    Capturer, EndReason, Note, RecordOptions, RunningSource, SourceEvent, StartError, Track,
};

/// The screen of Linux, through the ScreenCast portal and PipeWire. What it keeps of the portal, how it ends and what it makes of a
/// picture is plain code, built (and its tests run) on every system; the capture itself is for Linux.
pub mod linux;
/// Windows records what the default output device plays through the same calls as a microphone. That is all this is,
/// so it is built on every system, to be checked on all of them.
pub mod loopback_windows;
pub use linux::{NoTokens, TokenStore};
/// The screen and the system's audio of macOS: one ScreenCaptureKit stream.
#[cfg(target_os = "macos")]
pub mod macos;
pub mod mic;
/// The Linux way to hear the system's audio: a `parec` child. It is plain Unix code, built (and its tests run) on
/// macOS too.
#[cfg(unix)]
pub mod parec_linux;
/// The screen of Windows, through the Graphics Capture API. What it lists and how it ends is plain code, built (and its
/// tests run) on every system; the capture itself is for Windows.
pub mod windows;

/// A display or a window that can be recorded, as the picker lists it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SourceInfo {
    /// What `Mode::Screen::display` or `Mode::Window::id` takes to name it: the system's own number for it, as text.
    pub id: String,
    /// What the picker shows.
    pub name: String,
    /// The application that owns a window.
    pub app: Option<String>,
}

/// What the system offers to record.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct Sources {
    /// The main display first.
    pub displays: Vec<SourceInfo>,
    pub windows: Vec<SourceInfo>,
    /// The system asks the person for the display or the window (a portal on Linux), and no list of ours is needed.
    pub picker_is_the_system: bool,
}

/// What can be recorded on this system, for the picker. It asks the system, which may take a moment.
#[cfg(target_os = "macos")]
pub fn list_sources() -> Result<Sources, StartError> {
    macos::list_sources()
}

/// What can be recorded on this system, for the picker. It asks the system, which may take a moment.
#[cfg(windows)]
pub fn list_sources() -> Result<Sources, StartError> {
    windows::list_sources()
}

/// What can be recorded on this system, for the picker: the portal's dialog, when the desktop has a portal to show it. It asks the
/// system, which may take a moment.
#[cfg(target_os = "linux")]
pub fn list_sources() -> Result<Sources, StartError> {
    linux::list_sources()
}

/// What can be recorded on this system, for the picker.
#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
pub fn list_sources() -> Result<Sources, StartError> {
    Err(not_available_yet())
}

/// The sources of a recording on this system.
pub struct OsCapturer {
    /// ffmpeg, for the sources that need it: none of the sound does.
    pub ffmpeg: Option<PathBuf>,
    /// Where the restore token of the screen is kept between recordings, which lets a recording start without the dialog of the desktop
    /// that asks what to share. Only Linux has one (the ScreenCast portal); the other systems do not look at it.
    pub tokens: Arc<dyn TokenStore>,
}

impl OsCapturer {
    /// A capturer that keeps no restore token: on Linux the desktop asks what to share every time. See [`Self::with_token_store`].
    pub fn new(ffmpeg: Option<PathBuf>) -> OsCapturer {
        OsCapturer {
            ffmpeg,
            tokens: Arc::new(NoTokens),
        }
    }

    /// The capturer, with `tokens` to keep the restore token of the screen in.
    pub fn with_token_store(mut self, tokens: Arc<dyn TokenStore>) -> OsCapturer {
        self.tokens = tokens;
        self
    }
}

impl Capturer for OsCapturer {
    fn start(
        &self,
        track: Track,
        options: &RecordOptions,
        tx: Sender<SourceEvent>,
    ) -> Result<Box<dyn RunningSource>, StartError> {
        match track {
            Track::Mic => mic::start(tx),
            Track::System => start_system_audio(options, tx),
            Track::Screen => start_screen(options, &self.tokens, tx),
        }
    }
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn not_available_yet() -> StartError {
    StartError::Source("not available yet".into())
}

#[cfg(windows)]
fn start_system_audio(
    _options: &RecordOptions,
    tx: Sender<SourceEvent>,
) -> Result<Box<dyn RunningSource>, StartError> {
    loopback_windows::start(tx)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn start_system_audio(
    _options: &RecordOptions,
    tx: Sender<SourceEvent>,
) -> Result<Box<dyn RunningSource>, StartError> {
    parec_linux::start(tx)
}

/// The system's audio of macOS comes with the screen capture, from the same stream.
#[cfg(target_os = "macos")]
fn start_system_audio(
    options: &RecordOptions,
    tx: Sender<SourceEvent>,
) -> Result<Box<dyn RunningSource>, StartError> {
    macos::start(Track::System, options, tx)
}

#[cfg(target_os = "macos")]
fn start_screen(
    options: &RecordOptions,
    _tokens: &Arc<dyn TokenStore>,
    tx: Sender<SourceEvent>,
) -> Result<Box<dyn RunningSource>, StartError> {
    macos::start(Track::Screen, options, tx)
}

#[cfg(windows)]
fn start_screen(
    options: &RecordOptions,
    _tokens: &Arc<dyn TokenStore>,
    tx: Sender<SourceEvent>,
) -> Result<Box<dyn RunningSource>, StartError> {
    windows::start(options, tx)
}

#[cfg(target_os = "linux")]
fn start_screen(
    options: &RecordOptions,
    tokens: &Arc<dyn TokenStore>,
    tx: Sender<SourceEvent>,
) -> Result<Box<dyn RunningSource>, StartError> {
    linux::start(options, Arc::clone(tokens), tx)
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn start_screen(
    _options: &RecordOptions,
    _tokens: &Arc<dyn TokenStore>,
    _tx: Sender<SourceEvent>,
) -> Result<Box<dyn RunningSource>, StartError> {
    Err(not_available_yet())
}

/// What a track makes of what a stream of the system reports: a buffer is the track's sound, a stream that breaks is
/// the end of the track (the session starts it again), and a microphone that delivers nothing but silence is a note.
fn reporter(
    track: Track,
    rate: u32,
    tx: Sender<SourceEvent>,
) -> impl FnMut(Report) + Clone + Send + 'static {
    move |report| {
        let event = match report {
            Report::Samples { at, mono } => SourceEvent::Audio {
                track,
                at,
                rate,
                mono,
            },
            Report::Error(CaptureError::Stream(why)) => SourceEvent::Ended {
                track,
                reason: EndReason::DeviceLost(why),
            },
            Report::Error(error @ CaptureError::NoDevice) => SourceEvent::Ended {
                track,
                reason: EndReason::DeviceLost(error.to_string()),
            },
            // The sound goes on being recorded, as it is, and the user is told: a microphone that the privacy switch of
            // Windows silences would else be a meeting recorded without a word of it. What the system plays is silent
            // when nothing plays, which is nothing to tell.
            Report::Error(CaptureError::Silent) if track == Track::Mic => SourceEvent::Note {
                track,
                note: Note::Silent,
            },
            Report::Error(CaptureError::Silent) => return,
        };
        // A session that is over has no one to read.
        let _ = tx.send(event);
    }
}

/// Why a stream of the system could not be opened, as the text of a start that failed: `what` is the thing that was to
/// be recorded.
fn start_error(error: CaptureError, what: &str) -> StartError {
    StartError::Source(match error {
        CaptureError::NoDevice => format!("no {what} found"),
        CaptureError::Stream(detail) => detail,
        silent @ CaptureError::Silent => silent.to_string(),
    })
}

/// A stream of the system as a running source: stopped by dropping it.
struct CaptureSource(Capture);

impl RunningSource for CaptureSource {
    fn stop(self: Box<Self>) {
        self.0.stop();
    }
}

/// What the tests that play a sound and listen for it on a real system have in common. Those tests are for Windows
/// and Linux, which is where the sources that they test run.
#[cfg(all(test, any(windows, target_os = "linux")))]
mod real_audio {
    use crate::record::{SourceEvent, Track};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::Receiver;
    use std::time::{Duration, Instant};

    /// Stops a test that was not asked for: it plays a sound out loud.
    pub fn require_enabled() {
        assert_eq!(
            std::env::var("DICTATE_TEST_REAL_AUDIO").as_deref(),
            Ok("1"),
            "set DICTATE_TEST_REAL_AUDIO=1 to play a sound on this machine and record it"
        );
    }

    /// `folder/tone.wav`: a tone of 440 Hz, `seconds` long, at half of full scale.
    pub fn tone_wav(folder: &Path, seconds: u32) -> PathBuf {
        const RATE: u32 = 44_100;
        let path = folder.join("tone.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..RATE * seconds {
            let tone = (std::f32::consts::TAU * 440.0 * i as f32 / RATE as f32).sin() * 0.5;
            writer
                .write_sample((tone * f32::from(i16::MAX)) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();
        path
    }

    /// A program that plays a sound: it is ended when this is dropped.
    pub struct Playing(Child);

    impl Playing {
        pub fn start(player: &mut Command) -> Playing {
            Playing(
                player
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("the program that plays the sound could not be started"),
            )
        }
    }

    impl Drop for Playing {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Whether a buffer with some sound in it (not silence) comes from `track` within `within`. A source that ends
    /// meanwhile is a failure of the test.
    pub fn hears_sound(rx: &Receiver<SourceEvent>, track: Track, within: Duration) -> bool {
        let until = Instant::now() + within;
        while let Some(wait) = until.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(wait) {
                Ok(SourceEvent::Audio {
                    track: heard, mono, ..
                }) if heard == track && mono.iter().any(|s| s.abs() > 0.01) => return true,
                Ok(SourceEvent::Ended { reason, .. }) => panic!("the source ended: {reason:?}"),
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::capture::Tap;
    use crate::record::stem::StemWriter;
    use crate::record::{StartError, Track};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[cfg(not(any(target_os = "macos", windows)))]
    fn options() -> crate::record::RecordOptions {
        use crate::record::{quality::Quality, Mode};
        crate::record::RecordOptions {
            mode: Mode::Audio,
            quality: Quality::Standard,
            mic: true,
            system_audio: true,
        }
    }

    /// A system that has no screen capture at all (not macOS, Windows or Linux) says so as an error, and does not panic.
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    #[test]
    fn os_capturer_reports_unavailable_tracks_as_errors_not_panics() {
        use crate::record::Capturer;
        let capturer = OsCapturer::new(None);

        let (tx, _rx) = mpsc::channel();
        assert_eq!(
            capturer.start(Track::Screen, &options(), tx).err(),
            Some(StartError::Source("not available yet".into()))
        );
    }

    /// The screen of Linux is the portal's: a recording of sound has none to ask it for, and is told so before the portal is
    /// reached. (The tests of the screens of macOS, Windows and Linux that start a capture are ignored: they need the permission, or
    /// the dialog, of whoever runs them.)
    #[cfg(target_os = "linux")]
    #[test]
    fn os_capturer_starts_no_screen_for_a_recording_of_sound() {
        use crate::record::Capturer;
        let capturer = OsCapturer::new(None);

        let (tx, rx) = mpsc::channel();
        assert_eq!(
            capturer.start(Track::Screen, &options(), tx).err(),
            Some(StartError::Source(
                "the screen is not recorded in a recording of sound".into()
            ))
        );
        assert!(rx.try_recv().is_err());
    }

    /// A store of the restore token for the tests of what a capturer is given.
    struct Kept(&'static str);

    impl TokenStore for Kept {
        fn load(&self) -> Option<String> {
            Some(self.0.to_owned())
        }

        fn save(&self, _token: &str) {}
    }

    #[test]
    fn a_capturer_keeps_no_restore_token_until_it_is_given_a_store() {
        assert_eq!(OsCapturer::new(None).tokens.load(), None);

        let capturer = OsCapturer::new(None).with_token_store(Arc::new(Kept("display:abc")));

        assert_eq!(capturer.tokens.load().as_deref(), Some("display:abc"));
    }

    /// How many samples the stem has after what a track delivered (a buffer each at the instants given, which are
    /// milliseconds after `base`, of 960 samples: 20 ms at 48 kHz) was put in it, on a clock that began at `base`.
    fn samples_in_the_stem(arrivals: &[u64]) -> usize {
        let base = Instant::now() + Duration::from_secs(60);
        let (tx, rx) = mpsc::channel();
        let mut tap = Tap::new(1, 48_000, reporter(Track::Mic, 48_000, tx));
        for ms in arrivals {
            tap.push_at(&[0.25_f32; 960], base + Duration::from_millis(*ms));
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mic.wav");
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        for event in rx.try_iter() {
            let SourceEvent::Audio { at, mono, .. } = event else {
                panic!("not audio")
            };
            stem.push(at - base, &mono).unwrap();
        }
        stem.finish().unwrap();
        hound::WavReader::open(&path).unwrap().len() as usize
    }

    #[test]
    fn a_burst_of_buffers_is_in_the_stem_whole() {
        // A buffer on time, and then three that were captured at 20, 40 and 60 ms and come together at 80: the stem is
        // the four of them, with nothing missing and nothing between.
        assert_eq!(samples_in_the_stem(&[20, 80, 80, 80]), 4 * 960);
    }

    #[test]
    fn a_gap_of_the_source_is_silence_in_the_stem() {
        // The second buffer was captured at 200 ms, 180 ms after the first one ended.
        assert_eq!(samples_in_the_stem(&[20, 220]), 960 + 8_640 + 960);
    }

    #[test]
    fn what_the_system_plays_being_silent_is_not_a_note() {
        let (tx, rx) = mpsc::channel();
        let mut report = reporter(Track::System, 48_000, tx);

        report(Report::Error(CaptureError::Silent));

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_start_that_fails_says_what_was_missing_or_what_the_system_said() {
        assert_eq!(
            start_error(CaptureError::NoDevice, "microphone"),
            StartError::Source("no microphone found".into())
        );
        assert_eq!(
            start_error(CaptureError::Stream("access denied".into()), "microphone"),
            StartError::Source("access denied".into())
        );
    }
}
