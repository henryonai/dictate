//! The sources of a recording that belong to the system: the microphone, and the sound that the system plays. The
//! screen is another module's.
//!
//! What a source does for the session, whichever it is:
//! - It is started on the session's thread by [`Capturer::start`], and stopped on that same thread: a microphone of some
//!   systems cannot be moved to another. `stop` comes back promptly (it does not wait for anything of the system's to
//!   happen) with the device or the child process released, and dropping a source stops it as well.
//! - The rate of a track is the same for as long as it lives. A device that changes its format ends the track with
//!   [`EndReason::DeviceLost`], and the track is not carried on at another rate.
//! - Each buffer is stamped with when its first sample was captured: the time the system's callback ran, less the length
//!   of the buffer.
//! - Nothing that runs on the audio thread of the system blocks or logs: it sends on the channel and returns.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use crate::audio::capture::{Capture, CaptureError, Report};

use super::{Capturer, EndReason, RecordOptions, RunningSource, SourceEvent, StartError, Track};

/// Windows records what the default output device plays through the same calls as a microphone. That is all this is,
/// so it is built on every system, to be checked on all of them.
pub mod loopback_windows;
pub mod mic;
/// The Linux way to hear the system's audio: a `parec` child. It is plain Unix code, built (and its tests run) on
/// macOS too.
#[cfg(unix)]
pub mod parec_linux;

/// The sources of a recording on this system.
pub struct OsCapturer {
    /// ffmpeg, for the sources that need it: none of the sound does.
    pub ffmpeg: Option<PathBuf>,
}

impl Capturer for OsCapturer {
    fn start(
        &self,
        track: Track,
        _options: &RecordOptions,
        tx: Sender<SourceEvent>,
    ) -> Result<Box<dyn RunningSource>, StartError> {
        match track {
            Track::Mic => mic::start(tx),
            Track::System => start_system_audio(tx),
            Track::Screen => Err(not_available_yet()),
        }
    }
}

fn not_available_yet() -> StartError {
    StartError::Source("not available yet".into())
}

#[cfg(windows)]
fn start_system_audio(tx: Sender<SourceEvent>) -> Result<Box<dyn RunningSource>, StartError> {
    loopback_windows::start(tx)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn start_system_audio(tx: Sender<SourceEvent>) -> Result<Box<dyn RunningSource>, StartError> {
    parec_linux::start(tx)
}

/// The system's audio of macOS comes with the screen capture.
#[cfg(target_os = "macos")]
fn start_system_audio(_tx: Sender<SourceEvent>) -> Result<Box<dyn RunningSource>, StartError> {
    Err(not_available_yet())
}

/// What a track makes of what a stream of the system reports: a buffer is the track's sound, a stream that breaks is
/// the end of the track, and a stream that delivers nothing but silence is neither (see below).
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
            // The sound goes on being recorded, as it is. Nothing carries a note of it to the session yet: a
            // microphone that the privacy switch of Windows silences is a recording with a silent microphone.
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
    use crate::record::quality::Quality;
    use crate::record::{Capturer, Mode, RecordOptions, StartError, Track};
    use std::sync::mpsc;

    fn options() -> RecordOptions {
        RecordOptions {
            mode: Mode::Audio,
            quality: Quality::Standard,
            mic: true,
            system_audio: true,
        }
    }

    #[test]
    fn os_capturer_reports_unavailable_tracks_as_errors_not_panics() {
        let capturer = OsCapturer { ffmpeg: None };
        let unavailable = Some(StartError::Source("not available yet".into()));

        let (tx, _rx) = mpsc::channel();
        assert_eq!(
            capturer.start(Track::Screen, &options(), tx).err(),
            unavailable
        );
        // The system's audio of macOS comes with its screen capture.
        #[cfg(target_os = "macos")]
        {
            let (tx, _rx) = mpsc::channel();
            assert_eq!(
                capturer.start(Track::System, &options(), tx).err(),
                unavailable
            );
        }
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
