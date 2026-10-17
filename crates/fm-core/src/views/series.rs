//! Month by month: "spending by category for each month this year", and the trend
//! of each row against its own average.

use std::collections::BTreeMap;

use serde::Serialize;

use super::rank::rank;
use super::scope::{Scope, add, div_round};
use super::{GroupBy, Key, Measure, Period, Query, ViewError};
use crate::model::{Currency, Ledger, Money, TransactionId, YearMonth};

/// One calendar month of a series.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Column {
    pub month: YearMonth,
    /// The month isn't over at `as_of`, or the period covers only part of it. Partial
    /// months are left out of trend averages.
    pub partial: bool,
    /// Every row's value this month, added up.
    pub total: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeriesRow {
    pub key: Key,
    pub label: String,
    /// One per column, in the same order.
    pub values: Vec<Money>,
    /// The values added up.
    pub total: Money,
    pub count: usize,
    /// Every transaction counted in this row, over all months, ascending.
    pub transaction_ids: Vec<TransactionId>,
}

/// The result of [`monthly`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Series {
    pub currency: Currency,
    pub period: Period,
    pub measure: Measure,
    pub group_by: GroupBy,
    /// Every calendar month the period touches, in order, including empty ones.
    pub columns: Vec<Column>,
    /// By total over the period, largest first (by month: in calendar order).
    pub rows: Vec<SeriesRow>,
    /// The rows after the top `n`, added up. `None` when nothing was cut.
    pub other: Option<SeriesRow>,
    /// The measure over the whole period.
    pub total: Money,
}

struct Acc {
    values: Vec<i64>,
    total: i64,
    ids: Vec<TransactionId>,
}

impl Acc {
    fn new(months: usize) -> Acc {
        Acc {
            values: vec![0; months],
            total: 0,
            ids: Vec::new(),
        }
    }

    fn merge(&mut self, other: Acc) -> Result<(), ViewError> {
        for (v, o) in self.values.iter_mut().zip(other.values) {
            *v = add(*v, o)?;
        }
        self.total = add(self.total, other.total)?;
        self.ids.extend(other.ids);
        Ok(())
    }

    fn row(mut self, key: Key, label: String, currency: Currency) -> SeriesRow {
        self.ids.sort_unstable();
        SeriesRow {
            key,
            label,
            values: self
                .values
                .into_iter()
                .map(|v| Money::new(v, currency))
                .collect(),
            total: Money::new(self.total, currency),
            count: self.ids.len(),
            transaction_ids: self.ids,
        }
    }
}

/// Months from `first` to `month`.
fn month_index(first: YearMonth, month: YearMonth) -> usize {
    let months = |m: YearMonth| i64::from(m.year()) * 12 + i64::from(m.month());
    usize::try_from(months(month) - months(first)).unwrap_or(0)
}

/// Adds up `query.measure` for each calendar month of `query.period`, one row per
/// `query.group_by` key.
pub fn monthly(ledger: &Ledger, query: &Query) -> Result<Series, ViewError> {
    query.validate()?;
    let period = query.period;
    let scope = Scope::collect(ledger, &period, &query.filter)?;
    let months = period.year_months();
    let first = period.from.year_month();

    let mut rows: BTreeMap<Key, (String, Acc)> = BTreeMap::new();
    for item in &scope.items {
        if let Some(value) = item.value(query.measure)? {
            let (key, label) = scope.key(item, query.group_by)?;
            let acc = &mut rows
                .entry(key)
                .or_insert_with(|| (label, Acc::new(months.len())))
                .1;
            let cell = &mut acc.values[month_index(first, item.date.year_month())];
            *cell = add(*cell, value)?;
            acc.total = add(acc.total, value)?;
            acc.ids.push(item.id);
        }
    }

    let rows = rows.into_iter().map(|(k, (l, a))| (k, l, a)).collect();
    let (kept, rest) = rank(rows, query.group_by, query.top, |a| i128::from(a.total));
    let mut all = Acc::new(months.len());
    let mut other = Acc::new(months.len());
    for (_, _, a) in &kept {
        for (v, x) in all.values.iter_mut().zip(&a.values) {
            *v = add(*v, *x)?;
        }
        all.total = add(all.total, a.total)?;
    }
    let cut = !rest.is_empty();
    for (_, _, a) in rest {
        other.merge(a)?;
    }
    for (v, x) in all.values.iter_mut().zip(&other.values) {
        *v = add(*v, *x)?;
    }
    all.total = add(all.total, other.total)?;

    let c = scope.currency;
    let current = query.as_of.year_month();
    let columns = months
        .iter()
        .zip(&all.values)
        .map(|(&month, &total)| Column {
            month,
            partial: month >= current
                || period.from > month.first_day()
                || period.to < month.last_day(),
            total: Money::new(total, c),
        })
        .collect();

    Ok(Series {
        currency: c,
        period,
        measure: query.measure,
        group_by: query.group_by,
        columns,
        rows: kept.into_iter().map(|(k, l, a)| a.row(k, l, c)).collect(),
        other: cut.then(|| other.row(Key::Other, "Other".to_owned(), c)),
        total: Money::new(all.total, c),
    })
}

/// A row's values against its own average over the complete months.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrendStats {
    /// Columns that aren't partial.
    pub complete_months: usize,
    /// The values of the complete months, added up.
    pub complete_total: Money,
    /// `complete_total ÷ complete_months`, rounded half away from zero to the minor
    /// unit. `None` without a complete month.
    pub average: Option<Money>,
    /// The middle complete month's value (the mean of the middle two, rounded like
    /// `average`, for an even count). The planning baseline (SPEC decision 32) is the
    /// median over the last 6 complete months. `None` without a complete month.
    pub median: Option<Money>,
    /// The value in the last complete month.
    pub latest: Option<Money>,
    /// `latest − average`: positive when the last complete month was above average.
    pub latest_vs_average: Option<Money>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrendRow {
    pub key: Key,
    pub label: String,
    #[serde(flatten)]
    pub stats: TrendStats,
}

/// The result of [`trend`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Trend {
    pub series: Series,
    /// One per series row, in the same order.
    pub rows: Vec<TrendRow>,
    pub other: Option<TrendRow>,
    /// The column totals' trend.
    pub overall: TrendStats,
}

fn stats(values: &[Money], columns: &[Column], c: Currency) -> Result<TrendStats, ViewError> {
    let mut complete: Vec<i64> = values
        .iter()
        .zip(columns)
        .filter(|(_, col)| !col.partial)
        .map(|(v, _)| v.minor())
        .collect();
    let latest = complete.last().copied();
    let mut total = 0i64;
    for v in &complete {
        total = add(total, *v)?;
    }
    let months = complete.len();
    let average = match i128::try_from(months) {
        Ok(n) if n > 0 => Some(div_round(i128::from(total), n)?),
        _ => None,
    };
    complete.sort_unstable();
    let median = match months {
        0 => None,
        n if n % 2 == 1 => Some(complete[n / 2]),
        n => Some(div_round(
            i128::from(complete[n / 2 - 1]) + i128::from(complete[n / 2]),
            2,
        )?),
    };
    let latest_vs_average = match (latest, average) {
        (Some(l), Some(a)) => Some(l.checked_sub(a).ok_or(ViewError::Overflow)?),
        _ => None,
    };
    let money = |m: i64| Money::new(m, c);
    Ok(TrendStats {
        complete_months: months,
        complete_total: money(total),
        average: average.map(money),
        median: median.map(money),
        latest: latest.map(money),
        latest_vs_average: latest_vs_average.map(money),
    })
}

/// [`monthly`], plus each row's average over the complete months and how the last
/// complete month compares with it.
pub fn trend(ledger: &Ledger, query: &Query) -> Result<Trend, ViewError> {
    let series = monthly(ledger, query)?;
    let c = series.currency;
    let row = |r: &SeriesRow| -> Result<TrendRow, ViewError> {
        Ok(TrendRow {
            key: r.key.clone(),
            label: r.label.clone(),
            stats: stats(&r.values, &series.columns, c)?,
        })
    };
    let totals: Vec<Money> = series.columns.iter().map(|col| col.total).collect();
    Ok(Trend {
        rows: series.rows.iter().map(row).collect::<Result<_, _>>()?,
        other: series.other.as_ref().map(row).transpose()?,
        overall: stats(&totals, &series.columns, c)?,
        series,
    })
}
