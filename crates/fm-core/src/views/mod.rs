//! Spending views (T3.1): spending and income by category, group, merchant, month or
//! account; monthly series and trends; period-over-period changes.
//!
//! Pure functions over a [`Ledger`](crate::model::Ledger): no storage, no clock, no
//! floating point. The same ledger and [`Query`] always give the same numbers (SPEC
//! constraint 1). Structured questions (T4.1) are calls into these functions, and the
//! dashboard (T3.3) renders their results, so every result is `Serialize`.
//!
//! ```text
//! breakdown(ledger, query)            one period, rows by query.group_by
//! monthly(ledger, query)              one column per calendar month of the period
//! trend(ledger, query)                monthly, plus each row's average over complete months
//! compare(ledger, query, base)        query.period against an earlier base period
//! month_over_month(ledger, query)     compare with the month before (query.period is one month)
//! ```
//!
//! # Counting rules (SPEC decision 42)
//! 1. A transaction counts on its **purchase date** when the bank gives one, otherwise
//!    its **posted date**.
//! 2. **Posted transactions only.** Pending ones are a separate line
//!    ([`Totals::pending`]) and never in a total.
//! 3. Spending in a category is **net of refunds** filed to that category, so it can be
//!    negative in a month where a refund lands after its purchase.
//! 4. **Transfers never count** as spending or income; they are their own lines, for
//!    reconciliation. Income is the Income bucket only.
//! 5. Amounts are in the **account's currency**, and one result is in one currency: a
//!    query whose accounts hold more than one must set [`Filter::currency`].
//!    [`crate::model::Transaction::original_amount`] stays on the transaction for
//!    foreign-purchase questions.
//!
//! # Where the numbers come from
//! - **Bucket:** the transaction's one applied bucket decision. None, or more than one,
//!   counts as **Needs review**, which is its own line and never folded into a category
//!   (SPEC constraint 4).
//! - **Category:** a merged-away category counts toward the one it was merged into
//!   ([`Taxonomy::resolve`](crate::model::Taxonomy::resolve)). Hidden categories still
//!   count: hiding only removes them from pickers.
//! - **Merchant:** the applied merchant decision (the latest, if there are several), else
//!   the raw description, marked `from_description`.
//! - Every row lists its `transaction_ids`, so each number can be traced to the
//!   transactions behind it.
//!
//! # Adding up
//! Over the transactions in scope, `totals.net_flow` (every posted amount, signed)
//! equals `income + transfers_in − transfers_out − spending + needs_review`. The rows
//! plus `other` add up to the view's `total`.
//!
//! Errors name ids and codes only, never amounts, merchants or descriptions.

mod breakdown;
mod compare;
mod error;
mod query;
mod rank;
mod scope;
mod series;
#[cfg(test)]
mod td1_tests;
#[cfg(test)]
mod tests;

pub use breakdown::{Breakdown, Line, Row, Totals, breakdown};
pub use compare::{Change, Comparison, Delta, Ratio, compare, month_over_month};
pub use error::ViewError;
pub use query::{Filter, GroupBy, Measure, Period, Query};
pub use scope::Key;
pub use series::{Column, Series, SeriesRow, Trend, TrendRow, TrendStats, monthly, trend};
