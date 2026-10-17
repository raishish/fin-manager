use std::fmt;

use crate::model::{CategoryGroupId, TransactionId};

/// Why a view couldn't be computed. Carries ids and codes only, never amounts,
/// merchants or descriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewError {
    /// The period ends before it starts, or a year outside the supported range.
    InvalidPeriod,
    /// `month_over_month` needs a period of exactly one calendar month.
    NotOneMonth,
    /// No account is in scope, so there's no currency to report in.
    NoAccounts,
    /// The accounts in scope hold more than one currency; set `filter.currency`.
    MixedCurrency,
    /// Income has no category, so it can't be grouped by category or group.
    GroupingNotApplicable,
    /// A transaction is on an account that isn't in the ledger.
    UnknownAccount(TransactionId),
    /// A transaction's amount isn't in its account's currency.
    AmountCurrency(TransactionId),
    /// A transaction's bucket names a category that isn't in the taxonomy.
    UnknownCategory(TransactionId),
    /// A category is in a group that isn't in the taxonomy.
    UnknownGroup(CategoryGroupId),
    /// A sum left the range of `i64` minor units.
    Overflow,
}

impl fmt::Display for ViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPeriod => f.write_str("the period ends before it starts"),
            Self::NotOneMonth => f.write_str("the period is not one calendar month"),
            Self::NoAccounts => f.write_str("no account is in scope"),
            Self::MixedCurrency => {
                f.write_str("the accounts in scope hold more than one currency; choose one")
            }
            Self::GroupingNotApplicable => f.write_str("income can't be grouped by category"),
            Self::UnknownAccount(t) => write!(f, "transaction {} is on an unknown account", t.0),
            Self::AmountCurrency(t) => {
                write!(f, "transaction {} is not in its account's currency", t.0)
            }
            Self::UnknownCategory(t) => {
                write!(f, "transaction {} is filed to an unknown category", t.0)
            }
            Self::UnknownGroup(g) => write!(f, "category group {} is unknown", g.0),
            Self::Overflow => f.write_str("a total is too large"),
        }
    }
}

impl std::error::Error for ViewError {}
