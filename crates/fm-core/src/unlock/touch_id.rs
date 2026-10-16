//! When Touch ID may unlock instead of the password (SPEC §7).
//!
//! Touch ID only re-opens a ledger that the password already unlocked in this run of
//! the app, so the password is needed after every restart. On top of that, the
//! password is needed again after 5 failed scans in a row and 7 days after it was
//! last typed. All of this is kept in memory only: there is nothing on disk to edit.

use std::fmt;
use std::time::Duration;

use super::clock::Now;

pub const MAX_FAILED_SCANS: u32 = 5;
pub const PASSWORD_EVERY: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Why the password is needed instead of Touch ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordRequired {
    /// The password hasn't been typed since the app started.
    NotSinceStart,
    /// [`MAX_FAILED_SCANS`] scans failed in a row.
    TooManyFailedScans,
    /// [`PASSWORD_EVERY`] has passed since the password was last typed.
    Weekly,
    /// The wall clock was set back, so the 7-day limit can't be trusted.
    ClockChanged,
}

impl fmt::Display for PasswordRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotSinceStart => "enter your password after the app starts",
            Self::TooManyFailedScans => "too many failed Touch ID attempts; enter your password",
            Self::Weekly => "enter your password; it's needed at least once a week",
            Self::ClockChanged => "the system clock changed; enter your password",
        })
    }
}

/// The in-memory state behind the rules above.
#[derive(Debug, Clone, Default)]
pub struct TouchIdPolicy {
    last_password: Option<Now>,
    failed_scans: u32,
}

impl TouchIdPolicy {
    /// The password unlocked the ledger: Touch ID may be used again, for 7 days.
    pub fn password_unlocked(&mut self, now: Now) {
        self.last_password = Some(now);
        self.failed_scans = 0;
    }

    pub fn scan_failed(&mut self) {
        self.failed_scans = self.failed_scans.saturating_add(1);
    }

    pub fn scan_succeeded(&mut self) {
        self.failed_scans = 0;
    }

    pub fn failed_scans(&self) -> u32 {
        self.failed_scans
    }

    /// `Ok` if a Touch ID unlock may be offered now.
    pub fn check(&self, now: Now) -> Result<(), PasswordRequired> {
        let since = self.last_password.ok_or(PasswordRequired::NotSinceStart)?;
        if self.failed_scans >= MAX_FAILED_SCANS {
            return Err(PasswordRequired::TooManyFailedScans);
        }
        match now.since(since) {
            None => Err(PasswordRequired::ClockChanged),
            Some(elapsed) if elapsed >= PASSWORD_EVERY => Err(PasswordRequired::Weekly),
            Some(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    fn unlocked_at(t0: Now) -> TouchIdPolicy {
        let mut p = TouchIdPolicy::default();
        p.password_unlocked(t0);
        p
    }

    #[test]
    fn password_is_required_after_a_restart() {
        // A new run of the app starts with a fresh policy.
        assert_eq!(
            TouchIdPolicy::default().check(Now::current()),
            Err(PasswordRequired::NotSinceStart)
        );
    }

    #[test]
    fn touch_id_is_allowed_after_a_password_unlock() {
        let t0 = Now::current();
        assert_eq!(unlocked_at(t0).check(t0.plus(HOUR)), Ok(()));
    }

    #[test]
    fn five_failed_scans_require_the_password() {
        let t0 = Now::current();
        let mut p = unlocked_at(t0);
        for _ in 0..MAX_FAILED_SCANS - 1 {
            p.scan_failed();
        }
        assert_eq!(p.check(t0), Ok(()));
        p.scan_failed();
        assert_eq!(p.check(t0), Err(PasswordRequired::TooManyFailedScans));
        // Only the password clears it.
        p.scan_succeeded();
        p.scan_failed();
        p.password_unlocked(t0);
        assert_eq!(p.failed_scans(), 0);
        assert_eq!(p.check(t0), Ok(()));
    }

    #[test]
    fn a_success_resets_the_failure_count() {
        let t0 = Now::current();
        let mut p = unlocked_at(t0);
        for _ in 0..4 {
            p.scan_failed();
        }
        p.scan_succeeded();
        for _ in 0..4 {
            p.scan_failed();
        }
        assert_eq!(p.check(t0), Ok(()));
    }

    #[test]
    fn password_is_required_every_seven_days() {
        let t0 = Now::current();
        let p = unlocked_at(t0);
        assert_eq!(p.check(t0.plus(PASSWORD_EVERY - HOUR)), Ok(()));
        assert_eq!(
            p.check(t0.plus(PASSWORD_EVERY)),
            Err(PasswordRequired::Weekly)
        );
        // Days spent asleep count, although the monotonic clock stood still.
        let after_sleep = Now {
            wall: t0.wall + PASSWORD_EVERY,
            mono: t0.mono + HOUR,
        };
        assert_eq!(p.check(after_sleep), Err(PasswordRequired::Weekly));
        // Setting the wall clock back doesn't extend the week.
        let set_back = Now {
            wall: t0.wall - HOUR,
            mono: t0.mono + PASSWORD_EVERY,
        };
        assert_eq!(p.check(set_back), Err(PasswordRequired::ClockChanged));
        let set_back_a_little = Now {
            wall: t0.wall - HOUR,
            mono: t0.mono + HOUR,
        };
        assert_eq!(
            p.check(set_back_a_little),
            Err(PasswordRequired::ClockChanged)
        );
    }
}
