//! When the result chip goes away: its timer, and the two questions that tell whether what the user did afterwards
//! is a reason to hide it. All of it is pure, and the clock is an argument, so that it is tested without waiting.

use std::time::{Duration, Instant};

use crate::platform::paste::is_app;
use crate::platform::AppId;

/// How long a chip with written text stays when nobody touches it.
pub const WRITTEN_FOR: Duration = Duration::from_secs(10);
/// How long a chip that only has a notice stays.
pub const NOTICE_FOR: Duration = Duration::from_secs(4);

/// Input this soon after a key of ours was sent is ours: the paste of a rewrite comes back through the monitor.
pub const OWN_INPUT_FOR: Duration = Duration::from_millis(300);
/// How often the app in front is looked at while the chip shows.
pub const FRONT_POLL: Duration = Duration::from_millis(200);

/// The auto-hide timer of the chip. It runs from when the chip appears and stops while the mouse is over the chip or
/// its menu is open: the time that is left is kept.
#[derive(Debug, Clone)]
pub struct ChipTimer {
    limit: Duration,
    /// What ran before the current stretch began.
    spent: Duration,
    /// When the current stretch began; `None` while the timer is stopped.
    since: Option<Instant>,
}

impl ChipTimer {
    /// A timer that starts at `now`: 10 s for a chip with written text, 4 s for one that only has a notice.
    pub fn new(now: Instant, notice_only: bool) -> Self {
        Self {
            limit: if notice_only { NOTICE_FOR } else { WRITTEN_FOR },
            spent: Duration::ZERO,
            since: Some(now),
        }
    }

    /// Stops the timer. Stopping a timer that is stopped changes nothing.
    pub fn pause(&mut self, now: Instant) {
        if let Some(since) = self.since.take() {
            self.spent += now.saturating_duration_since(since);
        }
    }

    /// Starts it again. Starting a timer that runs changes nothing.
    pub fn resume(&mut self, now: Instant) {
        self.since.get_or_insert(now);
    }

    /// Whether the chip has been shown for its time.
    pub fn expired(&self, now: Instant) -> bool {
        let running = self
            .since
            .map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
        self.spent + running >= self.limit
    }
}

/// Whether an input at `now` is the echo of a key that the app sent itself, `last_post` ago at most [`OWN_INPUT_FOR`]. The
/// monitor hears every key and click of the system, ours included, and the chip stays for the user's.
pub fn is_own_input(now: Instant, last_post: Option<Instant>) -> bool {
    // An input stamped before the key it echoes (another thread's clock reading) is ours too: the gap is zero then.
    last_post.is_some_and(|posted| now.saturating_duration_since(posted) < OWN_INPUT_FOR)
}

/// Whether the app in front is a reason to hide the chip: it is neither the app of the text field the chip is by nor this
/// app (its Settings window, say). When the system doesn't say which app is in front there is no reason.
pub fn switched_away(front: Option<&AppId>, field: &AppId, own_pid: u32) -> bool {
    front.is_some_and(|front| front.pid != Some(own_pid) && !is_app(front, field))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn chip_timer_10s_and_4s() {
        let start = Instant::now();
        let written = ChipTimer::new(start, false);
        assert!(!written.expired(start));
        assert!(!written.expired(start + Duration::from_millis(9_999)));
        assert!(written.expired(start + secs(10)));

        let notice = ChipTimer::new(start, true);
        assert!(!notice.expired(start + Duration::from_millis(3_999)));
        assert!(notice.expired(start + secs(4)));
        // The notice goes sooner than the written text does.
        assert!(!written.expired(start + secs(4)));
    }

    #[test]
    fn chip_timer_pauses_while_hovered() {
        let start = Instant::now();
        let mut timer = ChipTimer::new(start, false);

        // 6 s go by, then the mouse comes over the chip and stays there for a minute.
        timer.pause(start + secs(6));
        assert!(!timer.expired(start + secs(66)));

        // It leaves: 4 s are left, and not a moment more.
        timer.resume(start + secs(66));
        assert!(!timer.expired(start + secs(69)));
        assert!(timer.expired(start + secs(70)));
    }

    #[test]
    fn chip_timer_pause_and_resume_are_idempotent() {
        // The hover and the menu each ask to stop it: two stops and one start must not leave it stopped or running twice.
        let start = Instant::now();
        let mut timer = ChipTimer::new(start, false);
        timer.pause(start + secs(2));
        timer.pause(start + secs(5));
        timer.resume(start + secs(20));
        timer.resume(start + secs(25));
        // 2 s ran before the stop, so 8 are left from the first resume at 20.
        assert!(!timer.expired(start + secs(27)));
        assert!(timer.expired(start + secs(28)));
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_panic() {
        let start = Instant::now();
        let mut timer = ChipTimer::new(start + secs(5), true);
        assert!(!timer.expired(start));
        timer.pause(start);
        timer.resume(start);
        assert!(!timer.expired(start));
    }

    // ---- What the user did ---------------------------------------------------------------------------------------

    #[test]
    fn input_right_after_our_own_key_is_ignored() {
        let posted = Instant::now();
        assert!(is_own_input(posted, Some(posted)));
        assert!(is_own_input(
            posted + Duration::from_millis(299),
            Some(posted)
        ));
        assert!(!is_own_input(
            posted + Duration::from_millis(300),
            Some(posted)
        ));
        assert!(!is_own_input(posted + secs(5), Some(posted)));
    }

    #[test]
    fn input_is_the_users_when_no_key_of_ours_was_sent() {
        assert!(!is_own_input(Instant::now(), None));
    }

    #[test]
    fn input_stamped_before_the_key_that_was_sent_counts_as_ours() {
        // The monitor and the sender are on different threads: the monitor may read the time just before the sender
        // writes down when it posted.
        let now = Instant::now();
        assert!(is_own_input(now, Some(now + Duration::from_millis(2))));
    }

    fn app(id: &str, pid: Option<u32>) -> AppId {
        AppId { id: id.into(), pid }
    }

    #[test]
    fn the_field_app_in_front_is_no_reason_to_hide() {
        let field = app("com.apple.TextEdit", Some(10));
        assert!(!switched_away(Some(&field.clone()), &field, 99));
        // The same app, named by its id when a pid is missing on one side.
        assert!(!switched_away(
            Some(&app("com.apple.TextEdit", None)),
            &field,
            99
        ));
    }

    #[test]
    fn another_app_in_front_hides_the_chip() {
        let field = app("com.apple.TextEdit", Some(10));
        assert!(switched_away(
            Some(&app("com.apple.Safari", Some(11))),
            &field,
            99
        ));
    }

    #[test]
    fn this_app_in_front_is_no_reason_to_hide() {
        let field = app("com.apple.TextEdit", Some(10));
        assert!(!switched_away(
            Some(&app("com.example.dictate", Some(99))),
            &field,
            99
        ));
    }

    #[test]
    fn an_app_the_system_does_not_name_is_no_reason_to_hide() {
        let field = app("com.apple.TextEdit", Some(10));
        assert!(!switched_away(None, &field, 99));
    }
}
