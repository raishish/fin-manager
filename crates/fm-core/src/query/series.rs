//! `recurring` items: T3.2's series, read-only, named through its labeler hook.

use std::collections::HashSet;

use super::period::resolve;
use super::{
    CadenceSel, QueryError, SeriesKindSel, SeriesOutput, SeriesSelect, SeriesStatus, Value,
};
use crate::categorize::normalize::normalize;
use crate::contract::LedgerIndex;
use crate::model::{Bucket, Date, Money, TransactionId};
use crate::recurring::{Cadence, Direction, LabelKind, Series, SeriesAmount, SeriesKind};

/// A fixed-price series and its price steps (date of the first charge, price).
type PriceSteps<'s> = (&'s Series, &'s [(Date, Money)]);

/// The name a series is known by: its label, else its merchant key.
pub(super) fn name(s: &Series) -> &str {
    s.label.as_ref().map_or(&s.merchant_key, |l| &l.name)
}

/// Whether `s` is of the kind the user asked for. With a labeler, its label decides;
/// a charge it didn't label is neither a subscription nor a bill. Without one, fixed
/// logic decides, and a charge counts as both.
fn kind_matches(s: &Series, wanted: SeriesKindSel, labelled: bool) -> bool {
    match &s.label {
        Some(l) => matches!(
            (l.kind, wanted),
            (LabelKind::Subscription, SeriesKindSel::Subscription)
                | (LabelKind::Bill, SeriesKindSel::Bill)
                | (LabelKind::Income, SeriesKindSel::Income)
                | (LabelKind::Interest, SeriesKindSel::Interest)
                | (LabelKind::Transfer, SeriesKindSel::Transfer)
        ),
        None => {
            matches!(
                (s.kind, wanted),
                (
                    SeriesKind::Charge,
                    SeriesKindSel::Subscription | SeriesKindSel::Bill
                ) if !labelled
            ) || matches!(
                (s.kind, wanted),
                (SeriesKind::Income, SeriesKindSel::Income)
                    | (SeriesKind::Interest, SeriesKindSel::Interest)
                    | (SeriesKind::Transfer, SeriesKindSel::Transfer)
            )
        }
    }
}

fn cadence(c: CadenceSel) -> Cadence {
    match c {
        CadenceSel::Weekly => Cadence::Weekly,
        CadenceSel::Biweekly => Cadence::Biweekly,
        CadenceSel::Semimonthly => Cadence::Semimonthly,
        CadenceSel::Monthly => Cadence::Monthly,
        CadenceSel::Quarterly => Cadence::Quarterly,
        CadenceSel::Semiannual => Cadence::Semiannual,
        CadenceSel::Annual => Cadence::Annual,
    }
}

/// The member's amount as a positive cost (money out) or income (money in).
fn signed(s: &Series, m: Money) -> Result<i64, QueryError> {
    match s.direction {
        Direction::Out => m.minor().checked_neg().ok_or(QueryError::Overflow),
        Direction::In => Ok(m.minor()),
    }
}

fn add(a: i64, b: i64) -> Result<i64, QueryError> {
    a.checked_add(b).ok_or(QueryError::Overflow)
}

pub(super) fn answer(
    ix: &LedgerIndex<'_>,
    all: &[Series],
    select: &SeriesSelect,
    output: SeriesOutput,
    as_of: Date,
    labelled: bool,
    item: &str,
) -> Result<Value, QueryError> {
    let categories = select
        .categories
        .as_ref()
        .map(|keys| super::facts::category_ids(ix.ledger, keys))
        .transpose()?;
    let active: HashSet<String> = all
        .iter()
        .filter(|s| !s.ended)
        .map(|s| normalize(name(s)))
        .collect();
    let wanted = select.name.as_deref().map(normalize);
    let in_categories = |s: &Series| {
        let Some(ids) = &categories else {
            return true;
        };
        let inside = s
            .members
            .iter()
            .filter(|m| {
                matches!(
                    ix.bucket(**m),
                    Some(Bucket::Category { category_id } | Bucket::Refund { category_id, .. })
                        if ix.ledger.taxonomy.resolve(*category_id).is_some_and(|c| ids.contains(&c.id))
                )
            })
            .count();
        2 * inside > s.members.len()
    };
    let chosen: Vec<&Series> = all
        .iter()
        .filter(|s| wanted.as_ref().is_none_or(|w| normalize(name(s)) == *w))
        .filter(|s| {
            select
                .kinds
                .as_ref()
                .is_none_or(|ks| ks.iter().any(|k| kind_matches(s, *k, labelled)))
        })
        .filter(|s| {
            select
                .cadences
                .as_ref()
                .is_none_or(|cs| cs.iter().any(|c| cadence(*c) == s.cadence))
        })
        .filter(|s| match select.status {
            SeriesStatus::Active => !s.ended,
            SeriesStatus::Ended => s.ended,
            SeriesStatus::Cancelled => s.ended && !active.contains(&normalize(name(s))),
            SeriesStatus::Any => true,
        })
        .filter(|s| in_categories(s))
        .collect();

    let none = || QueryError::NothingFound(item.to_owned());
    let money = |minor: i64, ids: Vec<TransactionId>, like: Money| {
        let mut ids = ids;
        ids.sort_unstable();
        Value::Money {
            amount: Money::new(minor, like.currency()),
            transaction_ids: ids,
        }
    };
    // The single series a "latest" or "price" output reports: the most recent.
    let latest = || {
        chosen
            .iter()
            .copied()
            .max_by_key(|s| s.last)
            .ok_or_else(none)
    };
    let steps = || -> Result<PriceSteps<'_>, QueryError> {
        let s = latest()?;
        match &s.amount {
            SeriesAmount::Fixed { steps } if steps.len() >= 2 => Ok((s, steps)),
            _ => Err(none()),
        }
    };
    Ok(match output {
        SeriesOutput::Names => {
            let mut names: Vec<String> = chosen.iter().map(|s| name(s).to_owned()).collect();
            names.sort();
            names.dedup();
            Value::Texts { values: names }
        }
        SeriesOutput::Count => Value::Count {
            value: i64::try_from(chosen.len()).map_err(|_| QueryError::Overflow)?,
            transaction_ids: Vec::new(),
        },
        SeriesOutput::Latest => {
            let s = latest()?;
            let m = s.amount.latest();
            money(
                signed(s, m)?,
                s.members.last().copied().into_iter().collect(),
                m,
            )
        }
        SeriesOutput::LatestTotal => {
            let first = chosen.first().ok_or_else(none)?;
            let like = first.amount.latest();
            let mut total = 0i64;
            let mut ids = Vec::new();
            for s in &chosen {
                let m = s.amount.latest();
                if m.currency() != like.currency() {
                    return Err(QueryError::WrongKind(item.to_owned()));
                }
                total = add(total, signed(s, m)?)?;
                ids.extend(s.members.last().copied());
            }
            money(total, ids, like)
        }
        SeriesOutput::PriceFrom => {
            let (_, steps) = steps()?;
            Value::Date {
                value: steps[steps.len() - 1].0,
            }
        }
        SeriesOutput::PriceBefore | SeriesOutput::PriceAfter => {
            let (s, steps) = steps()?;
            let at = if output == SeriesOutput::PriceAfter {
                steps.len() - 1
            } else {
                steps.len() - 2
            };
            let m = steps[at].1;
            money(signed(s, m)?, Vec::new(), m)
        }
        SeriesOutput::MemberCount | SeriesOutput::MemberTotal => {
            let period = select
                .period
                .as_ref()
                .map(|p| resolve(p, as_of, Date::MIN))
                .transpose()?;
            let mut total = 0i64;
            let mut ids = Vec::new();
            let mut like = None;
            for s in &chosen {
                for m in &s.members {
                    let Some(t) = ix.transaction(*m) else {
                        continue;
                    };
                    let date = t.transaction_date.unwrap_or(t.posted_date);
                    if period.is_some_and(|p| !p.contains(date)) {
                        continue;
                    }
                    if like.is_some_and(|c: Money| c.currency() != t.amount.currency()) {
                        return Err(QueryError::WrongKind(item.to_owned()));
                    }
                    like = Some(t.amount);
                    total = add(total, signed(s, t.amount)?)?;
                    ids.push(*m);
                }
            }
            if output == SeriesOutput::MemberCount {
                ids.sort_unstable();
                Value::Count {
                    value: i64::try_from(ids.len()).map_err(|_| QueryError::Overflow)?,
                    transaction_ids: ids,
                }
            } else {
                // No members: zero in the ledger's first account currency.
                let like = like
                    .or_else(|| ix.ledger.accounts.first().map(|a| Money::zero(a.currency)))
                    .ok_or_else(none)?;
                money(total, ids, like)
            }
        }
    })
}
