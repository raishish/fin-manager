//! What a view is asked for: a period, filters, a grouping and a measure.

use serde::{Deserialize, Serialize};

use super::ViewError;
use crate::model::{AccountId, CategoryGroupId, CategoryId, Currency, Date, YearMonth};

/// An inclusive range of dates, matched against each transaction's counting date
/// (purchase date when the bank gives one, else posted date).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Period {
    pub from: Date,
    pub to: Date,
}

impl Period {
    pub fn new(from: Date, to: Date) -> Result<Period, ViewError> {
        let period = Period { from, to };
        period.validate()?;
        Ok(period)
    }

    /// One calendar month.
    pub fn month(month: YearMonth) -> Period {
        Period {
            from: month.first_day(),
            to: month.last_day(),
        }
    }

    /// Whole calendar months from `first` to `last`, inclusive.
    pub fn months(first: YearMonth, last: YearMonth) -> Result<Period, ViewError> {
        Period::new(first.first_day(), last.last_day())
    }

    /// One calendar year.
    pub fn year(year: i32) -> Result<Period, ViewError> {
        let month = |m| YearMonth::new(year, m).map_err(|_| ViewError::InvalidPeriod);
        Period::months(month(1)?, month(12)?)
    }

    pub fn contains(&self, date: Date) -> bool {
        self.from <= date && date <= self.to
    }

    /// The calendar months the period touches, in order.
    pub fn year_months(&self) -> Vec<YearMonth> {
        let last = self.to.year_month();
        let mut months = Vec::new();
        let mut month = Some(self.from.year_month());
        while let Some(m) = month.filter(|m| *m <= last) {
            months.push(m);
            month = m.checked_add_months(1);
        }
        months
    }

    /// Whether the period reaches into the month of `as_of` (or later), which isn't
    /// over yet, so its numbers will still grow.
    pub fn is_partial(&self, as_of: Date) -> bool {
        self.to >= as_of.year_month().first_day()
    }

    pub(super) fn validate(&self) -> Result<(), ViewError> {
        if self.from <= self.to {
            Ok(())
        } else {
            Err(ViewError::InvalidPeriod)
        }
    }
}

/// Which transactions a view looks at. Every field narrows; the default keeps
/// everything. An empty list matches nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Filter {
    /// Only transactions on these accounts.
    pub accounts: Option<Vec<AccountId>>,
    /// Only spending and refunds in these categories (a merged-away id counts as the
    /// category it was merged into). Leaves out income, transfers and Needs review.
    pub categories: Option<Vec<CategoryId>>,
    /// Only spending and refunds in categories of these groups. Like `categories`.
    pub groups: Option<Vec<CategoryGroupId>>,
    /// Only transactions whose merchant has this name. Compared after the rules
    /// engine's [normalisation](crate::categorize::normalize::normalize), so case,
    /// punctuation and spacing don't matter: `"h-e-b"` matches `H-E-B`.
    pub merchant: Option<String>,
    /// Only accounts held in this currency. Required when the accounts in scope hold
    /// more than one.
    pub currency: Option<Currency>,
}

/// How a view's rows are keyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    Category,
    Group,
    Merchant,
    Month,
    Account,
}

/// What a view adds up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    /// Spending net of refunds, as a positive amount: money out in a category, less
    /// refunds filed to it.
    #[default]
    Spending,
    /// The Income bucket, as a positive amount.
    Income,
}

/// A view's question: what to add up, over which transactions, keyed how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub period: Period,
    #[serde(default)]
    pub filter: Filter,
    pub group_by: GroupBy,
    #[serde(default)]
    pub measure: Measure,
    /// Keep the `n` largest rows and add the rest up into one `other` row, so the
    /// total still adds up ("top 5 merchants"). `None` keeps every row.
    #[serde(default)]
    pub top: Option<usize>,
    /// The day the view is for ("today"). Passed in, never read from the clock, so a
    /// view is reproducible. A period reaching this day's month is flagged partial.
    pub as_of: Date,
}

impl Query {
    pub(super) fn validate(&self) -> Result<(), ViewError> {
        self.period.validate()?;
        if self.measure == Measure::Income
            && matches!(self.group_by, GroupBy::Category | GroupBy::Group)
        {
            return Err(ViewError::GroupingNotApplicable);
        }
        Ok(())
    }
}
