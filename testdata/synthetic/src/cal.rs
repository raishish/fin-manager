//! Calendar helpers on top of `fm_core::model::Date`.

use fm_core::model::Date;

/// The date `y-m-d`; panics on an impossible date (persona specs are constants).
pub fn ymd(y: i32, m: u8, d: u8) -> Date {
    Date::from_ymd(y, m, d).unwrap_or_else(|_| panic!("bad date {y}-{m}-{d}"))
}

/// Day `d` of the month, clamped to the month's last day (the 31st becomes the 30th).
pub fn day_of_month(y: i32, m: u8, d: u8) -> Date {
    let last = fm_core::model::YearMonth::new(y, m)
        .expect("valid month")
        .days_in_month();
    ymd(y, m, d.min(last))
}

pub fn add_days(d: Date, n: i32) -> Date {
    d.checked_add_days(n).expect("date in range")
}

/// Day of the week, Monday = 0 … Sunday = 6. 1970-01-01 was a Thursday.
pub fn weekday(d: Date) -> usize {
    (d.days_since_epoch() + 3).rem_euclid(7) as usize
}

/// The next business day on or after `d` (ACH debits and deposits don't post at
/// weekends). Bank holidays are ignored.
pub fn business_on_or_after(d: Date) -> Date {
    match weekday(d) {
        5 => add_days(d, 2),
        6 => add_days(d, 1),
        _ => d,
    }
}

/// The last business day on or before `d` (payroll moves earlier, not later).
pub fn business_on_or_before(d: Date) -> Date {
    match weekday(d) {
        5 => add_days(d, -1),
        6 => add_days(d, -2),
        _ => d,
    }
}

/// The `n`th (1-based) given weekday of a month, e.g. the second Wednesday.
pub fn nth_weekday(y: i32, m: u8, wd: usize, n: u8) -> Date {
    let first = ymd(y, m, 1);
    let offset = (wd + 7 - weekday(first)) % 7;
    add_days(first, offset as i32 + 7 * (i32::from(n) - 1))
}

/// Every month from `start`'s month to `end`'s month, inclusive, as (year, month).
pub fn months(start: Date, end: Date) -> Vec<(i32, u8)> {
    let mut out = Vec::new();
    let (mut y, mut m) = (start.year(), start.month());
    while (y, m) <= (end.year(), end.month()) {
        out.push((y, m));
        if m == 12 {
            y += 1;
            m = 1;
        } else {
            m += 1;
        }
    }
    out
}

/// `MM/DD`, as banks print dates inside descriptions.
pub fn mm_dd(d: Date) -> String {
    format!("{:02}/{:02}", d.month(), d.day())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekdays_and_business_days() {
        assert_eq!(weekday(ymd(2026, 10, 16)), 4); // a Friday
        assert_eq!(business_on_or_after(ymd(2026, 10, 17)), ymd(2026, 10, 19));
        assert_eq!(business_on_or_before(ymd(2026, 10, 18)), ymd(2026, 10, 16));
        assert_eq!(nth_weekday(2025, 1, 2, 2), ymd(2025, 1, 8)); // 2nd Wednesday
        assert_eq!(day_of_month(2025, 2, 31), ymd(2025, 2, 28));
        assert_eq!(months(ymd(2024, 11, 5), ymd(2025, 2, 1)).len(), 4);
    }
}
