use std::fmt;

use serde::Serialize;

use crate::model::{Date, Money, TransactionId};
use crate::views::ViewError;

/// One item's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub name: String,
    #[serde(flatten)]
    pub value: Value,
}

/// An answer's value. Money and counts list the transactions behind them, ascending
/// (empty for derived values), so every number traces back to the ledger.
///
/// Serialised with a `"kind"` tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Value {
    Money {
        amount: Money,
        transaction_ids: Vec<TransactionId>,
    },
    Count {
        value: i64,
        transaction_ids: Vec<TransactionId>,
    },
    Text {
        value: String,
    },
    Texts {
        values: Vec<String>,
    },
    Date {
        value: Date,
    },
    /// Basis points: 1 bp = 0.01%.
    Percent {
        basis_points: i64,
    },
}

/// Why a query couldn't be answered. Names items, keys and codes only, never amounts,
/// merchants or descriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// The query has no items, or two items share a name.
    BadItems,
    /// A category key that isn't in the taxonomy.
    UnknownCategory(String),
    /// A group key that isn't in the taxonomy.
    UnknownGroup(String),
    /// A currency code this build doesn't know.
    UnknownCurrency,
    /// A `derive` names an item that doesn't exist, or items refer to each other in a
    /// loop.
    BadReference(String),
    /// A `derive` or output got values of the wrong kind (or in two currencies).
    WrongKind(String),
    /// A ratio over a zero base.
    ZeroBase(String),
    /// `amounts: original` needs `original_currency`.
    OriginalNeedsCurrency,
    /// This measure can't be grouped that way (transfers have no category).
    GroupingNotApplicable,
    /// Nothing matched where one result was needed (a rank position past the end, a
    /// pick over no transactions, no such recurring series, or no price change).
    NothingFound(String),
    /// A period that ends before it starts or leaves the supported range.
    InvalidPeriod,
    /// A sum left the range of `i64` minor units.
    Overflow,
    /// The spending views refused (mixed currencies, an inconsistent ledger).
    View(ViewError),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadItems => f.write_str("the query needs items with distinct names"),
            Self::UnknownCategory(k) => write!(f, "unknown category {k:?}"),
            Self::UnknownGroup(k) => write!(f, "unknown category group {k:?}"),
            Self::UnknownCurrency => f.write_str("unknown currency code"),
            Self::BadReference(n) => write!(f, "item {n:?} refers to a missing item or a loop"),
            Self::WrongKind(n) => write!(f, "item {n:?} combines values of the wrong kind"),
            Self::ZeroBase(n) => write!(f, "item {n:?} divides by zero"),
            Self::OriginalNeedsCurrency => {
                f.write_str("original amounts need an original currency")
            }
            Self::GroupingNotApplicable => f.write_str("this measure can't be grouped that way"),
            Self::NothingFound(n) => write!(f, "item {n:?} found nothing"),
            Self::InvalidPeriod => f.write_str("invalid period"),
            Self::Overflow => f.write_str("a total is too large"),
            Self::View(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for QueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::View(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ViewError> for QueryError {
    fn from(e: ViewError) -> QueryError {
        QueryError::View(e)
    }
}
