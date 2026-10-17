//! Structured queries (T4.1): a typed description of a question, a JSON schema for it,
//! and an executor over the ledger.
//!
//! ```text
//! question → T4.2 model (constrained by schema.json) → Query → run(ledger, query, ctx) → answers
//!                                                        ↑ shown to the user as interpreted
//! ```
//!
//! A [`Query`] is a list of named items, and each item gives exactly one answer: money,
//! a count, a text, a list of texts, a date or a percent. Money and counts carry the
//! transaction ids behind them, so T4.3 can check every number against the ledger
//! (SPEC constraint 6) and the UI can show its sources.
//!
//! - **Counting rules:** SPEC decision 42, through the spending views (T3.1). This
//!   module asks the views which transactions are spending, refunds, income, transfers
//!   or pending in a period, and only narrows further (description, original currency,
//!   transfer counterpart).
//! - **Recurring:** T3.2's series, read-only, named by its
//!   [`Labeler`](crate::recurring::Labeler) hook when the app has one.
//! - **Today:** `as_of` comes from the app in the [`Context`], never from the model;
//!   relative periods ("last month") resolve against it.
//! - **Stable names:** categories and groups by key (`food.groceries`), so a model's
//!   query means the same in every install.
//! - **No floats.** Averages, medians and percents round half away from zero, to the
//!   minor unit or the basis point.
//!
//! Errors name items, keys and codes only, never amounts, merchants or descriptions.
//! See README.md for the language, and `schema.json` for its JSON schema.

mod answer;
mod exec;
mod facts;
mod period;
#[cfg(test)]
mod schema_tests;
mod series;
mod spec;
#[cfg(test)]
mod tests;

pub use answer::{Answer, QueryError, Value};
pub use exec::{Context, run};
pub use spec::{
    AccountSelect, Amounts, Ask, By, CadenceSel, DeriveOp, Item, Measure, Metric, Order, Per,
    PeriodSpec, PickOrder, PickOutput, Query, RankOutput, Scope, SeriesKindSel, SeriesOutput,
    SeriesSelect, SeriesStatus,
};

/// The JSON schema of [`Query`] (draft 2020-12), for constrained model output (T4.2).
pub const SCHEMA: &str = include_str!("schema.json");
