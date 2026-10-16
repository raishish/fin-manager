//! Auto-lock (SPEC §7): the ledger locks itself after a period without activity.

use std::time::Duration;

use super::clock::Now;

/// Idle timeouts the user can choose from, in minutes.
pub const CHOICES_MINUTES: [u64; 5] = [1, 5, 10, 30, 60];
pub const DEFAULT_MINUTES: u64 = 10;

/// Tracks activity and says when the idle timeout has passed. Time asleep counts
/// as idle, and a wall clock set backwards locks at once.
#[derive(Debug, Clone)]
pub struct AutoLock {
    timeout: Duration,
    last_activity: Now,
}

impl AutoLock {
    /// Starts the timer. `minutes` outside [`CHOICES_MINUTES`] falls back to the
    /// nearest allowed choice, so a damaged setting can't disable auto-lock.
    pub fn new(minutes: u64, now: Now) -> Self {
        Self {
            timeout: Duration::from_secs(60 * nearest_choice(minutes)),
            last_activity: now,
        }
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The user did something.
    pub fn activity(&mut self, now: Now) {
        self.last_activity = now;
    }

    pub fn set_minutes(&mut self, minutes: u64, now: Now) {
        self.timeout = Duration::from_secs(60 * nearest_choice(minutes));
        self.last_activity = now;
    }

    /// True when the ledger should lock now.
    pub fn expired(&self, now: Now) -> bool {
        now.since(self.last_activity)
            .is_none_or(|idle| idle >= self.timeout)
    }

    /// Time left before locking, for the UI's timer.
    pub fn remaining(&self, now: Now) -> Duration {
        now.since(self.last_activity)
            .map_or(Duration::ZERO, |idle| self.timeout.saturating_sub(idle))
    }
}

fn nearest_choice(minutes: u64) -> u64 {
    *CHOICES_MINUTES
        .iter()
        .min_by_key(|&&c| c.abs_diff(minutes))
        .expect("choices are not empty")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn locks_after_the_idle_timeout() {
        let t0 = Now::current();
        let lock = AutoLock::new(10, t0);
        assert!(!lock.expired(t0.plus(9 * MIN)));
        assert_eq!(lock.remaining(t0.plus(9 * MIN)), MIN);
        assert!(lock.expired(t0.plus(10 * MIN)));
        assert_eq!(lock.remaining(t0.plus(11 * MIN)), Duration::ZERO);
    }

    #[test]
    fn activity_restarts_the_timer() {
        let t0 = Now::current();
        let mut lock = AutoLock::new(5, t0);
        lock.activity(t0.plus(4 * MIN));
        assert!(!lock.expired(t0.plus(8 * MIN)));
        assert!(lock.expired(t0.plus(9 * MIN)));
    }

    #[test]
    fn time_asleep_counts_as_idle() {
        let t0 = Now::current();
        let lock = AutoLock::new(10, t0);
        let woke = Now {
            wall: t0.wall + 8 * 60 * MIN,
            mono: t0.mono,
        };
        assert!(lock.expired(woke));
    }

    #[test]
    fn a_clock_set_back_locks_at_once() {
        let t0 = Now::current();
        let lock = AutoLock::new(60, t0);
        let set_back = Now {
            wall: t0.wall - MIN,
            mono: t0.mono + Duration::from_secs(1),
        };
        assert!(lock.expired(set_back));
    }

    #[test]
    fn timeouts_are_limited_to_the_choices() {
        let t0 = Now::current();
        assert_eq!(AutoLock::new(0, t0).timeout(), MIN);
        assert_eq!(AutoLock::new(7, t0).timeout(), 5 * MIN);
        assert_eq!(AutoLock::new(u64::MAX, t0).timeout(), 60 * MIN);
        assert_eq!(AutoLock::new(DEFAULT_MINUTES, t0).timeout(), 10 * MIN);
    }
}
