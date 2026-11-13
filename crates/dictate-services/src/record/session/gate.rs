//! The recording's clock as the threads of a session share it: when it runs, when it is paused, and which of the sound
//! that arrives is on it.
//!
//! `ActiveClock::active_at` is right only for an instant at or after the latest resume (an instant inside a pause that
//! is over gives the time as if the clock had never stopped, minus the whole pause). So nothing is handed to the clock
//! as the source stamped it: a buffer of sound that began before the clock last started running is cut to start where the
//! clock did (`Gate::place`), and one from inside a pause is dropped.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::record::clock::ActiveClock;

/// The clock, shared by the threads of a session.
pub(super) type SharedGate = Arc<Mutex<Gate>>;

pub(super) fn shared() -> SharedGate {
    Arc::new(Mutex::new(Gate::new()))
}

/// A thread that panicked with the gate (or what else of the session is behind a mutex) locked has left it as it was:
/// a clock, a level, a reason, which are as good.
pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where in a recording a buffer of sound goes.
#[derive(Debug, PartialEq)]
pub(super) struct Placement {
    /// Where the first sample that is kept is, on the recording's clock.
    pub active: Duration,
    /// How many samples at the start of the buffer are not on the clock (they came before it ran).
    pub skip: usize,
}

pub(super) struct Gate {
    clock: ActiveClock,
    /// When the clock last started running: the start of the recording, or the latest resume.
    since: Instant,
    /// The recording is not running: it has not begun, or it is over.
    closed: bool,
}

impl Gate {
    /// A gate that is closed: nothing is on the clock until `begin`.
    pub fn new() -> Gate {
        let now = Instant::now();
        Gate {
            clock: ActiveClock::new(now),
            since: now,
            closed: true,
        }
    }

    /// The recording begins at `at`: the clock starts there, and the gate is open.
    pub fn begin(&mut self, at: Instant) {
        self.clock = ActiveClock::new(at);
        self.since = at;
        self.closed = false;
    }

    /// Stops the clock at `at`, for good: later pauses and resumes change nothing, and no sound is on the clock.
    pub fn close(&mut self, at: Instant) {
        self.clock.pause(at);
        self.closed = true;
    }

    /// Stops the clock at `at`.
    pub fn pause(&mut self, at: Instant) {
        if !self.closed {
            self.clock.pause(at);
        }
    }

    /// Starts the clock again at `at`.
    pub fn resume(&mut self, at: Instant) {
        if !self.closed && self.clock.is_paused() {
            self.clock.resume(at);
            self.since = at;
        }
    }

    /// Whether the clock stands still: it is paused, or the recording is not running.
    pub fn is_paused(&self) -> bool {
        self.closed || self.clock.is_paused()
    }

    /// How much of the recording has gone by.
    pub fn active_now(&self) -> Duration {
        self.clock.active_now()
    }

    /// Where a buffer of `len` samples at `rate` Hz that was captured from `at` goes on the clock: `None` when none of
    /// it does (the clock is stopped, or the whole of it came before the clock last ran).
    pub fn place(&self, at: Instant, rate: u32, len: usize) -> Option<Placement> {
        if self.is_paused() || rate == 0 {
            return None;
        }
        let from = at.max(self.since);
        let skip = (from.duration_since(at).as_secs_f64() * f64::from(rate)).round() as usize;
        if skip >= len {
            return None;
        }
        Some(Placement {
            active: self.clock.active_at(from)?,
            skip,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn ms(n: u64) -> Duration {
        MS * n as u32
    }

    /// A gate that began at `t0`, and the sound it is given at 1 000 Hz, so that a sample is a millisecond.
    fn gate(t0: Instant) -> Gate {
        let mut gate = Gate::new();
        gate.begin(t0);
        gate
    }

    #[test]
    fn a_buffer_on_the_clock_is_placed_where_it_was_captured() {
        let t0 = Instant::now();
        let gate = gate(t0);

        let placed = gate.place(t0 + ms(500), 1000, 100).unwrap();

        assert_eq!(
            placed,
            Placement {
                active: ms(500),
                skip: 0
            }
        );
    }

    #[test]
    fn a_buffer_that_began_before_the_start_is_cut_to_start_there() {
        let t0 = Instant::now() + ms(100);
        let gate = gate(t0);

        let placed = gate.place(t0 - ms(20), 1000, 100).unwrap();

        assert_eq!(
            placed,
            Placement {
                active: ms(0),
                skip: 20
            }
        );
    }

    #[test]
    fn a_gate_that_has_not_begun_places_nothing() {
        let gate = Gate::new();
        assert_eq!(gate.place(Instant::now(), 1000, 100), None);
        assert!(gate.is_paused());
    }

    #[test]
    fn nothing_is_placed_while_paused() {
        let t0 = Instant::now();
        let mut gate = gate(t0);
        gate.pause(t0 + ms(2000));

        assert!(gate.is_paused());
        // Even a buffer that was captured before the pause: it arrived after, and the clock does not take it.
        assert_eq!(gate.place(t0 + ms(1950), 1000, 10), None);
        assert_eq!(gate.place(t0 + ms(2500), 1000, 10), None);
    }

    #[test]
    fn a_buffer_that_began_before_a_resume_is_cut_to_start_at_it() {
        let t0 = Instant::now();
        let mut gate = gate(t0);
        gate.pause(t0 + ms(2000));
        gate.resume(t0 + ms(4000));

        // 3.9 s to 4.1 s: the first 100 ms are from the pause.
        let placed = gate.place(t0 + ms(3900), 1000, 200).unwrap();

        // The clock stood still from 2 s to 4 s, so 4 s of the wall clock is 2 s of the recording.
        assert_eq!(
            placed,
            Placement {
                active: ms(2000),
                skip: 100
            }
        );
    }

    #[test]
    fn a_buffer_from_inside_a_pause_that_is_over_is_dropped() {
        let t0 = Instant::now();
        let mut gate = gate(t0);
        gate.pause(t0 + ms(2000));
        gate.resume(t0 + ms(4000));

        // Without the cut, the clock would take it as 1.5 s into the recording, which it was not.
        assert_eq!(gate.place(t0 + ms(3500), 1000, 100), None);
        assert_eq!(gate.place(t0 + ms(3900), 1000, 100), None);
        // From the resume on, it is on the clock.
        assert_eq!(
            gate.place(t0 + ms(4000), 1000, 100),
            Some(Placement {
                active: ms(2000),
                skip: 0
            })
        );
    }

    #[test]
    fn a_buffer_that_began_before_a_second_resume_is_cut_there_not_at_the_first() {
        let t0 = Instant::now();
        let mut gate = gate(t0);
        gate.pause(t0 + ms(1000));
        gate.resume(t0 + ms(2000));
        gate.pause(t0 + ms(3000));
        gate.resume(t0 + ms(5000));

        let placed = gate.place(t0 + ms(4950), 1000, 200).unwrap();

        assert_eq!(
            placed,
            Placement {
                active: ms(2000),
                skip: 50
            }
        );
    }

    #[test]
    fn a_closed_gate_is_closed_for_good() {
        let t0 = Instant::now();
        let mut gate = gate(t0);
        gate.close(t0 + ms(2000));

        gate.pause(t0 + ms(2500));
        gate.resume(t0 + ms(3000));

        assert!(gate.is_paused());
        assert_eq!(gate.active_now(), ms(2000));
        assert_eq!(gate.place(t0 + ms(3500), 1000, 100), None);
    }

    #[test]
    fn a_buffer_without_a_rate_or_samples_is_not_placed() {
        let t0 = Instant::now();
        let gate = gate(t0);

        assert_eq!(gate.place(t0 + ms(10), 0, 100), None);
        assert_eq!(gate.place(t0 + ms(10), 1000, 0), None);
    }
}
