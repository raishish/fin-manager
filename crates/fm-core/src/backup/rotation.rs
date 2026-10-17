//! Which backups to keep (decision 28): the newest of each of the 7 most recent days,
//! 4 most recent ISO weeks and 12 most recent months that have a backup, plus the
//! newest backup overall. Days, weeks and months are UTC.
//!
//! "Most recent periods that have a backup" rather than "the last 7 calendar days", so
//! a machine that was off for a month still keeps a week of dailies, and a wrong clock
//! can't make the rule delete everything.

use std::collections::BTreeSet;

use crate::model::Date;

pub const DAILY: usize = 7;
pub const WEEKLY: usize = 4;
pub const MONTHLY: usize = 12;

/// Given backup times (Unix seconds, any order, duplicates allowed), the indexes of
/// the ones to keep.
pub fn keep(times: &[i64]) -> BTreeSet<usize> {
    let mut order: Vec<usize> = (0..times.len()).collect();
    // Newest first; ties broken by index so the result is deterministic.
    order.sort_by(|&a, &b| times[b].cmp(&times[a]).then(a.cmp(&b)));

    let mut kept = BTreeSet::new();
    if let Some(&newest) = order.first() {
        kept.insert(newest);
    }
    keep_newest_per_period(&order, times, DAILY, day, &mut kept);
    keep_newest_per_period(&order, times, WEEKLY, iso_week, &mut kept);
    keep_newest_per_period(&order, times, MONTHLY, month, &mut kept);
    kept
}

/// Walks backups newest first; the first one seen in each period is that period's
/// newest. Stops after `count` periods.
fn keep_newest_per_period(
    order: &[usize],
    times: &[i64],
    count: usize,
    period: fn(i64) -> i64,
    kept: &mut BTreeSet<usize>,
) {
    let mut seen = Vec::with_capacity(count);
    for &i in order {
        let p = period(times[i]);
        if seen.contains(&p) {
            continue;
        }
        if seen.len() == count {
            break;
        }
        seen.push(p);
        kept.insert(i);
    }
}

fn day(t: i64) -> i64 {
    t.div_euclid(86_400)
}

/// Monday-based week number. 1970-01-01 was a Thursday, so day 0 is 3 days into its
/// week.
fn iso_week(t: i64) -> i64 {
    (day(t) + 3).div_euclid(7)
}

fn month(t: i64) -> i64 {
    let date = i32::try_from(day(t))
        .ok()
        .and_then(Date::from_days_since_epoch);
    date.map_or(i64::MIN, |d| {
        i64::from(d.year()) * 12 + i64::from(d.month())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;
    /// Monday 2026-10-12, 12:00 UTC.
    const MONDAY: i64 = 1_791_806_400;

    fn kept_times(times: &[i64]) -> Vec<i64> {
        let mut out: Vec<i64> = keep(times).into_iter().map(|i| times[i]).collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn monday_is_the_start_of_a_week() {
        let (y, m, d) = Date::from_days_since_epoch(i32::try_from(day(MONDAY)).unwrap())
            .unwrap()
            .ymd();
        assert_eq!((y, m, d), (2026, 10, 12));
        assert_eq!(iso_week(MONDAY), iso_week(MONDAY + 6 * DAY));
        // Sunday 23:00, the evening before, is in the week before.
        assert_ne!(iso_week(MONDAY), iso_week(MONDAY - 13 * 3_600));
    }

    #[test]
    fn nothing_to_keep_from_nothing() {
        assert!(keep(&[]).is_empty());
        assert_eq!(kept_times(&[MONDAY]), [MONDAY]);
    }

    #[test]
    fn several_backups_a_day_keep_the_newest_of_each_day() {
        // Daily plus after-import backups: three a day for three days.
        let times: Vec<i64> = (0..3)
            .flat_map(|d| [0, 3_600, 7_200].map(|h| MONDAY + d * DAY + h))
            .collect();
        assert_eq!(
            kept_times(&times),
            [
                MONDAY + 7_200,
                MONDAY + DAY + 7_200,
                MONDAY + 2 * DAY + 7_200
            ]
        );
    }

    #[test]
    fn a_year_of_dailies_keeps_7_4_12() {
        let now = MONDAY;
        let times: Vec<i64> = (0..400).map(|d| now - d * DAY).collect();
        let kept = kept_times(&times);
        // 7 dailies: today and the 6 days before.
        for d in 0..7 {
            assert!(kept.contains(&(now - d * DAY)), "daily {d}");
        }
        // Weekly: the newest of each of the 4 most recent weeks: this Monday (also
        // daily), and the Sundays closing the 3 weeks before.
        for w in 1..4 {
            assert!(kept.contains(&(now - (7 * w - 6) * DAY)), "weekly {w}");
        }
        // Monthly: the newest of each of the 12 most recent months. With the dailies
        // and weeklies overlapping, the total stays small.
        let months: BTreeSet<i64> = kept.iter().map(|&t| month(t)).collect();
        assert_eq!(months.len(), 12);
        assert!(kept.len() <= DAILY + WEEKLY + MONTHLY);
        assert!(kept.len() >= MONTHLY);
        // Nothing older than 12 months survives.
        assert!(kept.iter().all(|&t| now - t < 366 * DAY));
    }

    #[test]
    fn a_long_gap_keeps_a_week_of_dailies() {
        // Seven days of backups, then the machine was off for two months.
        let times: Vec<i64> = (0..7).map(|d| MONDAY - 60 * DAY - d * DAY).collect();
        assert_eq!(kept_times(&times).len(), 7);
    }

    #[test]
    fn the_newest_is_always_kept_even_with_a_wrong_clock() {
        let future = MONDAY + 3_650 * DAY;
        let times = [MONDAY, MONDAY - DAY, future];
        assert!(kept_times(&times).contains(&future));
        assert!(kept_times(&times).contains(&MONDAY));
    }

    #[test]
    fn identical_times_keep_one() {
        assert_eq!(keep(&[MONDAY, MONDAY, MONDAY]).len(), 1);
    }
}
