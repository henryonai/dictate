//! A scripted [`Capturer`] for the tests of the recording session: what each track does once it is started is
//! set beforehand (a tone that comes in real time, frames of one colour at a rate, a start that fails, an end by
//! itself after a while), and every start and stop is noted. A track that is given no script delivers nothing by
//! itself, and the test sends what it wants through [`FakeCapturer::send_audio`], with the `at` it wants: no
//! sleeping, and no sound that has to be made to line up with the clock.
//!
//! Built for this crate's own tests, and for any other crate that turns on the `test-support` feature (the app's tests,
//! as a dev-dependency); no other build has it.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::video::Frame;
use super::{Capturer, EndReason, RecordOptions, RunningSource, SourceEvent, StartError, Track};

/// How much sound a live tone is delivered in: 20 ms, as a microphone does.
const CHUNK: Duration = Duration::from_millis(20);
/// The rate of a live tone.
const RATE: u32 = 48_000;

/// `count` samples of a sine wave at `hz` (half of full scale), sampled at `rate`, from the sample numbered `first`.
pub fn sine(hz: f32, rate: u32, first: usize, count: usize) -> Vec<f32> {
    (first..first + count)
        .map(|i| 0.5 * (std::f32::consts::TAU * hz * (i as f64 / f64::from(rate)) as f32).sin())
        .collect()
}

/// What a track does once it has started.
#[derive(Clone)]
enum Plan {
    Tone {
        hz: f32,
    },
    Frames {
        width: u32,
        height: u32,
        pixel: [u8; 4],
        fps: u32,
    },
}

#[derive(Default)]
struct Script {
    plan: Option<Plan>,
    ends: Option<(Duration, EndReason)>,
    fails: Option<StartError>,
}

#[derive(Default)]
struct Log {
    started: Vec<Track>,
    stopped: Vec<Track>,
    senders: HashMap<Track, Sender<SourceEvent>>,
}

/// The scripted capture: `FakeCapturer::new().tone(Track::Mic, 440.0).fails(Track::System, ..)`.
#[derive(Default)]
pub struct FakeCapturer {
    scripts: Mutex<HashMap<Track, Script>>,
    log: Arc<Mutex<Log>>,
}

impl FakeCapturer {
    pub fn new() -> FakeCapturer {
        FakeCapturer::default()
    }

    fn script(self, track: Track, set: impl FnOnce(&mut Script)) -> FakeCapturer {
        set(lock(&self.scripts).entry(track).or_default());
        self
    }

    /// `track` delivers a tone of `hz` as it goes: 48 kHz in chunks of 20 ms, each stamped with when its first sample
    /// was due, from the moment the track starts.
    pub fn tone(self, track: Track, hz: f32) -> FakeCapturer {
        self.script(track, |script| script.plan = Some(Plan::Tone { hz }))
    }

    /// The screen delivers frames of one colour (`pixel` is BGRA), `fps` of them a second, from the moment it starts.
    pub fn frames(self, width: u32, height: u32, pixel: [u8; 4], fps: u32) -> FakeCapturer {
        self.script(Track::Screen, |script| {
            script.plan = Some(Plan::Frames {
                width,
                height,
                pixel,
                fps,
            })
        })
    }

    /// `track` ends by itself `after` its start, with `reason`, and delivers nothing from then on.
    pub fn ends_after(self, track: Track, after: Duration, reason: EndReason) -> FakeCapturer {
        self.script(track, |script| script.ends = Some((after, reason)))
    }

    /// Starting `track` fails with `error` (the first time).
    pub fn fails(self, track: Track, error: StartError) -> FakeCapturer {
        self.script(track, |script| script.fails = Some(error))
    }

    /// The tracks that were started, in the order they were.
    pub fn started(&self) -> Vec<Track> {
        lock(&self.log).started.clone()
    }

    /// The tracks that were stopped, in the order they were.
    pub fn stopped(&self) -> Vec<Track> {
        lock(&self.log).stopped.clone()
    }

    /// Sends a buffer of sound as `track`. Panics when the track has not been started.
    pub fn send_audio(&self, track: Track, at: Instant, rate: u32, mono: Vec<f32>) {
        self.send(
            track,
            SourceEvent::Audio {
                track,
                at,
                rate,
                mono,
            },
        );
    }

    /// Sends a frame as the screen. Panics when the screen has not been started.
    pub fn send_frame(&self, frame: Frame) {
        self.send(Track::Screen, SourceEvent::Frame(frame));
    }

    /// Ends `track` as if it had by itself. Panics when the track has not been started.
    pub fn send_ended(&self, track: Track, reason: EndReason) {
        self.send(track, SourceEvent::Ended { track, reason });
    }

    fn send(&self, track: Track, event: SourceEvent) {
        let log = lock(&self.log);
        let tx = log
            .senders
            .get(&track)
            .unwrap_or_else(|| panic!("{track:?} has not been started"));
        // A session that is over has no one to read: what is sent then is lost, as it is by a real source.
        let _ = tx.send(event);
    }
}

impl Capturer for FakeCapturer {
    fn start(
        &self,
        track: Track,
        _options: &RecordOptions,
        tx: Sender<SourceEvent>,
    ) -> Result<Box<dyn RunningSource>, StartError> {
        let (plan, ends) = {
            let mut scripts = lock(&self.scripts);
            let script = scripts.entry(track).or_default();
            if let Some(error) = script.fails.take() {
                return Err(error);
            }
            (script.plan.clone(), script.ends.clone())
        };
        {
            let mut log = lock(&self.log);
            log.started.push(track);
            log.senders.insert(track, tx.clone());
        }
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("fake source".into())
            .spawn(move || deliver(track, plan, ends, tx, stopped))
            .map_err(|e| StartError::Source(e.to_string()))?;
        Ok(Box::new(FakeSource {
            track,
            log: self.log.clone(),
            stop: Some(stop),
            thread: Some(thread),
        }))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a started track does, on its own thread, until it is told to stop (the other end of `stopped` is dropped or
/// sent to).
fn deliver(
    track: Track,
    plan: Option<Plan>,
    ends: Option<(Duration, EndReason)>,
    tx: Sender<SourceEvent>,
    stopped: Receiver<()>,
) {
    let start = Instant::now();
    let period = match &plan {
        Some(Plan::Frames { fps, .. }) => Duration::from_secs(1) / (*fps).max(1),
        _ => CHUNK,
    };
    let mut delivered: u32 = 0;
    let mut samples = 0;
    loop {
        // The next thing to do is the next chunk (delivered at the end of the time it covers), or the end.
        let chunk_due = start + period * (delivered + 1);
        let wake = match &ends {
            Some((after, _)) => chunk_due.min(start + *after),
            None => chunk_due,
        };
        match stopped.recv_timeout(wake.saturating_duration_since(Instant::now())) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return,
        }
        let now = Instant::now();
        if let Some((after, reason)) = &ends {
            if now >= start + *after {
                let _ = tx.send(SourceEvent::Ended {
                    track,
                    reason: reason.clone(),
                });
                // Over: nothing more comes, and the source is stopped as usual.
                let _ = stopped.recv();
                return;
            }
        }
        while start + period * (delivered + 1) <= now {
            let at = start + period * delivered;
            let event = match &plan {
                Some(Plan::Tone { hz }) => {
                    let count = (RATE as usize * period.as_millis() as usize) / 1000;
                    let mono = sine(*hz, RATE, samples, count);
                    samples += count;
                    Some(SourceEvent::Audio {
                        track,
                        at,
                        rate: RATE,
                        mono,
                    })
                }
                Some(Plan::Frames {
                    width,
                    height,
                    pixel,
                    ..
                }) => Some(SourceEvent::Frame(Frame {
                    width: *width,
                    height: *height,
                    stride: width * 4,
                    bgra: pixel.repeat((width * height) as usize),
                })),
                None => None,
            };
            delivered += 1;
            if let Some(event) = event {
                if tx.send(event).is_err() {
                    return;
                }
            }
        }
    }
}

/// A started fake track: its thread, and the way to tell it to stop.
struct FakeSource {
    track: Track,
    log: Arc<Mutex<Log>>,
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl RunningSource for FakeSource {
    fn stop(mut self: Box<Self>) {
        // Dropping the sender is the stop; the thread is gone when this returns, as a real source's is.
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        lock(&self.log).stopped.push(self.track);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::quality::Quality;
    use crate::record::Mode;
    use std::sync::mpsc::{self, Receiver};

    fn options() -> RecordOptions {
        RecordOptions {
            mode: Mode::Audio,
            quality: Quality::Standard,
            mic: true,
            system_audio: false,
        }
    }

    fn start(
        fake: &FakeCapturer,
        track: Track,
    ) -> (
        Result<Box<dyn RunningSource>, StartError>,
        Receiver<SourceEvent>,
    ) {
        let (tx, rx) = mpsc::channel();
        (fake.start(track, &options(), tx), rx)
    }

    #[test]
    fn a_sine_is_half_of_full_scale_and_continues_where_the_last_one_ended() {
        let whole = sine(440.0, 48_000, 0, 960);
        let parts = [sine(440.0, 48_000, 0, 480), sine(440.0, 48_000, 480, 480)].concat();

        assert_eq!(whole, parts);
        let peak = whole.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.01, "{peak}");
    }

    #[test]
    fn a_tone_arrives_as_it_goes_with_the_time_each_chunk_was_due() {
        let fake = FakeCapturer::new().tone(Track::Mic, 440.0);
        let before = Instant::now();
        let (source, rx) = start(&fake, Track::Mic);

        let chunks: Vec<_> = (0..5)
            .map(|_| rx.recv_timeout(Duration::from_secs(2)).expect("a chunk"))
            .collect();
        source.unwrap().stop();

        for (i, chunk) in chunks.iter().enumerate() {
            let SourceEvent::Audio {
                track,
                at,
                rate,
                mono,
            } = chunk
            else {
                panic!("not audio")
            };
            assert_eq!((*track, *rate, mono.len()), (Track::Mic, 48_000, 960));
            // Stamped with when its first sample was due: 20 ms apart, the first no earlier than the start.
            let due = before + CHUNK * i as u32;
            assert!(*at >= due && *at < due + Duration::from_millis(200), "{i}");
        }
        // The chunks are one continuous tone.
        let (SourceEvent::Audio { mono: first, .. }, SourceEvent::Audio { mono: second, .. }) =
            (&chunks[0], &chunks[1])
        else {
            panic!()
        };
        assert_eq!(first, &sine(440.0, 48_000, 0, 960));
        assert_eq!(second, &sine(440.0, 48_000, 960, 960));
    }

    #[test]
    fn frames_come_at_their_rate_in_one_colour() {
        let fake = FakeCapturer::new().frames(8, 4, [1, 2, 3, 255], 50);
        let (source, rx) = start(&fake, Track::Screen);

        let started = Instant::now();
        for _ in 0..5 {
            let SourceEvent::Frame(frame) =
                rx.recv_timeout(Duration::from_secs(2)).expect("a frame")
            else {
                panic!("not a frame")
            };
            assert_eq!((frame.width, frame.height, frame.stride), (8, 4, 32));
            assert_eq!(frame.bgra, [1, 2, 3, 255].repeat(32));
        }
        let took = started.elapsed();
        source.unwrap().stop();

        // Five frames at 50 fps take about 100 ms (the first is due after one period).
        assert!(took >= Duration::from_millis(60), "{took:?}");
        assert!(took < Duration::from_millis(1000), "{took:?}");
    }

    #[test]
    fn starts_and_stops_are_noted_in_order() {
        let fake = FakeCapturer::new();
        let (screen, _rx1) = start(&fake, Track::Screen);
        let (mic, _rx2) = start(&fake, Track::Mic);
        assert_eq!(fake.started(), [Track::Screen, Track::Mic]);
        assert!(fake.stopped().is_empty());

        mic.unwrap().stop();
        screen.unwrap().stop();

        assert_eq!(fake.stopped(), [Track::Mic, Track::Screen]);
    }

    #[test]
    fn a_scripted_start_error_comes_back_and_nothing_is_started() {
        let fake = FakeCapturer::new().fails(Track::Mic, StartError::Source("no device".into()));

        let (result, _rx) = start(&fake, Track::Mic);

        assert_eq!(result.err(), Some(StartError::Source("no device".into())));
        assert!(fake.started().is_empty());
    }

    #[test]
    fn a_track_can_end_by_itself_and_then_says_nothing() {
        let fake = FakeCapturer::new().ends_after(
            Track::Screen,
            Duration::from_millis(60),
            EndReason::WindowClosed,
        );
        let (source, rx) = start(&fake, Track::Screen);

        let event = rx.recv_timeout(Duration::from_secs(2)).expect("the end");

        assert!(matches!(
            event,
            SourceEvent::Ended {
                track: Track::Screen,
                reason: EndReason::WindowClosed
            }
        ));
        source.unwrap().stop();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn what_the_test_sends_arrives_as_the_track() {
        let fake = FakeCapturer::new();
        let (_mic, rx) = start(&fake, Track::Mic);
        let at = Instant::now();

        fake.send_audio(Track::Mic, at, 16_000, vec![0.25; 4]);
        fake.send_ended(Track::Mic, EndReason::DeviceLost("unplugged".into()));

        let Ok(SourceEvent::Audio {
            track,
            at: sent_at,
            rate,
            mono,
        }) = rx.try_recv()
        else {
            panic!("no audio")
        };
        assert_eq!(
            (track, sent_at, rate, mono),
            (Track::Mic, at, 16_000, vec![0.25; 4])
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(SourceEvent::Ended {
                reason: EndReason::DeviceLost(_),
                ..
            })
        ));
    }

    #[test]
    fn stopping_a_source_ends_its_delivery() {
        let fake = FakeCapturer::new().tone(Track::System, 220.0);
        let (source, rx) = start(&fake, Track::System);
        rx.recv_timeout(Duration::from_secs(2)).expect("a chunk");

        source.unwrap().stop();

        while rx.try_recv().is_ok() {}
        std::thread::sleep(Duration::from_millis(100));
        assert!(rx.try_recv().is_err(), "a stopped source went on");
    }
}
