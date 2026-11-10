//! The video of a screen recording: the frames the screen gives, fitted to the size of the video, are written to ffmpeg's
//! standard input as raw pixels, and ffmpeg encodes them into `video.mp4` (or `video.webm`) as they come. When the
//! recording ends the audio is added to it (`mux`).
//!
//! The file is fragmented as it is written (an MP4 with a fragment at every keyframe, a WebM that is a list of clusters),
//! so what was recorded can be played even if the program, or the whole machine, stopped before the end: it is written
//! to `video.<ext>.<nonce>.part` and renamed when ffmpeg has finished, and a `.part` that is left behind can be renamed
//! by whoever finds it.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use process_wrap::std::StdChildWrapper;

use super::quality::{Quality, QualitySpec};
use crate::media::decode::Tail;
use crate::media::encode::ffmpeg::{spawn, Process};
use crate::media::encode::{part_path, publish, remove_part, EncodeError, M4A_BITRATE};
use crate::media::tools;

/// A picture of the screen as the capture gives it: BGRA, a row every `stride` bytes (which may be more than the
/// row needs).
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Bytes from the start of a row to the start of the next.
    pub stride: u32,
    pub bgra: Vec<u8>,
}

/// Where the capture leaves the latest frame, for the recorder to take at its own pace: the screen gives a frame
/// when something changes, and the video needs one every 1/fps of a second.
#[derive(Clone, Default)]
pub struct FrameSlot(Arc<Mutex<Option<Arc<Frame>>>>);

impl FrameSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the frame in the slot, if any.
    pub fn put(&self, frame: Frame) {
        *self.lock() = Some(Arc::new(frame));
    }

    /// The frame that was put last. It stays what it was: a later `put` does not change a frame that was taken.
    pub fn latest(&self) -> Option<Arc<Frame>> {
        self.lock().clone()
    }

    /// A thread that panicked with the slot locked has left it as it was, which is a frame or none: the slot is as good.
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Arc<Frame>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The frame as the pixels of a video of `size`: scaled to fit with its aspect kept (nearest neighbour: it is cheap,
/// and runs for every frame), centred, with black where it does not reach. Tightly packed BGRA, `size.0 * size.1 * 4`
/// bytes. A frame that has no pixels (or fewer than it says) gives all black.
pub fn fit_into(frame: &Frame, size: (u32, u32)) -> Vec<u8> {
    let (width, height) = (size.0 as usize, size.1 as usize);
    let mut fitted = vec![0u8; width * height * 4];
    let (source_width, source_height) = (frame.width as usize, frame.height as usize);
    let stride = frame.stride as usize;
    let has_pixels = source_width > 0
        && source_height > 0
        && stride >= source_width * 4
        && frame.bgra.len() >= (source_height - 1) * stride + source_width * 4;
    if width == 0 || height == 0 || !has_pixels {
        paint_black(&mut fitted);
        return fitted;
    }

    // The picture is as large as the video allows with its aspect kept (to the nearest pixel), in the middle.
    let (picture_width, picture_height) = if width * source_height <= height * source_width {
        (
            width,
            ((source_height * width + source_width / 2) / source_width).clamp(1, height),
        )
    } else {
        (
            ((source_width * height + source_height / 2) / source_height).clamp(1, width),
            height,
        )
    };
    let (left, top) = ((width - picture_width) / 2, (height - picture_height) / 2);
    // The byte in a row of the frame that each column of the picture is taken from: the pixel in the middle of the
    // part of the frame that the column stands for.
    let columns: Vec<usize> = (0..picture_width)
        .map(|x| (2 * x + 1) * source_width / (2 * picture_width) * 4)
        .collect();

    for (y, row) in fitted.chunks_exact_mut(width * 4).enumerate() {
        let Some(picture_row) = y.checked_sub(top).filter(|row| *row < picture_height) else {
            paint_black(row);
            continue;
        };
        let source_y = (2 * picture_row + 1) * source_height / (2 * picture_height);
        let source = &frame.bgra[source_y * stride..][..source_width * 4];
        let (before, rest) = row.split_at_mut(left * 4);
        let (picture, after) = rest.split_at_mut(picture_width * 4);
        paint_black(before);
        paint_black(after);
        if picture_width == source_width {
            picture.copy_from_slice(source);
        } else {
            for (pixel, &x) in picture.chunks_exact_mut(4).zip(&columns) {
                pixel.copy_from_slice(&source[x..x + 4]);
            }
        }
    }
    fitted
}

/// Opaque black, for every pixel of `bgra`.
fn paint_black(bgra: &mut [u8]) {
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[0, 0, 0, 255]);
    }
}

/// What the video is stored in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Container {
    /// H.264 (or MPEG-4) in a fragmented MP4: what the player of macOS and Windows plays.
    Mp4,
    /// VP9 in WebM: what the player of Linux plays (WebKitGTK decodes H.264 only with codecs that a system may lack).
    WebM,
}

impl Container {
    /// The extension of the files: `video.mp4`, `recording.mp4`.
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::WebM => "webm",
        }
    }
}

/// An encoder of ffmpeg and the container its video goes in.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoCodec {
    pub encoder: &'static str,
    pub container: Container,
}

/// The encoders to use, best first: the one of the system's own hardware where there is one, and then the ones that
/// any ffmpeg may have. `pick_codec` goes by what ffmpeg lists, which is what it was built with, and does not try the
/// encoder: `h264_nvenc`, `h264_qsv` and `h264_amf` are listed whether there is such a card or not.
#[cfg(target_os = "macos")]
pub const ENCODER_ORDER: &[&str] = &["h264_videotoolbox", "libopenh264", "libx264", "mpeg4"];
#[cfg(windows)]
pub const ENCODER_ORDER: &[&str] = &[
    "h264_mf",
    "h264_nvenc",
    "h264_qsv",
    "h264_amf",
    "libopenh264",
    "libx264",
    "mpeg4",
];
#[cfg(not(any(target_os = "macos", windows)))]
pub const ENCODER_ORDER: &[&str] = &["libvpx-vp9", "libopenh264", "libx264", "mpeg4"];

/// The first of `ENCODER_ORDER` that this ffmpeg has and that works, or `None` when there is none (or ffmpeg cannot
/// be run).
///
/// What ffmpeg lists (`-encoders`) is what it was built with, not what the machine can do: `h264_nvenc` is listed with
/// no NVIDIA card, `h264_mf` on a Windows "N" edition without the Media Feature Pack. So each listed encoder is tried,
/// in the order of the list, with two small frames written to nowhere (`works`), and the first that ends well is the
/// pick. That is a process started per candidate (a few at most; each is over in a fraction of a second, and one that
/// hangs is given up after 10 s), so the caller should keep the result and not ask again for every recording.
pub fn pick_codec(ffmpeg: &Path) -> Option<VideoCodec> {
    let output = tools::command(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let listed = candidates(&String::from_utf8_lossy(&output.stdout));
    first_working(listed, |codec| works(ffmpeg, codec))
}

/// `pick_codec` for the output of `ffmpeg -hide_banner -encoders`, by what is listed alone: the first of
/// `ENCODER_ORDER` that is there.
pub fn pick_from_list(encoders_output: &str) -> Option<VideoCodec> {
    candidates(encoders_output).into_iter().next()
}

/// The encoders of `ENCODER_ORDER` that the output of `ffmpeg -hide_banner -encoders` lists, in the order of
/// `ENCODER_ORDER`.
fn candidates(encoders_output: &str) -> Vec<VideoCodec> {
    // A line is the flags of the encoder (`V....D`: a video encoder) and its name, and then what it is.
    let video_encoders: Vec<&str> = encoders_output
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (flags, name) = (words.next()?, words.next()?);
            (flags.len() == 6 && flags.starts_with('V')).then_some(name)
        })
        .collect();
    ENCODER_ORDER
        .iter()
        .filter(|encoder| video_encoders.contains(encoder))
        .map(|&encoder| VideoCodec {
            encoder,
            // VP9 is the one encoder whose video does not go in an MP4.
            container: if encoder == "libvpx-vp9" {
                Container::WebM
            } else {
                Container::Mp4
            },
        })
        .collect()
}

/// The first of `candidates` that `works`: the ones after it are not tried.
fn first_working(
    candidates: Vec<VideoCodec>,
    works: impl Fn(&VideoCodec) -> bool,
) -> Option<VideoCodec> {
    candidates.into_iter().find(|codec| works(codec))
}

/// How long a probe of an encoder is given.
const PROBE_LIMIT: Duration = Duration::from_secs(10);
/// The size of the frames of a probe: large enough for the hardware encoders (some refuse a small picture), and small
/// enough to cost nothing.
const PROBE_SIZE: (u32, u32) = (256, 256);
const PROBE_FRAMES: usize = 2;

/// Whether `ffmpeg` can encode with `codec` here: it is made to encode two black frames to nowhere, with the options
/// that `VideoEncoder` gives the encoder, and it has to end well within `PROBE_LIMIT`.
fn works(ffmpeg: &Path, codec: &VideoCodec) -> bool {
    works_within(ffmpeg, codec, PROBE_LIMIT)
}

fn works_within(ffmpeg: &Path, codec: &VideoCodec, limit: Duration) -> bool {
    let Ok(Process {
        mut child,
        stdin,
        stderr,
        ..
    }) = spawn(
        ffmpeg,
        probe_arguments(PROBE_SIZE, &Quality::Small.spec(), codec),
    )
    else {
        return false;
    };
    // What ffmpeg says is read, and the frames are written, off this thread: a program that does not read its input
    // (or fills the pipe of its messages) must not keep this one from giving up on it. Both threads end by themselves
    // when ffmpeg does, killed or not.
    std::thread::spawn(move || Tail::read(stderr));
    std::thread::spawn(move || {
        let mut stdin = stdin;
        let frame = vec![0u8; PROBE_SIZE.0 as usize * PROBE_SIZE.1 as usize * 4];
        for _ in 0..PROBE_FRAMES {
            if stdin.write_all(&frame).is_err() {
                return;
            }
        }
        // Dropping the pipe is the end of the frames.
    });
    let give_up = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < give_up => {
                std::thread::sleep(Duration::from_millis(10));
            }
            // It did not end in time (or cannot be asked): it and what it started are killed.
            _ => {
                let _ = child.start_kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// ffmpeg, and how it ended once it has. It is shared by the encoder and the `AbortHandle`s of it, and is never
/// waited for twice: a process group that has been waited for may be another program's now, and is not killed.
struct Running {
    child: Box<dyn StdChildWrapper>,
    status: Option<ExitStatus>,
}

type SharedProcess = Arc<Mutex<Running>>;

/// A thread that panicked with the process locked has not changed it in a way that matters here.
fn lock(process: &SharedProcess) -> MutexGuard<'_, Running> {
    process.lock().unwrap_or_else(|e| e.into_inner())
}

impl Running {
    /// How ffmpeg ended, if it has.
    fn poll(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = self.child.try_wait()?;
        }
        Ok(self.status)
    }

    /// Kills ffmpeg and what it started, if it has not ended.
    fn kill(&mut self) {
        if self.status.is_none() {
            // It may be gone already, which is fine.
            let _ = self.child.start_kill();
        }
    }

    /// Makes ffmpeg end, if it has not, and waits for it and for what it started to be gone.
    fn end(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        self.kill();
        let status = self.child.wait()?;
        self.status = Some(status);
        Ok(status)
    }
}

/// Ends the ffmpeg of a `VideoEncoder` from another thread. A `push` waits for ffmpeg to take the frame, and that
/// goes on for ever when ffmpeg has stopped reading (a hardware encoder that hangs): it ends only when ffmpeg is gone.
#[derive(Clone)]
pub struct AbortHandle(SharedProcess);

impl AbortHandle {
    /// Kills ffmpeg and what it started. A `push` that waits on it fails; `finish` says it did not end well. Nothing
    /// happens when ffmpeg has ended already.
    pub fn abort(&self) {
        lock(&self.0).kill();
    }
}

/// How long `finish` waits for ffmpeg to write the end of the file.
const FINISH_LIMIT: Duration = Duration::from_secs(30);

/// ffmpeg encoding the frames that are pushed to it.
///
/// A `VideoEncoder` that is dropped without `finish` kills ffmpeg (and what it started) and leaves the `.part` file
/// as it was: what is in it can be played up to the last fragment.
pub struct VideoEncoder {
    process: SharedProcess,
    /// `None` once the end of the frames has been told to ffmpeg.
    stdin: Option<ChildStdin>,
    /// The thread that reads what ffmpeg says; it ends when ffmpeg does.
    stderr: Option<JoinHandle<Tail>>,
    /// `video.mp4` or `video.webm`, what the `.part` becomes.
    out: PathBuf,
    part: PathBuf,
    size: (u32, u32),
    frames: u64,
    /// Why ffmpeg stopped, when it did before the end.
    failure: Option<String>,
}

impl VideoEncoder {
    /// Starts ffmpeg writing `folder/video.mp4` (or `video.webm`, by the container of `codec`) from frames of `size`,
    /// `spec.fps` of them to a second of video.
    pub fn start(
        folder: &Path,
        size: (u32, u32),
        spec: &QualitySpec,
        codec: &VideoCodec,
        ffmpeg: &Path,
    ) -> Result<VideoEncoder, EncodeError> {
        let out = folder.join(format!("video.{}", codec.container.extension()));
        let part = part_path(&out);
        let Process {
            child,
            stdin,
            stderr,
            ..
        } = spawn(ffmpeg, arguments(size, spec, codec, &part))?;
        Ok(VideoEncoder {
            process: Arc::new(Mutex::new(Running {
                child,
                status: None,
            })),
            stdin: Some(stdin),
            // What ffmpeg prints is read all the time, whatever the video does: a program that fills the pipe stops.
            stderr: Some(std::thread::spawn(move || Tail::read(stderr))),
            out,
            part,
            size,
            frames: 0,
            failure: None,
        })
    }

    /// Gives ffmpeg the next frame: `size.0 * size.1 * 4` bytes of BGRA, tightly packed (what `fit_into` makes). It
    /// waits when ffmpeg is slower than the frames come. An error is the end of the video: ffmpeg is gone.
    pub fn push(&mut self, bgra_of_size: &[u8]) -> io::Result<()> {
        let expected = self.size.0 as usize * self.size.1 as usize * 4;
        if bgra_of_size.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "a frame of {}x{} is {expected} bytes, not {}",
                    self.size.0,
                    self.size.1,
                    bgra_of_size.len()
                ),
            ));
        }
        if let Some(failure) = &self.failure {
            return Err(io::Error::other(failure.clone()));
        }
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(io::Error::other("the video has already ended"));
        };
        if let Err(e) = stdin.write_all(bgra_of_size) {
            // ffmpeg is gone (a closed pipe), or no longer taking the frames.
            let failure = self.say_why(&format!("stopped taking the frames ({e})"));
            self.failure = Some(failure.clone());
            return Err(io::Error::other(failure));
        }
        self.frames += 1;
        Ok(())
    }

    /// A handle that ends ffmpeg from another thread, for the encoder that is stuck in `push`.
    pub fn abort_handle(&self) -> AbortHandle {
        AbortHandle(self.process.clone())
    }

    /// How many frames ffmpeg has been given.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Ends the video: ffmpeg writes what it holds and ends, and the `.part` file becomes `video.mp4` (the path is
    /// the result). ffmpeg is waited for 30 s at most. When this fails the `.part` is left, with what ffmpeg
    /// wrote of it.
    pub fn finish(self) -> Result<PathBuf, EncodeError> {
        self.finish_within(FINISH_LIMIT)
    }

    fn finish_within(mut self, limit: Duration) -> Result<PathBuf, EncodeError> {
        if let Some(failure) = &self.failure {
            return Err(EncodeError::Failed(failure.clone()));
        }
        // Closing the pipe is the end of the frames: ffmpeg writes what it holds and the end of the file, and ends.
        drop(self.stdin.take());
        let give_up = Instant::now() + limit;
        // Polled, not waited for: a program that does not end must not hold the recording for ever.
        let status = loop {
            let polled = lock(&self.process).poll();
            match polled {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < give_up => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Ok(None) => {
                    let why = self.say_why(&format!(
                        "did not finish the video in {} s",
                        limit.as_secs_f32()
                    ));
                    return Err(EncodeError::Failed(why));
                }
                Err(e) => {
                    let why = self.say_why(&format!("could not be waited for ({e})"));
                    return Err(EncodeError::Failed(why));
                }
            }
        };
        if !status.success() {
            return Err(EncodeError::Failed(
                self.say_why("did not finish the video"),
            ));
        }
        publish(&self.part, &self.out)
            .map_err(|e| EncodeError::Failed(format!("could not save the video: {e}")))?;
        Ok(self.out.clone())
    }

    /// Ends ffmpeg and everything it started, if it is still running, and waits for them to be gone.
    fn end(&mut self) -> io::Result<ExitStatus> {
        drop(self.stdin.take());
        lock(&self.process).end()
    }

    /// What ffmpeg said, once it has ended (it is made to end here, if it has not).
    fn say_why(&mut self, how: &str) -> String {
        let status = self.end();
        let tail = self
            .stderr
            .take()
            .and_then(|thread| thread.join().ok())
            .map(|tail| tail.text())
            .unwrap_or_default();
        format!("ffmpeg {how} ({}): {tail}", describe(status))
    }
}

impl Drop for VideoEncoder {
    /// An encoder that is dropped without being finished kills ffmpeg. The `.part` file is left: it is what there is
    /// of the video.
    fn drop(&mut self) {
        let _ = self.end();
    }
}

/// How ffmpeg ended, in words.
fn describe(status: io::Result<ExitStatus>) -> String {
    match status {
        Ok(status) => status
            .code()
            .map_or("no exit code".to_owned(), |c| format!("exit code {c}")),
        Err(e) => format!("could not be waited for: {e}"),
    }
}

/// The arguments of ffmpeg for a video of frames of `size`, ending with the name of the file it writes.
///
/// The frames are raw BGRA on the standard input, `spec.fps` of them to a second, and the video has a keyframe every
/// two seconds, which is also where a fragment of the file ends: a file that is cut off is playable up to the last
/// fragment (`-flush_packets 1` is what puts each of them in the file at once). The file is a `.part`, which has no
/// format of its own to ffmpeg, so the format is given; `file:` in front of the name keeps ffmpeg from taking a part of
/// it for a protocol, or a name that starts with `-` for an option.
fn arguments(
    size: (u32, u32),
    spec: &QualitySpec,
    codec: &VideoCodec,
    part: &Path,
) -> Vec<OsString> {
    let mut args = encoding_arguments(size, spec, codec);
    // What the muxer writes goes to the file as it is written. ffmpeg holds it in a buffer of its own until it is full
    // otherwise, and a screen that hardly changes fills it in minutes: a recording that was cut off would have
    // nothing in the file, not even the start of it, which is what makes the rest readable.
    args.extend(words(&["-flush_packets", "1"]));
    args.extend(match codec.container {
        Container::Mp4 => words(&[
            "-movflags",
            "+frag_keyframe+empty_moov+default_base_moof",
            "-f",
            "mp4",
        ]),
        Container::WebM => words(&["-f", "webm"]),
    });
    args.push(file_argument(part));
    args
}

/// The arguments of ffmpeg that make a probe of `codec` (see `works`): what `arguments` has up to the container, and
/// the output goes nowhere.
fn probe_arguments(size: (u32, u32), spec: &QualitySpec, codec: &VideoCodec) -> Vec<OsString> {
    let mut args = encoding_arguments(size, spec, codec);
    args.extend(words(&["-f", "null", "-"]));
    args
}

/// The arguments of ffmpeg from the start to the end of what it is told about the encoder: where the frames come from,
/// and how they are encoded.
fn encoding_arguments(size: (u32, u32), spec: &QualitySpec, codec: &VideoCodec) -> Vec<OsString> {
    let mut args = words(&["-hide_banner", "-nostdin", "-y", "-v", "error"]);
    args.extend(words(&[
        "-f",
        "rawvideo",
        "-pix_fmt",
        "bgra",
        "-video_size",
    ]));
    args.extend(words(&[&format!("{}x{}", size.0, size.1)]));
    args.extend(words(&[
        "-framerate",
        &spec.fps.to_string(),
        "-i",
        "pipe:0",
    ]));
    args.extend(words(&["-vf", "format=yuv420p", "-c:v", codec.encoder]));
    args.extend(words(&["-b:v", &spec.bitrate.to_string()]));
    args.extend(words(&["-g", &(spec.fps * 2).to_string()]));
    if codec.encoder == "h264_videotoolbox" {
        // The hardware encoder at the speed of the capture, and the software one of the system when there is no
        // hardware (a virtual machine).
        args.extend(words(&["-realtime", "1", "-allow_sw", "1"]));
    }
    if codec.encoder == "libvpx-vp9" {
        // The speed of the capture too: the encoder is not given the time that its best quality takes.
        args.extend(words(&[
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            "-row-mt",
            "1",
        ]));
    }
    args
}

/// `file:` and then the path.
fn file_argument(path: &Path) -> OsString {
    let mut argument = OsString::from("file:");
    argument.push(path);
    argument
}

/// How long `mux` waits for ffmpeg: copying the streams of a recording takes seconds, and this is for the one that
/// is stuck.
const MUX_LIMIT: Duration = Duration::from_secs(10 * 60);

/// Puts the audio under the video: `folder/recording.mp4` (or `.webm`), with the video as it is and the audio of
/// `audio` (the `audio.m4a`, or the `recording.wav` of a system that had no AAC encoder). ffmpeg is given ten
/// minutes at most (`mux_within`).
pub fn mux(
    video: &Path,
    audio: &Path,
    folder: &Path,
    container: Container,
    ffmpeg: &Path,
) -> Result<PathBuf, EncodeError> {
    mux_within(video, audio, folder, container, ffmpeg, MUX_LIMIT)
}

/// `mux`, with ffmpeg given `limit` to end: it and what it started are killed after that, and there is no file.
pub fn mux_within(
    video: &Path,
    audio: &Path,
    folder: &Path,
    container: Container,
    ffmpeg: &Path,
    limit: Duration,
) -> Result<PathBuf, EncodeError> {
    let out = folder.join(format!("recording.{}", container.extension()));
    let part = part_path(&out);
    let Process {
        mut child,
        stdin,
        stderr,
        ..
    } = spawn(ffmpeg, mux_arguments(video, audio, container, &part))?;
    // ffmpeg is told not to read, and has nothing to read.
    drop(stdin);
    let stderr = std::thread::spawn(move || Tail::read(stderr));
    let give_up = Instant::now() + limit;
    // Polled, not waited for: a program that does not end must not hold the recording for ever.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < give_up => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.start_kill();
                let _ = child.wait();
                let _ = stderr.join();
                remove_part(&part);
                return Err(EncodeError::Failed(format!(
                    "ffmpeg did not finish putting the audio under the video in {} s",
                    limit.as_secs_f32()
                )));
            }
            Err(e) => {
                let _ = child.start_kill();
                let _ = child.wait();
                break Err(e);
            }
        }
    };
    let tail = stderr.join().map(|tail| tail.text()).unwrap_or_default();
    match status {
        Ok(status) if status.success() => {
            if let Err(e) = publish(&part, &out) {
                remove_part(&part);
                return Err(EncodeError::Failed(format!(
                    "could not save the recording: {e}"
                )));
            }
            Ok(out)
        }
        status => {
            remove_part(&part);
            Err(EncodeError::Failed(format!(
                "ffmpeg could not put the audio under the video ({}): {tail}",
                describe(status)
            )))
        }
    }
}

/// How long `probe_seconds` waits for ffmpeg.
const PROBE_SECONDS_LIMIT: Duration = Duration::from_secs(30);

/// How long the video in `video` is, in seconds: what ffmpeg reaches when it reads the file to its end without
/// decoding it (it copies the packets to nowhere), which is as far as the video can be played. A file that was cut
/// off gives the length of what is in it. `None` when ffmpeg cannot be run, cannot read the file, or does not end
/// within 30 s.
pub fn probe_seconds(video: &Path, ffmpeg: &Path) -> Option<f64> {
    probe_within(video, ffmpeg, PROBE_SECONDS_LIMIT)
}

fn probe_within(video: &Path, ffmpeg: &Path, limit: Duration) -> Option<f64> {
    let mut args = words(&["-hide_banner", "-nostdin", "-i"]);
    args.push(file_argument(video));
    args.extend(words(&["-map", "0:v", "-c", "copy", "-f", "null", "-"]));
    let Process {
        mut child,
        stdin,
        mut stderr,
        ..
    } = spawn(ffmpeg, args).ok()?;
    drop(stdin);
    // What ffmpeg says is read all the time; only the end of it is kept, where the last `time=` is.
    let said = std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n @ 1..) = io::Read::read(&mut stderr, &mut chunk) {
            kept.extend_from_slice(&chunk[..n]);
            if kept.len() > 16 * 1024 {
                kept.drain(..kept.len() - 8 * 1024);
            }
        }
        String::from_utf8_lossy(&kept).into_owned()
    });
    let give_up = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < give_up => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.start_kill();
                let _ = child.wait();
                let _ = said.join();
                return None;
            }
        }
    }
    last_time(&said.join().ok()?)
}

/// The last `time=hh:mm:ss.ff` of ffmpeg's progress lines, in seconds.
pub(crate) fn last_time(stderr: &str) -> Option<f64> {
    let after = &stderr[stderr.rfind("time=")? + "time=".len()..];
    let clock = after.split_whitespace().next()?;
    let mut parts = clock.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

/// The arguments of ffmpeg for `mux`, ending with the name of the file it writes. The video is copied as it is.
/// The audio is copied too when the container can have it as it is (the AAC of an `audio.m4a` in an MP4), and made
/// into what the container takes when not: AAC for a `recording.wav` in an MP4, and Opus for anything in a WebM, which
/// cannot carry AAC. An MP4 has its index put at the start, so that a player can begin before it has all of the file.
fn mux_arguments(video: &Path, audio: &Path, container: Container, part: &Path) -> Vec<OsString> {
    let mut args = words(&["-hide_banner", "-nostdin", "-y", "-v", "error", "-i"]);
    args.push(file_argument(video));
    args.extend(words(&["-i"]));
    args.push(file_argument(audio));
    args.extend(words(&["-map", "0:v", "-map", "1:a"]));
    let aac_already = audio.extension().is_some_and(|extension| {
        ["m4a", "mp4", "aac"]
            .iter()
            .any(|e| extension.eq_ignore_ascii_case(e))
    });
    let aac_bitrate = format!("{}k", M4A_BITRATE / 1000);
    args.extend(match container {
        Container::Mp4 if aac_already => {
            words(&["-c", "copy", "-movflags", "+faststart", "-f", "mp4"])
        }
        Container::Mp4 => words(&[
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            &aac_bitrate,
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ]),
        Container::WebM => words(&[
            "-c:v", "copy", "-c:a", "libopus", "-b:a", "64k", "-f", "webm",
        ]),
    });
    args.push(file_argument(part));
    args
}

fn words(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::decode::MonoDecoder;
    use crate::media::encode::m4a_writer;
    #[cfg(unix)]
    use crate::media::test_support::script;
    use crate::media::test_support::{read_all, real_ffmpeg};
    use crate::record::quality::Quality;
    use crate::record::test_support::video_seconds;
    use std::fs;

    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    /// A frame of one colour, tightly packed.
    fn solid(width: u32, height: u32, pixel: [u8; 4]) -> Frame {
        Frame {
            width,
            height,
            stride: width * 4,
            bgra: pixel.repeat((width * height) as usize),
        }
    }

    fn pixel(bgra: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        let at = (y * width + x) * 4;
        bgra[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn fit_into_letterboxes_a_tall_frame() {
        // 100x200 into 200x100: it is as tall as the video and 50 wide, with 75 columns of black on each side.
        let frame = solid(100, 200, WHITE);

        let fitted = fit_into(&frame, (200, 100));

        assert_eq!(fitted.len(), 200 * 100 * 4);
        for y in [0, 50, 99] {
            for x in (0..75).chain(125..200) {
                assert_eq!(pixel(&fitted, 200, x, y), BLACK, "({x}, {y})");
            }
            for x in 75..125 {
                assert_eq!(pixel(&fitted, 200, x, y), WHITE, "({x}, {y})");
            }
        }
    }

    #[test]
    fn fit_into_letterboxes_a_wide_frame_above_and_below() {
        let frame = solid(200, 100, WHITE);

        let fitted = fit_into(&frame, (100, 100));

        // 100x50 in the middle: 25 rows of black above and below.
        for x in [0, 99] {
            assert_eq!(pixel(&fitted, 100, x, 24), BLACK);
            assert_eq!(pixel(&fitted, 100, x, 25), WHITE);
            assert_eq!(pixel(&fitted, 100, x, 74), WHITE);
            assert_eq!(pixel(&fitted, 100, x, 75), BLACK);
        }
    }

    #[test]
    fn fit_into_handles_a_stride_wider_than_the_row() {
        // 4x2 pixels in rows of 24 bytes: 16 of pixels and 8 of padding that is not part of the picture.
        let mut bgra = Vec::new();
        for y in 0..2u8 {
            for x in 0..4u8 {
                bgra.extend([x, y, 100 + x, 255]);
            }
            bgra.extend([0xEE; 8]);
        }
        let frame = Frame {
            width: 4,
            height: 2,
            stride: 24,
            bgra,
        };

        let same_size = fit_into(&frame, (4, 2));
        let doubled = fit_into(&frame, (8, 4));

        let expected: Vec<u8> = (0..2u8)
            .flat_map(|y| (0..4u8).flat_map(move |x| [x, y, 100 + x, 255]))
            .collect();
        assert_eq!(same_size, expected);
        // Nearest neighbour: every pixel becomes a block of 2x2.
        for y in 0..4 {
            for x in 0..8 {
                assert_eq!(
                    pixel(&doubled, 8, x, y),
                    pixel(&expected, 4, x / 2, y / 2),
                    "({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn fit_into_takes_the_pixel_in_the_middle_of_each_block_when_it_shrinks() {
        // 4x4 into 2x2: each pixel of the result stands for a block of 2x2, and is the one in it that is
        // nearest the middle (the second: the first of a block of 2 is as near as the second, and the later wins).
        let mut bgra = Vec::new();
        for y in 0..4u8 {
            for x in 0..4u8 {
                bgra.extend([x, y, 0, 255]);
            }
        }
        let frame = Frame {
            width: 4,
            height: 4,
            stride: 16,
            bgra,
        };

        let fitted = fit_into(&frame, (2, 2));

        assert_eq!(pixel(&fitted, 2, 0, 0), [1, 1, 0, 255]);
        assert_eq!(pixel(&fitted, 2, 1, 1), [3, 3, 0, 255]);
    }

    #[test]
    fn fit_into_gives_black_for_a_frame_that_has_no_pixels() {
        let empty = Frame {
            width: 0,
            height: 0,
            stride: 0,
            bgra: Vec::new(),
        };
        // It says 4x4 and brings 10 bytes.
        let short = Frame {
            width: 4,
            height: 4,
            stride: 16,
            bgra: vec![255; 10],
        };

        for frame in [&empty, &short] {
            let fitted = fit_into(frame, (6, 2));
            assert_eq!(fitted, BLACK.repeat(12));
        }
        assert_eq!(fit_into(&solid(2, 2, WHITE), (0, 0)), Vec::<u8>::new());
    }

    #[test]
    fn the_frame_slot_keeps_only_the_latest() {
        let slot = FrameSlot::new();
        assert!(slot.latest().is_none());

        slot.put(solid(2, 2, BLACK));
        slot.put(solid(4, 4, WHITE));

        let latest = slot.latest().unwrap();
        assert_eq!((latest.width, latest.height), (4, 4));
        // The capture and the recorder have a clone each, and a frame that was taken stays what it was.
        slot.clone().put(solid(6, 6, BLACK));
        assert_eq!(slot.latest().unwrap().width, 6);
        assert_eq!(latest.width, 4);
    }

    /// What `ffmpeg -encoders` prints (the start and a few of its lines) with the encoders in `names` as video
    /// encoders, and audio encoders that are named like two of them.
    fn listing(names: &[&str]) -> String {
        let mut text = String::from(
            "Encoders:\n V..... = Video\n A..... = Audio\n S..... = Subtitle\n ------\n A....D libx264  an audio encoder by that name, which cannot be one\n A....D mpeg4  nor this\n",
        );
        for name in names {
            text.push_str(&format!(
                " V....D {name:<18} {name} description (codec h264)\n"
            ));
        }
        text.push_str(" V....D libx264rgb          libx264 RGB\n");
        text
    }

    #[test]
    fn pick_from_list_follows_the_os_order() {
        let first = ENCODER_ORDER[0];

        let all = pick_from_list(&listing(&["libx264", "mpeg4", first])).unwrap();
        assert_eq!(all.encoder, first);

        let only_mpeg4 = pick_from_list(&listing(&["mpeg4"])).unwrap();
        assert_eq!(
            only_mpeg4,
            VideoCodec {
                encoder: "mpeg4",
                container: Container::Mp4
            }
        );

        // The encoder of the line that says it is for audio is not one: its name is only the same.
        assert_eq!(pick_from_list(&listing(&[])), None);
        assert_eq!(pick_from_list(""), None);
        // An encoder that merely starts with the name of one in the list is another encoder.
        assert_eq!(pick_from_list(" V....D libx264rgb  RGB\n"), None);
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn linux_picks_vp9_in_webm_when_that_is_all_there_is() {
        let vp9 = pick_from_list(&listing(&["libvpx-vp9"])).unwrap();

        assert_eq!(
            vp9,
            VideoCodec {
                encoder: "libvpx-vp9",
                container: Container::WebM
            }
        );
        // And an H.264 encoder goes in MP4 on Linux as anywhere.
        assert_eq!(
            pick_from_list(&listing(&["libx264"])).unwrap().container,
            Container::Mp4
        );
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn macos_and_windows_leave_vp9_alone() {
        assert_eq!(pick_from_list(&listing(&["libvpx-vp9"])), None);
    }

    #[test]
    fn every_listed_encoder_is_a_candidate_in_the_order_of_the_list() {
        let all: Vec<&str> = ENCODER_ORDER.to_vec();
        let every = candidates(&listing(&all));
        assert_eq!(every.iter().map(|c| c.encoder).collect::<Vec<_>>(), all);

        // The ones that are not listed are left out, and the order is the list's, not the listing's.
        let some = candidates(&listing(&["mpeg4", ENCODER_ORDER[0]]));
        assert_eq!(
            some.iter().map(|c| c.encoder).collect::<Vec<_>>(),
            [ENCODER_ORDER[0], "mpeg4"]
        );
        assert!(candidates("").is_empty());
    }

    fn codec(encoder: &'static str) -> VideoCodec {
        VideoCodec {
            encoder,
            container: Container::Mp4,
        }
    }

    #[test]
    fn first_working_skips_the_candidates_that_do_not() {
        let candidates = vec![codec("h264_mf"), codec("libx264"), codec("mpeg4")];
        let asked = std::cell::RefCell::new(Vec::new());

        let picked = first_working(candidates.clone(), |c| {
            asked.borrow_mut().push(c.encoder);
            c.encoder != "h264_mf"
        });

        assert_eq!(picked, Some(codec("libx264")));
        // Nothing after the one that works is tried.
        assert_eq!(*asked.borrow(), ["h264_mf", "libx264"]);
        // The first one is the pick when it works.
        assert_eq!(first_working(candidates, |_| true), Some(codec("h264_mf")));
    }

    #[test]
    fn first_working_gives_none_when_none_work() {
        let candidates = vec![codec("h264_mf"), codec("mpeg4")];

        assert_eq!(first_working(candidates, |_| false), None);
        assert_eq!(first_working(Vec::new(), |_| true), None);
    }

    /// A stand-in for ffmpeg that lists `names` for `-encoders`, and for anything else (the probe of an encoder)
    /// does what `probe` says: a shell command.
    #[cfg(unix)]
    fn listing_ffmpeg(dir: &Path, names: &[&str], probe: &str) -> PathBuf {
        let listing = listing(names);
        script(
            dir,
            "fake-ffmpeg",
            &format!(
                "case \"$*\" in\n\"-hide_banner -encoders\")\ncat <<'EOF'\n{listing}EOF\n;;\n*)\n{probe}\n;;\nesac"
            ),
            "",
        )
    }

    #[cfg(unix)]
    #[test]
    fn pick_codec_takes_the_first_listed_encoder_that_works() {
        let dir = tempfile::tempdir().unwrap();
        let first = ENCODER_ORDER[0];
        // The first choice is listed and cannot be started: the probe of it fails and the next one's passes.
        let fake = listing_ffmpeg(
            dir.path(),
            &["mpeg4", first],
            &format!("cat > /dev/null; case \"$*\" in *\"-c:v {first} \"*) exit 1;; esac"),
        );

        assert_eq!(pick_codec(&fake), Some(codec("mpeg4")));

        // One that works is the pick, as it is listed first.
        let works = listing_ffmpeg(dir.path(), &["mpeg4", first], "cat > /dev/null");
        assert_eq!(pick_codec(&works).unwrap().encoder, first);
    }

    #[cfg(unix)]
    #[test]
    fn pick_codec_gives_none_when_no_listed_encoder_works() {
        let dir = tempfile::tempdir().unwrap();
        let fake = listing_ffmpeg(
            dir.path(),
            &["mpeg4", ENCODER_ORDER[0]],
            "cat > /dev/null; exit 1",
        );

        assert_eq!(pick_codec(&fake), None);
        // Not a program at all: there is no encoder to speak of.
        assert_eq!(pick_codec(&dir.path().join("none")), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_probe_encodes_two_frames_to_nowhere_with_the_options_of_the_encoder() {
        let dir = tempfile::tempdir().unwrap();
        let fake = script(
            dir.path(),
            "fake-ffmpeg",
            r#"printf '%s\n' "$@" > "$0.args"; cat > "$0.in""#,
            "",
        );
        let videotoolbox = codec("h264_videotoolbox");
        let vp9 = VideoCodec {
            encoder: "libvpx-vp9",
            container: Container::WebM,
        };

        assert!(works(&fake, &videotoolbox));
        let args = fs::read_to_string(format!("{}.args", fake.display())).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>().join(" "),
            "-hide_banner -nostdin -y -v error -f rawvideo -pix_fmt bgra -video_size 256x256 -framerate 10 -i pipe:0 -vf format=yuv420p -c:v h264_videotoolbox -b:v 1000000 -g 20 -realtime 1 -allow_sw 1 -f null -"
        );
        let sent = fs::read(format!("{}.in", fake.display())).unwrap();
        assert_eq!(sent, vec![0u8; 2 * 256 * 256 * 4]);

        assert!(works(&fake, &vp9));
        let args = fs::read_to_string(format!("{}.args", fake.display())).unwrap();
        assert!(
            args.lines().collect::<Vec<_>>().join(" ").ends_with(
                "-c:v libvpx-vp9 -b:v 1000000 -g 20 -deadline realtime -cpu-used 8 -row-mt 1 -f null -"
            ),
            "{args}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_probe_that_fails_or_hangs_is_not_an_encoder_that_works() {
        let dir = tempfile::tempdir().unwrap();
        let fails = script(
            dir.path(),
            "fails",
            "echo 'Cannot load MFPlat.DLL' >&2; exit 1",
            "",
        );
        // Reads what it is sent and then does not end.
        let hangs = script(dir.path(), "hangs", "cat > /dev/null; sleep 60", "");
        let started = Instant::now();

        assert!(!works(&fails, &codec("h264_mf")));
        assert!(!works_within(
            &hangs,
            &codec("h264_mf"),
            Duration::from_millis(300)
        ));

        assert!(started.elapsed() < Duration::from_secs(20));
        assert!(!works(&dir.path().join("none"), &codec("mpeg4")));
    }

    #[test]
    fn a_probe_with_the_real_ffmpeg_tells_an_encoder_that_works_from_one_that_does_not() {
        let Some(ffmpeg) = real_ffmpeg() else { return };

        // mpeg4 is ffmpeg's own, and there is no encoder by the other name.
        assert!(works(&ffmpeg, &codec("mpeg4")));
        assert!(!works(&ffmpeg, &codec("no_such_encoder")));
    }

    #[test]
    fn pick_codec_with_the_real_ffmpeg_gives_an_encoder_that_works() {
        let Some(ffmpeg) = real_ffmpeg() else { return };

        let picked =
            pick_codec(&ffmpeg).expect("this ffmpeg has none of the encoders in ENCODER_ORDER");

        eprintln!("encoder: {}", picked.encoder);
        assert!(ENCODER_ORDER.contains(&picked.encoder));
        assert!(works(&ffmpeg, &picked));
    }

    /// A stand-in for ffmpeg that writes down its arguments (one to a line, in `<it>.args`) and what it is sent
    /// (`<it>.in`), and makes the file it is asked to write (the last argument).
    #[cfg(unix)]
    fn recording_ffmpeg(dir: &Path) -> PathBuf {
        script(
            dir,
            "fake-ffmpeg",
            r#"printf '%s\n' "$@" > "$0.args"; cat > "$0.in"; for a; do last=$a; done; echo made > "${last#file:}""#,
            "",
        )
    }

    /// The arguments that the stand-in got, without the name of the file it was to write.
    #[cfg(unix)]
    fn arguments_of(fake: &Path) -> (Vec<String>, PathBuf) {
        let args = fs::read_to_string(format!("{}.args", fake.display())).unwrap();
        let mut args: Vec<String> = args.lines().map(str::to_owned).collect();
        let file = args.pop().unwrap();
        (args, PathBuf::from(file.strip_prefix("file:").unwrap()))
    }

    #[cfg(unix)]
    #[test]
    fn the_encoder_gives_ffmpeg_the_documented_arguments_and_the_frames_as_they_are() {
        let spec = QualitySpec {
            fps: 15,
            max_long_edge: 1920,
            bitrate: 2_500_000,
        };
        let cases = [
            (
                VideoCodec {
                    encoder: "libx264",
                    container: Container::Mp4,
                },
                "video.mp4",
                "-flush_packets 1 -movflags +frag_keyframe+empty_moov+default_base_moof -f mp4",
            ),
            (
                VideoCodec {
                    encoder: "h264_videotoolbox",
                    container: Container::Mp4,
                },
                "video.mp4",
                "-realtime 1 -allow_sw 1 -flush_packets 1 -movflags +frag_keyframe+empty_moov+default_base_moof -f mp4",
            ),
            (
                VideoCodec {
                    encoder: "libvpx-vp9",
                    container: Container::WebM,
                },
                "video.webm",
                "-deadline realtime -cpu-used 8 -row-mt 1 -flush_packets 1 -f webm",
            ),
        ];
        for (codec, file, tail) in cases {
            let dir = tempfile::tempdir().unwrap();
            let fake = recording_ffmpeg(dir.path());
            let mut encoder =
                VideoEncoder::start(dir.path(), (4, 2), &spec, &codec, &fake).unwrap();

            encoder.push(&[1; 32]).unwrap();
            encoder.push(&[2; 32]).unwrap();
            assert_eq!(encoder.frames(), 2);
            let video = encoder.finish().unwrap();

            let (args, part) = arguments_of(&fake);
            assert_eq!(
                args.join(" "),
                format!(
                    "-hide_banner -nostdin -y -v error -f rawvideo -pix_fmt bgra -video_size 4x2 -framerate 15 -i pipe:0 -vf format=yuv420p -c:v {} -b:v 2500000 -g 30 {tail}",
                    codec.encoder
                ),
                "{}",
                codec.encoder
            );
            assert_eq!(part.parent(), Some(dir.path()));
            let name = part.file_name().unwrap().to_string_lossy();
            assert!(
                name.starts_with(&format!("{file}.")) && name.ends_with(".part"),
                "{name}"
            );
            assert_eq!(video, dir.path().join(file));
            assert_eq!(fs::read_to_string(&video).unwrap(), "made\n");
            assert!(parts(dir.path()).is_empty(), "the .part is renamed");
            let sent = fs::read(format!("{}.in", fake.display())).unwrap();
            assert_eq!(sent, [vec![1u8; 32], vec![2u8; 32]].concat());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_frame_that_is_not_the_size_of_the_video_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let fake = recording_ffmpeg(dir.path());
        let codec = VideoCodec {
            encoder: "mpeg4",
            container: Container::Mp4,
        };
        let mut encoder =
            VideoEncoder::start(dir.path(), (4, 2), &Quality::Small.spec(), &codec, &fake).unwrap();

        let error = encoder.push(&[0; 31]).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(encoder.frames(), 0);
        // The encoder goes on with the frames that are the right size.
        encoder.push(&[0; 32]).unwrap();
        assert_eq!(encoder.frames(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn an_ffmpeg_that_fails_makes_finish_say_what_it_said_and_the_part_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let fake = script(
            dir.path(),
            "fake-ffmpeg",
            r#"echo 'Unknown encoder h264_nvenc' >&2; for a; do last=$a; done; echo half > "${last#file:}"; exit 3"#,
            "",
        );
        let codec = VideoCodec {
            encoder: "h264_nvenc",
            container: Container::Mp4,
        };
        let mut encoder =
            VideoEncoder::start(dir.path(), (4, 2), &Quality::Small.spec(), &codec, &fake).unwrap();

        // ffmpeg has ended or is about to: a push may fail, and when it does the encoder says why from then on.
        let _ = encoder.push(&[0; 32]);
        let error = encoder.finish().unwrap_err();

        let EncodeError::Failed(message) = error else {
            panic!("{error:?}")
        };
        assert!(message.contains("Unknown encoder h264_nvenc"), "{message}");
        assert!(message.contains("exit code 3"), "{message}");
        assert!(!dir.path().join("video.mp4").exists());
        assert_eq!(parts(dir.path()).len(), 1, "what ffmpeg wrote is kept");
    }

    #[cfg(unix)]
    #[test]
    fn a_push_to_an_ffmpeg_that_has_gone_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let fake = script(
            dir.path(),
            "fake-ffmpeg",
            "echo 'Error initializing the encoder' >&2; exit 1",
            "",
        );
        let codec = VideoCodec {
            encoder: "mpeg4",
            container: Container::Mp4,
        };
        let mut encoder =
            VideoEncoder::start(dir.path(), (4, 2), &Quality::Small.spec(), &codec, &fake).unwrap();

        // The pipe is closed once the script has ended: it takes a moment, and then every push fails the same way.
        let mut error = None;
        for _ in 0..200 {
            if let Err(e) = encoder.push(&[0; 32]) {
                error = Some(e);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let message = error
            .expect("a push to a closed pipe must fail")
            .to_string();
        assert!(
            message.contains("Error initializing the encoder"),
            "{message}"
        );
        assert_eq!(encoder.push(&[0; 32]).unwrap_err().to_string(), message);
    }

    #[cfg(unix)]
    #[test]
    fn finish_does_not_wait_for_ever_for_an_ffmpeg_that_does_not_end() {
        let dir = tempfile::tempdir().unwrap();
        let fake = script(dir.path(), "fake-ffmpeg", "cat > /dev/null; sleep 60", "");
        let codec = VideoCodec {
            encoder: "mpeg4",
            container: Container::Mp4,
        };
        let encoder =
            VideoEncoder::start(dir.path(), (4, 2), &Quality::Small.spec(), &codec, &fake).unwrap();
        let started = Instant::now();

        let error = encoder
            .finish_within(Duration::from_millis(300))
            .unwrap_err();

        let EncodeError::Failed(message) = error else {
            panic!("{error:?}")
        };
        assert!(message.contains("did not finish"), "{message}");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn an_ffmpeg_that_cannot_start_is_a_failure_that_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let codec = VideoCodec {
            encoder: "mpeg4",
            container: Container::Mp4,
        };

        let error = VideoEncoder::start(
            dir.path(),
            (4, 2),
            &Quality::Small.spec(),
            &codec,
            Path::new("/no/such/ffmpeg"),
        )
        .err();

        let Some(EncodeError::Failed(message)) = error else {
            panic!("{error:?}")
        };
        assert!(message.contains("could not start ffmpeg"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn mux_gives_ffmpeg_the_documented_arguments() {
        let video = Path::new("/folder/video.mp4");
        // What goes in is copied when the container can have it, and made into AAC (for MP4) or Opus (for WebM,
        // which cannot carry AAC) when not.
        let cases = [
            (
                Container::Mp4,
                "audio.m4a",
                "-c copy -movflags +faststart -f mp4",
                "recording.mp4",
            ),
            (
                Container::Mp4,
                "recording.wav",
                "-c:v copy -c:a aac -b:a 96k -movflags +faststart -f mp4",
                "recording.mp4",
            ),
            (
                Container::WebM,
                "audio.m4a",
                "-c:v copy -c:a libopus -b:a 64k -f webm",
                "recording.webm",
            ),
            (
                Container::WebM,
                "recording.wav",
                "-c:v copy -c:a libopus -b:a 64k -f webm",
                "recording.webm",
            ),
        ];
        for (container, audio, tail, file) in cases {
            let dir = tempfile::tempdir().unwrap();
            let fake = recording_ffmpeg(dir.path());
            let audio_path = Path::new("/folder").join(audio);

            let recording = mux(video, &audio_path, dir.path(), container, &fake).unwrap();

            let (args, part) = arguments_of(&fake);
            assert_eq!(
                args.join(" "),
                format!(
                    "-hide_banner -nostdin -y -v error -i file:/folder/video.mp4 -i file:/folder/{audio} -map 0:v -map 1:a {tail}"
                ),
                "{container:?} {audio}"
            );
            let name = part.file_name().unwrap().to_string_lossy();
            assert!(
                name.starts_with(&format!("{file}.")) && name.ends_with(".part"),
                "{name}"
            );
            assert_eq!(recording, dir.path().join(file));
            assert!(recording.exists() && !part.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_mux_that_fails_says_what_ffmpeg_said_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let fake = script(
            dir.path(),
            "fake-ffmpeg",
            r#"echo 'Invalid data found' >&2; for a; do last=$a; done; echo half > "${last#file:}"; exit 1"#,
            "",
        );

        let error = mux(
            Path::new("video.mp4"),
            Path::new("audio.m4a"),
            dir.path(),
            Container::Mp4,
            &fake,
        )
        .unwrap_err();

        let EncodeError::Failed(message) = error else {
            panic!("{error:?}")
        };
        assert!(message.contains("Invalid data found"), "{message}");
        assert!(message.contains("exit code 1"), "{message}");
        let files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("recording."))
            .collect();
        assert!(files.is_empty(), "{files:?}");
    }

    // A video of frames that are all different, so that an encoder has something to spend its bits on.
    fn moving_frame(width: u32, height: u32, n: u32) -> Vec<u8> {
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                bgra.extend([(x + n * 7) as u8, (y * 2 + n * 3) as u8, (n * 5) as u8, 255]);
            }
        }
        bgra
    }

    const SIZE: (u32, u32) = (320, 180);

    /// The codec of this machine's ffmpeg.
    fn codec_of(ffmpeg: &Path) -> VideoCodec {
        let codec = pick_codec(ffmpeg)
            .expect("this ffmpeg has none of the encoders in ENCODER_ORDER that work");
        // What a test ran with, for the output of one that fails (or `--nocapture`): CI's ffmpeg is whatever the
        // system's package manager has.
        eprintln!("encoder: {}", codec.encoder);
        codec
    }

    /// `video.<ext>.<nonce>.part` files in `folder`.
    fn parts(folder: &Path) -> Vec<PathBuf> {
        fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                let name = path.file_name().unwrap().to_string_lossy();
                name.starts_with("video.") && name.ends_with(".part")
            })
            .collect()
    }

    fn encode(folder: &Path, codec: &VideoCodec, ffmpeg: &Path, seconds: u32) -> PathBuf {
        let spec = Quality::Standard.spec();
        let mut encoder = VideoEncoder::start(folder, SIZE, &spec, codec, ffmpeg).unwrap();
        for n in 0..spec.fps * seconds {
            encoder.push(&moving_frame(SIZE.0, SIZE.1, n)).unwrap();
        }
        assert_eq!(encoder.frames(), u64::from(spec.fps * seconds));
        encoder.finish().unwrap()
    }

    #[test]
    fn ten_seconds_of_frames_make_a_ten_second_video() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let codec = codec_of(&ffmpeg);

        let video = encode(dir.path(), &codec, &ffmpeg, 10);

        assert_eq!(
            video,
            dir.path()
                .join(format!("video.{}", codec.container.extension()))
        );
        assert!(parts(dir.path()).is_empty(), "the .part file is renamed");
        let seconds = video_seconds(&video, &ffmpeg);
        assert!(
            (seconds - 10.0).abs() <= 0.2,
            "{seconds} s with {}",
            codec.encoder
        );
    }

    #[test]
    fn the_software_encoder_that_every_ffmpeg_has_makes_a_playable_video_too() {
        // `mpeg4` is the end of the list: what `pick_codec` falls back to on a machine with nothing better.
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let codec = VideoCodec {
            encoder: "mpeg4",
            container: Container::Mp4,
        };

        let video = encode(dir.path(), &codec, &ffmpeg, 3);

        let seconds = video_seconds(&video, &ffmpeg);
        assert!((seconds - 3.0).abs() <= 0.2, "{seconds} s");
    }

    /// Pushes 6 s of frames of a screen that hardly changes (a colour that is a shade different each time, which an
    /// encoder spends a few hundred bytes a frame on), waits until ffmpeg has written what it will (it has all it was
    /// given and is waiting for more), and drops the encoder: the program that stopped, as far as the file can tell. A
    /// fragment ends where the next keyframe is (every 2 s), so the last 2 s of what was pushed are not in the file,
    /// and the 4 s before are.
    fn cut_off(folder: &Path, codec: &VideoCodec, ffmpeg: &Path) -> PathBuf {
        let spec = Quality::Standard.spec();
        let mut encoder = VideoEncoder::start(folder, SIZE, &spec, codec, ffmpeg).unwrap();
        for n in 0..90u8 {
            let shade = 100 + n;
            let frame = [shade, 255 - shade, shade / 2, 255].repeat((SIZE.0 * SIZE.1) as usize);
            encoder.push(&frame).unwrap();
        }
        let mut last = 0;
        let mut steady = 0;
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(50));
            let size = parts(folder)
                .first()
                .and_then(|part| fs::metadata(part).ok())
                .map_or(0, |meta| meta.len());
            steady = if size > 0 && size == last {
                steady + 1
            } else {
                0
            };
            last = size;
            if steady == 10 {
                break;
            }
        }
        drop(encoder);
        let parts = parts(folder);
        assert_eq!(parts.len(), 1, "a dropped encoder leaves its .part");
        parts.into_iter().next().unwrap()
    }

    #[test]
    fn a_video_cut_off_is_still_playable() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let codec = codec_of(&ffmpeg);

        let part = cut_off(dir.path(), &codec, &ffmpeg);

        // What recovery does with it.
        let recovered = dir
            .path()
            .join(format!("video.{}", codec.container.extension()));
        publish(&part, &recovered).unwrap();
        let seconds = video_seconds(&recovered, &ffmpeg);
        assert!(
            seconds >= 2.0,
            "{seconds} s readable with {}",
            codec.encoder
        );
    }

    /// An `audio.m4a` of a tone, `seconds` long.
    fn tone_m4a(folder: &Path, seconds: usize, ffmpeg: &Path) -> PathBuf {
        let out = folder.join("audio.m4a");
        let mut sink = m4a_writer(&out, Some(ffmpeg)).unwrap();
        let tone: Vec<f32> = (0..48_000 * seconds)
            .map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin())
            .collect();
        sink.write(&tone).unwrap();
        sink.finish().unwrap();
        out
    }

    fn audio_seconds(path: &Path, ffmpeg: &Path) -> f64 {
        let mut decoder = MonoDecoder::open(path, Some(ffmpeg)).unwrap();
        read_all(&mut decoder).len() as f64 / f64::from(decoder.rate())
    }

    fn mux_puts_the_audio_under(codec: &VideoCodec, ffmpeg: &Path) {
        let dir = tempfile::tempdir().unwrap();
        let video = encode(dir.path(), codec, ffmpeg, 3);
        let audio = tone_m4a(dir.path(), 3, ffmpeg);

        let recording = mux(&video, &audio, dir.path(), codec.container, ffmpeg).unwrap();

        assert_eq!(
            recording,
            dir.path()
                .join(format!("recording.{}", codec.container.extension()))
        );
        // The default audio track is the one that was added, and it is as long as the video.
        let seconds = audio_seconds(&recording, ffmpeg);
        assert!((seconds - 3.0).abs() <= 0.1, "{seconds} s of audio");
        let seconds = video_seconds(&recording, ffmpeg);
        assert!((seconds - 3.0).abs() <= 0.2, "{seconds} s of video");
        assert!(video.exists(), "the video is left for the caller to remove");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn mux_puts_the_audio_under_the_video() {
        let Some(ffmpeg) = real_ffmpeg() else { return };

        mux_puts_the_audio_under(&codec_of(&ffmpeg), &ffmpeg);
    }

    #[test]
    fn mux_puts_the_audio_under_a_webm_video_when_there_is_a_vp9_encoder() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let listing = tools::command(&ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .output()
            .unwrap();
        if !String::from_utf8_lossy(&listing.stdout).contains("libvpx-vp9") {
            eprintln!("skipped: this ffmpeg has no libvpx-vp9");
            return;
        }
        // On macOS and Windows the encoder order has no VP9, and the mux is the same whatever made the video.
        let codec = VideoCodec {
            encoder: "libvpx-vp9",
            container: Container::WebM,
        };

        mux_puts_the_audio_under(&codec, &ffmpeg);
    }

    #[test]
    fn mux_takes_a_wav_as_the_audio_too() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let codec = codec_of(&ffmpeg);
        let video = encode(dir.path(), &codec, &ffmpeg, 3);
        let wav = dir.path().join(crate::record::mix::WAV);
        let mut writer = hound::WavWriter::create(&wav, crate::record::stem::spec()).unwrap();
        for i in 0..48_000 * 3 {
            let sample =
                (f32::sin(std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0) * 16_000.0) as i16;
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();

        let recording = mux(&video, &wav, dir.path(), codec.container, &ffmpeg).unwrap();

        let seconds = audio_seconds(&recording, &ffmpeg);
        assert!((seconds - 3.0).abs() <= 0.1, "{seconds} s of audio");
    }

    #[cfg(unix)]
    #[test]
    fn an_abort_frees_a_push_that_is_blocked_on_an_ffmpeg_that_does_not_read() {
        let dir = tempfile::tempdir().unwrap();
        // It never reads what it is sent: the pipe fills up, and a push waits for ever.
        let fake = script(dir.path(), "fake-ffmpeg", "sleep 60", "");
        let codec = VideoCodec {
            encoder: "mpeg4",
            container: Container::Mp4,
        };
        let mut encoder =
            VideoEncoder::start(dir.path(), SIZE, &Quality::Small.spec(), &codec, &fake).unwrap();
        let abort = encoder.abort_handle();
        let (done, ended) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let frame = vec![0u8; (SIZE.0 * SIZE.1 * 4) as usize];
            let mut result = Ok(());
            for _ in 0..1000 {
                result = encoder.push(&frame);
                if result.is_err() {
                    break;
                }
            }
            let _ = done.send(result);
        });
        assert!(
            ended.recv_timeout(Duration::from_millis(500)).is_err(),
            "the push should be stuck"
        );

        abort.abort();

        let result = ended
            .recv_timeout(Duration::from_secs(20))
            .expect("the push was not freed");
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn an_abort_after_the_encoder_has_finished_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let fake = recording_ffmpeg(dir.path());
        let codec = VideoCodec {
            encoder: "mpeg4",
            container: Container::Mp4,
        };
        let mut encoder =
            VideoEncoder::start(dir.path(), (4, 2), &Quality::Small.spec(), &codec, &fake).unwrap();
        let abort = encoder.abort_handle();
        encoder.push(&[0; 32]).unwrap();
        let video = encoder.finish().unwrap();

        abort.abort();

        assert_eq!(fs::read_to_string(video).unwrap(), "made\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_mux_that_does_not_end_is_killed_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let fake = script(
            dir.path(),
            "fake-ffmpeg",
            r#"for a; do last=$a; done; echo half > "${last#file:}"; sleep 60"#,
            "",
        );
        let started = Instant::now();

        let error = mux_within(
            Path::new("video.mp4"),
            Path::new("audio.m4a"),
            dir.path(),
            Container::Mp4,
            &fake,
            Duration::from_millis(300),
        )
        .unwrap_err();

        let EncodeError::Failed(message) = error else {
            panic!("{error:?}")
        };
        assert!(message.contains("did not finish"), "{message}");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
        let files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("recording."))
            .collect();
        assert!(files.is_empty(), "{files:?}");
    }

    #[test]
    fn the_length_of_a_video_is_read_without_decoding_it() {
        let Some(ffmpeg) = real_ffmpeg() else { return };
        let dir = tempfile::tempdir().unwrap();
        let codec = codec_of(&ffmpeg);
        let video = encode(dir.path(), &codec, &ffmpeg, 3);

        let seconds = probe_seconds(&video, &ffmpeg).expect("a length");

        assert!((seconds - 3.0).abs() <= 0.2, "{seconds} s");
        assert_eq!(
            probe_seconds(&dir.path().join("missing.mp4"), &ffmpeg),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_probe_that_does_not_end_gives_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let fake = script(dir.path(), "fake-ffmpeg", "sleep 60", "");
        let started = Instant::now();

        let seconds = probe_within(Path::new("video.mp4"), &fake, Duration::from_millis(300));

        assert_eq!(seconds, None);
        assert!(started.elapsed() < Duration::from_secs(20));
    }
}
