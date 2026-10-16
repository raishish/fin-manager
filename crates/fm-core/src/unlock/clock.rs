use std::time::{Duration, Instant, SystemTime};

/// A moment read from both clocks. Time limits use both, because each one alone
/// can be fooled: the monotonic clock stops while a Mac sleeps, and the wall clock
/// can be set back by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    pub wall: SystemTime,
    pub mono: Instant,
}

impl Now {
    pub fn current() -> Self {
        Self {
            wall: SystemTime::now(),
            mono: Instant::now(),
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
        let mono = self.mono.saturating_duration_since(earlier.mono);
        Some(wall.max(mono))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_larger_clock_wins() {
        let t0 = Now::current();
        let hour = Duration::from_secs(3600);
        // Slept for an hour: the monotonic clock didn't move, the wall clock did.
        let after_sleep = Now {
            wall: t0.wall + hour,
            mono: t0.mono,
        };
        assert_eq!(after_sleep.since(t0), Some(hour));
        // Wall clock set back an hour while a minute passed.
        let set_back = Now {
            wall: t0.wall - hour,
            mono: t0.mono + Duration::from_secs(60),
        };
        assert_eq!(set_back.since(t0), None);
        assert_eq!(t0.plus(hour).since(t0), Some(hour));
    }
}
