//! Ground-truth file types. `README.md` documents the JSON form.

use fm_core::model::{AccountId, Date, Money, TaxonomyVersion, TransactionId};
use serde::{Deserialize, Serialize};

/// The correct bucket for a transaction. Like `fm_core::model::Bucket`, but categories
/// are named by their stable key, so the truth doesn't depend on category ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TruthBucket {
    Category {
        key: String,
    },
    Transfer {
        counterpart: Option<TransactionId>,
    },
    Income,
    Refund {
        key: String,
        original: Option<TransactionId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingTruth {
    pub amount: Money,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TxTruth {
    pub id: TransactionId,
    pub bucket: TruthBucket,
    /// Clean merchant or counterparty name; `null` for transfers and cash.
    pub merchant: Option<String>,
    /// Recurring series id, if the transaction belongs to one.
    pub recurring: Option<String>,
    /// A large irregular amount that a monthly spending baseline should leave out.
    pub one_off: bool,
    /// Hard-case and provenance tags (see README).
    pub tags: Vec<String>,
    /// How the transaction looked while pending, when that differed (tips, fuel holds).
    pub pending: Option<PendingTruth>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairKind {
    /// Between two of the user's bank accounts.
    OwnTransfer,
    /// From a bank account to a credit card.
    CardPayment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferPair {
    /// The side where money left (negative amount).
    pub out: TransactionId,
    /// The side where it arrived (positive amount).
    #[serde(rename = "in")]
    pub into: TransactionId,
    pub kind: PairKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub from: Date,
    pub amount: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AmountTruth {
    /// The same amount every time, except at the listed price changes.
    Fixed { steps: Vec<Step> },
    /// Varies within this range (utilities, interest, card balances).
    Variable { min: Money, max: Money },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeriesTruth {
    pub id: String,
    /// `subscription`, `bill`, `income`, `transfer` or `interest`.
    pub kind: String,
    pub merchant: String,
    pub account_id: AccountId,
    /// `weekly`, `biweekly`, `semimonthly`, `monthly`, `quarterly`, `semiannual` or
    /// `annual`.
    pub cadence: String,
    pub amount: AmountTruth,
    /// Date of the first and last occurrence in the period.
    pub first: Date,
    pub last: Date,
    /// True if the series stopped before the period ended (a cancellation or a loan
    /// paid off); false if it was still running.
    pub ended: bool,
    pub transaction_ids: Vec<TransactionId>,
}

/// Everything known to be true about one persona's ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaTruth {
    pub persona: String,
    pub description: String,
    pub seed: u64,
    pub generator_version: u32,
    pub taxonomy_version: TaxonomyVersion,
    pub start: Date,
    pub end: Date,
    /// One entry per ledger transaction, in the same order.
    pub transactions: Vec<TxTruth>,
    pub transfer_pairs: Vec<TransferPair>,
    pub recurring: Vec<SeriesTruth>,
}
