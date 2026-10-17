//! What the ledger screens (T3.3) share: money and transactions as the UI shows them,
//! errors, today's date, and which model decisions still wait for the user.
//!
//! **Seen (SPEC constraint 7, decision 46):** viewing never marks a decision seen, wherever
//! it is shown (review queue, drill-downs, dashboard). Only the user's actions do:
//! confirming (Approve), rejecting or correcting. So no command here writes on read.
//!
//! Every number comes from fm-core. These types only carry it: money travels as minor
//! units plus fm-core's own decimal string, and the UI formats that string without doing
//! arithmetic. `app/src/routes/dashboard/ledger.ts` mirrors these types.

use std::collections::HashMap;

use fm_core::categorize::CategorizeError;
use fm_core::model::{
    Assignment, Bucket, Date, Decision, DecisionSource, DecisionState, Ledger, Money, Taxonomy,
    Timestamp, Transaction, TransactionId, TransactionStatus,
};
use fm_core::storage::StorageError;
use fm_core::views::ViewError;
use serde::Serialize;

/// An amount: fm-core's minor units, its currency, and its exact decimal string
/// (`"-1234.50"`), which the UI formats as currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoneyView {
    pub minor: i64,
    pub currency: &'static str,
    pub decimal: String,
}

impl From<Money> for MoneyView {
    fn from(m: Money) -> Self {
        Self {
            minor: m.minor(),
            currency: m.currency().code(),
            decimal: m.to_decimal_string(),
        }
    }
}

/// A ratio in basis points (from fm-core) as a signed percentage with one decimal,
/// `"+12.5%"`. Integer formatting only, rounding half away from zero.
pub fn percent(basis_points: i64) -> String {
    let sign = match basis_points.signum() {
        1 => "+",
        -1 => "−",
        _ => "",
    };
    let tenths = (basis_points.unsigned_abs() + 5) / 10;
    format!("{sign}{}.{}%", tenths / 10, tenths % 10)
}

/// A confidence in basis points as a whole percentage, rounding half up: `"87%"`.
pub fn confidence(basis_points: u16) -> String {
    format!("{}%", (u32::from(basis_points) + 50) / 100)
}

/// An error for the UI: a stable code and a message. Messages name ids and codes only,
/// never amounts, merchants or descriptions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UiError {
    pub code: &'static str,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, UiError>;

pub fn ui_error(code: &'static str, message: impl Into<String>) -> UiError {
    UiError {
        code,
        message: message.into(),
    }
}

impl From<StorageError> for UiError {
    fn from(e: StorageError) -> Self {
        ui_error("Storage", e.to_string())
    }
}

impl From<CategorizeError> for UiError {
    fn from(e: CategorizeError) -> Self {
        let code = match &e {
            CategorizeError::DoesNotFit => "DoesNotFit",
            CategorizeError::UnknownRule(_)
            | CategorizeError::UnknownDecision(_)
            | CategorizeError::UnknownTransaction => "NotFound",
            CategorizeError::UnknownCategory => "UnknownCategory",
            CategorizeError::NotASuggestion(_) => "NotASuggestion",
            // Something else was decided since: the UI reloads and shows what is current.
            CategorizeError::StaleSuggestion(_) => "StaleSuggestion",
            // The decision on screen isn't the one in effect any more.
            CategorizeError::NotApplied(_) => {
                return ui_error(
                    "Changed",
                    "this changed since the page loaded; reload to see what is current",
                );
            }
            CategorizeError::NotRejectable(_) => "NotRejectable",
            CategorizeError::NotConfirmable(_) => "NotConfirmable",
            CategorizeError::InvalidPair => "InvalidPair",
            CategorizeError::TransactionOpen => "Busy",
            _ => "Storage",
        };
        ui_error(code, e.to_string())
    }
}

impl From<ViewError> for UiError {
    fn from(e: ViewError) -> Self {
        ui_error("View", e.to_string())
    }
}

/// The unlock gate already refuses commands while locked; this covers a lock that lands
/// between the gate and the command.
pub fn locked() -> UiError {
    ui_error("Locked", "the ledger is locked")
}

/// The UTC date of `now`.
pub(crate) fn utc_date(now: Timestamp) -> Date {
    let days = now.unix_seconds().div_euclid(86_400);
    i32::try_from(days)
        .ok()
        .and_then(Date::from_days_since_epoch)
        .unwrap_or_else(|| Date::from_ymd(1970, 1, 1).expect("a valid date"))
}

/// "Today" for the views' `as_of` is the user's local calendar date (review T3.3 #3), which
/// only the UI knows: Rust's standard library has no time zones. The UI sends it as
/// `YYYY-MM-DD`; it must be within a day of the UTC date (time zones run from −12 to
/// +14 hours), so a wrong clock or a bad value can't move "this month" around.
pub fn local_today(given: &str, now: Timestamp) -> Result<Date> {
    let date: Date = given
        .parse()
        .map_err(|_| ui_error("InvalidDate", "today's date isn't YYYY-MM-DD"))?;
    let utc = utc_date(now);
    if date.days_since(utc).abs() > 1 {
        return Err(ui_error(
            "InvalidDate",
            "today's date is more than a day from the system clock",
        ));
    }
    Ok(date)
}

/// The date a transaction counts on (SPEC decision 42): the purchase date when the bank
/// gives one, otherwise the posted date.
pub fn counting_date(t: &Transaction) -> Date {
    t.transaction_date.unwrap_or(t.posted_date)
}

/// Where a transaction's money counts, as the UI labels it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BucketView {
    /// `category`, `refund`, `income`, `transfer` or `needsReview`.
    pub kind: &'static str,
    pub label: String,
    /// The category it counts toward (after merges), for `category` and `refund`.
    pub category_id: Option<i64>,
    /// For a transfer: whether the other side is in the ledger.
    pub paired: bool,
}

pub fn bucket_view(taxonomy: &Taxonomy, bucket: Option<&Bucket>) -> BucketView {
    let category = |id| {
        let c = taxonomy.resolve(id);
        (
            c.map_or_else(|| format!("category {}", id.0), |c| c.name.clone()),
            c.map(|c| c.id.0),
        )
    };
    match bucket {
        Some(Bucket::Category { category_id }) => {
            let (label, id) = category(*category_id);
            BucketView {
                kind: "category",
                label,
                category_id: id,
                paired: false,
            }
        }
        Some(Bucket::Refund { category_id, .. }) => {
            let (name, id) = category(*category_id);
            BucketView {
                kind: "refund",
                label: format!("Refund · {name}"),
                category_id: id,
                paired: false,
            }
        }
        Some(Bucket::Income) => BucketView {
            kind: "income",
            label: "Income".into(),
            category_id: None,
            paired: false,
        },
        Some(Bucket::Transfer { counterpart }) => BucketView {
            kind: "transfer",
            label: "Transfer".into(),
            category_id: None,
            paired: counterpart.is_some(),
        },
        Some(Bucket::NeedsReview) | None => BucketView {
            kind: "needsReview",
            label: "Needs review".into(),
            category_id: None,
            paired: false,
        },
    }
}

/// Who made a decision, in the UI's words.
pub fn source_label(source: &DecisionSource) -> &'static str {
    use fm_core::model::BuiltinLogic;
    match source {
        DecisionSource::User => "You",
        DecisionSource::Rule { .. } => "Your rule",
        DecisionSource::Builtin { logic, .. } => match logic {
            BuiltinLogic::TransferMatcher => "Transfer detection",
            BuiltinLogic::MerchantCleaner => "Merchant clean-up",
            BuiltinLogic::ImportPipeline => "Import",
        },
        DecisionSource::Model(_) => "Model",
    }
}

/// A transaction's decisions as they stand.
#[derive(Debug, Default)]
pub struct Current<'a> {
    /// Applied bucket decisions. Exactly one is the transaction's bucket; none or several
    /// count as Needs review (SPEC constraint 4), as in fm-core's views.
    pub buckets: Vec<&'a Decision>,
    /// Applied merchant decisions; the latest names the merchant.
    pub merchants: Vec<&'a Decision>,
    pub suggestions: Vec<&'a Decision>,
}

impl<'a> Current<'a> {
    pub fn bucket(&self) -> Option<&'a Bucket> {
        match self.buckets.as_slice() {
            [d] => match &d.assignment {
                Assignment::Bucket(b) => Some(b),
                Assignment::Merchant(_) => None,
            },
            _ => None,
        }
    }

    pub fn bucket_decision(&self) -> Option<&'a Decision> {
        match self.buckets.as_slice() {
            [d] => Some(d),
            _ => None,
        }
    }

    pub fn merchant(&self) -> Option<&'a str> {
        self.merchants
            .iter()
            .max_by_key(|d| (d.decided_at, d.id))
            .and_then(|d| match &d.assignment {
                Assignment::Merchant(m) => Some(m.as_str()),
                Assignment::Bucket(_) => None,
            })
    }

    /// In Needs review: no bucket, several, or the Needs-review bucket itself.
    pub fn needs_review(&self) -> bool {
        matches!(self.bucket(), None | Some(Bucket::NeedsReview))
    }

    /// The bucket comes from the model or fixed logic (transfer detection) and the user
    /// hasn't confirmed or corrected it yet (decision 46): it waits in the review queue for
    /// Approve or Reject. Needs-review placeholders can't be approved; they are filed.
    pub fn awaits_confirmation(&self) -> bool {
        use fm_core::model::BuiltinLogic;
        self.bucket_decision().is_some_and(|d| {
            d.seen_at.is_none()
                && matches!(
                    d.source,
                    DecisionSource::Model(_)
                        | DecisionSource::Builtin {
                            logic: BuiltinLogic::TransferMatcher,
                            ..
                        }
                )
        }) && !self.needs_review()
    }

    /// In the review queue: Needs review, or a model or fixed-logic decision awaiting
    /// confirmation.
    pub fn in_review(&self) -> bool {
        self.needs_review() || self.awaits_confirmation()
    }
}

pub fn current_by_transaction(ledger: &Ledger) -> HashMap<TransactionId, Current<'_>> {
    let mut map: HashMap<TransactionId, Current<'_>> = HashMap::new();
    for d in &ledger.decisions {
        let entry = map.entry(d.transaction_id).or_default();
        match (d.state, &d.assignment) {
            (DecisionState::Applied, Assignment::Bucket(_)) => entry.buckets.push(d),
            (DecisionState::Applied, Assignment::Merchant(_)) => entry.merchants.push(d),
            (DecisionState::Suggested, _) => entry.suggestions.push(d),
            _ => {}
        }
    }
    map
}

/// A suggestion waiting for the user (SPEC constraint 7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionView {
    pub decision_id: i64,
    pub source: &'static str,
    /// What it would set: a bucket label, or `Merchant: <name>`.
    pub label: String,
    pub bucket: Option<BucketView>,
    pub merchant: Option<String>,
}

pub fn suggestion_view(taxonomy: &Taxonomy, d: &Decision) -> SuggestionView {
    let (label, bucket, merchant) = match &d.assignment {
        Assignment::Bucket(b) => {
            let view = bucket_view(taxonomy, Some(b));
            (view.label.clone(), Some(view), None)
        }
        Assignment::Merchant(m) => (format!("Merchant: {m}"), None, Some(m.clone())),
    };
    SuggestionView {
        decision_id: d.id.0,
        source: source_label(&d.source),
        label,
        bucket,
        merchant,
    }
}

/// One transaction, as the lists show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionView {
    pub id: i64,
    /// The date it counts on (SPEC decision 42).
    pub date: String,
    pub posted_date: String,
    /// Pending transactions are never in a total.
    pub pending: bool,
    pub account: String,
    pub description: String,
    pub merchant: Option<String>,
    pub amount: MoneyView,
    pub bucket: BucketView,
    /// Who set the bucket, when exactly one decision does.
    pub bucket_source: Option<&'static str>,
    /// That decision: what Approve and Reject act on, so the user only ever confirms
    /// the decision they were shown.
    pub bucket_decision_id: Option<i64>,
    /// The model's confidence in the bucket, as a percentage (`"87%"`), for model decisions.
    pub bucket_confidence: Option<String>,
    /// The bucket waits for Approve or Reject (decision 46).
    pub awaits_confirmation: bool,
    /// For a paired transfer: the other side. Approve, Reject and corrections change it
    /// too.
    pub paired_with: Option<PairedView>,
    pub suggestions: Vec<SuggestionView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedView {
    pub id: i64,
    pub account: String,
    /// The other side's bucket decision, sent with this one on Approve.
    pub decision_id: Option<i64>,
}

/// The transactions with these ids, in the order given; unknown ids are skipped.
pub fn transaction_views(ledger: &Ledger, ids: &[TransactionId]) -> Vec<TransactionView> {
    let current = current_by_transaction(ledger);
    let by_id: HashMap<TransactionId, &Transaction> =
        ledger.transactions.iter().map(|t| (t.id, t)).collect();
    let account_names: HashMap<_, _> = ledger
        .accounts
        .iter()
        .map(|a| (a.id, a.name.as_str()))
        .collect();
    let account_name = |id: fm_core::model::AccountId| {
        account_names
            .get(&id)
            .map_or_else(|| format!("account {}", id.0), |n| (*n).to_string())
    };
    let empty = Current::default();
    ids.iter()
        .filter_map(|id| by_id.get(id))
        .map(|t| {
            let c = current.get(&t.id).unwrap_or(&empty);
            TransactionView {
                id: t.id.0,
                date: counting_date(t).to_string(),
                posted_date: t.posted_date.to_string(),
                pending: t.status == TransactionStatus::Pending,
                account: account_name(t.account_id),
                description: t.description.clone(),
                merchant: c.merchant().map(Into::into),
                amount: t.amount.into(),
                bucket: bucket_view(&ledger.taxonomy, c.bucket()),
                bucket_source: c.bucket_decision().map(|d| source_label(&d.source)),
                bucket_decision_id: c.bucket_decision().map(|d| d.id.0),
                bucket_confidence: c.bucket_decision().and_then(|d| match &d.source {
                    DecisionSource::Model(m) => Some(confidence(m.confidence.basis_points())),
                    _ => None,
                }),
                awaits_confirmation: c.awaits_confirmation(),
                paired_with: match c.bucket() {
                    Some(Bucket::Transfer {
                        counterpart: Some(other),
                    }) => by_id.get(other).map(|o| PairedView {
                        id: other.0,
                        account: account_name(o.account_id),
                        decision_id: current
                            .get(other)
                            .and_then(Current::bucket_decision)
                            .map(|d| d.id.0),
                    }),
                    _ => None,
                },
                suggestions: c
                    .suggestions
                    .iter()
                    .map(|d| suggestion_view(&ledger.taxonomy, d))
                    .collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_round_half_away_from_zero() {
        assert_eq!(percent(1_250), "+12.5%");
        assert_eq!(percent(-1_250), "−12.5%");
        assert_eq!(percent(0), "0.0%");
        assert_eq!(percent(4), "+0.0%");
        assert_eq!(percent(5), "+0.1%");
        assert_eq!(percent(-15), "−0.2%");
        assert_eq!(percent(100_000), "+1000.0%");
        assert_eq!(confidence(8_749), "87%");
        assert_eq!(confidence(8_750), "88%");
        assert_eq!(confidence(10_000), "100%");
    }

    /// Review T3.3 #3: "today" is the local date the UI sends, within a day of UTC.
    #[test]
    fn today_is_the_local_date_within_a_day_of_utc() {
        // 2026-10-17 23:30 UTC: still the 17th in UTC, already the 18th east of +00:30,
        // still the 17th in New York.
        let now = Timestamp::from_unix_seconds(1_792_279_800);
        assert_eq!(utc_date(now).to_string(), "2026-10-17");
        for local in ["2026-10-16", "2026-10-17", "2026-10-18"] {
            assert_eq!(local_today(local, now).unwrap().to_string(), local);
        }
        for bad in ["2026-10-19", "2026-10-15", "2026-13-01", "today", ""] {
            assert_eq!(
                local_today(bad, now).unwrap_err().code,
                "InvalidDate",
                "{bad}"
            );
        }
    }

    #[test]
    fn money_carries_fm_cores_decimal_string() {
        let usd = fm_core::model::Currency::USD;
        let m: MoneyView = Money::new(-123_450, usd).into();
        assert_eq!(m.decimal, "-1234.50");
        assert_eq!(m.minor, -123_450);
        assert_eq!(m.currency, "USD");
        let jpy = fm_core::model::Currency::new("JPY").unwrap();
        assert_eq!(MoneyView::from(Money::new(500, jpy)).decimal, "500");
    }
}
