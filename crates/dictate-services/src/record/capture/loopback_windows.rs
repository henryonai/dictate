//! What the system plays, on Windows: the default output device opened as an input, which WASAPI records as what is
//! going to the speakers (loopback), in the format it mixes in.
//!
//! A device that plays nothing delivers no buffers at all, and not buffers of silence. Each buffer is stamped with when
//! it was captured, so that the gaps are filled with silence by the stem (`record::stem`): the stem of a source that
//! was quiet for a minute is a minute longer, as it has to be.
//!
//! The same calls record the default output on macOS (14.6 and later), but the system audio of macOS comes with the
//! screen capture and does not use this.

use std::sync::mpsc::Sender;

use super::{reporter, start_error, CaptureSource};
use crate::audio::capture::{default_output, Capture};
use crate::record::{RunningSource, SourceEvent, StartError, Track};

/// Starts what the default output device plays as [`Track::System`]: its buffers are sent to `tx` as they are captured,
/// at the rate the device mixes in, with its channels averaged to one. A device that is lost (the headphones are pulled
/// out, and the system's default moves) ends the track with `DeviceLost`.
pub fn start(tx: Sender<SourceEvent>) -> Result<Box<dyn RunningSource>, StartError> {
    let (device, supported) =
        default_output().map_err(|e| start_error(e, "audio output device"))?;
    let rate = supported.sample_rate();
    let capture = Capture::start_device(&device, supported, reporter(Track::System, rate, tx))
        .map_err(|e| start_error(e, "audio output device"))?;
    Ok(Box::new(CaptureSource(capture)))
}

#[cfg(all(test, windows))]
mod tests {
    use super::super::real_audio::{self, Playing};
    use super::*;
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Plays a tone through the default output device with the player of Windows and listens for it. Needs a sound
    /// system and a device that plays: run it with `--ignored`, on a machine that is not muted.
    #[test]
    #[ignore = "needs a sound system that plays: set DICTATE_TEST_REAL_AUDIO=1"]
    fn windows_loopback_delivers_audio_while_something_plays() {
        real_audio::require_enabled();
        let dir = tempfile::tempdir().unwrap();
        let wav = real_audio::tone_wav(dir.path(), 10);
        let _player = Playing::start(
            Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command"])
                .arg(format!(
                    "(New-Object System.Media.SoundPlayer '{}').PlaySync()",
                    wav.display()
                )),
        );
        // The player takes a moment to start; the tone lasts for much longer than this.
        std::thread::sleep(Duration::from_secs(1));

        let (tx, rx) = mpsc::channel();
        let source = start(tx).expect("the default output device is recorded");
        let heard = real_audio::hears_sound(&rx, Track::System, Duration::from_secs(3));
        source.stop();

        assert!(heard, "no sound in 3 s of a tone that was playing");
    }
}
