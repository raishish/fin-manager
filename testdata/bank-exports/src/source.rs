//! What an account's export contains: TD.1 transactions seen from a given day.
//!
//! The corpus is built from TD.1's committed files (`testdata/synthetic/data`), not
//! from its generator, so the exports are exactly the ledgers the ground truth
//! describes.

use std::path::{Path, PathBuf};

use fm_core::model::{
    Account, AccountId, AccountKind, Date, Ledger, Money, Transaction, TransactionId,
    TransactionStatus,
};
use fm_synth::truth::{PersonaTruth, TruthBucket, TxTruth};

/// One persona's ledger and truth, as committed by TD.1.
pub struct Source {
    pub persona: String,
    pub ledger: Ledger,
    pub truth: PersonaTruth,
}

/// TD.1's data folder.
pub fn synthetic_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../synthetic/data")
}

/// Loads every persona TD.1 committed.
pub fn load(dir: &Path) -> Vec<Source> {
    let manifest: fm_synth::Manifest = read_json(&dir.join("manifest.json"));
    manifest
        .personas
        .iter()
        .map(|p| Source {
            persona: p.key.clone(),
            ledger: read_json(&dir.join(&p.ledger.path)),
            truth: read_json(&dir.join(&p.truth.path)),
        })
        .collect()
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let bytes =
        std::fs::read(path).unwrap_or_else(|e| panic!("can't read {}: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{} doesn't parse: {e}", path.display()))
}

impl Source {
    pub fn account(&self, key: &str) -> &Account {
        let ext = format!("{}-{key}", self.persona);
        self.ledger
            .accounts
            .iter()
            .find(|a| a.external_id.as_deref() == Some(ext.as_str()))
            .unwrap_or_else(|| panic!("persona {} has no account {key}", self.persona))
    }
}

/// What kind of entry a row is, for banks' "Type" columns and OFX `TRNTYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Purchase,
    Refund,
    /// A payment received on a card.
    CardPaymentIn,
    /// A card payment leaving a bank account.
    CardPaymentOut,
    TransferOut,
    TransferIn,
    AchDebit,
    AchCredit,
    Check,
    Atm,
    Fee,
    InterestCharged,
    InterestEarned,
    P2pOut,
    P2pIn,
}

/// One line of an export.
#[derive(Debug, Clone)]
pub struct Row {
    pub truth_id: TransactionId,
    pub pending: bool,
    pub posted_date: Date,
    pub transaction_date: Option<Date>,
    /// Signed cents, negative = money out of the account.
    pub amount: i64,
    pub description: String,
    pub kind: Kind,
    pub check_number: Option<String>,
    /// The truth category key, for banks that print their own category.
    pub category: Option<String>,
    /// Balance after this row; `None` while pending.
    pub balance: Option<i64>,
    /// TD.1's id for the transaction (posted rows only).
    pub external_id: Option<String>,
    /// The clean merchant or counterparty, from the truth.
    pub merchant: Option<String>,
}

fn has(x: &TxTruth, tag: &str) -> bool {
    x.tags.iter().any(|t| t == tag)
}

fn kind_of(t: &Transaction, x: &TxTruth, account: &Account) -> Kind {
    let out = t.amount.is_negative();
    let card = account.kind == AccountKind::CreditCard;
    if has(x, "check") {
        Kind::Check
    } else if has(x, "atm") {
        Kind::Atm
    } else if has(x, "card_payment") {
        if card {
            Kind::CardPaymentIn
        } else {
            Kind::CardPaymentOut
        }
    } else if has(x, "card_interest") {
        Kind::InterestCharged
    } else if matches!(&x.bucket, TruthBucket::Category { key } if key == "financial.bank_fees") {
        Kind::Fee
    } else if x
        .recurring
        .as_deref()
        .is_some_and(|r| r.ends_with(".interest"))
    {
        Kind::InterestEarned
    } else if has(x, "p2p") {
        if out { Kind::P2pOut } else { Kind::P2pIn }
    } else if matches!(x.bucket, TruthBucket::Transfer { .. }) && !has(x, "untracked_counterpart") {
        if out {
            Kind::TransferOut
        } else {
            Kind::TransferIn
        }
    } else if t.transaction_date.is_some() {
        if out { Kind::Purchase } else { Kind::Refund }
    } else if out {
        Kind::AchDebit
    } else {
        Kind::AchCredit
    }
}

/// An opening balance that keeps the account above $500 throughout, rounded up to
/// $100, so running-balance columns look like a real account. Cards start at 0.
fn opening_balance(ledger: &Ledger, account: AccountId, kind: AccountKind) -> i64 {
    if kind == AccountKind::CreditCard {
        return 0;
    }
    let mut running = 0i64;
    let mut lowest = 0i64;
    for t in ledger
        .transactions
        .iter()
        .filter(|t| t.account_id == account)
    {
        running += t.amount.minor();
        lowest = lowest.min(running);
    }
    let need = -lowest + 50_000;
    (need + 9_999) / 10_000 * 10_000
}

/// The account's transactions as an export made on `as_of` would show them, posted
/// between `start` and `end` (inclusive), oldest first. With `include_pending`,
/// purchases made by `as_of` that hadn't posted yet appear as pending, at the amount
/// the bank showed then; otherwise they are left out.
pub fn rows(
    src: &Source,
    account: &Account,
    start: Date,
    end: Date,
    as_of: Date,
    include_pending: bool,
) -> Vec<Row> {
    let opening = opening_balance(&src.ledger, account.id, account.kind);
    let mut balance = opening;
    let mut out = Vec::new();
    for (t, x) in src.ledger.transactions.iter().zip(&src.truth.transactions) {
        assert_eq!(t.id, x.id, "truth is aligned with the ledger");
        if t.account_id != account.id {
            continue;
        }
        let ledger_pending = t.status == TransactionStatus::Pending;
        let posted_by_then = !ledger_pending && t.posted_date <= as_of;
        if posted_by_then {
            balance += t.amount.minor();
        }
        let happened = t.transaction_date.unwrap_or(t.posted_date);
        let pending_then = !posted_by_then && happened <= as_of && t.transaction_date.is_some();
        let base = Row {
            truth_id: t.id,
            pending: false,
            posted_date: t.posted_date,
            transaction_date: t.transaction_date,
            amount: t.amount.minor(),
            description: t.description.clone(),
            kind: kind_of(t, x, account),
            check_number: has(x, "check").then(|| {
                t.description
                    .rsplit(['#', ' '])
                    .next()
                    .unwrap_or("")
                    .to_string()
            }),
            category: match &x.bucket {
                TruthBucket::Category { key } | TruthBucket::Refund { key, .. } => {
                    Some(key.clone())
                }
                _ => None,
            },
            balance: Some(balance),
            external_id: t.source.external_id.clone(),
            merchant: x.merchant.clone(),
        };
        if posted_by_then {
            if (start..=end).contains(&t.posted_date) {
                out.push(base);
            }
        } else if pending_then && include_pending && (start..=end).contains(&happened) {
            // Before posting, the bank shows the authorisation: the purchase date, the
            // pre-tip or hold amount, no balance and no final id.
            let (amount, description) = match &x.pending {
                Some(p) if !ledger_pending => (p.amount.minor(), p.description.clone()),
                _ => (t.amount.minor(), t.description.clone()),
            };
            out.push(Row {
                pending: true,
                posted_date: happened,
                amount,
                description,
                balance: None,
                external_id: None,
                ..base
            });
        }
    }
    out
}

pub fn money(minor: i64) -> Money {
    Money::new(minor, fm_synth::sim::usd())
}

impl Kind {
    /// What banks print as the counterparty when there isn't a merchant.
    pub fn label(self) -> &'static str {
        match self {
            Kind::CardPaymentIn => "Payment Thank You",
            Kind::CardPaymentOut => "Credit Card Payment",
            Kind::TransferOut | Kind::TransferIn => "Transfer",
            Kind::Atm => "ATM Withdrawal",
            Kind::Fee => "Fee",
            Kind::InterestCharged => "Interest Charge",
            Kind::InterestEarned => "Interest Paid",
            Kind::Check => "Check",
            _ => "Other",
        }
    }
}

impl Row {
    /// The merchant, or a label for the kind of entry.
    pub fn payee(&self) -> String {
        self.merchant
            .clone()
            .unwrap_or_else(|| self.kind.label().to_string())
    }
}
