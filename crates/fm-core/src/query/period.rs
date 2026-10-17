//! Turns a [`PeriodSpec`] into dates, relative to the `as_of` day the app passes in.

use super::{PeriodSpec, QueryError};
use crate::model::{Date, YearMonth};
use crate::views::Period;

fn month(year: i32, m: u8) -> Result<YearMonth, QueryError> {
    YearMonth::new(year, m).map_err(|_| QueryError::InvalidPeriod)
}

fn months(first: YearMonth, last: YearMonth) -> Result<Period, QueryError> {
    Period::months(first, last).map_err(|_| QueryError::InvalidPeriod)
}

fn shift(m: YearMonth, by: i32) -> Result<YearMonth, QueryError> {
    m.checked_add_months(by).ok_or(QueryError::InvalidPeriod)
}

fn days(d: Date, by: i32) -> Result<Date, QueryError> {
    d.checked_add_days(by).ok_or(QueryError::InvalidPeriod)
}

fn quarter(year: i32, q: u8) -> Result<Period, QueryError> {
    if !(1..=4).contains(&q) {
        return Err(QueryError::InvalidPeriod);
    }
    months(month(year, 3 * q - 2)?, month(year, 3 * q)?)
}

/// The dates `spec` covers, given today is `as_of`. `all_time` starts at `earliest`
/// (the ledger's first transaction).
pub(super) fn resolve(
    spec: &PeriodSpec,
    as_of: Date,
    earliest: Date,
) -> Result<Period, QueryError> {
    let this_month = as_of.year_month();
    let n = |n: u32| i32::try_from(n).map_err(|_| QueryError::InvalidPeriod);
    match *spec {
        PeriodSpec::Range { from, to } => {
            Period::new(from, to).map_err(|_| QueryError::InvalidPeriod)
        }
        PeriodSpec::Month(m) => Ok(Period::month(m)),
        PeriodSpec::Months { from, to } => months(from, to),
        PeriodSpec::Quarter { year, quarter: q } => quarter(year, q),
        PeriodSpec::Year(year) => Period::year(year).map_err(|_| QueryError::InvalidPeriod),
        PeriodSpec::LastMonths(k) if k > 0 => {
            months(shift(this_month, -n(k)?)?, shift(this_month, -1)?)
        }
        PeriodSpec::LastDays(k) if k > 0 => Period::new(days(as_of, -n(k)?)?, days(as_of, -1)?)
            .map_err(|_| QueryError::InvalidPeriod),
        PeriodSpec::LastMonths(_) | PeriodSpec::LastDays(_) => Err(QueryError::InvalidPeriod),
        PeriodSpec::LastWeek => {
            // 1970-01-01 was a Thursday: days since the epoch + 3, mod 7, is 0 on Monday.
            let weekday = (as_of.days_since_epoch() + 3).rem_euclid(7);
            let monday = days(as_of, -weekday)?;
            Period::new(days(monday, -7)?, days(monday, -1)?).map_err(|_| QueryError::InvalidPeriod)
        }
        PeriodSpec::LastQuarter => {
            let q = (this_month.month() - 1) / 3 + 1;
            if q == 1 {
                quarter(this_month.year() - 1, 4)
            } else {
                quarter(this_month.year(), q - 1)
            }
        }
        PeriodSpec::YearToDate => Period::new(month(as_of.year(), 1)?.first_day(), as_of)
            .map_err(|_| QueryError::InvalidPeriod),
        PeriodSpec::AllTime => Ok(Period {
            from: earliest.min(as_of),
            to: as_of,
        }),
    }
}
