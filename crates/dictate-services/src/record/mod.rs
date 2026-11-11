//! Recording the screen: the audio of a recording as separate stems on the recording's own clock, the mix of
//! them into the one `audio.m4a` that the rest of the app works from, the quality of the video, and the video itself,
//! which goes through ffmpeg. `session` runs all of it: it starts the sources of the system, puts what they deliver
//! in the stems and the video, and ends with the one file that goes to the meetings pipeline.

use std::sync::mpsc::Sender;
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub mod capture;
pub mod clock;
/// Scripted sources for the tests of the session, and of whatever runs one: with the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub mod fakes;
pub mod marker;
pub mod mix;
pub mod quality;
pub mod session;
pub mod stem;
pub mod video;

use quality::Quality;
use video::Frame;

/// What is recorded.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mode {
    /// A whole display; `None` is the main one.
    Screen { display: Option<String> },
    /// One window, by the id the system's list of windows gave.
    Window { id: String },
    /// Sound only.
    Audio,
}

impl Mode {
    /// Whether this mode has a video.
    pub(crate) fn has_video(&self) -> bool {
        !matches!(self, Mode::Audio)
    }
}

/// What the user chose for a recording.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordOptions {
    pub mode: Mode,
    pub quality: Quality,
    /// The microphone.
    pub mic: bool,
    /// What the system plays.
    pub system_audio: bool,
}

/// One source of a recording: the capture that delivers it is started and stopped on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Track {
    Mic,
    System,
    Screen,
}

/// What a source tells the session, through the channel `Capturer::start` is given.
pub enum SourceEvent {
    /// A buffer of sound. `at` is when its first sample was captured (not when it arrived: the time a capture
    /// callback gets a buffer is later by the latency of the device), and `rate` is the same for every buffer
    /// of a start of a track (a track that is started again, after the device it had was lost, may have another).
    Audio {
        track: Track,
        at: Instant,
        rate: u32,
        mono: Vec<f32>,
    },
    /// The screen as it is now.
    Frame(Frame),
    /// The source is over without being asked to stop.
    Ended { track: Track, reason: EndReason },
    /// The source has something to say that is not an end: the recording goes on, and the user should know.
    Note { track: Track, note: Note },
}

/// Why a source ended by itself.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum EndReason {
    WindowClosed,
    DisplayGone,
    DeviceLost(String),
    PermissionRevoked,
    Failed(String),
}

/// Something about a track that the user should be told, though the recording goes on. A source sends the ones that
/// it knows of (`SourceEvent::Note`); the session makes the others, about what it did with a source that was lost.
///
/// The session keeps the latest of each track in its state (`SessionState`), for the app to show, and the whole of them,
/// in English, in the notes of the recording (`Finished::notes`, which end up in its log).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Note {
    /// The track hears nothing but silence: on Windows the privacy switch for desktop apps keeps a microphone silent.
    Silent,
    /// The device of the track was lost (the detail says how), and it went on with the one that the system has now.
    /// What came between is silence.
    Restarted(String),
    /// The device of the track was lost (the detail says how), and could not be taken up again: the track is off for the
    /// rest of the recording, which goes on without it.
    Off(String),
}

impl Track {
    /// What the track is called in the notes of a recording.
    fn name(self) -> &'static str {
        match self {
            Track::Mic => "The microphone",
            Track::System => "The system's audio",
            Track::Screen => "The screen",
        }
    }
}

impl Note {
    /// The note in English, as the log of a recording has it.
    pub(crate) fn describe(&self, track: Track) -> String {
        let name = track.name();
        match self {
            Note::Silent => format!(
                "{name} hears only silence. Check that it is allowed and not muted (on Windows: Settings, Privacy, Microphone, let desktop apps use it)."
            ),
            Note::Restarted(why) => format!(
                "{name} was lost ({why}) and was started again on the device that the system has now. What came between is silence."
            ),
            Note::Off(why) => format!(
                "{name} was lost ({why}) and could not be started again: it is off for the rest of the recording."
            ),
        }
    }
}

/// Why a recording could not begin.
#[derive(Debug, PartialEq)]
pub enum StartError {
    /// Sound only, with the microphone and the system's audio both off.
    NothingToRecord,
    NeedsScreenPermission,
    /// A screen mode without ffmpeg to encode the video.
    NeedsFfmpeg,
    /// ffmpeg has no video encoder that works here.
    NoVideoEncoder,
    /// The system's audio of Linux, with no `parec` (the package `pulseaudio-utils`) to record it with.
    NeedsParec,
    PortalUnavailable,
    PortalCancelled,
    WindowGone,
    /// A source that could not start; the text is what it said.
    Source(String),
    /// The recording's folder could not be written.
    Io(String),
}

/// A source that is delivering.
pub trait RunningSource: Send {
    /// Ends the capture. It returns when the source has stopped sending.
    fn stop(self: Box<Self>);
}

/// Starts the sources of a recording: the part of it that is different on every system.
pub trait Capturer: Send + Sync {
    /// Starts one track, which sends what it captures to `tx` until it is stopped. Called on the session's thread;
    /// the source it returns is stopped on that same thread. A sound track whose source ended with
    /// `EndReason::DeviceLost` is started again by a call to this, after that source has been stopped: it should take up
    /// the device that the system has then.
    fn start(
        &self,
        track: Track,
        options: &RecordOptions,
        tx: Sender<SourceEvent>,
    ) -> Result<Box<dyn RunningSource>, StartError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_says_which_track_it_is_about_and_why() {
        assert!(Note::Silent
            .describe(Track::Mic)
            .starts_with("The microphone hears only silence"));
        let restarted = Note::Restarted("unplugged".into()).describe(Track::System);
        assert!(restarted.contains("The system's audio was lost (unplugged)"));
        assert!(restarted.contains("started again"));
        let off = Note::Off("no device".into()).describe(Track::Mic);
        assert!(off.contains("The microphone was lost (no device)"));
        assert!(off.contains("off for the rest of the recording"));
    }

    #[test]
    fn a_note_is_a_kind_and_its_detail_to_the_app() {
        assert_eq!(
            serde_json::to_string(&Note::Silent).unwrap(),
            r#"{"kind":"silent"}"#
        );
        assert_eq!(
            serde_json::to_string(&Note::Off("gone".into())).unwrap(),
            r#"{"kind":"off","detail":"gone"}"#
        );
    }
}

/// Probes for the tests of the recording's files.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::media::tools;
    use std::path::Path;
    use std::process::Stdio;

    /// How long the video of `path` is, in seconds: ffmpeg decodes it to nowhere and the last `time=` it printed is
    /// where it got to. This reads all of the video, so a file that stops short (a cut off one) gives how much of it
    /// can be played.
    pub fn video_seconds(path: &Path, ffmpeg: &Path) -> f64 {
        let output = tools::command(ffmpeg)
            .args(["-hide_banner", "-nostdin", "-i"])
            .arg(path)
            .args(["-map", "0:v", "-f", "null", "-"])
            .stdin(Stdio::null())
            .output()
            .expect("ffmpeg could not be run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        super::video::last_time(&stderr)
            .unwrap_or_else(|| panic!("no time= in what ffmpeg said: {stderr}"))
    }

    // --- Files of a recording, for the tests of what is done with them ---

    use crate::record::stem::StemWriter;
    use crate::record::video::{pick_codec, probe_seconds, VideoCodec, VideoEncoder};
    use crate::record::{fakes, quality::Quality};
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    /// The size of the frames of the test videos.
    pub const SIZE: (u32, u32) = (320, 180);

    /// The codec of this machine's ffmpeg.
    pub fn codec_of(ffmpeg: &Path) -> VideoCodec {
        let codec = pick_codec(ffmpeg)
            .expect("this ffmpeg has none of the encoders in ENCODER_ORDER that work");
        eprintln!("encoder: {}", codec.encoder);
        codec
    }

    /// A frame of the test videos, different from the one before it, so that an encoder has something to spend its
    /// bits on.
    pub fn moving_frame(n: u32) -> Vec<u8> {
        let mut bgra = Vec::with_capacity((SIZE.0 * SIZE.1 * 4) as usize);
        for y in 0..SIZE.1 {
            for x in 0..SIZE.0 {
                bgra.extend([(x + n * 7) as u8, (y * 2 + n * 3) as u8, (n * 5) as u8, 255]);
            }
        }
        bgra
    }

    /// `folder/video.<ext>` of `seconds` of the Standard quality: a video that was finished.
    pub fn encode_video(folder: &Path, ffmpeg: &Path, seconds: u32) -> PathBuf {
        let spec = Quality::Standard.spec();
        let codec = codec_of(ffmpeg);
        let mut encoder = VideoEncoder::start(folder, SIZE, &spec, &codec, ffmpeg).unwrap();
        for n in 0..spec.fps * seconds {
            encoder.push(&moving_frame(n)).unwrap();
        }
        encoder.finish().unwrap()
    }

    /// The `video.<ext>.<nonce>.part` that an encoder leaves when the program stops, and how many seconds of it can be
    /// read: 90 frames at 15 fps (6 s) are pushed, with the pipe left open, and ffmpeg is waited for until the first
    /// fragment is in the file. A fragment ends where the next keyframe is (every 2 s), and an encoder may hold some
    /// frames back before it writes any (the more it holds, the less is in the file), so the part holds 2 s or more of
    /// the 6 and never all of them: how much is measured, on the part that is left, and not assumed.
    ///
    /// What is waited for is what recovery needs, the part being readable for 2 s, and not a pause in the growth of the
    /// file: on a machine that is busy ffmpeg can take longer than any pause to write the first fragment.
    pub fn cut_off_video(folder: &Path, ffmpeg: &Path) -> (PathBuf, f64) {
        let spec = Quality::Standard.spec();
        let codec = codec_of(ffmpeg);
        let mut encoder = VideoEncoder::start(folder, SIZE, &spec, &codec, ffmpeg).unwrap();
        for n in 0..90u8 {
            let shade = 100 + n;
            let frame = [shade, 255 - shade, shade / 2, 255].repeat((SIZE.0 * SIZE.1) as usize);
            encoder.push(&frame).unwrap();
        }
        let parts = || -> Vec<PathBuf> {
            fs::read_dir(folder)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    let name = path.file_name().unwrap().to_string_lossy();
                    name.starts_with("video.") && name.ends_with(".part")
                })
                .collect()
        };
        let readable = || {
            parts()
                .first()
                .and_then(|part| probe_seconds(part, ffmpeg))
                .unwrap_or(0.0)
        };
        let give_up = std::time::Instant::now() + Duration::from_secs(60);
        while readable() < 1.9 {
            assert!(
                std::time::Instant::now() < give_up,
                "ffmpeg did not write the first fragment"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(encoder);
        let mut parts = parts();
        assert_eq!(parts.len(), 1, "a dropped encoder leaves its .part");
        // The encoder is gone: what is in the part now is all there will be.
        (parts.remove(0), readable())
    }

    /// A stem of a tone, `seconds` long, finished.
    pub fn tone_stem(folder: &Path, name: &str, hz: f32, seconds: f32) -> PathBuf {
        let path = folder.join(name);
        let mut stem = StemWriter::create(&path, 48_000).unwrap();
        let samples = fakes::sine(hz, 48_000, 0, (48_000.0 * seconds) as usize);
        stem.push(Duration::ZERO, &samples).unwrap();
        stem.finish().unwrap();
        path
    }

    #[cfg(test)]
    mod tests {
        use crate::record::video::last_time;

        #[test]
        fn the_last_time_of_the_progress_is_the_length() {
            let said = "frame=  9 fps=0.0 q=-0.0 size=N/A time=00:00:00.60 bitrate=N/A speed=1x    \r\
                        frame=150 fps=0.0 q=-0.0 Lsize=N/A time=01:02:10.25 bitrate=N/A speed=9x    \n";

            assert_eq!(last_time(said), Some(3600.0 + 120.0 + 10.25));
            assert_eq!(last_time("Output file is empty"), None);
            assert_eq!(last_time("size=N/A time=N/A bitrate=N/A"), None);
        }
    }
}
