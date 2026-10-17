//! Recurring charges, subscriptions, income and transfers (T3.2): series found by fixed
//! logic, named through a [`Labeler`] hook. See README.md.

mod detect;
mod key;
mod label;
#[cfg(test)]
mod tests;

pub use detect::{detect, detect_in};
pub use label::{Label, LabelKind, Labeler, SeriesSketch, label_all};

use crate::model::{AccountId, Date, Money, TransactionId};

/// How often a series comes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cadence {
    Weekly,
    Biweekly,
    /// Twice a month on about the same days (the 1st and the 15th).
    Semimonthly,
    Monthly,
    Quarterly,
    Semiannual,
    Annual,
}

impl Cadence {
    /// The usual number of days between occurrences.
    pub fn nominal_days(self) -> i32 {
        match self {
            Self::Weekly => 7,
            Self::Biweekly => 14,
            Self::Semimonthly => 15,
            Self::Monthly => 30,
            Self::Quarterly => 91,
            Self::Semiannual => 182,
            Self::Annual => 365,
        }
    }
}

/// Which way the money goes, seen from the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    Out,
    In,
}

/// What fixed logic can tell about a series. A [`Label`] says more (subscription or bill).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeriesKind {
    /// Money out to someone else: subscriptions and bills.
    Charge,
    /// Money in: pay, pensions, benefits.
    Income,
    /// Interest or dividends paid into an account.
    Interest,
    /// Money out to one of the user's own accounts.
    Transfer,
}

/// The amounts of a series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesAmount {
    /// The same amount each time, changing at each step (a price rise, a raise).
    Fixed { steps: Vec<(Date, Money)> },
    /// Different every time, within a range (utilities, interest).
    Variable {
        min: Money,
        max: Money,
        latest: Money,
    },
}

impl SeriesAmount {
    /// The amount of the latest occurrence.
    pub fn latest(&self) -> Money {
        match self {
            Self::Fixed { steps } => steps.last().expect("a series has members").1,
            Self::Variable { latest, .. } => *latest,
        }
    }
}

/// One recurring series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Series {
    /// The accounts it was paid from or into, in order of use (more than one when the
    /// user switched cards).
    pub accounts: Vec<AccountId>,
    pub direction: Direction,
    pub cadence: Cadence,
    pub amount: SeriesAmount,
    /// Members in date order.
    pub members: Vec<TransactionId>,
    /// Dates of the first and last members (purchase date when given, SPEC decision 42).
    pub first: Date,
    pub last: Date,
    /// When the next one is due, if the series hasn't ended.
    pub next_expected: Option<Date>,
    /// No occurrence for one and a half periods before the `as_of` date.
    pub ended: bool,
    pub kind: SeriesKind,
    /// The merchant key it was grouped by.
    pub merchant_key: String,
    /// Up to three distinct descriptions of its members.
    pub descriptions: Vec<String>,
    pub label: Option<Label>,
}
