use std::time::{Duration, SystemTime};

/// A moment read from two clocks. Time limits use both, so that neither alone can be
/// fooled:
/// - `mono` keeps counting while the machine sleeps (macOS `CLOCK_MONOTONIC`, Linux
///   `CLOCK_BOOTTIME`) and can't be set by the user. std's `Instant` isn't used: on
///   macOS it stops during sleep, so days asleep would not count.
/// - `wall` is the system clock. A reading earlier than a previous one means the clock
///   was set back, which the callers treat as "time is up".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    pub wall: SystemTime,
    /// Time since an arbitrary fixed point (usually boot), including sleep.
    pub mono: Duration,
}

impl Now {
    pub fn current() -> Self {
        Self {
            wall: SystemTime::now(),
            mono: monotonic_including_sleep(),
        }
    }

    /// `self` moved forward by `d` on both clocks (for tests and simulations).
    pub fn plus(self, d: Duration) -> Self {
        Self {
            wall: self.wall + d,
            mono: self.mono + d,
        }
    }

    /// Time since `earlier`: the larger of the two clocks' readings, or `None` if
    /// the wall clock now reads earlier than it did then.
    pub fn since(self, earlier: Now) -> Option<Duration> {
        let wall = self.wall.duration_since(earlier.wall).ok()?;
        let mono = self.mono.saturating_sub(earlier.mono);
        Some(wall.max(mono))
    }
}

#[cfg(target_vendor = "apple")]
const CLOCK: rustix::time::ClockId = rustix::time::ClockId::Monotonic;
#[cfg(not(target_vendor = "apple"))]
const CLOCK: rustix::time::ClockId = rustix::time::ClockId::Boottime;

fn monotonic_including_sleep() -> Duration {
    let t = rustix::time::clock_gettime(CLOCK);
    Duration::new(
        u64::try_from(t.tv_sec).unwrap_or(0),
        u32::try_from(t.tv_nsec).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    #[test]
    fn the_larger_clock_wins() {
        let t0 = Now::current();
        // Wall clock moved further than the monotonic one.
        let wall_ahead = Now {
            wall: t0.wall + HOUR,
            mono: t0.mono,
        };
        assert_eq!(wall_ahead.since(t0), Some(HOUR));
        // Wall clock set back an hour while a minute passed.
        let set_back = Now {
            wall: t0.wall - HOUR,
            mono: t0.mono + Duration::from_secs(60),
        };
        assert_eq!(set_back.since(t0), None);
        assert_eq!(t0.plus(HOUR).since(t0), Some(HOUR));
    }

    /// Review finding 1: ten days asleep, then the wall clock set back to just after
    /// the start. The sleep-counting clock still shows ten days.
    #[test]
    fn sleep_counts_even_if_the_wall_clock_is_set_back() {
        let t0 = Now::current();
        let ten_days = 240 * HOUR;
        let after = Now {
            wall: t0.wall + HOUR,
            mono: t0.mono + ten_days,
        };
        assert_eq!(after.since(t0), Some(ten_days));
    }

    #[test]
    fn the_clock_moves_forward() {
        let a = Now::current();
        std::thread::sleep(Duration::from_millis(20));
        let b = Now::current();
        assert!(b.mono > a.mono);
        assert!(b.mono - a.mono >= Duration::from_millis(20));
    }
}
