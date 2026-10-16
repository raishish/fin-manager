use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::ModelError;

const MIN_YEAR: i32 = 1;
const MAX_YEAR: i32 = 9999;

/// Whether `year` is a Gregorian leap year.
pub const fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days in `month` (1–12) of `year`; 0 for an invalid month.
pub const fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

// Proleptic Gregorian conversions between (year, month, day) and days since 1970-01-01.
// Howard Hinnant's algorithms (https://howardhinnant.github.io/date_algorithms.html);
// inputs are always within years 1..=9999, so the `as` casts can't truncate.

const fn days_from_civil(year: i32, month: u8, day: u8) -> i32 {
    let m = month as i64;
    let y = year as i64 - if m <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as i32
}

const fn civil_from_days(days: i32) -> (i32, u8, u8) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u8, day as u8)
}

/// A calendar date with no time or time zone, as banks report it (posted date,
/// transaction date). Range 0001-01-01 to 9999-12-31, proleptic Gregorian.
///
/// Serialised as an ISO 8601 `YYYY-MM-DD` string. Ordering is chronological.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    /// Days since 1970-01-01 (negative before it).
    days: i32,
}

impl Date {
    /// 0001-01-01.
    pub const MIN: Date = Date {
        days: days_from_civil(MIN_YEAR, 1, 1),
    };
    /// 9999-12-31.
    pub const MAX: Date = Date {
        days: days_from_civil(MAX_YEAR, 12, 31),
    };

    /// The date `year-month-day`, if it exists.
    pub fn from_ymd(year: i32, month: u8, day: u8) -> Result<Date, ModelError> {
        if !(MIN_YEAR..=MAX_YEAR).contains(&year)
            || !(1..=12).contains(&month)
            || day == 0
            || day > days_in_month(year, month)
        {
            return Err(ModelError::InvalidDate(format!(
                "{year:04}-{month:02}-{day:02}"
            )));
        }
        Ok(Date {
            days: days_from_civil(year, month, day),
        })
    }

    /// The date `days` after 1970-01-01, if it is in range.
    pub fn from_days_since_epoch(days: i32) -> Option<Date> {
        (Self::MIN.days..=Self::MAX.days)
            .contains(&days)
            .then_some(Date { days })
    }

    /// Days since 1970-01-01 (negative before it).
    pub fn days_since_epoch(self) -> i32 {
        self.days
    }

    /// `(year, month, day)`.
    pub fn ymd(self) -> (i32, u8, u8) {
        civil_from_days(self.days)
    }

    pub fn year(self) -> i32 {
        self.ymd().0
    }

    /// 1–12.
    pub fn month(self) -> u8 {
        self.ymd().1
    }

    /// 1–31.
    pub fn day(self) -> u8 {
        self.ymd().2
    }

    /// The month this date falls in.
    pub fn year_month(self) -> YearMonth {
        let (year, month, _) = self.ymd();
        YearMonth { year, month }
    }

    /// This date moved by `days` (negative goes back), if the result is in range.
    pub fn checked_add_days(self, days: i32) -> Option<Date> {
        Self::from_days_since_epoch(self.days.checked_add(days)?)
    }

    /// Signed days from `earlier` to `self` (negative if `self` is before `earlier`).
    pub fn days_since(self, earlier: Date) -> i32 {
        self.days - earlier.days
    }

    /// This date moved by `months`, keeping the day of the month but clamping it to the
    /// last day of the target month: 2026-01-31 + 1 month = 2026-02-28.
    pub fn checked_add_months(self, months: i32) -> Option<Date> {
        let (_, _, day) = self.ymd();
        let target = self.year_month().checked_add_months(months)?;
        let day = day.min(target.days_in_month());
        Date::from_ymd(target.year, target.month, day).ok()
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = self.ymd();
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

/// Parses a non-empty run of ASCII digits (no sign, no spaces).
fn parse_fixed_digits(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

impl FromStr for Date {
    type Err = ModelError;

    /// Parses exactly `YYYY-MM-DD`: no time, no zone, no other layouts. Importers turn
    /// bank-specific layouts into this one.
    fn from_str(s: &str) -> Result<Date, ModelError> {
        let invalid = || ModelError::InvalidDate(s.to_owned());
        let b = s.as_bytes();
        if !s.is_ascii() || b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return Err(invalid());
        }
        let year = parse_fixed_digits(&s[0..4]).ok_or_else(invalid)?;
        let month = parse_fixed_digits(&s[5..7]).ok_or_else(invalid)?;
        let day = parse_fixed_digits(&s[8..10]).ok_or_else(invalid)?;
        // Each part has at most four digits, so the casts can't truncate.
        Date::from_ymd(year as i32, month as u8, day as u8).map_err(|_| invalid())
    }
}

serde_as_str!(Date);

/// A calendar month, the unit of the spending views. Serialised as `YYYY-MM`.
/// Ordering is chronological.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YearMonth {
    // Field order matters: the derived `Ord` compares `year` first.
    year: i32,
    month: u8,
}

impl YearMonth {
    pub fn new(year: i32, month: u8) -> Result<YearMonth, ModelError> {
        if !(MIN_YEAR..=MAX_YEAR).contains(&year) || !(1..=12).contains(&month) {
            return Err(ModelError::InvalidYearMonth(format!(
                "{year:04}-{month:02}"
            )));
        }
        Ok(YearMonth { year, month })
    }

    pub fn year(self) -> i32 {
        self.year
    }

    /// 1–12.
    pub fn month(self) -> u8 {
        self.month
    }

    pub fn days_in_month(self) -> u8 {
        days_in_month(self.year, self.month)
    }

    pub fn first_day(self) -> Date {
        Date {
            days: days_from_civil(self.year, self.month, 1),
        }
    }

    pub fn last_day(self) -> Date {
        Date {
            days: days_from_civil(self.year, self.month, self.days_in_month()),
        }
    }

    /// This month moved by `months` (negative goes back), if the result is in range.
    pub fn checked_add_months(self, months: i32) -> Option<YearMonth> {
        let index = i64::from(self.year) * 12 + i64::from(self.month) - 1 + i64::from(months);
        let year = i32::try_from(index.div_euclid(12)).ok()?;
        let month = u8::try_from(index.rem_euclid(12) + 1).ok()?;
        YearMonth::new(year, month).ok()
    }
}

impl fmt::Display for YearMonth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

impl FromStr for YearMonth {
    type Err = ModelError;

    /// Parses exactly `YYYY-MM`.
    fn from_str(s: &str) -> Result<YearMonth, ModelError> {
        let invalid = || ModelError::InvalidYearMonth(s.to_owned());
        let b = s.as_bytes();
        if !s.is_ascii() || b.len() != 7 || b[4] != b'-' {
            return Err(invalid());
        }
        let year = parse_fixed_digits(&s[0..4]).ok_or_else(invalid)?;
        let month = parse_fixed_digits(&s[5..7]).ok_or_else(invalid)?;
        YearMonth::new(year as i32, month as u8).map_err(|_| invalid())
    }
}

serde_as_str!(YearMonth);

/// A moment in UTC, in whole seconds since the Unix epoch. Used for when something
/// happened in the app (an import, a decision), never for bank dates (those are [`Date`]).
///
/// Serialised as a plain integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn from_unix_seconds(seconds: i64) -> Timestamp {
        Timestamp(seconds)
    }

    pub const fn unix_seconds(self) -> i64 {
        self.0
    }

    /// The current time from the system clock.
    pub fn now() -> Timestamp {
        let seconds = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
            Err(before) => i64::try_from(before.duration().as_secs()).map_or(i64::MIN, |s| -s),
        };
        Timestamp(seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> Date {
        s.parse().unwrap()
    }

    #[test]
    fn epoch_anchor() {
        assert_eq!(Date::from_ymd(1970, 1, 1).unwrap().days_since_epoch(), 0);
        assert_eq!(
            Date::from_ymd(2000, 3, 1).unwrap().days_since_epoch(),
            11_017
        );
        assert_eq!(Date::from_ymd(1969, 12, 31).unwrap().days_since_epoch(), -1);
    }

    /// Every day of a full 400-year Gregorian cycle (146 097 days) converts to
    /// (y, m, d) and back, days advance one at a time, and the calendar fields roll over
    /// exactly as `days_in_month` says.
    #[test]
    fn four_hundred_year_cycle_round_trips() {
        let start = Date::from_ymd(2000, 1, 1).unwrap();
        let mut expected = (2000, 1u8, 1u8);
        for offset in 0..146_097 {
            let d = start.checked_add_days(offset).unwrap();
            assert_eq!(d.ymd(), expected, "offset {offset}");
            let (y, m, dd) = expected;
            assert_eq!(Date::from_ymd(y, m, dd).unwrap(), d);
            assert_eq!(d.to_string().parse::<Date>().unwrap(), d);
            expected = if dd < days_in_month(y, m) {
                (y, m, dd + 1)
            } else if m < 12 {
                (y, m + 1, 1)
            } else {
                (y + 1, 1, 1)
            };
        }
        assert_eq!(expected, (2400, 1, 1));
        assert_eq!(
            Date::from_ymd(2400, 1, 1).unwrap().days_since(start),
            146_097
        );
    }

    #[test]
    fn whole_range_ends_round_trip() {
        assert_eq!(Date::MIN.to_string(), "0001-01-01");
        assert_eq!(Date::MAX.to_string(), "9999-12-31");
        assert_eq!(Date::MIN.checked_add_days(-1), None);
        assert_eq!(Date::MAX.checked_add_days(1), None);
        assert_eq!(Date::MAX.checked_add_days(i32::MAX), None);
        assert_eq!(Date::MIN.checked_add_days(i32::MIN), None);
    }

    #[test]
    fn leap_years() {
        assert!(!is_leap_year(1900));
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(2100));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2026));

        assert!(Date::from_ymd(1900, 2, 29).is_err());
        assert!(Date::from_ymd(2000, 2, 29).is_ok());
        assert!(Date::from_ymd(2100, 2, 29).is_err());
        assert!(Date::from_ymd(2024, 2, 29).is_ok());
        assert!(Date::from_ymd(2026, 2, 29).is_err());
        assert!("2100-02-29".parse::<Date>().is_err());

        assert_eq!(
            date("2024-02-28").checked_add_days(1),
            Some(date("2024-02-29"))
        );
        assert_eq!(
            date("2026-02-28").checked_add_days(1),
            Some(date("2026-03-01"))
        );
        assert_eq!(
            date("1900-02-28").checked_add_days(1),
            Some(date("1900-03-01"))
        );
        assert_eq!(
            date("2000-02-28").checked_add_days(1),
            Some(date("2000-02-29"))
        );
    }

    #[test]
    fn month_arithmetic_clamps_to_month_end() {
        assert_eq!(
            date("2026-01-31").checked_add_months(1),
            Some(date("2026-02-28"))
        );
        assert_eq!(
            date("2024-01-31").checked_add_months(1),
            Some(date("2024-02-29"))
        );
        assert_eq!(
            date("2026-03-31").checked_add_months(1),
            Some(date("2026-04-30"))
        );
        assert_eq!(
            date("2026-03-31").checked_add_months(-1),
            Some(date("2026-02-28"))
        );
        assert_eq!(
            date("2024-02-29").checked_add_months(12),
            Some(date("2025-02-28"))
        );
        assert_eq!(
            date("2024-02-29").checked_add_months(48),
            Some(date("2028-02-29"))
        );
        assert_eq!(
            date("2026-10-16").checked_add_months(3),
            Some(date("2027-01-16"))
        );
        assert_eq!(
            date("2026-01-15").checked_add_months(-1),
            Some(date("2025-12-15"))
        );
        assert_eq!(
            date("2026-05-15").checked_add_months(0),
            Some(date("2026-05-15"))
        );
        // Clamping is not reversible: Jan 31 → Feb 28 → Mar 28.
        assert_eq!(
            date("2026-01-31")
                .checked_add_months(1)
                .and_then(|d| d.checked_add_months(1)),
            Some(date("2026-03-28"))
        );
        assert_eq!(Date::MAX.checked_add_months(1), None);
        assert_eq!(Date::MIN.checked_add_months(-1), None);
        assert_eq!(Date::MIN.checked_add_months(i32::MIN), None);
    }

    #[test]
    fn year_month() {
        let ym: YearMonth = "2026-02".parse().unwrap();
        assert_eq!(ym.first_day(), date("2026-02-01"));
        assert_eq!(ym.last_day(), date("2026-02-28"));
        assert_eq!(ym.checked_add_months(11).unwrap().to_string(), "2027-01");
        assert_eq!(ym.checked_add_months(-2).unwrap().to_string(), "2025-12");
        assert_eq!(date("2024-12-31").year_month().to_string(), "2024-12");
        assert!("2026-12".parse::<YearMonth>().unwrap() < "2027-01".parse().unwrap());
        for bad in [
            "2026-13", "2026-00", "0000-01", "2026-1", "2026/01", "26-01", "2026-01-",
        ] {
            assert!(bad.parse::<YearMonth>().is_err(), "{bad}");
        }
    }

    #[test]
    fn parse_rejects_everything_but_iso_dates() {
        for bad in [
            "",
            "2026-1-16",
            "2026-01-6",
            "26-01-16",
            "2026/01/16",
            "2026-01-16T00:00:00",
            " 2026-01-16",
            "2026-01-16 ",
            "+026-01-16",
            "2026-+1-16",
            "2026-01-+6",
            "2026-00-10",
            "2026-13-10",
            "2026-01-00",
            "2026-01-32",
            "2026-04-31",
            "0000-01-01",
            "２０２６-01-16",
        ] {
            assert!(bad.parse::<Date>().is_err(), "{bad:?} should be rejected");
        }
        assert_eq!(date("0001-01-01"), Date::MIN);
        assert_eq!(date("2026-10-16").to_string(), "2026-10-16");
    }

    #[test]
    fn ordering_and_differences() {
        assert!(date("2025-12-31") < date("2026-01-01"));
        assert_eq!(date("2026-03-01").days_since(date("2026-02-01")), 28);
        assert_eq!(date("2026-02-01").days_since(date("2026-03-01")), -28);
    }

    #[test]
    fn timestamp_now_is_after_2026() {
        assert!(Timestamp::now().unix_seconds() > 1_767_225_600); // 2026-01-01T00:00:00Z
    }
}
