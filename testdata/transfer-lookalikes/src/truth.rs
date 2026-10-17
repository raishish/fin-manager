//! The truth file's types. `README.md` documents them.

use fm_core::model::Date;
use fm_synth::truth::TruthBucket;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TxTruth {
    pub id: i64,
    pub bucket: TruthBucket,
    /// The scenario the row belongs to (see README), `background` for everyday rows.
    pub pattern: String,
    pub merchant: Option<String>,
}

/// A true transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pair {
    pub out: i64,
    #[serde(rename = "in")]
    pub into: i64,
    /// `own_transfer`, `card_payment` or `card_to_card`.
    pub kind: String,
    pub pattern: String,
    /// `strong` (both sides say transfer or name the other account), `one_sided` or `none`.
    pub wording: String,
    pub lag_days: i32,
}

/// A same-amount, opposite-sign, near-date pair across own accounts that is NOT a transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub out: i64,
    #[serde(rename = "in")]
    pub into: i64,
    pub pattern: String,
    pub lag_days: i32,
    /// `p2p` (either side is a person-to-person payment), `leaves_card` (money out of a
    /// card). Informational: some matchers rule these out before scoring.
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HouseholdTruth {
    pub household: String,
    pub description: String,
    pub start: Date,
    pub end: Date,
    pub candidate_window_days: i32,
    /// One entry per ledger transaction, in the same order.
    pub transactions: Vec<TxTruth>,
    pub transfer_pairs: Vec<Pair>,
    /// Every candidate pair in the ledger that is not a transfer.
    pub lookalikes: Vec<Candidate>,
}
