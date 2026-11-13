//! The video of a recording: a thread that gives ffmpeg the latest frame of the screen at the rate of the video, for as
//! long as the recording's clock runs.
//!
//! The screen gives a frame when something changes; the video needs one every 1/fps of a second, so the ticker takes
//! the latest each time one is due and gives it again when nothing has changed. It goes by the recording's clock, not
//! by how many times it was woken: when ffmpeg takes a frame late (or the machine is busy) the frames that are due
//! meanwhile follow at once, so that the video is as long as the recording was running.
//!
//! The encoder is the thread's, and a `push` waits until ffmpeg takes the frame; when ffmpeg has stopped reading that is
//! for ever. So the thread can be freed from outside, by killing ffmpeg (`Kill`): that is what `Ticker::finish` does
//! when the video does not end in time, and what `Ticker::abort` does at once.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::finalize::detail;
use super::gate::{lock, SharedGate};
use crate::record::quality::{output_size, QualitySpec};
use crate::record::video::{fit_into, AbortHandle, Frame, FrameSlot, VideoCodec, VideoEncoder};

/// How often the thread looks at the clock when there is nothing to do yet (no frame has come, or the clock stands
/// still), and so how soon it takes a command.
const IDLE: Duration = Duration::from_millis(20);
/// How long a thread that has been told to kill ffmpeg is given to end.
const GRACE: Duration = Duration::from_secs(10);

pub(super) struct VideoSetup {
    pub folder: PathBuf,
    pub ffmpeg: PathBuf,
    pub codec: VideoCodec,
    pub spec: QualitySpec,
    pub gate: SharedGate,
    /// Where the screen leaves its latest frame.
    pub frames: FrameSlot,
    /// Called once, with what went wrong, when the video can go no further (ffmpeg could not start, or is gone).
    pub failed: Box<dyn Fn(String) + Send>,
}

/// How the video ended.
pub(super) struct VideoEnd {
    /// Frames that ffmpeg was given.
    pub frames: u64,
    /// `video.mp4` (or `.webm`), when the video was finished. When it was not, what ffmpeg wrote is a `.part` in the folder.
    pub path: Option<PathBuf>,
    /// Why the video is not as it should be, in the words of whatever failed.
    pub problem: Option<String>,
}

enum Command {
    /// Give the frames that are due, and finish the video.
    Finish,
    /// Stop where it is: ffmpeg is killed and the `.part` is left as it is.
    Abort,
}

/// The way to kill ffmpeg from outside the thread that is using it.
#[derive(Default)]
struct Kill {
    asked: AtomicBool,
    handle: Mutex<Option<AbortHandle>>,
}

impl Kill {
    /// Kills ffmpeg, or makes sure it is as soon as it has started.
    fn trigger(&self) {
        self.asked.store(true, Ordering::SeqCst);
        if let Some(handle) = lock(&self.handle).as_ref() {
            handle.abort();
        }
    }

    /// Where ffmpeg can be killed from, once it has started (and now, if it was asked to be killed before).
    fn publish(&self, handle: AbortHandle) {
        *lock(&self.handle) = Some(handle.clone());
        if self.asked.load(Ordering::SeqCst) {
            handle.abort();
        }
    }
}

pub(super) struct Ticker {
    commands: Sender<Command>,
    done: Receiver<VideoEnd>,
    kill: Arc<Kill>,
    /// Not joined: a thread that is stuck must not keep anything waiting.
    _thread: JoinHandle<()>,
}

impl Ticker {
    pub fn start(setup: VideoSetup) -> io::Result<Ticker> {
        let (commands, command_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let kill = Arc::new(Kill::default());
        let worker = Worker {
            fps: f64::from(setup.spec.fps.max(1)),
            folder: setup.folder,
            ffmpeg: setup.ffmpeg,
            codec: setup.codec,
            spec: setup.spec,
            gate: setup.gate,
            frames: setup.frames,
            failed: setup.failed,
            kill: kill.clone(),
            encoder: None,
            size: (0, 0),
            fitted: None,
            problem: None,
        };
        let thread = std::thread::Builder::new()
            .name("recording video".into())
            .spawn(move || worker.run(&command_rx, &done_tx))?;
        Ok(Ticker {
            commands,
            done,
            kill,
            _thread: thread,
        })
    }

    /// Gives the frames that are due, ends the video and waits for ffmpeg to write the end of the file, `wait` at most:
    /// after that ffmpeg is killed, and the video is what it had written (`path` is `None`).
    pub fn finish(self, wait: Duration) -> VideoEnd {
        let _ = self.commands.send(Command::Finish);
        if let Ok(end) = self.done.recv_timeout(wait) {
            return end;
        }
        self.kill.trigger();
        let problem = format!(
            "the video did not end in {} s, so ffmpeg was stopped",
            wait.as_secs_f32()
        );
        match self.done.recv_timeout(GRACE) {
            Ok(end) => VideoEnd {
                path: None,
                problem: Some(problem),
                ..end
            },
            Err(_) => VideoEnd {
                frames: 0,
                path: None,
                problem: Some(problem),
            },
        }
    }

    /// Stops the video where it is, at once: ffmpeg is killed, and what it wrote is left as a `.part`.
    pub fn abort(self) {
        self.kill.trigger();
        let _ = self.commands.send(Command::Abort);
        let _ = self.done.recv_timeout(GRACE);
    }
}

struct Worker {
    folder: PathBuf,
    ffmpeg: PathBuf,
    codec: VideoCodec,
    spec: QualitySpec,
    fps: f64,
    gate: SharedGate,
    frames: FrameSlot,
    failed: Box<dyn Fn(String) + Send>,
    kill: Arc<Kill>,
    /// Started with the first frame, which is what tells its size.
    encoder: Option<VideoEncoder>,
    size: (u32, u32),
    /// The frame that was last fitted into the size of the video, and how it came out: a frame that is given again is
    /// not fitted again.
    fitted: Option<(Arc<Frame>, Vec<u8>)>,
    problem: Option<String>,
}

impl Worker {
    fn run(mut self, commands: &Receiver<Command>, done: &Sender<VideoEnd>) {
        let mut wait = IDLE;
        let end = loop {
            match commands.recv_timeout(wait) {
                Err(RecvTimeoutError::Timeout) => wait = self.tick(),
                Ok(Command::Finish) => {
                    self.tick();
                    break self.finish();
                }
                Ok(Command::Abort) | Err(RecvTimeoutError::Disconnected) => break self.abandon(),
            }
        };
        let _ = done.send(end);
    }

    /// Gives the frames that are due by the recording's clock, and says how long until the next is.
    fn tick(&mut self) -> Duration {
        if self.problem.is_some() {
            return IDLE;
        }
        let active = lock(&self.gate).active_now().as_secs_f64();
        loop {
            // Frame number `n` of the video is due when `n` frames' time has gone by on the clock.
            let given = self.encoder.as_ref().map_or(0, VideoEncoder::frames);
            let due_in = given as f64 / self.fps - active;
            if due_in >= 0.0 {
                return Duration::from_secs_f64(due_in).min(IDLE);
            }
            // Nothing has come from the screen yet: nothing to give.
            let Some(frame) = self.frames.latest() else {
                return IDLE;
            };
            if !self.give(&frame) {
                return IDLE;
            }
        }
    }

    /// Gives ffmpeg `frame`, and starts it first if this is the first. `false` when the video can go no further.
    fn give(&mut self, frame: &Arc<Frame>) -> bool {
        if self.encoder.is_none() {
            self.size = output_size((frame.width, frame.height), &self.spec);
            match VideoEncoder::start(
                &self.folder,
                self.size,
                &self.spec,
                &self.codec,
                &self.ffmpeg,
            ) {
                Ok(encoder) => {
                    self.kill.publish(encoder.abort_handle());
                    self.encoder = Some(encoder);
                }
                Err(e) => {
                    self.fail(detail(&e));
                    return false;
                }
            }
        }
        let size = self.size;
        let exact = (frame.width, frame.height) == size
            && frame.stride == size.0 * 4
            && frame.bgra.len() == size.0 as usize * size.1 as usize * 4;
        let encoder = self.encoder.as_mut().expect("it was started above");
        let result = if exact {
            encoder.push(&frame.bgra)
        } else {
            if !matches!(&self.fitted, Some((fitted, _)) if Arc::ptr_eq(fitted, frame)) {
                self.fitted = Some((frame.clone(), fit_into(frame, size)));
            }
            let (_, pixels) = self.fitted.as_ref().expect("it was set above");
            encoder.push(pixels)
        };
        match result {
            Ok(()) => true,
            Err(e) => {
                self.fail(e.to_string());
                false
            }
        }
    }

    fn fail(&mut self, why: String) {
        if self.problem.is_none() {
            self.problem = Some(why.clone());
            (self.failed)(why);
        }
    }

    /// Ends the video: ffmpeg writes what it holds and the end of the file.
    fn finish(mut self) -> VideoEnd {
        let problem = self.problem.take();
        let Some(encoder) = self.encoder.take() else {
            return VideoEnd {
                frames: 0,
                path: None,
                problem,
            };
        };
        let frames = encoder.frames();
        if frames == 0 {
            // A video of no frame is a file with a header and nothing to play (ffmpeg writes one when it is told it is
            // over), which is of no use to anything.
            drop(encoder);
            return VideoEnd {
                frames: 0,
                path: None,
                problem: problem.or_else(|| Some("no frame reached the encoder".to_owned())),
            };
        }
        match (encoder.finish(), problem) {
            (Ok(path), None) => VideoEnd {
                frames,
                path: Some(path),
                problem: None,
            },
            // What went wrong first is what is said.
            (_, Some(problem)) => VideoEnd {
                frames,
                path: None,
                problem: Some(problem),
            },
            (Err(e), None) => VideoEnd {
                frames,
                path: None,
                problem: Some(detail(&e)),
            },
        }
    }

    /// Drops the encoder: ffmpeg is killed, and the `.part` is as it was.
    fn abandon(mut self) -> VideoEnd {
        let frames = self.encoder.as_ref().map_or(0, VideoEncoder::frames);
        drop(self.encoder.take());
        VideoEnd {
            frames,
            path: None,
            problem: self.problem.take(),
        }
    }
}
