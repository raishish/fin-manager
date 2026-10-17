use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A calendar date as the bank wrote it: no time and no time zone.
///
/// Always a real date between 0001-01-01 and 9999-12-31. Serialised as `"YYYY-MM-DD"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: u16,
    month: u8,
    day: u8,
}

/// Why a date was refused. Carries no input text, so it is safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateError {
    /// Not in `YYYY-MM-DD` form.
    Format,
    /// The form is right, but no such calendar day exists.
    NoSuchDay,
}

impl fmt::Display for DateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DateError::Format => "date is not in YYYY-MM-DD form",
            DateError::NoSuchDay => "no such calendar day",
        })
    }
}

impl std::error::Error for DateError {}

fn is_leap(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

impl Date {
    /// The date, if it is a real calendar day in years 1 to 9999.
    pub fn new(year: u16, month: u8, day: u8) -> Result<Date, DateError> {
        if (1..=9999).contains(&year)
            && (1..=12).contains(&month)
            && day >= 1
            && day <= days_in_month(year, month)
        {
            Ok(Date { year, month, day })
        } else {
            Err(DateError::NoSuchDay)
        }
    }

    pub const fn year(self) -> u16 {
        self.year
    }

    pub const fn month(self) -> u8 {
        self.month
    }

    pub const fn day(self) -> u8 {
        self.day
    }

    /// Parses exactly `YYYY-MM-DD` (four, two and two digits).
    pub fn parse_iso(text: &str) -> Result<Date, DateError> {
        let b = text.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return Err(DateError::Format);
        }
        let num = |range: std::ops::Range<usize>| -> Result<u16, DateError> {
            let part = &text[range];
            if part.bytes().all(|c| c.is_ascii_digit()) {
                part.parse().map_err(|_| DateError::Format)
            } else {
                Err(DateError::Format)
            }
        };
        // Month and day are at most 99 here, so the narrowing casts can't truncate.
        Date::new(num(0..4)?, num(5..7)? as u8, num(8..10)? as u8)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl Serialize for Date {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Date, D::Error> {
        let text = String::deserialize(deserializer)?;
        Date::parse_iso(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_days_only() {
        assert!(Date::new(2024, 2, 29).is_ok());
        assert_eq!(Date::new(2023, 2, 29), Err(DateError::NoSuchDay));
        assert!(Date::new(2000, 2, 29).is_ok());
        assert_eq!(Date::new(1900, 2, 29), Err(DateError::NoSuchDay));
        assert_eq!(Date::new(2100, 2, 29), Err(DateError::NoSuchDay));
        assert_eq!(Date::new(2026, 4, 31), Err(DateError::NoSuchDay));
        assert_eq!(Date::new(2026, 13, 1), Err(DateError::NoSuchDay));
        assert_eq!(Date::new(0, 1, 1), Err(DateError::NoSuchDay));
        assert_eq!(Date::new(2026, 1, 0), Err(DateError::NoSuchDay));
    }

    #[test]
    fn parses_and_prints_iso() {
        let d = Date::parse_iso("2026-10-16").unwrap();
        assert_eq!((d.year(), d.month(), d.day()), (2026, 10, 16));
        assert_eq!(d.to_string(), "2026-10-16");
        assert_eq!(
            Date::parse_iso("0001-01-01").unwrap().to_string(),
            "0001-01-01"
        );
        for bad in [
            "2026-1-16",
            "2026/10/16",
            "26-10-16",
            "2026-10-16 ",
            "+026-10-16",
            "2026-10-1x",
            "",
        ] {
            assert_eq!(Date::parse_iso(bad), Err(DateError::Format), "{bad:?}");
        }
        assert_eq!(Date::parse_iso("2026-02-30"), Err(DateError::NoSuchDay));
    }

    #[test]
    fn serde_round_trip() {
        let d = Date::new(2025, 3, 1).unwrap();
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "\"2025-03-01\"");
        assert_eq!(serde_json::from_str::<Date>(&json).unwrap(), d);
        assert!(serde_json::from_str::<Date>("\"2025-02-29\"").is_err());
    }

    #[test]
    fn orders_chronologically() {
        let a = Date::new(2025, 12, 31).unwrap();
        let b = Date::new(2026, 1, 1).unwrap();
        assert!(a < b);
    }
}
