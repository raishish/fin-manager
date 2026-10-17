//! Expected parsed output: what an importer should produce from each file.

use fm_core::model::{AccountId, Date, Money, TransactionId, TransactionStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceAccount {
    pub persona: String,
    /// The TD.1 ledger account the rows come from.
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedAccount {
    /// `checking`, `savings` or `credit_card`.
    pub kind: String,
    pub currency: String,
    /// The account number as the file states it (OFX `ACCTID`), when it does. Always fake.
    pub number: Option<String>,
    /// The routing number (OFX `BANKID`), when the file states it. Always fake.
    pub routing: Option<String>,
    /// The statement's closing balance (OFX `LEDGERBAL`), when the file states it.
    pub ledger_balance: Option<Money>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedTx {
    /// The TD.1 transaction this row is (join with TD.1's truth.json).
    pub truth_id: TransactionId,
    pub status: TransactionStatus,
    pub posted_date: Date,
    pub transaction_date: Option<Date>,
    /// Signed, negative = money out of the account (for cards too).
    pub amount: Money,
    pub description: String,
    pub payee: Option<String>,
    pub memo: Option<String>,
    /// The bank's id for the row (OFX `FITID`, a reference column).
    pub external_id: Option<String>,
    pub check_number: Option<String>,
    /// Running balance after this row, when the file has one.
    pub balance: Option<Money>,
    /// The bank's own category, when the file has one.
    pub bank_category: Option<String>,
}

/// `<file>.expected.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedFile {
    pub file: String,
    pub layout: String,
    pub institution: String,
    pub source: SourceAccount,
    /// The day the export was made.
    pub as_of: Date,
    /// The date range requested.
    pub start: Date,
    pub end: Date,
    pub account: ExpectedAccount,
    /// In file order.
    pub transactions: Vec<ExpectedTx>,
}
