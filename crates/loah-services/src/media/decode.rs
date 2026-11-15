//! Decoding an imported audio or video file to mono f32, block by block.
//!
//! The file is read with symphonia (wav, mp3, AAC in m4a and mp4, flac, ogg, mkv, caf and more; Opus through
//! `opus-decoder`, which symphonia has no decoder for). Only the default audio track is decoded: a video file's
//! picture is left alone. What symphonia cannot read (AC-3, WMA, AMR, damaged files, an odd AAC) goes to ffmpeg,
//! when there is one, which decodes the file to 48 kHz mono PCM and writes it to a pipe that is read here as it comes.
//!
//! Nothing is held back: `next_block` gives the audio of one packet (or one read of the pipe) at a time, so that the
//! pipeline can resample each block to 16 kHz for Whisper and to 48 kHz for the audio.m4a encoder without ever holding
//! a whole recording at the decoder's own rate.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::process::{Child, ChildStdout, Stdio};
use std::thread::JoinHandle;

use opus_decoder::{OpusDecoder, OpusError};
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, MediaInfo, Track, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::core::units::{Duration, TimeBase};

use super::tools::{self, install_command, Tool};

/// The rate of what ffmpeg is asked for.
const FFMPEG_RATE: u32 = 48_000;
/// The rate Opus decodes to, whatever rate the file says: the header of an Opus stream tells the rate of the source,
/// which is only a hint to a player.
const OPUS_RATE: u32 = 48_000;
/// How much of ffmpeg's output is read at a time: 16 384 samples, a third of a second.
const PIPE_BYTES: usize = 64 * 1024;
/// How many lines at the start and at the end of what ffmpeg printed an error keeps.
const KEPT_LINES: usize = 2;

/// Why a file could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// There is no such file.
    Missing,
    /// The file has no audio track.
    NoAudio,
    /// The built-in decoders cannot read the file, and there is no ffmpeg to try instead. `what` is the detail for the
    /// log, `ffmpeg_hint` the command that installs ffmpeg on this system.
    Unsupported {
        what: String,
        ffmpeg_hint: &'static str,
    },
    /// The file could not be read; the text is the system's detail.
    Io(String),
    /// ffmpeg could not read the file either: its exit code (`None` when it could not be started or was ended by the
    /// system) and the lines it printed (the first two and the last two).
    Ffmpeg { code: Option<i32>, tail: String },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Missing => f.write_str("The file was not found."),
            DecodeError::NoAudio => f.write_str("The file has no audio."),
            DecodeError::Unsupported { ffmpeg_hint, .. } => {
                write!(f, "This format needs ffmpeg: {ffmpeg_hint}")
            }
            DecodeError::Io(detail) => write!(f, "The file could not be read: {detail}"),
            DecodeError::Ffmpeg {
                code: Some(code),
                tail,
            } => write!(f, "ffmpeg failed ({code}): {tail}"),
            DecodeError::Ffmpeg { code: None, tail } => write!(f, "ffmpeg failed: {tail}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// A file read to mono f32, a block at a time.
pub struct MonoDecoder {
    rate: u32,
    duration: Option<f64>,
    /// The first block, decoded by `open` so that a file that cannot be decoded is found out there (and ffmpeg tried
    /// instead), not at the first `next_block`.
    first: Option<Vec<f32>>,
    source: Source,
}

enum Source {
    Symphonia(Box<SymphoniaReader>),
    Ffmpeg(Box<FfmpegPipe>),
}

impl MonoDecoder {
    /// Opens `path` for decoding: with symphonia, and when that cannot read it (`Unsupported`) and `ffmpeg` is the path
    /// of an ffmpeg, with ffmpeg.
    pub fn open(path: &Path, ffmpeg: Option<&Path>) -> Result<MonoDecoder, DecodeError> {
        let file = File::open(path).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                DecodeError::Missing
            } else {
                DecodeError::Io(e.to_string())
            }
        })?;
        // A folder opens (on Unix) and fails at the first read.
        if file.metadata().is_ok_and(|m| m.is_dir()) {
            return Err(DecodeError::Io("it is a folder, not a file".into()));
        }
        match (open_with_symphonia(file, path), ffmpeg) {
            (Err(DecodeError::Unsupported { .. }), Some(ffmpeg)) => open_with_ffmpeg(path, ffmpeg),
            (result, _) => result,
        }
    }

    /// The sample rate of the blocks. It is the rate the decoder gives, which is the file's own except for Opus and
    /// ffmpeg: they are 48 kHz.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// How long the file is, in seconds, as the file says; `None` when it does not say (and after ffmpeg, which is not
    /// asked). It can be a little more than what is decoded: the container does not always take the encoder's delay off.
    pub fn duration(&self) -> Option<f64> {
        self.duration
    }

    /// The next block of audio: mono (the channels averaged), at `rate()`. `None` is the end of the file; a block is
    /// never empty. A packet that cannot be decoded is skipped, and a file that is cut short or damaged ends where it
    /// can no longer be read.
    pub fn next_block(&mut self) -> Result<Option<Vec<f32>>, DecodeError> {
        if let Some(block) = self.first.take() {
            return Ok(Some(block));
        }
        match &mut self.source {
            Source::Symphonia(reader) => reader.next_block(),
            Source::Ffmpeg(pipe) => pipe.next_block(),
        }
    }
}

fn unsupported(what: impl Into<String>) -> DecodeError {
    DecodeError::Unsupported {
        what: what.into(),
        ffmpeg_hint: install_command(Tool::Ffmpeg),
    }
}

/// What symphonia's error means for the caller. Before any audio has come out, a file that symphonia cannot make sense
/// of is one that ffmpeg may still read (`Unsupported`); a real read error is not. After that, what is left of a
/// damaged file is not asked for (see `SymphoniaReader::next_block`).
fn from_symphonia(error: SymphoniaError, produced_audio: bool) -> DecodeError {
    match error {
        SymphoniaError::Unsupported(what) => unsupported(what),
        SymphoniaError::IoError(e) if e.kind() != io::ErrorKind::UnexpectedEof => {
            DecodeError::Io(e.to_string())
        }
        other if !produced_audio => unsupported(other.to_string()),
        other => DecodeError::Io(other.to_string()),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// symphonia

fn open_with_symphonia(file: File, path: &Path) -> Result<MonoDecoder, DecodeError> {
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    // The extension makes the probe try the right format first.
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| from_symphonia(e, false))?;

    let Some(track) = format.default_track(TrackType::Audio) else {
        // A track that symphonia does not know the codec of is not "no audio". One with a codec that is known but has
        // no decoder is an audio track here; one with no codec parameters at all (an AMR, IMA4 or other entry that the
        // container reader does not map) cannot even say what it is, and may well be the sound.
        let unknown_track = format.tracks().iter().any(|t| t.codec_params.is_none());
        return Err(
            if format.first_track(TrackType::Audio).is_some() || unknown_track {
                unsupported("the audio codec is not known")
            } else {
                DecodeError::NoAudio
            },
        );
    };
    let Some(params) = track.codec_params.as_ref().and_then(|p| p.audio()).cloned() else {
        return Err(unsupported("the audio track has no parameters"));
    };
    let duration = duration_of(track, format.media_info());
    let track_id = track.id;
    let decoder = if params.codec == CODEC_ID_OPUS {
        Decoder::Opus(Box::new(OpusReader::new(&params, track.time_base)?))
    } else {
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| match e {
                SymphoniaError::Unsupported(what) => {
                    unsupported(format!("{what} ({:?})", params.codec))
                }
                other => from_symphonia(other, false),
            })?;
        Decoder::Generic(Generic {
            decoder,
            scratch: Vec::new(),
        })
    };

    let mut reader = SymphoniaReader {
        format,
        track_id,
        decoder,
        rate: if params.codec == CODEC_ID_OPUS {
            OPUS_RATE
        } else {
            params.sample_rate.unwrap_or(0)
        },
        produced_audio: false,
    };
    // Decoding the first block finds out what the parameters could not say (the rate of an AAC with SBR, say) and
    // what the decoder cannot do.
    let Some(first) = reader.next_block()? else {
        return Err(unsupported("no audio could be decoded"));
    };
    Ok(MonoDecoder {
        rate: reader.rate,
        duration,
        first: Some(first),
        source: Source::Symphonia(Box::new(reader)),
    })
}

/// The length of the file in seconds, from the track or else from the container as a whole.
fn duration_of(track: &Track, media: &MediaInfo) -> Option<f64> {
    let seconds = |time_base: Option<TimeBase>, duration: Option<Duration>| {
        time_base?
            .calc_duration(duration?)
            .map(|time| time.as_secs_f64())
    };
    seconds(track.time_base, track.duration)
        .or_else(|| seconds(media.time_base, media.duration))
        .filter(|s| s.is_finite() && *s > 0.0)
}

struct SymphoniaReader {
    format: Box<dyn FormatReader>,
    track_id: u32,
    decoder: Decoder,
    /// The rate of the last block that was decoded.
    rate: u32,
    produced_audio: bool,
}

enum Decoder {
    Generic(Generic),
    Opus(Box<OpusReader>),
}

struct Generic {
    decoder: Box<dyn AudioDecoder>,
    /// The interleaved samples of one packet, kept to be used again.
    scratch: Vec<f32>,
}

impl SymphoniaReader {
    fn next_block(&mut self) -> Result<Option<Vec<f32>>, DecodeError> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return Ok(None),
                // A file that ends in the middle of a packet is a file that was cut short.
                Err(SymphoniaError::IoError(e))
                    if e.kind() == io::ErrorKind::UnexpectedEof && self.produced_audio =>
                {
                    return Ok(None)
                }
                // The tracks changed (a chained stream): what comes after is not read.
                Err(SymphoniaError::ResetRequired) if self.produced_audio => return Ok(None),
                // The container is damaged from here on and cannot be read on: the audio before it is still audio.
                Err(SymphoniaError::DecodeError(_)) if self.produced_audio => return Ok(None),
                Err(e) => return Err(from_symphonia(e, self.produced_audio)),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = match &mut self.decoder {
                Decoder::Generic(generic) => generic.decode(&packet).map(|(rate, block)| {
                    self.rate = rate;
                    block
                }),
                Decoder::Opus(opus) => Ok(opus.decode(&packet)),
            };
            match decoded {
                Ok(block) if !block.is_empty() => {
                    self.produced_audio = true;
                    return Ok(Some(block));
                }
                // Nothing in this packet (all of it was delay), or a packet that cannot be decoded: the next one.
                Ok(_)
                | Err(
                    SymphoniaError::DecodeError(_)
                    | SymphoniaError::IoError(_)
                    | SymphoniaError::ResetRequired,
                ) => {}
                Err(e) => return Err(from_symphonia(e, self.produced_audio)),
            }
        }
    }
}

impl Generic {
    /// The rate and the mono samples of one packet.
    fn decode(&mut self, packet: &Packet) -> Result<(u32, Vec<f32>), SymphoniaError> {
        let buffer = self.decoder.decode(packet)?;
        let channels = buffer.spec().channels().count().max(1);
        self.scratch.resize(buffer.samples_interleaved(), 0.0);
        buffer.copy_to_slice_interleaved::<f32, _>(&mut self.scratch[..]);
        Ok((buffer.spec().rate(), average(&self.scratch, channels)))
    }
}

/// The frames of `interleaved` samples, each averaged to one sample.
fn average(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------------
// Opus

/// Opus in an ogg, mkv or webm file, decoded by `opus-decoder`: a mono or stereo stream at 48 kHz, which is what
/// nearly all Opus files are. (Surround, with its channel mapping, goes to ffmpeg.)
struct OpusReader {
    decoder: OpusDecoder,
    channels: usize,
    /// Samples (of each channel) still to be dropped from the start: the decoder's warm-up, which the Opus header counts.
    skip: usize,
    /// The gain the header asks for, as a factor; 1 when there is none.
    gain: f32,
    time_base: Option<TimeBase>,
    pcm: Vec<f32>,
}

impl OpusReader {
    fn new(
        params: &AudioCodecParameters,
        time_base: Option<TimeBase>,
    ) -> Result<OpusReader, DecodeError> {
        // The header ("OpusHead": version, channels, pre-skip, input rate, gain, mapping family) is the codec's extra data.
        let head = params
            .extra_data
            .as_deref()
            .filter(|d| d.len() >= 19 && d.starts_with(b"OpusHead"));
        let (channels, skip, gain, family) = match head {
            Some(h) => (
                usize::from(h[9]),
                usize::from(u16::from_le_bytes([h[10], h[11]])),
                i16::from_le_bytes([h[16], h[17]]),
                h[18],
            ),
            None => (params.channels.as_ref().map_or(0, |c| c.count()), 0, 0, 0),
        };
        if family != 0 || !(1..=2).contains(&channels) {
            return Err(unsupported(format!(
                "Opus with {channels} channels (mapping {family})"
            )));
        }
        let decoder =
            OpusDecoder::new(OPUS_RATE, channels).map_err(|e| unsupported(format!("Opus: {e}")))?;
        Ok(OpusReader {
            decoder,
            channels,
            skip,
            // The gain is in 1/256 dB.
            gain: if gain == 0 {
                1.0
            } else {
                10_f32.powf(f32::from(gain) / (20.0 * 256.0))
            },
            time_base,
            pcm: vec![0.0; OpusDecoder::MAX_FRAME_SIZE_48K * channels],
        })
    }

    /// The mono samples of one packet, without the warm-up at the start of the stream and the padding that the packet says
    /// is at its end. Empty when the packet cannot be decoded.
    fn decode(&mut self, packet: &Packet) -> Vec<f32> {
        // An empty packet would make the decoder invent a frame of concealment.
        if packet.data.is_empty() {
            return Vec::new();
        }
        let Some(frames) =
            self.decode_frames(|decoder, pcm| decoder.decode_float(&packet.data, pcm, false))
        else {
            return Vec::new();
        };
        let from = self.skip.min(frames);
        self.skip -= from;
        let to = frames - self.padding(packet).min(frames - from);
        let mut mono = average(
            &self.pcm[from * self.channels..to * self.channels],
            self.channels,
        );
        if self.gain != 1.0 {
            mono.iter_mut().for_each(|s| *s *= self.gain);
        }
        mono
    }

    /// Runs `decode` (the decoder and its output buffer are given to it) and returns the number of samples of each
    /// channel that it made; `None` when it failed or panicked. A panic is caught because the decoder is a young crate
    /// that is given files from anywhere: a packet that it cannot take costs that packet, and not the whole import.
    /// What the decoder remembers from before the panic may be half changed, so it starts over.
    fn decode_frames(
        &mut self,
        decode: impl FnOnce(&mut OpusDecoder, &mut [f32]) -> Result<usize, OpusError>,
    ) -> Option<usize> {
        let (decoder, pcm) = (&mut self.decoder, &mut self.pcm[..]);
        match catch_unwind(AssertUnwindSafe(|| decode(decoder, pcm))) {
            Ok(result) => result.ok(),
            Err(_) => {
                self.decoder.reset();
                None
            }
        }
    }

    /// The samples at the end of the packet that are padding, from its end trim (in the time base of the track).
    fn padding(&self, packet: &Packet) -> usize {
        match self.time_base {
            Some(base) => base.calc_duration(packet.trim_end).map_or(0, |t| {
                (t.as_secs_f64() * f64::from(OPUS_RATE)).round() as usize
            }),
            None => packet.trim_end.get() as usize,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// ffmpeg

/// The decoder of last resort: `ffmpeg -i <file> -vn -ac 1 -ar 48000 -f f32le pipe:1`, read from its stdout.
fn open_with_ffmpeg(path: &Path, ffmpeg: &Path) -> Result<MonoDecoder, DecodeError> {
    // `file:` in front of the name keeps ffmpeg from taking a part of it for a protocol (`meeting: notes.mp3`), or a
    // name that starts with `-` for an option.
    let mut input = OsString::from("file:");
    input.push(path);
    let mut child = tools::command(ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(input)
        .args(["-vn", "-ac", "1", "-ar", "48000", "-f", "f32le", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| DecodeError::Ffmpeg {
            code: None,
            tail: format!("could not start ffmpeg: {e}"),
        })?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        // Both were asked to be pipes. Whatever happened, the program must not be left running.
        let _ = child.kill();
        let _ = child.wait();
        return Err(DecodeError::Ffmpeg {
            code: None,
            tail: "could not read from ffmpeg".into(),
        });
    };
    // What ffmpeg prints is read all the time, whatever the audio does: a program that fills the pipe stops.
    let stderr = std::thread::spawn(move || Tail::read(stderr));
    let mut pipe = FfmpegPipe {
        child,
        stdout,
        stderr: Some(stderr),
        carry: Vec::new(),
        buffer: vec![0; PIPE_BYTES].into_boxed_slice(),
        produced_audio: false,
        ended: false,
    };
    // The first read waits for ffmpeg to say whether it can read the file at all. If it cannot, `pipe` is dropped here,
    // and with it the program.
    let first = pipe.next_block()?;
    Ok(MonoDecoder {
        rate: FFMPEG_RATE,
        duration: None,
        first,
        source: Source::Ffmpeg(Box::new(pipe)),
    })
}

/// A running ffmpeg and the end of its stdout that is read. Dropping it kills the program and waits for it, so that
/// a decode that is given up leaves neither a running process nor a zombie.
struct FfmpegPipe {
    child: Child,
    stdout: ChildStdout,
    /// The thread that reads stderr; it ends when ffmpeg does.
    stderr: Option<JoinHandle<Tail>>,
    /// The first bytes of a sample whose other bytes have not been read yet.
    carry: Vec<u8>,
    buffer: Box<[u8]>,
    produced_audio: bool,
    ended: bool,
}

impl FfmpegPipe {
    fn next_block(&mut self) -> Result<Option<Vec<f32>>, DecodeError> {
        if self.ended {
            return Ok(None);
        }
        loop {
            // A pipe gives its bytes in any sizes: a sample (4 bytes) can come in two reads.
            let carried = self.carry.len();
            self.buffer[..carried].copy_from_slice(&self.carry);
            self.carry.clear();
            let read = match self.stdout.read(&mut self.buffer[carried..]) {
                Ok(read) => read,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    self.carry.extend_from_slice(&self.buffer[..carried]);
                    continue;
                }
                Err(e) => {
                    self.ended = true;
                    return Err(DecodeError::Io(format!("could not read from ffmpeg: {e}")));
                }
            };
            if read == 0 {
                return self.end();
            }
            let total = carried + read;
            let whole = total - total % 4;
            self.carry.extend_from_slice(&self.buffer[whole..total]);
            if whole == 0 {
                continue;
            }
            self.produced_audio = true;
            return Ok(Some(
                self.buffer[..whole]
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect(),
            ));
        }
    }

    /// stdout is closed: ffmpeg has ended. Its exit tells whether the audio was all there.
    fn end(&mut self) -> Result<Option<Vec<f32>>, DecodeError> {
        self.ended = true;
        let status = self
            .child
            .wait()
            .map_err(|e| DecodeError::Io(format!("could not wait for ffmpeg: {e}")))?;
        let tail = self
            .stderr
            .take()
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default();
        if status.success() {
            // Nothing at all is nothing to listen to.
            return if self.produced_audio {
                Ok(None)
            } else {
                Err(DecodeError::NoAudio)
            };
        }
        if !self.produced_audio && tail.no_stream {
            return Err(DecodeError::NoAudio);
        }
        Err(DecodeError::Ffmpeg {
            code: status.code(),
            tail: tail.text(),
        })
    }
}

impl Drop for FfmpegPipe {
    fn drop(&mut self) {
        // Killing a program that has already been waited for does nothing.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What ffmpeg printed to stderr: the first lines and the last lines of it. The cause of a failure is usually in
/// the first, and the last say what came of it (with `-v error` ffmpeg prints a line per thing that went wrong).
#[derive(Default)]
pub(crate) struct Tail {
    first: Vec<String>,
    last: VecDeque<String>,
    /// Lines were left out between `first` and `last`.
    cut: bool,
    /// ffmpeg said there was no audio to write.
    no_stream: bool,
}

impl Tail {
    pub(crate) fn read(stderr: impl Read) -> Tail {
        let mut tail = Tail::default();
        let mut reader = BufReader::new(stderr);
        let mut line = Vec::new();
        loop {
            line.clear();
            if !matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
                return tail;
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim();
            if !text.is_empty() {
                tail.add(text);
            }
        }
    }

    fn add(&mut self, line: &str) {
        // `Output file does not contain any stream` is ffmpeg's word for `-vn` on a file that has only video.
        if line.contains("does not contain any stream") || line.contains("matches no streams") {
            self.no_stream = true;
        }
        let line: String = line.chars().take(300).collect();
        if self.first.len() < KEPT_LINES {
            self.first.push(line);
            return;
        }
        if self.last.len() == KEPT_LINES {
            self.last.pop_front();
            self.cut = true;
        }
        self.last.push_back(line);
    }

    pub(crate) fn text(&self) -> String {
        let mut lines: Vec<&str> = self.first.iter().map(String::as_str).collect();
        if self.cut {
            lines.push("...");
        }
        lines.extend(self.last.iter().map(String::as_str));
        lines.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::media::test_support::script;
    use crate::media::test_support::{read_all, real_ffmpeg, zero_crossings};
    use std::fs;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/media")
            .join(name)
    }

    fn ffmpeg_hint() -> &'static str {
        install_command(Tool::Ffmpeg)
    }

    #[test]
    fn decode_reads_every_fixture_format() {
        // Each file is 1 s of a 440 Hz tone: the length is checked at the rate the decoder says, and the pitch by
        // the zero crossings (880 of them).
        for (name, rate, tolerance) in [
            ("tone.wav", 44_100, 0.02),
            ("tone.mp3", 44_100, 0.02),
            // symphonia reads the edit list of an MP4 but does not apply it, so the delay of the AAC encoder stays in:
            // one frame of 1024 samples (ffmpeg's encoder; Apple's is 2112), which makes this second 1.024 s long.
            ("tone.m4a", 48_000, 0.03),
            ("tone.mp4", 48_000, 0.03),
            ("tone.webm", 48_000, 0.02),
            ("tone.opus", 48_000, 0.02),
            ("tone-16k.webm", 48_000, 0.02),
        ] {
            let mut decoder =
                MonoDecoder::open(&fixture(name), None).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(decoder.rate(), rate, "{name}: rate");

            let samples = read_all(&mut decoder);

            let seconds = samples.len() as f64 / f64::from(rate);
            assert!(
                (seconds - 1.0).abs() <= tolerance,
                "{name}: {seconds} s long"
            );
            let crossings = zero_crossings(&samples);
            assert!(
                crossings.abs_diff(880) <= 20,
                "{name}: {crossings} zero crossings"
            );
        }
    }

    #[test]
    fn decode_video_only_is_no_audio() {
        let video = fixture("video-only.mp4");

        assert_eq!(
            MonoDecoder::open(&video, None).err(),
            Some(DecodeError::NoAudio)
        );
        // symphonia can tell, so ffmpeg is not started for it (this path is not a program).
        assert_eq!(
            MonoDecoder::open(&video, Some(Path::new("/no/such/ffmpeg"))).err(),
            Some(DecodeError::NoAudio)
        );
    }

    #[test]
    fn decode_opus_is_48_khz_whatever_the_file_says() {
        // Opus always decodes to 48 kHz. A WebM made from a 16 kHz source says 16 kHz (symphonia takes 8 kHz when the
        // file says nothing), and a rate that is read from there turns 1 s of audio into 3 s.
        for name in ["tone.webm", "tone-16k.webm", "tone.opus"] {
            let mut decoder = MonoDecoder::open(&fixture(name), None).unwrap();
            assert_eq!(decoder.rate(), 48_000, "{name}: rate");

            let seconds = read_all(&mut decoder).len() as f64 / f64::from(decoder.rate());

            assert!((seconds - 1.0).abs() < 0.02, "{name}: {seconds} s");
        }
    }

    #[test]
    fn decode_an_audio_codec_symphonia_does_not_know_is_unsupported_not_no_audio() {
        // IMA ADPCM in QuickTime: symphonia gives the track no codec parameters at all, so it cannot even say that it
        // is audio. That is a file ffmpeg can read, not a file without sound.
        let error = MonoDecoder::open(&fixture("tone-ima4.mov"), None)
            .err()
            .unwrap();

        let DecodeError::Unsupported { ffmpeg_hint, .. } = error else {
            panic!("{error:?}")
        };
        assert_eq!(ffmpeg_hint, install_command(Tool::Ffmpeg));
    }

    #[test]
    fn decode_missing_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("moved away.mp3");

        assert_eq!(
            MonoDecoder::open(&gone, None).err(),
            Some(DecodeError::Missing)
        );
        assert_eq!(
            MonoDecoder::open(&gone, Some(Path::new("/no/such/ffmpeg"))).err(),
            Some(DecodeError::Missing)
        );
    }

    #[test]
    fn decode_a_folder_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();

        assert!(matches!(
            MonoDecoder::open(dir.path(), None),
            Err(DecodeError::Io(_))
        ));
    }

    #[test]
    fn decode_unsupported_without_ffmpeg_names_ffmpeg() {
        let dir = tempfile::tempdir().unwrap();
        let garbage = dir.path().join("recording.ac3");
        fs::write(&garbage, b"this is not audio, whatever the name says").unwrap();

        for file in [garbage, fixture("tone-ac3.mkv")] {
            let error = MonoDecoder::open(&file, None).err().unwrap();
            let DecodeError::Unsupported {
                what,
                ffmpeg_hint: hint,
            } = &error
            else {
                panic!("{file:?}: {error:?}")
            };
            assert_eq!(*hint, ffmpeg_hint(), "{file:?}");
            assert!(!what.is_empty(), "{file:?}: the detail is empty");
            assert!(error.to_string().contains(ffmpeg_hint()), "{error}");
        }
    }

    #[test]
    fn decode_does_not_start_ffmpeg_for_what_symphonia_reads() {
        let mut decoder =
            MonoDecoder::open(&fixture("tone.wav"), Some(Path::new("/no/such/ffmpeg"))).unwrap();

        assert_eq!(decoder.rate(), 44_100);
        assert!(!read_all(&mut decoder).is_empty());
    }

    /// A WAV file of `seconds` of 16-bit audio at 48 kHz: `frame(i)` gives the samples of frame `i`.
    fn write_wav(path: &Path, channels: u16, seconds: u32, frame: impl Fn(usize) -> Vec<i16>) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..(48_000 * seconds) as usize {
            for sample in frame(i) {
                writer.write_sample(sample).unwrap();
            }
        }
        writer.finalize().unwrap();
    }

    fn tone(i: usize) -> f32 {
        (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin()
    }

    #[test]
    fn decode_averages_the_channels_to_mono() {
        // Left a tone at half scale, right a constant 0.1: the average is a quarter-scale tone on top of 0.05. The left
        // alone would peak at 0.5, and the sum at 0.6.
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("stereo.wav");
        write_wav(&wav, 2, 1, |i| vec![(tone(i) * 16_384.0) as i16, 3_277]);

        let mut decoder = MonoDecoder::open(&wav, None).unwrap();
        let samples = read_all(&mut decoder);

        assert_eq!(samples.len(), 48_000);
        let peak = samples.iter().cloned().fold(f32::MIN, f32::max);
        let trough = samples.iter().cloned().fold(f32::MAX, f32::min);
        assert!((peak - 0.30).abs() < 0.01, "peak {peak}");
        assert!((trough + 0.20).abs() < 0.01, "trough {trough}");
    }

    #[test]
    fn decode_gives_a_long_file_in_blocks_not_all_at_once() {
        // 60 s: the pipeline resamples each block as it comes and never holds the whole file at the decoder's rate.
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("long.wav");
        write_wav(&wav, 1, 60, |i| vec![(tone(i) * 8_000.0) as i16]);

        let mut decoder = MonoDecoder::open(&wav, None).unwrap();
        let (mut blocks, mut longest, mut total) = (0, 0, 0);
        while let Some(block) = decoder.next_block().unwrap() {
            blocks += 1;
            longest = longest.max(block.len());
            total += block.len();
        }

        assert_eq!(total, 60 * 48_000);
        assert!(blocks >= 60, "{blocks} blocks");
        assert!(longest <= 48_000, "a block of {longest} samples");
    }

    #[test]
    fn decode_a_file_cut_short_gives_what_it_has() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("cut.wav");
        write_wav(&wav, 1, 10, |i| vec![(tone(i) * 8_000.0) as i16]);
        let bytes = fs::read(&wav).unwrap();
        fs::write(&wav, &bytes[..bytes.len() / 2]).unwrap();
        let mp3 = dir.path().join("cut.mp3");
        let bytes = fs::read(fixture("tone.mp3")).unwrap();
        fs::write(&mp3, &bytes[..bytes.len() / 2]).unwrap();

        for (file, rate, whole_seconds) in [(wav, 48_000, 10.0), (mp3, 44_100, 1.0)] {
            let mut decoder = MonoDecoder::open(&file, None).unwrap();
            let seconds = read_all(&mut decoder).len() as f64 / f64::from(rate);

            assert!(
                (seconds / whole_seconds - 0.5).abs() < 0.1,
                "{file:?}: {seconds} s of {whole_seconds}"
            );
        }
    }

    #[test]
    fn decode_a_damaged_container_ends_with_what_was_decoded() {
        // 64 bytes of zeros in the middle of a WebM: the container reader gives up there with an error of its own ("invalid
        // ebml element id length"), and the first 32 of its 51 packets (0.64 s of the 1 s) were fine. Losing the rest of
        // a long recording is better than losing all of it.
        let dir = tempfile::tempdir().unwrap();
        let webm = dir.path().join("damaged.webm");
        let mut bytes = fs::read(fixture("tone.webm")).unwrap();
        let at = bytes.len() * 6 / 10;
        bytes[at..at + 64].fill(0);
        fs::write(&webm, bytes).unwrap();

        let mut decoder = MonoDecoder::open(&webm, None).unwrap();
        let seconds = read_all(&mut decoder).len() as f64 / f64::from(decoder.rate());

        assert!((seconds - 0.64).abs() < 0.1, "{seconds} s");
    }

    #[test]
    fn decode_tells_how_long_the_file_is() {
        for name in [
            "tone.wav",
            "tone.mp3",
            "tone.m4a",
            "tone.mp4",
            "tone.webm",
            "tone.opus",
        ] {
            let decoder = MonoDecoder::open(&fixture(name), None).unwrap();

            let seconds = decoder
                .duration()
                .unwrap_or_else(|| panic!("{name}: no duration"));
            assert!((seconds - 1.0).abs() < 0.05, "{name}: {seconds} s");
        }
    }

    #[test]
    fn decode_opus_in_ogg_has_the_exact_length() {
        // The Opus header says how many samples at the start are the decoder's warm-up (312), and the last packet says
        // how many at the end are padding (648): 48 912 decoded samples are the 48 000 of the tone.
        let mut decoder = MonoDecoder::open(&fixture("tone.opus"), None).unwrap();

        assert_eq!(read_all(&mut decoder).len(), 48_000);
    }

    /// The first packet of `tone.opus`, as an Opus decoder takes it.
    fn first_opus_packet() -> Vec<u8> {
        let file = fs::File::open(fixture("tone.opus")).unwrap();
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let mut format = symphonia::default::get_probe()
            .probe(
                &Hint::new(),
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .unwrap();
        format.next_packet().unwrap().unwrap().data.to_vec()
    }

    #[test]
    fn a_packet_that_makes_the_opus_decoder_panic_is_skipped_and_the_next_one_decodes() {
        // opus-decoder is a young crate (0.1) that is fed files from anywhere: a packet it panics on (nothing in the
        // fixtures does, so the decoder is told to) must cost that packet, not the import.
        let mut head = b"OpusHead".to_vec();
        // Version 1, 1 channel, pre-skip 0, 48 000 Hz, gain 0, channel mapping 0.
        head.extend([1, 1, 0, 0, 0x80, 0xbb, 0, 0, 0, 0, 0]);
        let mut params = AudioCodecParameters::new();
        params.for_codec(CODEC_ID_OPUS).with_extra_data(head.into());
        let mut reader = OpusReader::new(&params, None).unwrap();
        let packet = first_opus_packet();

        let after_a_panic =
            reader.decode_frames(|_, _| panic!("index out of bounds, in a decoder far away"));
        let next = reader.decode_frames(|decoder, pcm| decoder.decode_float(&packet, pcm, false));

        assert_eq!(after_a_panic, None);
        assert_eq!(next, Some(960));
    }

    #[test]
    fn the_decoder_can_move_to_another_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<MonoDecoder>();
    }

    #[test]
    fn decode_errors_say_what_to_do() {
        assert_eq!(DecodeError::NoAudio.to_string(), "The file has no audio.");
        let unsupported = DecodeError::Unsupported {
            what: "an odd codec".into(),
            ffmpeg_hint: ffmpeg_hint(),
        };
        assert_eq!(
            unsupported.to_string(),
            format!("This format needs ffmpeg: {}", ffmpeg_hint())
        );
        let failed = DecodeError::Ffmpeg {
            code: Some(69),
            tail: "Invalid data".into(),
        };
        assert_eq!(failed.to_string(), "ffmpeg failed (69): Invalid data");
    }

    // The 8 bytes of two samples, 1.0 and -0.5, as ffmpeg writes them (f32, little-endian), for `printf`.
    #[cfg(unix)]
    const TWO_SAMPLES: &str = r"\000\000\200\077\000\000\000\277";

    /// A file that symphonia cannot read, in a folder and with a name that are awkward on a command line.
    fn awkward_input(dir: &Path) -> PathBuf {
        let folder = dir.join("Cuộc họp tháng 11");
        fs::create_dir_all(&folder).unwrap();
        // A colon cannot be in a file name on Windows (it would start a data stream of the file before it).
        let name = if cfg!(windows) {
            "bản ghi 1.ac3"
        } else {
            "bản ghi: 1.ac3"
        };
        let file = folder.join(name);
        fs::write(&file, b"this is not audio, whatever the name says").unwrap();
        file
    }

    /// A stand-in for ffmpeg: a shell script with the given body.
    #[cfg(unix)]
    fn fake_ffmpeg(dir: &Path, body: &str) -> PathBuf {
        script(dir, "ffmpeg", body, "")
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_fallback_runs_the_documented_command_and_reads_its_samples() {
        let dir = tempfile::tempdir().unwrap();
        let input = awkward_input(dir.path());
        let ffmpeg = fake_ffmpeg(
            dir.path(),
            &format!(r#"printf '%s\n' "$@" > "$0.args"; printf '{TWO_SAMPLES}'"#),
        );

        let mut decoder = MonoDecoder::open(&input, Some(&ffmpeg)).unwrap();

        assert_eq!(decoder.rate(), 48_000);
        assert_eq!(decoder.duration(), None);
        assert_eq!(read_all(&mut decoder), [1.0, -0.5]);
        let args = fs::read_to_string(format!("{}.args", ffmpeg.display())).unwrap();
        // `file:` in front of the name: ffmpeg takes `bản ghi: 1.ac3` for a protocol, but not `file:bản ghi: 1.ac3`.
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            [
                "-nostdin".to_string(),
                "-v".into(),
                "error".into(),
                "-i".into(),
                format!("file:{}", input.display()),
                "-vn".into(),
                "-ac".into(),
                "1".into(),
                "-ar".into(),
                "48000".into(),
                "-f".into(),
                "f32le".into(),
                "pipe:1".into(),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_fallback_joins_a_sample_that_arrives_in_pieces() {
        let dir = tempfile::tempdir().unwrap();
        let input = awkward_input(dir.path());
        // 3 bytes, 3 bytes, then 2: the first sample is whole only with the second piece, the second with the third.
        let ffmpeg = fake_ffmpeg(
            dir.path(),
            r"printf '\000\000\200'; sleep 0.1; printf '\077\000\000'; sleep 0.1; printf '\000\277'",
        );

        let mut decoder = MonoDecoder::open(&input, Some(&ffmpeg)).unwrap();

        assert_eq!(read_all(&mut decoder), [1.0, -0.5]);
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_fallback_reports_the_exit_code_and_what_ffmpeg_said() {
        let dir = tempfile::tempdir().unwrap();
        let input = awkward_input(dir.path());
        let ffmpeg = fake_ffmpeg(
            dir.path(),
            "for n in 1 2 3 4 5 6; do echo \"line $n\" >&2; done; exit 69",
        );

        let error = MonoDecoder::open(&input, Some(&ffmpeg)).err().unwrap();

        // The cause is usually in the first lines and the summary in the last: both are kept.
        assert_eq!(
            error,
            DecodeError::Ffmpeg {
                code: Some(69),
                tail: "line 1 line 2 ... line 5 line 6".into()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_fallback_failing_after_some_audio_is_an_error_too() {
        let dir = tempfile::tempdir().unwrap();
        let input = awkward_input(dir.path());
        let ffmpeg = fake_ffmpeg(
            dir.path(),
            &format!("printf '{TWO_SAMPLES}'; echo 'Invalid data found' >&2; exit 1"),
        );

        let mut decoder = MonoDecoder::open(&input, Some(&ffmpeg)).unwrap();

        assert_eq!(decoder.next_block().unwrap(), Some(vec![1.0, -0.5]));
        assert_eq!(
            decoder.next_block().err(),
            Some(DecodeError::Ffmpeg {
                code: Some(1),
                tail: "Invalid data found".into()
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_fallback_for_a_file_with_no_stream_to_write_is_no_audio() {
        let dir = tempfile::tempdir().unwrap();
        let input = awkward_input(dir.path());
        // What ffmpeg says for `-vn` on a file that has only video.
        let ffmpeg = fake_ffmpeg(
            dir.path(),
            "echo 'Output file does not contain any stream' >&2; echo 'Error opening output files' >&2; exit 234",
        );

        assert_eq!(
            MonoDecoder::open(&input, Some(&ffmpeg)).err(),
            Some(DecodeError::NoAudio)
        );
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_fallback_that_writes_nothing_and_succeeds_is_no_audio() {
        let dir = tempfile::tempdir().unwrap();
        let input = awkward_input(dir.path());
        let ffmpeg = fake_ffmpeg(dir.path(), "exit 0");

        assert_eq!(
            MonoDecoder::open(&input, Some(&ffmpeg)).err(),
            Some(DecodeError::NoAudio)
        );
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_fallback_does_not_stall_when_ffmpeg_prints_a_lot() {
        // A pipe holds 64 KB: an ffmpeg that is not read from while it writes more than that to stderr stops, and the
        // decoder with it. It runs on its own thread so that a stall is a failure and not a hang.
        let dir = tempfile::tempdir().unwrap();
        let input = awkward_input(dir.path());
        let ffmpeg = fake_ffmpeg(
            dir.path(),
            &format!(
                "yes 'a long line of error output' | head -c 400000 >&2; printf '{TWO_SAMPLES}'"
            ),
        );
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut decoder = MonoDecoder::open(&input, Some(&ffmpeg)).unwrap();
            sender.send(read_all(&mut decoder)).unwrap();
        });

        let samples = receiver
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the decoder stalled on ffmpeg's output");

        assert_eq!(samples, [1.0, -0.5]);
    }

    #[cfg(unix)]
    #[test]
    fn dropping_the_decoder_kills_and_reaps_ffmpeg() {
        let dir = tempfile::tempdir().unwrap();
        let input = awkward_input(dir.path());
        // One sample, and then ffmpeg goes on for a minute (`exec` keeps the process id).
        let ffmpeg = fake_ffmpeg(
            dir.path(),
            &format!("echo $$ > \"$0.pid\"; printf '{TWO_SAMPLES}'; exec sleep 60"),
        );
        let decoder = MonoDecoder::open(&input, Some(&ffmpeg)).unwrap();
        let pid = fs::read_to_string(format!("{}.pid", ffmpeg.display()))
            .unwrap()
            .trim()
            .to_owned();
        let alive = || {
            // A process that has ended but has not been waited for (a zombie) still answers `kill -0`.
            std::process::Command::new("sh")
                .args(["-c", &format!("kill -0 {pid}")])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        assert!(
            alive(),
            "ffmpeg should be running while the decoder is open"
        );

        let started = std::time::Instant::now();
        drop(decoder);

        assert!(!alive(), "ffmpeg is still there (running, or a zombie)");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn ffmpeg_that_cannot_start_is_an_ffmpeg_error() {
        let dir = tempfile::tempdir().unwrap();
        let input = awkward_input(dir.path());

        let error = MonoDecoder::open(&input, Some(Path::new("/no/such/ffmpeg")))
            .err()
            .unwrap();

        let DecodeError::Ffmpeg { code: None, tail } = error else {
            panic!("{error:?}")
        };
        assert!(tail.contains("start"), "{tail}");
    }

    #[test]
    fn decode_falls_back_to_ffmpeg() {
        let Some(ffmpeg) = real_ffmpeg() else { return };

        // AC-3 is a codec that symphonia cannot decode.
        let mut decoder = MonoDecoder::open(&fixture("tone-ac3.mkv"), Some(&ffmpeg)).unwrap();
        assert_eq!(decoder.rate(), 48_000);
        let samples = read_all(&mut decoder);

        assert!(
            samples.len().abs_diff(48_000) <= 960,
            "{} samples",
            samples.len()
        );
        let crossings = zero_crossings(&samples);
        assert!(crossings.abs_diff(880) <= 20, "{crossings} zero crossings");
    }

    #[test]
    fn decode_falls_back_to_ffmpeg_for_an_audio_codec_symphonia_does_not_know() {
        let Some(ffmpeg) = real_ffmpeg() else { return };

        let mut decoder = MonoDecoder::open(&fixture("tone-ima4.mov"), Some(&ffmpeg)).unwrap();
        assert_eq!(decoder.rate(), 48_000);
        let samples = read_all(&mut decoder);

        assert!(
            samples.len().abs_diff(48_000) <= 960,
            "{} samples",
            samples.len()
        );
        let crossings = zero_crossings(&samples);
        assert!(crossings.abs_diff(880) <= 20, "{crossings} zero crossings");
    }

    #[test]
    fn decode_with_ffmpeg_handles_vietnamese_names_with_spaces_and_colons() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Cuộc họp tháng 11");
        fs::create_dir_all(&folder).unwrap();
        // A colon cannot be in a file name on Windows.
        let name = if cfg!(windows) {
            "bản ghi 1.mkv"
        } else {
            "bản ghi: 1.mkv"
        };
        let input = folder.join(name);
        fs::copy(fixture("tone-ac3.mkv"), &input).unwrap();

        let mut decoder = MonoDecoder::open(&input, Some(&ffmpeg)).unwrap();

        assert!(read_all(&mut decoder).len().abs_diff(48_000) <= 960);
    }

    #[test]
    fn ffmpeg_says_a_file_with_only_video_has_no_audio() {
        let Some(ffmpeg) = real_ffmpeg() else { return };

        assert_eq!(
            open_with_ffmpeg(&fixture("video-only.mp4"), &ffmpeg).err(),
            Some(DecodeError::NoAudio)
        );
    }

    #[test]
    fn ffmpeg_failing_on_garbage_says_why() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();

        let error = open_with_ffmpeg(&awkward_input(dir.path()), &ffmpeg)
            .err()
            .unwrap();

        let DecodeError::Ffmpeg {
            code: Some(code),
            tail,
        } = error
        else {
            panic!("{error:?}")
        };
        assert_ne!(code, 0);
        assert!(tail.contains("Invalid data"), "{tail}");
    }
}
