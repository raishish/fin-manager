//! TD.1 households: loading, the starting state the models work from, and scoring.
//!
//! The starting state is T2.2's `T2.2-BASELINE+T1.4` at k = 20: transfer detection
//! (T1.4), the rules engine, then a simulated user's 20 corrections of their top
//! merchants. What is still in Needs review after that is what the model sees.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use fm_core::categorize::{Correction, Learn, Scope, correct_ledger, run_on_ledger};
use fm_core::model::{
    Assignment, Bucket, CategoryId, DecisionState, Ledger, Timestamp, TransactionId,
};
use serde_json::Value;

pub const PERSONAS: [&str; 4] = ["renter", "family", "freelancer", "retiree"];
/// Corrections per household in the starting state.
pub const K: usize = 20;
pub const NOW: Timestamp = Timestamp::from_unix_seconds(1_770_000_000);

pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

pub struct Household {
    pub persona: &'static str,
    pub ledger: Ledger,
    pub truth: HashMap<TransactionId, Bucket>,
    pub merchant: HashMap<TransactionId, String>,
    /// The transactions the simulated user corrected, in order.
    pub corrected: Vec<TransactionId>,
}

pub fn load(persona: &'static str) -> Household {
    let dir = data_dir().join("synthetic/data").join(persona);
    let ledger: Ledger =
        serde_json::from_slice(&fs::read(dir.join("ledger.json")).unwrap()).expect("TD.1 ledger");
    let truth: Value =
        serde_json::from_slice(&fs::read(dir.join("truth.json")).unwrap()).expect("TD.1 truth");
    let by_key: HashMap<&str, CategoryId> = ledger
        .taxonomy
        .categories
        .iter()
        .map(|c| (c.key.as_str(), c.id))
        .collect();
    let mut buckets = HashMap::new();
    let mut merchants = HashMap::new();
    for t in truth["transactions"].as_array().unwrap() {
        let id = TransactionId(t["id"].as_i64().unwrap());
        let b = &t["bucket"];
        let category = || by_key[b["key"].as_str().unwrap()];
        let other = |v: &Value| v.as_i64().map(TransactionId);
        let bucket = match b["kind"].as_str().unwrap() {
            "category" => Bucket::Category {
                category_id: category(),
            },
            "transfer" => Bucket::Transfer {
                counterpart: other(&b["counterpart"]),
            },
            "income" => Bucket::Income,
            "refund" => Bucket::Refund {
                category_id: category(),
                original: other(&b["original"]),
            },
            _ => panic!("unexpected truth bucket kind"),
        };
        buckets.insert(id, bucket);
        if let Some(m) = t["merchant"].as_str() {
            merchants.insert(id, m.to_string());
        }
    }
    Household {
        persona,
        ledger,
        truth: buckets,
        merchant: merchants,
        corrected: Vec::new(),
    }
}

/// Runs T1.4's transfer detection in a throwaway encrypted database.
fn detect_transfers(h: &mut Household, scratch: &Path) {
    let dir = scratch.join(format!("td1-{}-{}", h.persona, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let mut u = fm_core::storage::Vault::at(&dir.join("ledger"))
        .create(b"t2.4 evaluation ledger")
        .expect("create the scratch ledger");
    u.save_ledger(&h.ledger).expect("save TD.1");
    fm_core::transfers::detect(&mut u, NOW).expect("transfer detection");
    h.ledger = u.load_ledger().expect("load TD.1 back");
    drop(u);
    let _ = fs::remove_dir_all(&dir);
}

/// The household in the starting state (see the module docs).
pub fn starting_state(persona: &'static str, scratch: &Path) -> Household {
    let mut h = load(persona);
    detect_transfers(&mut h, scratch);
    run_on_ledger(&mut h.ledger, Scope::All, NOW);
    let top = top_merchants(&h);
    for merchant in top.iter().take(K) {
        if let Some(id) = first_wrong(&h, merchant) {
            correct_ledger(
                &mut h.ledger,
                id,
                Correction::Bucket(h.truth[&id]),
                Learn::Default,
                NOW,
            )
            .expect("simulated correction");
            h.corrected.push(id);
        }
    }
    h
}

pub fn applied_buckets(ledger: &Ledger) -> HashMap<TransactionId, Bucket> {
    ledger
        .decisions
        .iter()
        .filter(|d| d.state == DecisionState::Applied)
        .filter_map(|d| match d.assignment {
            Assignment::Bucket(b) => Some((d.transaction_id, b)),
            Assignment::Merchant(_) => None,
        })
        .collect()
}

/// Refunds count as correct when the category is: nothing but the user can know the
/// original purchase.
pub fn same(applied: Bucket, truth: Bucket) -> bool {
    match (applied, truth) {
        (Bucket::Refund { category_id: a, .. }, Bucket::Refund { category_id: b, .. }) => a == b,
        (a, b) => a == b,
    }
}

fn top_merchants(h: &Household) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for m in h.merchant.values() {
        *counts.entry(m.as_str()).or_default() += 1;
    }
    let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    ranked.into_iter().map(|(m, _)| m.to_string()).collect()
}

fn first_wrong(h: &Household, merchant: &str) -> Option<TransactionId> {
    let applied = applied_buckets(&h.ledger);
    let mut ids: Vec<TransactionId> = h
        .merchant
        .iter()
        .filter(|(_, m)| m.as_str() == merchant)
        .map(|(id, _)| *id)
        .collect();
    ids.sort();
    ids.into_iter()
        .find(|id| !applied.get(id).is_some_and(|b| same(*b, h.truth[id])))
}

/// Correct, wrong and Needs-review counts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Score {
    pub correct: usize,
    pub wrong: usize,
    pub needs_review: usize,
}

impl Score {
    pub fn total(&self) -> usize {
        self.correct + self.wrong + self.needs_review
    }

    pub fn count(&mut self, applied: Option<Bucket>, truth: Bucket) {
        match applied {
            None | Some(Bucket::NeedsReview) => self.needs_review += 1,
            Some(b) if same(b, truth) => self.correct += 1,
            Some(_) => self.wrong += 1,
        }
    }
}

/// The whole household as it stands.
pub fn score(h: &Household) -> Score {
    let applied = applied_buckets(&h.ledger);
    let mut s = Score::default();
    for t in &h.ledger.transactions {
        s.count(applied.get(&t.id).copied(), h.truth[&t.id]);
    }
    s
}
