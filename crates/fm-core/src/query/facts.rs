//! The transactions one item counts, with what the answer needs from each.
//!
//! Membership comes from the spending views (T3.1), so SPEC decision 42 lives in one
//! place: the views say which transactions are spending, refunds, income, transfers
//! or pending in the period. This module only narrows that further (description,
//! original currency, transfer counterpart) and signs each amount for the measure.

use std::collections::HashMap;

use super::{AccountSelect, Amounts, Measure, QueryError, Scope};
use crate::categorize::normalize::text_matches;
use crate::contract::LedgerIndex;
use crate::model::{
    Account, AccountId, Assignment, Bucket, Category, CategoryId, Currency, Date, Ledger,
    TextMatch, Transaction, TransactionId,
};
use crate::views::{self, Filter, GroupBy, Key, Period};

pub(super) struct Fact<'a> {
    pub id: TransactionId,
    /// Purchase date, else posted date (as the views count it).
    pub date: Date,
    /// What it adds to the measure, positive for money out and money in alike.
    pub value: i64,
    pub refund: bool,
    /// The category it counts toward, for spending and refunds.
    pub category: Option<&'a Category>,
    pub merchant: &'a str,
    pub account: &'a Account,
}

pub(super) struct Facts<'a> {
    pub currency: Currency,
    pub period: Period,
    pub list: Vec<Fact<'a>>,
}

/// The ids of the accounts `select` names.
pub(super) fn accounts(ledger: &Ledger, select: &AccountSelect) -> Vec<AccountId> {
    ledger
        .accounts
        .iter()
        .filter(|a| select.kinds.contains(&a.kind) || select.ids.contains(&a.id))
        .map(|a| a.id)
        .collect()
}

pub(super) fn category_ids(
    ledger: &Ledger,
    keys: &[String],
) -> Result<Vec<CategoryId>, QueryError> {
    keys.iter()
        .map(|k| {
            ledger
                .taxonomy
                .categories
                .iter()
                .find(|c| &c.key == k)
                .map(|c| c.id)
                .ok_or_else(|| QueryError::UnknownCategory(k.clone()))
        })
        .collect()
}

/// The merchant the views would show: the latest applied merchant decision, else the
/// raw description.
pub(super) fn merchant<'a>(ix: &LedgerIndex<'a>, t: &'a Transaction) -> &'a str {
    ix.applied(t.id)
        .iter()
        .rev()
        .find_map(|d| match &d.assignment {
            Assignment::Merchant(name) => Some(name.as_str()),
            Assignment::Bucket(_) => None,
        })
        .unwrap_or(&t.description)
}

pub(super) fn gather<'a>(
    ledger: &'a Ledger,
    ix: &LedgerIndex<'a>,
    measure: Measure,
    scope: &Scope,
    period: Period,
    as_of: Date,
) -> Result<Facts<'a>, QueryError> {
    let groups = scope
        .groups
        .as_ref()
        .map(|keys| {
            keys.iter()
                .map(|k| {
                    ledger
                        .taxonomy
                        .groups
                        .iter()
                        .find(|g| &g.key == k)
                        .map(|g| g.id)
                        .ok_or_else(|| QueryError::UnknownGroup(k.clone()))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let filter = Filter {
        accounts: scope.accounts.as_ref().map(|s| accounts(ledger, s)),
        categories: scope
            .categories
            .as_ref()
            .map(|keys| category_ids(ledger, keys))
            .transpose()?,
        groups,
        merchant: scope.merchant.clone(),
        currency: None,
    };
    let income = measure == Measure::Income;
    let b = views::breakdown(
        ledger,
        &views::Query {
            period,
            filter,
            group_by: if income {
                GroupBy::Month
            } else {
                GroupBy::Category
            },
            measure: if income {
                views::Measure::Income
            } else {
                views::Measure::Spending
            },
            top: None,
            as_of,
        },
    )?;

    // Which transactions, and the sign that makes money out positive.
    let t = &b.totals;
    let posted = || {
        [
            &t.spending,
            &t.income,
            &t.transfers_out,
            &t.transfers_in,
            &t.needs_review,
        ]
        .into_iter()
        .flat_map(|l| l.transaction_ids.iter().copied())
    };
    let refunds: std::collections::HashSet<TransactionId> =
        t.refunds.transaction_ids.iter().copied().collect();
    let (ids, negate): (Vec<TransactionId>, bool) = match measure {
        Measure::Spending => (t.spending.transaction_ids.clone(), true),
        Measure::Purchases => (
            t.spending
                .transaction_ids
                .iter()
                .copied()
                .filter(|id| !refunds.contains(id))
                .collect(),
            true,
        ),
        Measure::Refunds => (t.refunds.transaction_ids.clone(), false),
        Measure::Income => (t.income.transaction_ids.clone(), false),
        Measure::TransfersIn => (t.transfers_in.transaction_ids.clone(), false),
        Measure::TransfersOut => (t.transfers_out.transaction_ids.clone(), true),
        Measure::MoneyIn => (
            posted()
                .filter(|id| ix.transaction(*id).is_some_and(|x| x.amount.is_positive()))
                .collect(),
            false,
        ),
        Measure::MoneyOut => (
            posted()
                .filter(|id| ix.transaction(*id).is_some_and(|x| x.amount.is_negative()))
                .collect(),
            true,
        ),
        Measure::Pending => (t.pending.transaction_ids.clone(), true),
    };

    let category_of: HashMap<TransactionId, CategoryId> = b
        .rows
        .iter()
        .filter_map(|r| match r.key {
            Key::Category { id } => Some((id, &r.transaction_ids)),
            _ => None,
        })
        .flat_map(|(c, ids)| ids.iter().map(move |t| (*t, c)))
        .collect();
    let counterpart_accounts = scope
        .counterpart_accounts
        .as_ref()
        .map(|s| accounts(ledger, s));
    let description = scope.description.clone().map(TextMatch::Contains);
    let original = scope
        .original_currency
        .as_deref()
        .map(|code| Currency::new(code).map_err(|_| QueryError::UnknownCurrency))
        .transpose()?;
    let currency = match (scope.amounts, original) {
        (Amounts::Account, _) => b.currency,
        (Amounts::Original, Some(c)) => c,
        (Amounts::Original, None) => return Err(QueryError::OriginalNeedsCurrency),
    };

    let mut list = Vec::new();
    for id in ids {
        let Some(tx) = ix.transaction(id) else {
            continue;
        };
        if let Some(m) = &description
            && !text_matches(m, &tx.description)
        {
            continue;
        }
        let recorded = tx
            .original_amount
            .filter(|o| Some(o.currency()) == original);
        if original.is_some() && recorded.is_none() {
            continue;
        }
        if let Some(wanted) = &counterpart_accounts {
            let other = match ix.bucket(id) {
                Some(Bucket::Transfer {
                    counterpart: Some(c),
                }) => ix.transaction(*c).map(|o| o.account_id),
                _ => None,
            };
            if !other.is_some_and(|a| wanted.contains(&a)) {
                continue;
            }
        }
        let amount = match (scope.amounts, recorded) {
            (Amounts::Original, Some(o)) => o.minor(),
            _ => tx.amount.minor(),
        };
        let value = if negate {
            amount.checked_neg().ok_or(QueryError::Overflow)?
        } else {
            amount
        };
        let account = ix
            .account(tx.account_id)
            .ok_or(QueryError::View(views::ViewError::UnknownAccount(id)))?;
        list.push(Fact {
            id,
            date: tx.transaction_date.unwrap_or(tx.posted_date),
            value,
            refund: refunds.contains(&id),
            category: category_of.get(&id).and_then(|c| ix.category(*c)),
            merchant: merchant(ix, tx),
            account,
        });
    }
    list.sort_by_key(|f| f.id);
    Ok(Facts {
        currency,
        period,
        list,
    })
}
