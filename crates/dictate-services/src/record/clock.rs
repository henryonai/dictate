//! The recording's own clock: the time that has gone by with the recording running, not counting the time it was
//! paused. Every stem and every frame of video is placed on it, so that what was captured while paused leaves no hole.

use std::time::{Duration, Instant};

/// Time since the recording began, minus the pauses.
pub struct ActiveClock {
    start: Instant,
    /// How long the pauses that are over lasted in all.
    paused_total: Duration,
    /// When the pause that is going on began.
    paused_since: Option<Instant>,
}

impl ActiveClock {
    /// A running clock whose zero is `start`.
    pub fn new(start: Instant) -> ActiveClock {
        ActiveClock {
            start,
            paused_total: Duration::ZERO,
            paused_since: None,
        }
    }

    /// Stops the clock at `at`. Nothing happens when it is stopped already.
    pub fn pause(&mut self, at: Instant) {
        self.paused_since.get_or_insert(at);
    }

    /// Starts the clock again at `at`, which is when the pause ends. Nothing happens when it is running.
    pub fn resume(&mut self, at: Instant) {
        if let Some(since) = self.paused_since.take() {
            self.paused_total += at.saturating_duration_since(since);
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused_since.is_some()
    }

    /// How much of the recording has gone by at `at`: `None` while the clock is paused (whatever `at` is) and for
    /// an instant before the start.
    ///
    /// Only right for an `at` that is at or after the latest resume. An instant from inside a pause that is over is
    /// taken as if the clock had never stopped, minus the whole pause (so, early on, as an instant that came
    /// before the pause). So what a source stamped, which can be from before a pause that has ended by the time it
    /// is looked at, must not be given as it is: `session::gate::Gate::place` cuts it to start at the resume.
    pub fn active_at(&self, at: Instant) -> Option<Duration> {
        if self.is_paused() || at < self.start {
            return None;
        }
        Some(self.elapsed(at))
    }

    /// How much of the recording has gone by now. It stands still while the clock is paused.
    pub fn active_now(&self) -> Duration {
        self.elapsed(Instant::now())
    }

    /// The time from the start to `at` (to the beginning of the pause, when there is one going on), minus the
    /// pauses that are over.
    fn elapsed(&self, at: Instant) -> Duration {
        let until = self.paused_since.unwrap_or(at);
        until
            .saturating_duration_since(self.start)
            .saturating_sub(self.paused_total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn second(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn active_time_excludes_pauses() {
        let t0 = Instant::now();
        let mut clock = ActiveClock::new(t0);

        clock.pause(t0 + second(2));
        assert!(clock.is_paused());
        assert_eq!(clock.active_at(t0 + second(3)), None);
        assert_eq!(clock.active_now(), second(2));

        clock.resume(t0 + second(5));
        assert!(!clock.is_paused());
        assert_eq!(clock.active_at(t0 + second(6)), Some(second(3)));
    }

    #[test]
    fn a_pause_while_paused_keeps_the_first_pause() {
        let t0 = Instant::now();
        let mut clock = ActiveClock::new(t0);
        clock.pause(t0 + second(2));
        clock.pause(t0 + second(4));
        clock.resume(t0 + second(5));
        assert_eq!(clock.active_at(t0 + second(6)), Some(second(3)));
    }

    #[test]
    fn a_resume_while_running_changes_nothing() {
        let t0 = Instant::now();
        let mut clock = ActiveClock::new(t0);
        clock.resume(t0 + second(3));
        assert_eq!(clock.active_at(t0 + second(4)), Some(second(4)));
    }

    #[test]
    fn pauses_add_up() {
        let t0 = Instant::now();
        let mut clock = ActiveClock::new(t0);
        clock.pause(t0 + second(1));
        clock.resume(t0 + second(2));
        clock.pause(t0 + second(4));
        clock.resume(t0 + second(7));
        assert_eq!(clock.active_at(t0 + second(10)), Some(second(6)));
    }

    #[test]
    fn an_instant_before_the_start_has_no_time() {
        let t0 = Instant::now();
        let clock = ActiveClock::new(t0 + second(10));
        assert_eq!(clock.active_at(t0 + second(5)), None);
        assert_eq!(clock.active_at(t0 + second(10)), Some(Duration::ZERO));
    }

    #[test]
    fn the_clock_runs_by_itself_until_it_is_paused() {
        let clock = ActiveClock::new(Instant::now());
        std::thread::sleep(Duration::from_millis(30));
        let now = clock.active_now();
        assert!(now >= Duration::from_millis(30), "only {now:?}");
        assert!(now < second(60), "{now:?}");
    }
}
