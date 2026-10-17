//! The rules-only baseline on TD.1 (T2.2), which the model (T2.4, T2.5) improves on.
//!
//! For each household: the rules engine runs on the bare ledger, then a simulated user
//! corrects their top merchants one at a time (by number of transactions), each time
//! the first transaction still wrong, with the truth bucket and a learned rule. After
//! k = 0, 5, 10, 20 and 40 corrections it counts every transaction as correct, wrong or
//! in Needs review against `truth.json`. Merchant clean-up accuracy is measured on the
//! bare run, with and without the alias table.
//!
//! Two scenarios: rules alone (`T2.2-BASELINE`), and the import order the app uses,
//! transfer detection (T1.4) first and then the rules (`T2.2-BASELINE+T1.4`).
//!
//! Run `cargo test -p fm-core --lib td1_eval -- --nocapture` to see the table. The lines
//! starting `T2.2-BASELINE` are stable: one per household and k, then `ALL`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use super::correct::{Correction, Learn, correct_ledger};
use super::merchant;
use super::normalize::normalize;
use super::plan::Scope;
use super::run_on_ledger;
use crate::contract;
use crate::model::*;

const PERSONAS: [&str; 4] = ["renter", "family", "freelancer", "retiree"];
const STEPS: [usize; 5] = [0, 5, 10, 20, 40];
const NOW: Timestamp = Timestamp::from_unix_seconds(1_770_000_000);

struct Household {
    ledger: Ledger,
    truth: HashMap<TransactionId, Bucket>,
    merchant: HashMap<TransactionId, String>,
}

fn load(persona: &str) -> Household {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/synthetic/data")
        .join(persona);
    let ledger: Ledger =
        serde_json::from_slice(&fs::read(dir.join("ledger.json")).unwrap()).unwrap();
    let truth: Value = serde_json::from_slice(&fs::read(dir.join("truth.json")).unwrap()).unwrap();
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
            kind => panic!("unexpected truth bucket {kind}"),
        };
        buckets.insert(id, bucket);
        if let Some(m) = t["merchant"].as_str() {
            merchants.insert(id, m.to_string());
        }
    }
    Household {
        ledger,
        truth: buckets,
        merchant: merchants,
    }
}

/// A rule can't know a refund's original purchase, so refunds count as correct when the
/// category is.
fn same(applied: Bucket, truth: Bucket) -> bool {
    match (applied, truth) {
        (Bucket::Refund { category_id: a, .. }, Bucket::Refund { category_id: b, .. }) => a == b,
        (a, b) => a == b,
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Score {
    corrections: usize,
    correct: usize,
    wrong: usize,
    needs_review: usize,
    /// Of `needs_review`: halves of transfer pairs, which are T1.4's job.
    pairs_in_review: usize,
    total: usize,
}

impl Score {
    fn add(&mut self, o: Score) {
        self.corrections += o.corrections;
        self.correct += o.correct;
        self.wrong += o.wrong;
        self.needs_review += o.needs_review;
        self.pairs_in_review += o.pairs_in_review;
        self.total += o.total;
    }

    fn line(&self, scenario: &str, name: &str, k: usize) -> String {
        let pct = |n: usize| 100.0 * n as f64 / self.total as f64;
        format!(
            "{scenario} {name:<10} k={k:<2} corrections={:<3} correct={:<5} ({:>5.1}%) wrong={:<4} ({:>4.1}%) needs_review={:<5} ({:>5.1}%) pairs_in_review={:<4} total={}",
            self.corrections,
            self.correct,
            pct(self.correct),
            self.wrong,
            pct(self.wrong),
            self.needs_review,
            pct(self.needs_review),
            self.pairs_in_review,
            self.total
        )
    }
}

fn applied_buckets(ledger: &Ledger) -> HashMap<TransactionId, Bucket> {
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

fn score(h: &Household, corrections: usize) -> Score {
    let applied = applied_buckets(&h.ledger);
    let mut s = Score {
        corrections,
        total: h.ledger.transactions.len(),
        ..Score::default()
    };
    for t in &h.ledger.transactions {
        let truth = h.truth[&t.id];
        match applied.get(&t.id) {
            Some(Bucket::NeedsReview) | None => {
                s.needs_review += 1;
                if matches!(
                    truth,
                    Bucket::Transfer {
                        counterpart: Some(_)
                    }
                ) {
                    s.pairs_in_review += 1;
                }
            }
            Some(b) if same(*b, truth) => s.correct += 1,
            Some(_) => s.wrong += 1,
        }
    }
    s
}

/// Truth merchants by number of transactions, most first (ties by name). Transfer
/// pairs and cash have no merchant and aren't corrected here.
fn top_merchants(h: &Household) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for m in h.merchant.values() {
        *counts.entry(m.as_str()).or_default() += 1;
    }
    let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    ranked.into_iter().map(|(m, _)| m.to_string()).collect()
}

/// The first transaction of `merchant` whose bucket is still wrong.
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

fn merchant_accuracy(
    h: &Household,
    clean: fn(&str) -> Option<merchant::Cleaned>,
) -> (usize, usize) {
    let mut exact = 0;
    let mut total = 0;
    for t in &h.ledger.transactions {
        let Some(truth) = h.merchant.get(&t.id) else {
            continue;
        };
        total += 1;
        if clean(&t.description).is_some_and(|c| normalize(&c.name) == normalize(truth)) {
            exact += 1;
        }
    }
    (exact, total)
}

/// Runs T1.4's transfer detection on the household in an encrypted database and takes
/// the ledger back with its pairs.
fn with_transfers(h: &mut Household) {
    let dir = tempfile::tempdir().unwrap();
    let mut u = crate::storage::Vault::at(&dir.path().join("ledger"))
        .create_with_params(b"td1 eval", crate::storage::KdfParams::INSECURE_TEST)
        .unwrap();
    u.save_ledger(&h.ledger).unwrap();
    crate::transfers::detect(&mut u, NOW).unwrap();
    h.ledger = u.load_ledger().unwrap();
}

/// The k-sweep for one scenario; returns the totals per k.
fn sweep(scenario: &str, transfers_first: bool) -> HashMap<usize, Score> {
    let mut all: HashMap<usize, Score> = HashMap::new();
    for persona in PERSONAS {
        let mut h = load(persona);
        if transfers_first {
            with_transfers(&mut h);
        }
        run_on_ledger(&mut h.ledger, Scope::All, NOW);
        let report = contract::check(&h.ledger);
        assert!(report.is_ok(), "{persona}: {report}");

        let top = top_merchants(&h);
        let mut corrections = 0;
        let mut next = 0;
        let mut previous: Option<Score> = None;
        for k in STEPS {
            while next < k.min(top.len()) {
                if let Some(id) = first_wrong(&h, &top[next]) {
                    correct_ledger(
                        &mut h.ledger,
                        id,
                        Correction::Bucket(h.truth[&id]),
                        Learn::Default,
                        NOW,
                    )
                    .unwrap();
                    corrections += 1;
                }
                next += 1;
            }
            let report = contract::check(&h.ledger);
            assert!(report.is_ok(), "{persona} k={k}: {report}");
            let s = score(&h, corrections);
            println!("{}", s.line(scenario, persona, k));
            if let Some(p) = previous {
                assert!(s.correct >= p.correct, "{persona}: k={k} lost ground");
            }
            previous = Some(s);
            all.entry(k).or_default().add(s);
        }
    }
    for k in STEPS {
        println!("{}", all[&k].line(scenario, "ALL", k));
    }
    all
}

#[test]
fn td1_rules_only_baseline() {
    let all = sweep("T2.2-BASELINE", false);
    // Loose sanity bounds: the numbers are the deliverable, not a target.
    let k0 = all[&0];
    let k20 = all[&20];
    assert_eq!(k0.correct, 0, "no rules, no corrections: nothing decided");
    assert!(k20.correct > k20.total / 3, "{}", k20.line("", "ALL", 20));
    assert!(k20.wrong * 10 < k20.total, "{}", k20.line("", "ALL", 20));
}

#[test]
fn td1_baseline_after_transfer_detection() {
    let all = sweep("T2.2-BASELINE+T1.4", true);
    let rules_only_pairs = 618;
    assert!(
        all[&0].pairs_in_review < rules_only_pairs / 2,
        "T1.4 pairs most transfers: {}",
        all[&0].line("", "ALL", 0)
    );
    assert!(all[&20].wrong * 10 < all[&20].total);
}

#[test]
fn td1_merchant_cleanup_accuracy() {
    let (mut exact, mut exact_plain, mut named) = (0, 0, 0);
    for persona in PERSONAS {
        let h = load(persona);
        let (e, n) = merchant_accuracy(&h, merchant::clean);
        let (p, _) = merchant_accuracy(&h, merchant::clean_without_aliases);
        exact += e;
        exact_plain += p;
        named += n;
    }
    println!(
        "T2.2-BASELINE merchant_cleaner v{} exact={} ({:.1}%) without_aliases={} ({:.1}%) of {} named transactions",
        merchant::VERSION,
        exact,
        100.0 * exact as f64 / named as f64,
        exact_plain,
        100.0 * exact_plain as f64 / named as f64,
        named
    );
    assert!(
        exact * 2 > named,
        "most merchant names come out exactly right"
    );
}
