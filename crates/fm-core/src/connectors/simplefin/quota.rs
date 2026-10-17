//! The bridge's limits, enforced before any request is sent:
//! - "24 requests or fewer per day" per access token (sustained overuse disables the
//!   token). Every `/accounts` request is counted in a rolling 24 hours, kept in the
//!   ledger so a restart doesn't reset it.
//! - "The date range … is limited to 90 days at a time", and the bridge warns past 45
//!   days ("In the future, this may be capped"). Windows are at most 45 days. The first
//!   sync fetches 90 days of history (two requests); each later sync overlaps the last
//!   by 5 days so late-posting transactions aren't missed (the pipeline drops the
//!   duplicates).

pub const MAX_REQUESTS_PER_DAY: usize = 24;
pub const MAX_WINDOW_DAYS: i64 = 45;
/// How far back the first sync goes.
pub const HISTORY_DAYS: i64 = 90;
pub const OVERLAP_DAYS: i64 = 5;
const DAY: i64 = 86_400;

/// Unix seconds of the requests made in the last 24 hours.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Quota {
    requests: Vec<i64>,
}

impl Quota {
    /// From the stored text (comma-separated Unix seconds). Unreadable entries count as
    /// a request made now, so damage never frees quota. A stamp more than a day ahead
    /// (the clock was far ahead, or the text was tampered with) counts as a day ahead,
    /// so it blocks for two days at most.
    pub(super) fn decode(text: &str, now: i64) -> Self {
        let latest = now.saturating_add(DAY);
        let requests = text
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().parse::<i64>().unwrap_or(now).min(latest))
            .collect();
        Self { requests }
    }

    pub(super) fn encode(&self) -> String {
        let parts: Vec<String> = self.requests.iter().map(i64::to_string).collect();
        parts.join(",")
    }

    /// Requests that still count at `now`. One stamped later than `now` (the clock was
    /// set back) counts until 24 hours after its stamp, so setting the clock back
    /// never frees quota early.
    fn counting(&self, now: i64) -> impl Iterator<Item = i64> + '_ {
        self.requests
            .iter()
            .copied()
            .filter(move |&t| t > now.saturating_sub(DAY))
    }

    pub(super) fn used(&self, now: i64) -> usize {
        self.counting(now).count()
    }

    pub(super) fn left(&self, now: i64) -> usize {
        MAX_REQUESTS_PER_DAY.saturating_sub(self.used(now))
    }

    /// When the next request is allowed, if not now.
    pub(super) fn next_allowed(&self, now: i64) -> Option<i64> {
        if self.left(now) > 0 {
            return None;
        }
        let mut counting: Vec<i64> = self.counting(now).collect();
        counting.sort_unstable();
        // The oldest of the newest 24 has to age out.
        let oldest_needed = counting[counting.len() - MAX_REQUESTS_PER_DAY];
        Some(oldest_needed.saturating_add(DAY))
    }

    /// Records a request at `now` and forgets ones that no longer count.
    pub(super) fn record(&mut self, now: i64) {
        self.requests.retain(|&t| t > now.saturating_sub(DAY));
        self.requests.push(now);
    }
}

/// The windows to fetch, oldest first: from 5 days before the end of the last sync (or
/// 90 days ago for the first) up to `now`, each at most 45 days. At most a day's
/// requests: after a longer gap, the next sync carries on from where this one ends.
pub(super) fn windows(synced_to: Option<i64>, now: i64) -> Vec<(i64, i64)> {
    let start = match synced_to {
        // A stamp in the future means the clock went back: fetch the overlap only.
        Some(t) => t.min(now).saturating_sub(OVERLAP_DAYS * DAY),
        None => now.saturating_sub(HISTORY_DAYS * DAY),
    };
    let mut from = start;
    let mut out = Vec::new();
    while from < now && out.len() < MAX_REQUESTS_PER_DAY {
        let to = from.saturating_add(MAX_WINDOW_DAYS * DAY).min(now);
        if to <= from {
            break;
        }
        out.push((from, to));
        from = to;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_792_152_000;

    #[test]
    fn twenty_four_a_day_then_wait() {
        let mut q = Quota::default();
        for i in 0..24 {
            assert_eq!(q.left(NOW + i), (24 - i) as usize);
            q.record(NOW + i);
        }
        assert_eq!(q.left(NOW + 30), 0);
        assert_eq!(q.next_allowed(NOW + 30), Some(NOW + DAY));
        assert_eq!(q.left(NOW + DAY), 1, "the first one aged out");
        assert_eq!(q.next_allowed(NOW + DAY), None);
    }

    #[test]
    fn the_count_survives_storage_and_damage() {
        let mut q = Quota::default();
        for i in 0..5 {
            q.record(NOW + i);
        }
        let back = Quota::decode(&q.encode(), NOW + 10);
        assert_eq!(back.used(NOW + 10), 5);
        let damaged = Quota::decode("garbage,,1", NOW);
        assert_eq!(
            damaged.used(NOW),
            1,
            "unreadable counts as now; ancient doesn't count"
        );
    }

    #[test]
    fn setting_the_clock_back_frees_nothing() {
        let mut q = Quota::default();
        for i in 0..24 {
            q.record(NOW + i);
        }
        // The clock goes back a week: the 24 recent requests still count.
        let earlier = NOW - 7 * DAY;
        assert_eq!(q.left(earlier), 0);
        assert!(q.next_allowed(earlier).unwrap() > earlier);
        // A request recorded now doesn't drop the "future" ones.
        let before = q.used(earlier);
        q.record(earlier);
        assert!(q.used(earlier) >= before);
    }

    #[test]
    fn windows_are_at_most_45_days_and_overlap_by_5() {
        let first = windows(None, NOW);
        assert_eq!(
            first,
            [(NOW - 90 * DAY, NOW - 45 * DAY), (NOW - 45 * DAY, NOW)],
            "90 days of history in two requests"
        );
        let next = windows(Some(NOW - DAY), NOW);
        assert_eq!(next, [(NOW - 6 * DAY, NOW)], "5 days of overlap");
        let after_a_long_gap = windows(Some(NOW - 200 * DAY), NOW);
        assert_eq!(after_a_long_gap.len(), 5);
        for &(from, to) in &after_a_long_gap {
            assert!(to - from <= 45 * DAY && from < to);
        }
        assert_eq!(after_a_long_gap[0].0, NOW - 205 * DAY);
        assert_eq!(after_a_long_gap.last().unwrap().1, NOW);
        for w in after_a_long_gap.windows(2) {
            assert_eq!(w[0].1, w[1].0, "contiguous");
        }
        // A last sync in the future (the clock went back) fetches the overlap only.
        assert_eq!(windows(Some(NOW + 10 * DAY), NOW), [(NOW - 5 * DAY, NOW)]);
    }

    /// Review finding L7: no overflow, and the loop ends, whatever the clock or the
    /// stored values say.
    #[test]
    fn extreme_times_neither_overflow_nor_loop() {
        for now in [i64::MIN, i64::MIN + 1, -1, 0, i64::MAX - 1, i64::MAX] {
            for synced_to in [None, Some(i64::MIN), Some(0), Some(i64::MAX)] {
                let ws = windows(synced_to, now);
                assert!(ws.len() <= MAX_REQUESTS_PER_DAY, "{synced_to:?} {now}");
                assert!(ws.iter().all(|&(f, t)| f < t && t <= now));
            }
            let mut q = Quota::decode("-9223372036854775808,9223372036854775807", now);
            let _ = (q.used(now), q.left(now), q.next_allowed(now));
            q.record(now);
        }
    }

    /// Review finding L2: a stamp far in the future blocks for two days at most.
    #[test]
    fn stamps_far_ahead_count_as_a_day_ahead() {
        let far = (NOW + 365 * DAY).to_string();
        let text = vec![far.as_str(); 24].join(",");
        let q = Quota::decode(&text, NOW);
        assert_eq!(q.left(NOW), 0);
        assert_eq!(q.next_allowed(NOW), Some(NOW + 2 * DAY));
        assert_eq!(q.left(NOW + 2 * DAY), 24);
    }
}
