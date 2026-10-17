//! Categorisation by fixed logic: merchant clean-up, the user's rules, and corrections
//! that become rules (T2.2). The model fallback (T2.5) runs after this, for what's left.
//!
//! ```text
//! import (T1.3) → transfers::detect (T1.4) → categorize::run (rules, clean-up) → model (T2.5)
//! ```
//!
//! # Who wins (SPEC constraint 2)
//! Every decision is about one assignment kind of one transaction: its **bucket** or its
//! **merchant name**. For each kind, sources rank:
//!
//! | Rank | Source | |
//! |---|---|---|
//! | 4 | `user` | A correction. Replaced only by a newer correction |
//! | 3 | `rule` | The user's rules, written or learned from corrections |
//! | 2 | `builtin`: `transfer_matcher`, `merchant_cleaner` | Fixed logic |
//! | 1 | `model` | The model fallback (T2.5) |
//! | 0 | `builtin`: `import_pipeline` | **The Needs-review placeholder**: every new transaction's first bucket, also written here for a transaction with no bucket at all (constraint 4) |
//!
//! A source never replaces a decision from a higher rank. Two refinements:
//! - **Transfer pairs** (a transfer with a counterpart) are replaced only by the user:
//!   a rule that matches one side would break the pair. The transfer matcher's pair may
//!   replace an unpaired transfer from a rule or the model: it adds the other side.
//! - **Seen decisions** (SPEC constraint 7): once the user has seen a decision
//!   (`seen_at`), a change from `builtin` or `model` is written as a **suggestion**, not
//!   applied. A change from `user` or `rule` is the user's own doing and applies: that's
//!   how a correction re-applies to history the user has looked at.
//!
//! The policy is [`plan::decide`], shared with T2.5. Decisions are never edited: a change
//! is a new decision that `replaces` the old one, which becomes `superseded`.
//!
//! # Rules
//! - **Matching** ([`normalize`]): case, punctuation and spacing are ignored, and a
//!   pattern matches whole words only (`TARGET` doesn't match `STARGET`). An account and
//!   a signed amount (or range) narrow it further.
//! - **Choosing** ([`rules::winners`]): among matching enabled rules, the highest
//!   priority wins, then the newest. The bucket and the merchant name are chosen
//!   separately, so a merchant-only rule and a category rule can both apply.
//! - **Fitting** ([`rules::fit`]): a category rule files the merchant's money coming in
//!   as a refund of that category; an income rule never files money going out (the
//!   transaction stays as it is, counted in [`RunReport::rule_did_not_fit`]). Rules
//!   never name a transfer counterpart: "ROBINHOOD → transfer" files every Robinhood
//!   deposit as a transfer to an untracked own account.
//! - A disabled or deleted rule stops matching. What it already decided stays until
//!   something stronger replaces it.
//!
//! # Merchant clean-up ([`merchant`])
//! `builtin/merchant_cleaner` at [`merchant::VERSION`], from the description alone. The
//! raw description is never changed. A rule's merchant name ranks above it.
//!
//! # Corrections ([`correct`])
//! A correction is a `user` decision. Unless asked not to, it also becomes a rule
//! (origin `correction`) matching the merchant's first words ([`correct::learned_matcher`]),
//! and the rules are re-applied to every transaction it matches, so the same merchant's past and
//! future transactions follow. Deleting the transaction's import deletes the rule
//! (storage cascades it, SPEC decision 28).
//!
//! # Entry points
//! - [`run`]: plan and store over the encrypted ledger, in one savepoint.
//! - [`correct::correct`]: a correction, its rule and the re-run, in one savepoint.
//! - [`plan::plan`], [`run_on_ledger`], [`correct::correct_ledger`]: the same over an
//!   in-memory [`Ledger`](crate::model::Ledger), for fixtures and evals.
//! - [`actions`]: turning a rule on or off, deleting it, undoing what it decided, and
//!   accepting or dismissing a suggestion (both sides of a transfer pair together).
//!   A dismissal sticks: fixed logic and the model never bring the same assignment
//!   back for that transaction.
//! - [`actions::confirm_decisions`] and [`actions::reject_decisions`]: the review
//!   queue's bulk approve and reject (SPEC decision 46: a decision counts as seen only
//!   once the user acts on it). A rejection sticks like a dismissal.

pub mod actions;
pub mod apply;
pub mod correct;
pub mod merchant;
pub mod normalize;
pub mod plan;
pub mod rules;

#[cfg(test)]
mod td1_eval;
#[cfg(test)]
mod tests;

pub use actions::{
    accept_suggestion, confirm_decisions, delete_rule, dismiss_suggestion, reject_decisions,
    set_rule_enabled, undo_rule_decisions,
};
pub use apply::{CategorizeError, apply_to_ledger, run, run_on_ledger};
pub use correct::{Correction, CorrectionReport, Learn, correct, correct_ledger};
pub use plan::{LedgerView, Plan, RunReport, Scope};
