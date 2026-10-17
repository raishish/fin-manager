//! Reads the ledger for one period and filter: which transactions are in scope, the
//! date each counts on, and where its money goes. Every view starts here.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::{Filter, GroupBy, Measure, Period, ViewError};
use crate::categorize::normalize::normalize;
use crate::contract::LedgerIndex;
use crate::model::{
    Account, AccountId, Assignment, Bucket, Category, CategoryGroup, CategoryGroupId, CategoryId,
    Currency, Date, Ledger, TransactionId, TransactionStatus, YearMonth,
};

/// What a row is about.
///
/// Serialised with a `"kind"` tag, e.g. `{"kind": "category", "id": 12}` or
/// `{"kind": "month", "month": "2025-03"}`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Key {
    Category {
        id: CategoryId,
    },
    Group {
        id: CategoryGroupId,
    },
    /// `from_description` is set when no merchant decision applies and the raw
    /// description stands in for the name.
    Merchant {
        name: String,
        from_description: bool,
    },
    Month {
        month: YearMonth,
    },
    Account {
        id: AccountId,
    },
    /// Everything after the top `n` rows, added up.
    Other,
}

/// Where a transaction's money goes in the views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Spending,
    Refund,
    Income,
    TransferOut,
    TransferIn,
    NeedsReview,
    Pending,
}

pub(super) struct Item<'a> {
    pub id: TransactionId,
    /// Purchase date, else posted date.
    pub date: Date,
    /// Signed minor units in the scope's currency.
    pub amount: i64,
    pub kind: Kind,
    /// The category it counts toward (after merges), for spending and refunds; also
    /// for a pending transaction that already has one.
    pub category: Option<&'a Category>,
    pub account: &'a Account,
    pub merchant: &'a str,
    pub merchant_from_description: bool,
}

impl Item<'_> {
    /// What this transaction adds to `measure`, or `None` if it isn't part of it.
    pub fn value(&self, measure: Measure) -> Result<Option<i64>, ViewError> {
        Ok(match (measure, self.kind) {
            (Measure::Spending, Kind::Spending | Kind::Refund) => {
                Some(self.amount.checked_neg().ok_or(ViewError::Overflow)?)
            }
            (Measure::Income, Kind::Income) => Some(self.amount),
            _ => None,
        })
    }
}

pub(super) struct Scope<'a> {
    pub currency: Currency,
    pub items: Vec<Item<'a>>,
    groups: HashMap<CategoryGroupId, &'a CategoryGroup>,
}

impl<'a> Scope<'a> {
    pub fn collect(
        ledger: &'a Ledger,
        period: &Period,
        filter: &Filter,
    ) -> Result<Scope<'a>, ViewError> {
        period.validate()?;
        let ix = LedgerIndex::new(ledger);

        let accounts: HashSet<AccountId> = ledger
            .accounts
            .iter()
            .filter(|a| {
                filter
                    .accounts
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&a.id))
            })
            .filter(|a| filter.currency.is_none_or(|c| a.currency == c))
            .map(|a| a.id)
            .collect();
        let currency = match filter.currency {
            Some(c) => c,
            None => {
                let mut currencies = ledger
                    .accounts
                    .iter()
                    .filter(|a| accounts.contains(&a.id))
                    .map(|a| a.currency);
                let first = currencies.next().ok_or(ViewError::NoAccounts)?;
                if currencies.any(|c| c != first) {
                    return Err(ViewError::MixedCurrency);
                }
                first
            }
        };

        let taxonomy = &ledger.taxonomy;
        let resolved = |id: CategoryId, t: TransactionId| {
            taxonomy.resolve(id).ok_or(ViewError::UnknownCategory(t))
        };
        let categories: Option<HashSet<CategoryId>> = filter.categories.as_ref().map(|ids| {
            ids.iter()
                .filter_map(|id| taxonomy.resolve(*id))
                .map(|c| c.id)
                .collect()
        });
        let merchant = filter.merchant.as_deref().map(normalize);

        let mut items = Vec::new();
        for t in &ledger.transactions {
            let account = ix
                .account(t.account_id)
                .ok_or(ViewError::UnknownAccount(t.id))?;
            if !accounts.contains(&account.id) {
                continue;
            }
            if t.amount.currency() != account.currency {
                return Err(ViewError::AmountCurrency(t.id));
            }
            let date = t.transaction_date.unwrap_or(t.posted_date);
            if !period.contains(date) {
                continue;
            }

            let named = ix
                .applied(t.id)
                .iter()
                .rev()
                .find_map(|d| match &d.assignment {
                    Assignment::Merchant(name) => Some(name.as_str()),
                    Assignment::Bucket(_) => None,
                });
            let name = named.unwrap_or(&t.description);
            if let Some(wanted) = &merchant
                && (wanted.is_empty() || normalize(name) != *wanted)
            {
                continue;
            }

            let bucket = ix.bucket(t.id);
            let (kind, category) = match bucket {
                _ if t.status == TransactionStatus::Pending => {
                    let category = match bucket {
                        Some(
                            Bucket::Category { category_id } | Bucket::Refund { category_id, .. },
                        ) => Some(resolved(*category_id, t.id)?),
                        _ => None,
                    };
                    (Kind::Pending, category)
                }
                None | Some(Bucket::NeedsReview) => (Kind::NeedsReview, None),
                Some(Bucket::Category { category_id }) => {
                    (Kind::Spending, Some(resolved(*category_id, t.id)?))
                }
                Some(Bucket::Refund { category_id, .. }) => {
                    (Kind::Refund, Some(resolved(*category_id, t.id)?))
                }
                Some(Bucket::Income) => (Kind::Income, None),
                Some(Bucket::Transfer { .. }) if t.amount.is_negative() => {
                    (Kind::TransferOut, None)
                }
                Some(Bucket::Transfer { .. }) => (Kind::TransferIn, None),
            };

            if filter.categories.is_some() || filter.groups.is_some() {
                let Some(c) = category else { continue };
                if categories.as_ref().is_some_and(|ids| !ids.contains(&c.id))
                    || filter
                        .groups
                        .as_ref()
                        .is_some_and(|ids| !ids.contains(&c.group_id))
                {
                    continue;
                }
            }

            items.push(Item {
                id: t.id,
                date,
                amount: t.amount.minor(),
                kind,
                category,
                account,
                merchant: name,
                merchant_from_description: named.is_none(),
            });
        }

        Ok(Scope {
            currency,
            items,
            groups: taxonomy.groups.iter().map(|g| (g.id, g)).collect(),
        })
    }

    /// The row an item belongs to under `by`, with its display label. Only called for
    /// items that count toward the measure, so spending items always have a category.
    pub fn key(&self, item: &Item<'_>, by: GroupBy) -> Result<(Key, String), ViewError> {
        let category = || item.category.ok_or(ViewError::GroupingNotApplicable);
        Ok(match by {
            GroupBy::Category => {
                let c = category()?;
                (Key::Category { id: c.id }, c.name.clone())
            }
            GroupBy::Group => {
                let c = category()?;
                let g = self
                    .groups
                    .get(&c.group_id)
                    .ok_or(ViewError::UnknownGroup(c.group_id))?;
                (Key::Group { id: g.id }, g.name.clone())
            }
            GroupBy::Merchant => (
                Key::Merchant {
                    name: item.merchant.to_owned(),
                    from_description: item.merchant_from_description,
                },
                item.merchant.to_owned(),
            ),
            GroupBy::Month => {
                let month = item.date.year_month();
                (Key::Month { month }, month.to_string())
            }
            GroupBy::Account => (
                Key::Account {
                    id: item.account.id,
                },
                item.account.name.clone(),
            ),
        })
    }
}

pub(super) fn add(a: i64, b: i64) -> Result<i64, ViewError> {
    a.checked_add(b).ok_or(ViewError::Overflow)
}

/// `n / d` rounded half away from zero; `d` must be positive.
pub(super) fn div_round(n: i128, d: i128) -> Result<i64, ViewError> {
    let (q, r) = (n / d, n % d);
    let q = if 2 * r.abs() >= d { q + n.signum() } else { q };
    i64::try_from(q).map_err(|_| ViewError::Overflow)
}
