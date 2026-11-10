//! Where what the sources deliver goes: the sound into the stems (on the recording's clock, with silence where a
//! source had nothing to say), the meters, the latest frame into the slot the video takes from, and the end of a source
//! to whoever is waiting for one.
//!
//! Two threads, neither of them a callback of a source (a callback that waited for a disk would lose sound in the
//! device). One routes what arrives: it puts the frame in the slot, measures the sound and puts it on the clock, and
//! hands it on. The other writes the stems: a stem that has a long gap to fill, or a disk that is slow, holds up
//! only that, and not the frames, which are large and would pile up behind it.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::finalize::STEMS;
use super::gate::{lock, SharedGate};
use super::Levels;
use crate::audio::level::level;
use crate::record::stem::StemWriter;
use crate::record::video::FrameSlot;
use crate::record::{EndReason, SourceEvent, Track};

/// How long a level stays on the meter after the buffer it was measured from: a source that delivers nothing (a system
/// that plays no sound sends no buffers) is quiet, not as loud as the last thing it said.
const STALE: Duration = Duration::from_millis(500);

/// How often the thread looks at whether it has been told to stop, when nothing arrives.
const LOOK: Duration = Duration::from_millis(20);

/// The level of the latest buffer of each track.
#[derive(Default)]
pub(super) struct Meters {
    mic: Option<(f32, Instant)>,
    system: Option<(f32, Instant)>,
}

pub(super) type SharedMeters = Arc<Mutex<Meters>>;

impl Meters {
    fn set(&mut self, track: Track, level: f32, at: Instant) {
        match track {
            Track::Mic => self.mic = Some((level, at)),
            Track::System => self.system = Some((level, at)),
            Track::Screen => {}
        }
    }

    /// The levels at `now`; a track that is off has none.
    pub fn read(&self, mic_on: bool, system_on: bool, now: Instant) -> Levels {
        let shown = |on: bool, latest: Option<(f32, Instant)>| {
            on.then(|| match latest {
                Some((level, at)) if now.saturating_duration_since(at) < STALE => level,
                _ => 0.0,
            })
        };
        Levels {
            mic: shown(mic_on, self.mic),
            system: shown(system_on, self.system),
        }
    }
}

pub(super) struct PumpSetup {
    /// What the sources send.
    pub events: Receiver<SourceEvent>,
    pub gate: SharedGate,
    /// Where the latest frame goes.
    pub frames: FrameSlot,
    pub meters: SharedMeters,
    /// Where the stems are made.
    pub folder: PathBuf,
    /// Called for a source that ended by itself, and for a stem that could not be written (as the source of it).
    pub ended: Box<dyn Fn(Track, EndReason) + Send + Sync>,
}

/// How a stem ended.
pub(super) struct StemEnd {
    pub path: PathBuf,
    /// The length of the stem in seconds, or why it could not be finished.
    pub result: io::Result<f64>,
}

pub(super) struct Pump {
    stop: Arc<AtomicBool>,
    router: JoinHandle<()>,
    writer: JoinHandle<Vec<StemEnd>>,
}

impl Pump {
    pub fn start(setup: PumpSetup) -> io::Result<Pump> {
        let stop = Arc::new(AtomicBool::new(false));
        let ended: Arc<dyn Fn(Track, EndReason) + Send + Sync> = Arc::from(setup.ended);
        let (jobs, to_write) = mpsc::channel();
        let (folder, report) = (setup.folder, ended.clone());
        let writer = std::thread::Builder::new()
            .name("recording stems".into())
            .spawn(move || write(&folder, &to_write, &*report))?;
        let seen = stop.clone();
        let router = std::thread::Builder::new()
            .name("recording sound".into())
            .spawn(move || {
                route(
                    &setup.events,
                    &setup.gate,
                    &setup.frames,
                    &setup.meters,
                    &*ended,
                    &jobs,
                    &seen,
                )
            })?;
        Ok(Pump {
            stop,
            router,
            writer,
        })
    }

    /// Stops the threads and finishes the stems. What the sources have sent and the router has not taken yet is not
    /// taken: the clock is stopped by then. What the router has handed on is written.
    pub fn finish(self) -> Vec<StemEnd> {
        self.stop.store(true, Ordering::SeqCst);
        // The router ends first, which closes the writer's queue: the writer is done when the queue is empty.
        let _ = self.router.join();
        self.writer.join().unwrap_or_default()
    }
}

/// Sound that is on the clock, for the writer: `mono[skip..]` begins at `active`.
struct Job {
    track: Track,
    rate: u32,
    active: Duration,
    skip: usize,
    mono: Vec<f32>,
}

fn route(
    events: &Receiver<SourceEvent>,
    gate: &SharedGate,
    frames: &FrameSlot,
    meters: &SharedMeters,
    ended: &(dyn Fn(Track, EndReason) + Send + Sync),
    jobs: &Sender<Job>,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::SeqCst) {
        match events.recv_timeout(LOOK) {
            Ok(SourceEvent::Frame(frame)) => frames.put(frame),
            Ok(SourceEvent::Ended { track, reason }) => ended(track, reason),
            Ok(SourceEvent::Audio {
                track,
                at,
                rate,
                mono,
            }) => {
                // The stems are those of the tracks that have sound (the screen has none).
                if !STEMS.iter().any(|(stem_track, _)| *stem_track == track) {
                    continue;
                }
                lock(meters).set(track, level(&mono), Instant::now());
                let placed = lock(gate).place(at, rate, mono.len());
                if let Some(placed) = placed {
                    let _ = jobs.send(Job {
                        track,
                        rate,
                        active: placed.active,
                        skip: placed.skip,
                        mono,
                    });
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// A stem that is being written, or one that could not be.
enum Stem {
    Open(StemWriter),
    Failed(String),
}

/// Writes the stems as the jobs come, until there are no more (the router is gone), and finishes them.
fn write(
    folder: &Path,
    jobs: &Receiver<Job>,
    ended: &(dyn Fn(Track, EndReason) + Send + Sync),
) -> Vec<StemEnd> {
    let mut stems: HashMap<Track, Stem> = HashMap::new();
    for job in jobs {
        let Some((_, name)) = STEMS.iter().find(|(track, _)| *track == job.track) else {
            continue;
        };
        let stem = stems.entry(job.track).or_insert_with(|| {
            match StemWriter::create(&folder.join(name), job.rate) {
                Ok(writer) => Stem::Open(writer),
                Err(e) => fail(ended, job.track, &e),
            }
        });
        if let Stem::Open(writer) = stem {
            if let Err(e) = writer.push(job.active, &job.mono[job.skip..]) {
                *stem = fail(ended, job.track, &e);
            }
        }
    }
    let mut ends = Vec::new();
    for (track, name) in STEMS {
        let Some(stem) = stems.remove(&track) else {
            continue;
        };
        let result = match stem {
            Stem::Open(writer) => writer.finish(),
            Stem::Failed(why) => Err(io::Error::other(why)),
        };
        ends.push(StemEnd {
            path: folder.join(name),
            result,
        });
    }
    ends
}

/// A stem that cannot be written ends the recording (a disk that is full gives no more): the source is told to
/// whoever waits for ends.
fn fail(ended: &(dyn Fn(Track, EndReason) + Send + Sync), track: Track, error: &io::Error) -> Stem {
    let why = format!("could not write the sound: {error}");
    ended(track, EndReason::Failed(why.clone()));
    Stem::Failed(why)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::fakes::sine;
    use crate::record::session::gate;
    use crate::record::video::Frame;
    use std::sync::mpsc;

    struct Rig {
        pump: Pump,
        events: mpsc::Sender<SourceEvent>,
        frames: FrameSlot,
        ended: mpsc::Receiver<EndReason>,
        base: Instant,
        folder: tempfile::TempDir,
    }

    fn rig() -> Rig {
        let folder = tempfile::tempdir().unwrap();
        let (events, rx) = mpsc::channel();
        let (ended_tx, ended) = mpsc::channel();
        let gate = gate::shared();
        let base = Instant::now();
        lock(&gate).begin(base);
        let frames = FrameSlot::new();
        let pump = Pump::start(PumpSetup {
            events: rx,
            gate,
            frames: frames.clone(),
            meters: SharedMeters::default(),
            folder: folder.path().to_path_buf(),
            ended: Box::new(move |_, reason| {
                let _ = ended_tx.send(reason);
            }),
        })
        .unwrap();
        Rig {
            pump,
            events,
            frames,
            ended,
            base,
            folder,
        }
    }

    fn audio(base: Instant, seconds: u64, track: Track) -> SourceEvent {
        SourceEvent::Audio {
            track,
            at: base + Duration::from_secs(seconds),
            rate: 48_000,
            mono: sine(440.0, 48_000, 0, 480),
        }
    }

    fn frame() -> SourceEvent {
        SourceEvent::Frame(Frame {
            width: 2,
            height: 2,
            stride: 8,
            bgra: vec![7; 16],
        })
    }

    #[test]
    fn a_long_silence_to_fill_does_not_hold_up_the_frames() {
        let rig = rig();
        rig.events.send(audio(rig.base, 0, Track::Mic)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let began = Instant::now();
        // The next sound is twenty minutes on: 58 million samples of silence to write before it, which takes a moment.
        rig.events.send(audio(rig.base, 1200, Track::Mic)).unwrap();
        rig.events.send(frame()).unwrap();

        let mut frame_came = None;
        while frame_came.is_none() {
            assert!(began.elapsed() < Duration::from_secs(60), "no frame came");
            if rig.frames.latest().is_some() {
                frame_came = Some(began.elapsed());
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        // And all of it is written in the end, which is when the silence is.
        let ends = rig.pump.finish();
        let written = began.elapsed();
        assert_eq!(ends.len(), 1);
        let seconds = ends[0].result.as_ref().unwrap();
        assert!((seconds - 1200.01).abs() < 0.01, "{seconds} s");
        // The frame came while the silence was being written, and not after: well before it was all written (it would
        // be at the same time, if the one thread did both). How long either takes depends on the machine, and the
        // two are compared with each other.
        let frame_came = frame_came.unwrap();
        assert!(
            frame_came * 2 < written,
            "the frame came after {frame_came:?} and the silence was written after {written:?}"
        );
    }

    #[test]
    fn the_end_of_a_source_is_passed_on() {
        let rig = rig();

        rig.events
            .send(SourceEvent::Ended {
                track: Track::Screen,
                reason: EndReason::DisplayGone,
            })
            .unwrap();

        assert_eq!(
            rig.ended.recv_timeout(Duration::from_secs(2)),
            Ok(EndReason::DisplayGone)
        );
        rig.pump.finish();
    }

    #[test]
    fn a_stem_that_cannot_be_written_ends_the_recording() {
        let rig = rig();
        // The folder is gone: the stem cannot be made.
        std::fs::remove_dir_all(rig.folder.path()).unwrap();

        rig.events.send(audio(rig.base, 0, Track::System)).unwrap();

        let Ok(EndReason::Failed(why)) = rig.ended.recv_timeout(Duration::from_secs(2)) else {
            panic!("no end")
        };
        assert!(why.contains("could not write the sound"), "{why}");
        let ends = rig.pump.finish();
        assert!(ends[0].result.is_err());
    }
}
