//! The persona's transactions joined with their ground truth, and the counting rules
//! every answer follows.

use std::path::{Path, PathBuf};

use fm_core::model::{AccountKind, Date, Ledger, TransactionStatus};
use fm_synth::truth::{PersonaTruth, TruthBucket};

/// Which date places a transaction in a period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateBasis {
    /// The purchase date when the bank gives one, else the posted date.
    PurchaseThenPosted,
    Posted,
}

/// The counting conventions (see README). Changing one means regenerating the answers.
pub const DATE_BASIS: DateBasis = DateBasis::PurchaseThenPosted;
/// Pending transactions are left out of every total; questions about pending items ask
/// for them explicitly.
pub const COUNT_PENDING: bool = false;
/// The fixed "today" for relative dates ("last month", "this year").
pub fn today() -> Date {
    fm_synth::cal::ymd(2026, 10, 1)
}
pub const PERSONA: &str = "freelancer";
pub const CURRENCY: &str = "USD";

/// Where a transaction's money counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Money out in a category.
    Spending,
    /// Money back, offsetting spending in a category.
    Refund,
    Income,
    Transfer,
}

/// One transaction with everything the questions need.
#[derive(Debug, Clone)]
pub struct Fact {
    pub id: i64,
    pub account: String,
    pub on_card: bool,
    pub posted: Date,
    pub purchase: Option<Date>,
    /// Signed cents, negative = money out of the account.
    pub amount: i64,
    pub original: Option<(i64, String)>,
    pub kind: Kind,
    /// Category key for spending and refunds.
    pub key: Option<String>,
    pub merchant: Option<String>,
    pub recurring: Option<String>,
    pub tags: Vec<String>,
    pub pending: bool,
    pub one_off: bool,
}

impl Fact {
    /// The date that places it in a period.
    pub fn date(&self) -> Date {
        match DATE_BASIS {
            DateBasis::PurchaseThenPosted => self.purchase.unwrap_or(self.posted),
            DateBasis::Posted => self.posted,
        }
    }

    /// Group key (`food` for `food.coffee`).
    pub fn group(&self) -> Option<&str> {
        self.key.as_deref().and_then(|k| k.split('.').next())
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Counts toward category spending (purchases, and refunds that offset them).
    pub fn is_spending(&self) -> bool {
        matches!(self.kind, Kind::Spending | Kind::Refund)
    }

    /// Whether totals include it (pending items are excluded unless the rules say not).
    pub fn counted(&self) -> bool {
        COUNT_PENDING || !self.pending
    }
}

pub fn synthetic_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../synthetic/data")
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let bytes =
        std::fs::read(path).unwrap_or_else(|e| panic!("can't read {}: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{} doesn't parse: {e}", path.display()))
}

/// The persona's ledger and truth, as TD.1 committed them.
pub fn load(dir: &Path) -> (Ledger, PersonaTruth) {
    load_persona(dir, PERSONA)
}

/// Any TD.1 persona's ledger and truth.
pub fn load_persona(dir: &Path, persona: &str) -> (Ledger, PersonaTruth) {
    (
        read(&dir.join(persona).join("ledger.json")),
        read(&dir.join(persona).join("truth.json")),
    )
}

pub fn facts(ledger: &Ledger, truth: &PersonaTruth) -> Vec<Fact> {
    ledger
        .transactions
        .iter()
        .zip(&truth.transactions)
        .map(|(t, x)| {
            assert_eq!(t.id, x.id, "truth is aligned with the ledger");
            let account = ledger
                .accounts
                .iter()
                .find(|a| a.id == t.account_id)
                .expect("account exists");
            let (kind, key) = match &x.bucket {
                TruthBucket::Category { key } => (Kind::Spending, Some(key.clone())),
                TruthBucket::Refund { key, .. } => (Kind::Refund, Some(key.clone())),
                TruthBucket::Income => (Kind::Income, None),
                TruthBucket::Transfer { .. } => (Kind::Transfer, None),
            };
            Fact {
                id: t.id.0,
                account: account.name.clone(),
                on_card: account.kind == AccountKind::CreditCard,
                posted: t.posted_date,
                purchase: t.transaction_date,
                amount: t.amount.minor(),
                original: t
                    .original_amount
                    .map(|o| (o.minor(), o.currency().code().to_string())),
                kind,
                key,
                merchant: x.merchant.clone(),
                recurring: x.recurring.clone(),
                tags: x.tags.clone(),
                pending: t.status == TransactionStatus::Pending,
                one_off: x.one_off,
            }
        })
        .collect()
}

/// An inclusive date range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Period {
    pub start: Date,
    pub end: Date,
}

fn ymd(y: i32, m: u8, d: u8) -> Date {
    fm_synth::cal::ymd(y, m, d)
}

impl Period {
    pub fn new(start: Date, end: Date) -> Period {
        Period { start, end }
    }

    pub fn month(y: i32, m: u8) -> Period {
        Period::new(ymd(y, m, 1), fm_synth::cal::day_of_month(y, m, 31))
    }

    pub fn months(y: i32, from: u8, to: u8) -> Period {
        Period::new(Period::month(y, from).start, Period::month(y, to).end)
    }

    pub fn year(y: i32) -> Period {
        Period::new(ymd(y, 1, 1), ymd(y, 12, 31))
    }

    pub fn quarter(y: i32, q: u8) -> Period {
        Period::months(y, 3 * q - 2, 3 * q)
    }

    /// The `n` full calendar months before today's month.
    pub fn last_months(n: i32) -> Period {
        let t = today();
        let this = fm_core::model::YearMonth::new(t.year(), t.month()).expect("valid month");
        let first = this.checked_add_months(-n).expect("in range");
        let last = this.checked_add_months(-1).expect("in range");
        Period::new(first.first_day(), last.last_day())
    }

    /// The `n` days before today (today excluded).
    pub fn last_days(n: i32) -> Period {
        let t = today();
        Period::new(
            fm_synth::cal::add_days(t, -n),
            fm_synth::cal::add_days(t, -1),
        )
    }

    pub fn contains(&self, d: Date) -> bool {
        (self.start..=self.end).contains(&d)
    }
}

/// Integer division rounded half away from zero (for averages and medians).
pub fn div_round(a: i64, b: i64) -> i64 {
    assert!(b > 0);
    let q = a / b;
    let r = a % b;
    if 2 * r.abs() >= b { q + a.signum() } else { q }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_periods() {
        assert_eq!(Period::last_months(1), Period::month(2026, 9));
        assert_eq!(Period::last_months(3), Period::months(2026, 7, 9));
        assert_eq!(
            Period::last_months(12),
            Period::new(ymd(2025, 10, 1), ymd(2026, 9, 30))
        );
        assert_eq!(
            Period::last_days(30),
            Period::new(ymd(2026, 9, 1), ymd(2026, 9, 30))
        );
        assert_eq!(Period::quarter(2026, 3), Period::months(2026, 7, 9));
    }

    #[test]
    fn rounding() {
        assert_eq!(div_round(5, 2), 3);
        assert_eq!(div_round(-5, 2), -3);
        assert_eq!(div_round(4, 3), 1);
        assert_eq!(div_round(-7, 4), -2);
    }
}
