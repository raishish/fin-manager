use serde::{Deserialize, Serialize};

use super::{AccountId, Date, ImportId, Money, TransactionId};

/// Whether the source reported the transaction as settled.
///
/// Serialised in `snake_case`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStatus {
    /// Authorised but not settled. The import pipeline replaces it with the posted
    /// transaction when that arrives.
    Pending,
    Posted,
}

/// Where in a raw import a transaction came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    pub import_id: ImportId,
    /// Zero-based position within the import (CSV data row, OFX `STMTTRN`, SimpleFIN
    /// transaction), so the original record can be found again.
    pub index: u32,
    /// The source's own transaction id, if it gives one (OFX `FITID`, SimpleFIN id).
    /// Used to remove duplicates.
    pub external_id: Option<String>,
}

/// One transaction on one account, as the source reported it.
///
/// Holds only what the source said, normalised (dates to ISO, amounts to signed minor
/// units). Everything derived from it, such as the cleaned merchant name and the
/// category, is a [`Decision`](super::Decision), so the raw fields are never overwritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transaction {
    pub id: TransactionId,
    pub account_id: AccountId,
    /// The raw import this transaction was read from (needed for cascade deletion).
    pub source: SourceRef,
    pub status: TransactionStatus,
    /// The date the transaction posted to the account. For a pending transaction, the
    /// date the source reports for it.
    pub posted_date: Date,
    /// The date of the purchase or transfer itself, when the source gives it separately.
    pub transaction_date: Option<Date>,
    /// Signed, in the account's currency: negative is money out of the account.
    pub amount: Money,
    /// The amount in the currency it was spent in, when that differs from the account's
    /// (a card purchase abroad). Same sign as `amount`.
    pub original_amount: Option<Money>,
    /// The description exactly as the source gave it.
    pub description: String,
    /// Payee or counterparty, if the source gives it separately (OFX `NAME`).
    pub payee: Option<String>,
    /// Memo or extra details, if the source gives them (OFX `MEMO`).
    pub memo: Option<String>,
}
