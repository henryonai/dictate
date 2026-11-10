//! The recording session: starts the sources of a recording, keeps what they deliver in the stems and the video, and
//! ends with the one file that goes to the meetings pipeline.

use serde::Serialize;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dictate_core::meetings::library::files;

use super::marker::{Marker, MARKER};
use super::stem::pad_to;
use super::video::{pick_codec, FrameSlot, VideoCodec};
use super::{
    Capturer, EndReason, Mode, RecordOptions, RunningSource, SourceEvent, StartError, Track,
};
use crate::media::encode::remove_part;

mod finalize;
mod gate;
mod pump;
#[cfg(test)]
mod tests;
mod ticker;

use finalize::{finalize_with, Hints, STEMS};
use gate::{lock, SharedGate};
use pump::{Pump, PumpSetup, SharedMeters, StemEnd};
use ticker::{Ticker, VideoSetup};

pub use finalize::{finalize, recover};

/// How loud each source is, from 0 to 1 (`audio::level::level`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Levels {
    /// `None` when the track is off.
    pub mic: Option<f32>,
    /// `None` when the track is off.
    pub system: Option<f32>,
}

/// What a session tells its owner, as it goes.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionState {
    Recording {
        active: Duration,
        levels: Levels,
    },
    Paused {
        active: Duration,
        levels: Levels,
    },
    /// A source ended by itself: the sources are stopped, and what was recorded is waiting for `Session::stop`.
    Ended(EndReason),
}

/// A recording that is over.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    /// The file that goes to the pipeline: `recording.mp4` (`.webm`), `audio.m4a` (or `recording.wav`).
    pub media: PathBuf,
    /// Seconds.
    pub duration: f64,
    /// What went wrong without making the recording fail, in English: for the log of the recording (`log.txt`).
    pub notes: Vec<String>,
}

/// How often the state is reported, at the most.
const REPORT_EVERY: Duration = Duration::from_millis(50);

/// How long the parts of a session wait on each other, as the tests want them.
#[derive(Clone, Copy)]
struct Timing {
    /// How long `stop` waits for the video to be finished before ffmpeg is killed.
    video_finish_wait: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            // ffmpeg has 30 s to write the end of the file (`VideoEncoder::finish`): this is for the one that is stuck
            // in taking a frame.
            video_finish_wait: Duration::from_secs(40),
        }
    }
}

/// What the owner of a session asks of the thread that runs it.
enum Control {
    /// The clock was paused or resumed: report it now.
    Changed,
    /// A source ended by itself.
    Ended(EndReason),
    /// Finish the recording.
    Stop,
    /// Stop it and remove what it made.
    Discard,
    /// Stop it, and leave what it made for `recover`.
    Abandon,
}

/// What the thread of a session ends with: the recording (`Stop`), nothing (`Discard`, `Abandon`).
type Outcome = io::Result<Option<Finished>>;

/// A recording that is going on.
///
/// It runs on threads of its own: the one that starts and stops the sources and reports the state, one that takes
/// what the sources deliver (the stems, the meters), and one that gives ffmpeg the frames. A `Session` that is
/// dropped without `stop` or `discard` stops the sources and leaves the files as they are, with `recording.json`, for
/// `recover` (as a crash would, but with the stems closed).
pub struct Session {
    control: Sender<Control>,
    gate: SharedGate,
    ended: Arc<Mutex<Option<EndReason>>>,
    thread: Option<JoinHandle<Outcome>>,
}

impl Session {
    /// Starts the recording in `folder` (which exists, and is the recording's own): writes `recording.json`, and starts
    /// the sources, the screen first, then the system's audio, then the microphone. It returns when every one of
    /// them has started, or at the first that cannot: the others are stopped, what was made in the folder is removed
    /// (the caller removes the folder) and the error is returned.
    ///
    /// This waits for ffmpeg to be asked which of its encoders work (a few seconds at the most), and for the sources
    /// to start (which can be as long as a person takes to answer a question): call it off the thread of the
    /// interface. `on_state` is called on the session's own thread, at most every 50 ms; it must not call back
    /// into the session (`stop` waits for that thread).
    pub fn start(
        options: RecordOptions,
        folder: &Path,
        capturer: Arc<dyn Capturer>,
        ffmpeg: Option<PathBuf>,
        on_state: Box<dyn Fn(SessionState) + Send>,
    ) -> Result<Session, StartError> {
        Session::start_with(
            options,
            folder,
            capturer,
            ffmpeg,
            on_state,
            Timing::default(),
        )
    }

    fn start_with(
        options: RecordOptions,
        folder: &Path,
        capturer: Arc<dyn Capturer>,
        ffmpeg: Option<PathBuf>,
        on_state: Box<dyn Fn(SessionState) + Send>,
        timing: Timing,
    ) -> Result<Session, StartError> {
        if options.mode == Mode::Audio && !options.mic && !options.system_audio {
            return Err(StartError::NothingToRecord);
        }
        let codec = if options.mode.has_video() {
            let ffmpeg = ffmpeg.as_deref().ok_or(StartError::NeedsFfmpeg)?;
            Some(pick_codec(ffmpeg).ok_or(StartError::NoVideoEncoder)?)
        } else {
            None
        };
        Marker {
            version: 1,
            options: options.clone(),
            codec: codec.as_ref().map(|codec| codec.encoder.to_owned()),
            container: codec
                .as_ref()
                .map(|codec| codec.container.extension().to_owned()),
            started: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0.0, |since| since.as_secs_f64()),
        }
        .write(folder)
        .map_err(|e| StartError::Io(e.to_string()))?;

        let (control, control_rx) = mpsc::channel();
        let gate = gate::shared();
        let ended = Arc::new(Mutex::new(None));
        let worker = Worker {
            options,
            folder: folder.to_path_buf(),
            capturer,
            ffmpeg,
            codec,
            on_state,
            timing,
            gate: gate.clone(),
            ended: ended.clone(),
            control_tx: control.clone(),
            control_rx,
            meters: SharedMeters::default(),
            sources: Vec::new(),
            pump: None,
            ticker: None,
        };
        let (ready_tx, ready) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("recording".into())
            .spawn(move || worker.run(&ready_tx))
            .map_err(|e| {
                remove_recording_files(folder);
                StartError::Io(e.to_string())
            })?;
        match ready.recv() {
            Ok(Ok(())) => Ok(Session {
                control,
                gate,
                ended,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                // The thread has undone what it did; it is over when it has said so.
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                remove_recording_files(folder);
                Err(StartError::Io(
                    "the recording stopped before it began".to_owned(),
                ))
            }
        }
    }

    /// Stops the clock: what the sources deliver from now on is not recorded, and the video is given no frame, until
    /// `resume`. Nothing happens when it is stopped already, or the recording is over.
    pub fn pause(&self) {
        lock(&self.gate).pause(Instant::now());
        let _ = self.control.send(Control::Changed);
    }

    /// Starts the clock again. Nothing happens when it is running, or the recording is over.
    pub fn resume(&self) {
        lock(&self.gate).resume(Instant::now());
        let _ = self.control.send(Control::Changed);
    }

    /// Ends the recording: the sources are stopped, the video is finished and the sound is mixed and put under it
    /// (`finalize`). It waits for all that, which is as long as the mix of a long recording takes.
    ///
    /// When it cannot make the recording (`Err`) the files are where they were, and `recover` can try again, except
    /// when nothing was recorded at all (`InvalidData`).
    pub fn stop(mut self) -> io::Result<Finished> {
        let Some(thread) = self.thread.take() else {
            return Err(io::Error::other("the recording is over already"));
        };
        // The recording ends now. The clock stops here, on the caller's thread as `pause` does, and not where the
        // session's thread gets to the message: that thread may be busy (it reports the state, and the report may be
        // slow) or not scheduled, and what it takes is then recorded, by the stems and the video that go on until the
        // clock stops.
        lock(&self.gate).close(Instant::now());
        let _ = self.control.send(Control::Stop);
        match thread.join() {
            Ok(Ok(Some(finished))) => Ok(finished),
            Ok(Ok(None)) => Err(io::Error::other("the recording was not finished")),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::other("the recording stopped unexpectedly")),
        }
    }

    /// Throws the recording away: the sources are stopped and ffmpeg is killed, and the stems, the video and
    /// `recording.json` are removed (the folder is left, for the caller to remove). It waits for that.
    pub fn discard(mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.control.send(Control::Discard);
            let _ = thread.join();
        }
    }

    /// Why the recording ended by itself, when it did: a source ended (the window was closed, the display unplugged, the
    /// device lost, the disk full, ffmpeg gone). The sources are stopped by then; `stop` makes the recording of what
    /// was recorded. Does not wait.
    pub fn wait_end(&self) -> Option<EndReason> {
        lock(&self.ended).clone()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            // As `stop` does: the recording ends now, and not when the session's thread hears of it.
            lock(&self.gate).close(Instant::now());
            let _ = self.control.send(Control::Abandon);
            let _ = thread.join();
        }
    }
}

/// The session's own thread.
struct Worker {
    options: RecordOptions,
    folder: PathBuf,
    capturer: Arc<dyn Capturer>,
    ffmpeg: Option<PathBuf>,
    codec: Option<VideoCodec>,
    on_state: Box<dyn Fn(SessionState) + Send>,
    timing: Timing,
    gate: SharedGate,
    ended: Arc<Mutex<Option<EndReason>>>,
    control_tx: Sender<Control>,
    control_rx: Receiver<Control>,
    meters: SharedMeters,
    sources: Vec<Box<dyn RunningSource>>,
    pump: Option<Pump>,
    ticker: Option<Ticker>,
}

impl Worker {
    fn run(mut self, ready: &Sender<Result<(), StartError>>) -> Outcome {
        match self.begin() {
            Ok(()) => {
                let _ = ready.send(Ok(()));
            }
            Err(error) => {
                let _ = ready.send(Err(error));
                return Ok(None);
            }
        }
        self.keep_going()
    }

    /// The tracks of this recording, in the order they start.
    fn tracks(&self) -> Vec<Track> {
        let mut tracks = Vec::new();
        if self.options.mode.has_video() {
            tracks.push(Track::Screen);
        }
        if self.options.system_audio {
            tracks.push(Track::System);
        }
        if self.options.mic {
            tracks.push(Track::Mic);
        }
        tracks
    }

    /// Takes in what the sources deliver, and starts them. The recording's clock starts when the last has: what a
    /// source that started earlier delivered before that is not on it.
    fn begin(&mut self) -> Result<(), StartError> {
        let (tx, events) = mpsc::channel();
        let frames = FrameSlot::new();
        let control = self.control_tx.clone();
        let pump = Pump::start(PumpSetup {
            events,
            gate: self.gate.clone(),
            frames: frames.clone(),
            meters: self.meters.clone(),
            folder: self.folder.clone(),
            ended: Box::new(move |_track, reason| {
                let _ = control.send(Control::Ended(reason));
            }),
        })
        .map_err(|e| {
            remove_recording_files(&self.folder);
            StartError::Io(e.to_string())
        })?;
        self.pump = Some(pump);

        for track in self.tracks() {
            match self.capturer.start(track, &self.options, tx.clone()) {
                Ok(source) => self.sources.push(source),
                Err(error) => {
                    self.undo();
                    return Err(error);
                }
            }
        }
        let failed = {
            let tx = tx.clone();
            Box::new(move |why: String| {
                let _ = tx.send(SourceEvent::Ended {
                    track: Track::Screen,
                    reason: EndReason::Failed(why),
                });
            })
        };
        drop(tx);
        lock(&self.gate).begin(Instant::now());

        if let (Some(codec), Some(ffmpeg)) = (&self.codec, &self.ffmpeg) {
            let started = Ticker::start(VideoSetup {
                folder: self.folder.clone(),
                ffmpeg: ffmpeg.clone(),
                codec: codec.clone(),
                spec: self.options.quality.spec(),
                gate: self.gate.clone(),
                frames,
                failed,
            });
            match started {
                Ok(ticker) => self.ticker = Some(ticker),
                Err(e) => {
                    self.freeze();
                    self.undo();
                    return Err(StartError::Io(e.to_string()));
                }
            }
        }
        Ok(())
    }

    /// Takes back what a start that failed did: the sources that had started are stopped, and the files are removed.
    fn undo(&mut self) {
        self.stop_sources();
        if let Some(pump) = self.pump.take() {
            pump.finish();
        }
        remove_recording_files(&self.folder);
    }

    /// Reports the state now and then, and does what the owner asks, until it asks for the end.
    fn keep_going(mut self) -> Outcome {
        let mut reported = Instant::now();
        self.report();
        loop {
            let wait = REPORT_EVERY.saturating_sub(reported.elapsed());
            match self.control_rx.recv_timeout(wait) {
                Ok(Control::Changed) => {
                    self.report();
                    reported = Instant::now();
                }
                Ok(Control::Ended(reason)) => self.end_by_itself(reason),
                Ok(Control::Stop) => return self.stop(),
                Ok(Control::Discard) => return self.discard(),
                Ok(Control::Abandon) | Err(RecvTimeoutError::Disconnected) => {
                    return self.abandon()
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if reported.elapsed() >= REPORT_EVERY {
                self.report();
                reported = Instant::now();
            }
        }
    }

    /// Tells the owner how the recording is, unless it is over.
    fn report(&self) {
        if lock(&self.ended).is_some() {
            return;
        }
        let (active, paused) = {
            let gate = lock(&self.gate);
            (gate.active_now(), gate.is_paused())
        };
        let levels =
            lock(&self.meters).read(self.options.mic, self.options.system_audio, Instant::now());
        (self.on_state)(if paused {
            SessionState::Paused { active, levels }
        } else {
            SessionState::Recording { active, levels }
        });
    }

    /// A source ended by itself (the first to: the others are stopped with it, and whatever they have to say is of no
    /// interest). The clock stops where it is, and the owner is told; what was recorded waits for `stop`.
    fn end_by_itself(&mut self, reason: EndReason) {
        {
            let mut ended = lock(&self.ended);
            if ended.is_some() {
                return;
            }
            *ended = Some(reason.clone());
        }
        self.freeze();
        self.stop_sources();
        (self.on_state)(SessionState::Ended(reason));
    }

    /// Stops the clock for good, now. Returns how much of the recording there is.
    fn freeze(&self) -> Duration {
        let mut gate = lock(&self.gate);
        gate.close(Instant::now());
        gate.active_now()
    }

    /// Stops every source that is running (the last to start first).
    fn stop_sources(&mut self) {
        while let Some(source) = self.sources.pop() {
            source.stop();
        }
    }

    /// Ends the recording and makes the file.
    fn stop(mut self) -> Outcome {
        let active = self.freeze();
        self.stop_sources();
        let video = self
            .ticker
            .take()
            .map(|ticker| ticker.finish(self.timing.video_finish_wait));
        let stems = self.pump.take().map(Pump::finish).unwrap_or_default();

        let mut notes = Vec::new();
        let mut hints = Hints::default();
        if let Some(end) = video {
            if let Some(problem) = &end.problem {
                notes.push(format!("The video stopped: {problem}."));
            } else if end.frames == 0 {
                notes.push("No video frames arrived, so there is no video.".to_owned());
            }
            if end.path.is_some() {
                let fps = f64::from(self.options.quality.spec().fps.max(1));
                hints.video_seconds = Some(end.frames as f64 / fps);
            }
        }
        pad_stems(stems, active, &mut notes);
        let mut finished = finalize_with(&self.folder, self.ffmpeg.as_deref(), &hints)?;
        notes.append(&mut finished.notes);
        finished.notes = notes;
        Ok(Some(finished))
    }

    /// Throws the recording away.
    fn discard(mut self) -> Outcome {
        self.freeze();
        self.stop_sources();
        if let Some(ticker) = self.ticker.take() {
            ticker.abort();
        }
        if let Some(pump) = self.pump.take() {
            pump.finish();
        }
        remove_recording_files(&self.folder);
        Ok(None)
    }

    /// Stops everything and leaves the files for `recover`: the stems as long as the recording ran, and the video as it
    /// is.
    fn abandon(mut self) -> Outcome {
        let active = self.freeze();
        self.stop_sources();
        if let Some(ticker) = self.ticker.take() {
            ticker.abort();
        }
        if let Some(pump) = self.pump.take() {
            pad_stems(pump.finish(), active, &mut Vec::new());
        }
        Ok(None)
    }
}

/// Makes every stem as long as the recording (`active`): a source that went quiet at the end (a system that plays no
/// sound sends no buffers) is shorter than the clock says. What goes wrong is told in `notes`.
fn pad_stems(stems: Vec<StemEnd>, active: Duration, notes: &mut Vec<String>) {
    let seconds = active.as_secs_f64();
    for stem in stems {
        let long_enough = match &stem.result {
            Ok(length) => *length >= seconds,
            Err(e) => {
                notes.push(format!(
                    "{} could not be finished: {e}.",
                    file_name(&stem.path)
                ));
                false
            }
        };
        if !long_enough && stem.path.is_file() {
            if let Err(e) = pad_to(&stem.path, seconds) {
                notes.push(format!(
                    "{} could not be made as long as the recording: {e}.",
                    file_name(&stem.path)
                ));
            }
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// Whether a file of the recording's folder is one that a recording makes (and none of the library's: `meta.json`, the
/// log, the transcript, are not).
fn is_recording_file(name: &str) -> bool {
    name == MARKER
        || STEMS.iter().any(|(_, stem)| name == *stem)
        || name.starts_with("video.")
        || name.starts_with("recording.")
        || name.starts_with(files::AUDIO)
        || name.ends_with(".part")
}

/// Removes what a recording makes in `folder`: the stems, the video and its parts, the sound and the recording, and
/// `recording.json`.
fn remove_recording_files(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        if is_recording_file(&entry.file_name().to_string_lossy()) {
            remove_part(&entry.path());
        }
    }
}
