//! One period against another: "how did September compare with August", "Q2 this
//! year against Q2 last year".

use std::collections::BTreeMap;

use serde::Serialize;

use super::breakdown::Acc;
use super::rank::rank;
use super::scope::{Scope, add, div_round};
use super::{GroupBy, Key, Measure, Period, Query, ViewError};
use crate::model::{Currency, Ledger, Money, TransactionId};

/// `change ÷ |base|`, exactly, plus the same rounded to whole basis points (1 bp =
/// 0.01%), half away from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Ratio {
    pub numerator: i64,
    pub denominator: i64,
    pub basis_points: i64,
}

/// The measure in both periods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Delta {
    pub current: Money,
    pub base: Money,
    /// `current − base`.
    pub change: Money,
    /// `None` when `base` is zero.
    pub ratio: Option<Ratio>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub key: Key,
    pub label: String,
    #[serde(flatten)]
    pub delta: Delta,
    /// Ascending.
    pub current_ids: Vec<TransactionId>,
    /// Ascending.
    pub base_ids: Vec<TransactionId>,
}

/// The result of [`compare`] and [`month_over_month`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Comparison {
    pub currency: Currency,
    pub measure: Measure,
    pub group_by: GroupBy,
    pub current: Period,
    pub base: Period,
    /// The current period reaches the month of `as_of`, which isn't over yet: a
    /// comparison of part of a month with a whole one.
    pub current_partial: bool,
    pub base_partial: bool,
    /// By size of change (either way), largest first (by month: in calendar order).
    /// Keys in either period appear.
    pub rows: Vec<Change>,
    /// The rows after the top `n`, added up. `None` when nothing was cut.
    pub other: Option<Change>,
    pub total: Delta,
}

fn delta(current: i64, base: i64, c: Currency) -> Result<Delta, ViewError> {
    let change = current.checked_sub(base).ok_or(ViewError::Overflow)?;
    let ratio = if base == 0 {
        None
    } else {
        let denominator = base.checked_abs().ok_or(ViewError::Overflow)?;
        Some(Ratio {
            numerator: change,
            denominator,
            basis_points: div_round(i128::from(change) * 10_000, i128::from(denominator))?,
        })
    };
    Ok(Delta {
        current: Money::new(current, c),
        base: Money::new(base, c),
        change: Money::new(change, c),
        ratio,
    })
}

fn change(
    key: Key,
    label: String,
    (cur, base): (Acc, Acc),
    c: Currency,
) -> Result<Change, ViewError> {
    let sorted = |mut ids: Vec<TransactionId>| {
        ids.sort_unstable();
        ids
    };
    Ok(Change {
        key,
        label,
        delta: delta(cur.total, base.total, c)?,
        current_ids: sorted(cur.ids),
        base_ids: sorted(base.ids),
    })
}

/// Adds up `query.measure` over `query.period` and over `base`, one row per
/// `query.group_by` key, with the change between them.
pub fn compare(ledger: &Ledger, query: &Query, base: Period) -> Result<Comparison, ViewError> {
    query.validate()?;
    let scopes = [
        Scope::collect(ledger, &query.period, &query.filter)?,
        Scope::collect(ledger, &base, &query.filter)?,
    ];
    let mut rows: BTreeMap<Key, (String, (Acc, Acc))> = BTreeMap::new();
    for (side, scope) in scopes.iter().enumerate() {
        for item in &scope.items {
            if let Some(value) = item.value(query.measure)? {
                let (key, label) = scope.key(item, query.group_by)?;
                let pair = &mut rows
                    .entry(key)
                    .or_insert_with(|| (label, Default::default()))
                    .1;
                let acc = if side == 0 { &mut pair.0 } else { &mut pair.1 };
                acc.add(item, value)?;
            }
        }
    }

    let rows = rows.into_iter().map(|(k, (l, a))| (k, l, a)).collect();
    let (kept, rest) = rank(
        rows,
        query.group_by,
        query.top,
        |(cur, base): &(Acc, Acc)| (i128::from(cur.total) - i128::from(base.total)).abs(),
    );
    let (mut all_cur, mut all_base) = (0i64, 0i64);
    for (_, _, (cur, base)) in kept.iter().chain(&rest) {
        all_cur = add(all_cur, cur.total)?;
        all_base = add(all_base, base.total)?;
    }
    let c = scopes[0].currency;
    let other = if rest.is_empty() {
        None
    } else {
        let (mut cur, mut old) = (Acc::default(), Acc::default());
        for (_, _, (a, b)) in rest {
            cur.merge(a)?;
            old.merge(b)?;
        }
        Some(change(Key::Other, "Other".to_owned(), (cur, old), c)?)
    };

    Ok(Comparison {
        currency: c,
        measure: query.measure,
        group_by: query.group_by,
        current: query.period,
        base,
        current_partial: query.period.is_partial(query.as_of),
        base_partial: base.is_partial(query.as_of),
        rows: kept
            .into_iter()
            .map(|(k, l, pair)| change(k, l, pair, c))
            .collect::<Result<_, _>>()?,
        other,
        total: delta(all_cur, all_base, c)?,
    })
}

/// [`compare`] `query.period`, which must be one calendar month, with the month before.
pub fn month_over_month(ledger: &Ledger, query: &Query) -> Result<Comparison, ViewError> {
    let month = query.period.from.year_month();
    if query.period != Period::month(month) {
        return Err(ViewError::NotOneMonth);
    }
    let previous = month
        .checked_add_months(-1)
        .ok_or(ViewError::InvalidPeriod)?;
    compare(ledger, query, Period::month(previous))
}
