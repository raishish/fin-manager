//! One period, rows by a grouping: "spending by category in March", "top 5 merchants
//! this year".

use std::collections::BTreeMap;

use serde::Serialize;

use super::rank::rank;
use super::scope::{Item, Kind, Scope, add};
use super::{GroupBy, Key, Measure, Period, Query, ViewError};
use crate::model::{Currency, Ledger, Money, TransactionId};

/// One row of a breakdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub key: Key,
    /// Display name: the category, group, merchant or account name, or `YYYY-MM`.
    pub label: String,
    /// The measure: net spending (`gross − refunds`), or income.
    pub total: Money,
    /// Spending before refunds, or income.
    pub gross: Money,
    /// Refunds filed to this row's categories (zero for income).
    pub refunds: Money,
    pub count: usize,
    /// Every transaction counted in this row, ascending.
    pub transaction_ids: Vec<TransactionId>,
}

/// A sum of transactions of one kind, with the transactions behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Line {
    pub amount: Money,
    pub count: usize,
    /// Ascending.
    pub transaction_ids: Vec<TransactionId>,
}

/// Every transaction in scope, by where its money went. Positive amounts except
/// `needs_review`, `pending` and `net_flow`, which keep their sign.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Totals {
    /// Spending net of refunds (spending and refund transactions).
    pub spending: Line,
    /// Refunds (part of `spending`, shown on their own).
    pub refunds: Line,
    pub income: Line,
    pub transfers_out: Line,
    pub transfers_in: Line,
    /// Posted transactions without one applied bucket, or in Needs review. Signed.
    pub needs_review: Line,
    /// Pending transactions: never in any other line or total. Signed.
    pub pending: Line,
    /// Every posted amount in scope, signed:
    /// `income + transfers_in − transfers_out − spending + needs_review`.
    pub net_flow: Money,
}

/// The result of [`breakdown`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Breakdown {
    pub currency: Currency,
    pub period: Period,
    pub measure: Measure,
    pub group_by: GroupBy,
    /// The period reaches the month of `as_of`, which isn't over yet.
    pub partial_month: bool,
    /// By `total`, largest first (by month: in calendar order).
    pub rows: Vec<Row>,
    /// The rows after the top `n`, added up. `None` when nothing was cut.
    pub other: Option<Row>,
    /// The measure over the whole scope; `rows` plus `other` add up to it.
    pub total: Money,
    pub totals: Totals,
}

/// Running sums for one row.
#[derive(Default)]
pub(super) struct Acc {
    pub total: i64,
    pub gross: i64,
    pub refunds: i64,
    pub ids: Vec<TransactionId>,
}

impl Acc {
    /// Adds an item worth `value` toward the measure.
    pub fn add(&mut self, item: &Item<'_>, value: i64) -> Result<(), ViewError> {
        self.total = add(self.total, value)?;
        if item.kind == Kind::Refund {
            self.refunds = add(self.refunds, item.amount)?;
        } else {
            self.gross = add(self.gross, value)?;
        }
        self.ids.push(item.id);
        Ok(())
    }

    pub fn merge(&mut self, other: Acc) -> Result<(), ViewError> {
        self.total = add(self.total, other.total)?;
        self.gross = add(self.gross, other.gross)?;
        self.refunds = add(self.refunds, other.refunds)?;
        self.ids.extend(other.ids);
        Ok(())
    }

    fn row(mut self, key: Key, label: String, currency: Currency) -> Row {
        self.ids.sort_unstable();
        Row {
            key,
            label,
            total: Money::new(self.total, currency),
            gross: Money::new(self.gross, currency),
            refunds: Money::new(self.refunds, currency),
            count: self.ids.len(),
            transaction_ids: self.ids,
        }
    }
}

#[derive(Default)]
struct LineAcc {
    sum: i64,
    ids: Vec<TransactionId>,
}

impl LineAcc {
    fn add(&mut self, id: TransactionId, amount: i64) -> Result<(), ViewError> {
        self.sum = add(self.sum, amount)?;
        self.ids.push(id);
        Ok(())
    }

    fn line(mut self, currency: Currency) -> Line {
        self.ids.sort_unstable();
        Line {
            amount: Money::new(self.sum, currency),
            count: self.ids.len(),
            transaction_ids: self.ids,
        }
    }
}

pub(super) fn totals(scope: &Scope<'_>) -> Result<Totals, ViewError> {
    let [
        mut spending,
        mut refunds,
        mut income,
        mut out,
        mut inflow,
        mut review,
        mut pending,
    ] = std::array::from_fn(|_| LineAcc::default());
    let mut net_flow = 0i64;
    for item in &scope.items {
        let (id, amount) = (item.id, item.amount);
        let negated = || amount.checked_neg().ok_or(ViewError::Overflow);
        if item.kind != Kind::Pending {
            net_flow = add(net_flow, amount)?;
        }
        match item.kind {
            Kind::Spending => spending.add(id, negated()?)?,
            Kind::Refund => {
                spending.add(id, negated()?)?;
                refunds.add(id, amount)?;
            }
            Kind::Income => income.add(id, amount)?,
            Kind::TransferOut => out.add(id, negated()?)?,
            Kind::TransferIn => inflow.add(id, amount)?,
            Kind::NeedsReview => review.add(id, amount)?,
            Kind::Pending => pending.add(id, amount)?,
        }
    }
    let c = scope.currency;
    Ok(Totals {
        spending: spending.line(c),
        refunds: refunds.line(c),
        income: income.line(c),
        transfers_out: out.line(c),
        transfers_in: inflow.line(c),
        needs_review: review.line(c),
        pending: pending.line(c),
        net_flow: Money::new(net_flow, c),
    })
}

/// Adds up `query.measure` over `query.period`, one row per `query.group_by` key.
pub fn breakdown(ledger: &Ledger, query: &Query) -> Result<Breakdown, ViewError> {
    query.validate()?;
    let scope = Scope::collect(ledger, &query.period, &query.filter)?;
    let mut rows: BTreeMap<Key, (String, Acc)> = BTreeMap::new();
    for item in &scope.items {
        if let Some(value) = item.value(query.measure)? {
            let (key, label) = scope.key(item, query.group_by)?;
            rows.entry(key)
                .or_insert_with(|| (label, Acc::default()))
                .1
                .add(item, value)?;
        }
    }

    let rows = rows.into_iter().map(|(k, (l, a))| (k, l, a)).collect();
    let (kept, rest) = rank(rows, query.group_by, query.top, |a| i128::from(a.total));
    let c = scope.currency;
    let mut total = 0i64;
    for (_, _, a) in kept.iter().chain(&rest) {
        total = add(total, a.total)?;
    }
    let other = if rest.is_empty() {
        None
    } else {
        let mut acc = Acc::default();
        for (_, _, a) in rest {
            acc.merge(a)?;
        }
        Some(acc.row(Key::Other, "Other".to_owned(), c))
    };

    Ok(Breakdown {
        currency: c,
        period: query.period,
        measure: query.measure,
        group_by: query.group_by,
        partial_month: query.period.is_partial(query.as_of),
        rows: kept.into_iter().map(|(k, l, a)| a.row(k, l, c)).collect(),
        other,
        total: Money::new(total, c),
        totals: totals(&scope)?,
    })
}
