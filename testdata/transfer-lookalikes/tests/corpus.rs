//! Checks on the transfer look-alike corpus: fresh and deterministic, readable as
//! fm-core ledgers, and its truth complete when re-derived from the ledgers alone.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use fm_core::model::{AccountKind, Ledger};
use fm_synth::truth::TruthBucket;
use fm_transfer_lookalikes::truth::HouseholdTruth;
use fm_transfer_lookalikes::{Manifest, SEED, render};

type Files = BTreeMap<String, Vec<u8>>;

fn files() -> Files {
    render(SEED)
}

fn households(f: &Files) -> Vec<(Ledger, HouseholdTruth)> {
    let m: Manifest = serde_json::from_slice(&f["manifest.json"]).unwrap();
    m.households
        .iter()
        .map(|h| {
            (
                serde_json::from_slice(&f[&h.ledger]).unwrap(),
                serde_json::from_slice(&f[&h.truth]).unwrap(),
            )
        })
        .collect()
}

#[test]
fn committed_data_is_fresh_and_deterministic() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    let f = files();
    for (path, bytes) in &f {
        let on_disk = std::fs::read(dir.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(
            &on_disk == bytes,
            "data/{path} is out of date: run `cargo run --release`"
        );
    }
    assert!(render(SEED) == f, "deterministic");
    let other = render(SEED + 1);
    assert!(
        other["alex/ledger.json"] != f["alex/ledger.json"],
        "the seed matters"
    );
}

#[test]
fn ledgers_are_well_formed_with_fake_numbers() {
    for (ledger, truth) in households(&files()) {
        ledger.taxonomy.validate().unwrap();
        assert!(ledger.decisions.is_empty() && ledger.rules.is_empty());
        assert_eq!(ledger.transactions.len(), truth.transactions.len());
        for (t, x) in ledger.transactions.iter().zip(&truth.transactions) {
            assert_eq!(t.id.0, x.id);
        }
        for a in &ledger.accounts {
            assert!(a.own_account);
            assert!(
                a.external_id.as_deref().unwrap().starts_with("9999"),
                "{}: fake number",
                truth.household
            );
        }
    }
}

/// Re-derives every candidate from the ledger alone and checks the truth names each one
/// exactly once: as a true pair or as a look-alike.
#[test]
fn truth_lists_every_candidate_exactly_once() {
    for (ledger, truth) in households(&files()) {
        let window = i64::from(truth.candidate_window_days);
        let mut found = BTreeSet::new();
        for o in &ledger.transactions {
            for i in &ledger.transactions {
                let lag = i64::from(i.posted_date.days_since(o.posted_date));
                if o.amount.is_negative()
                    && i.amount.minor() == -o.amount.minor()
                    && o.account_id != i.account_id
                    && (0..=window).contains(&lag)
                {
                    found.insert((o.id.0, i.id.0));
                }
            }
        }
        let pairs: BTreeSet<(i64, i64)> = truth
            .transfer_pairs
            .iter()
            .map(|p| (p.out, p.into))
            .collect();
        let looks: BTreeSet<(i64, i64)> =
            truth.lookalikes.iter().map(|c| (c.out, c.into)).collect();
        assert!(
            pairs.is_disjoint(&looks),
            "{}: a pair listed as both",
            truth.household
        );
        let listed: BTreeSet<(i64, i64)> = pairs.union(&looks).copied().collect();
        assert_eq!(
            found, listed,
            "{}: candidates in the ledger vs the truth",
            truth.household
        );
    }
}

#[test]
fn true_pairs_net_to_zero_and_point_at_each_other() {
    for (ledger, truth) in households(&files()) {
        let tx: BTreeMap<i64, _> = ledger.transactions.iter().map(|t| (t.id.0, t)).collect();
        let kind: BTreeMap<_, _> = ledger.accounts.iter().map(|a| (a.id, a.kind)).collect();
        let bucket: BTreeMap<i64, &TruthBucket> = truth
            .transactions
            .iter()
            .map(|x| (x.id, &x.bucket))
            .collect();
        for p in &truth.transfer_pairs {
            let (o, i) = (tx[&p.out], tx[&p.into]);
            assert_eq!(
                o.amount.minor() + i.amount.minor(),
                0,
                "{}: nets to zero",
                truth.household
            );
            assert!((0..=5).contains(&p.lag_days), "{}: lag", truth.household);
            assert!(
                matches!(bucket[&p.out], TruthBucket::Transfer { counterpart: Some(c) } if c.0 == p.into)
            );
            assert!(
                matches!(bucket[&p.into], TruthBucket::Transfer { counterpart: Some(c) } if c.0 == p.out)
            );
            let (ko, ki) = (kind[&o.account_id], kind[&i.account_id]);
            let want = match (ko == AccountKind::CreditCard, ki == AccountKind::CreditCard) {
                (false, true) => "card_payment",
                (true, true) => "card_to_card",
                (false, false) => "own_transfer",
                (true, false) => panic!("money from a card to a bank account isn't scripted"),
            };
            assert_eq!(p.kind, want, "{}: kind", truth.household);
            assert!(["strong", "one_sided", "none"].contains(&p.wording.as_str()));
        }
        // Look-alike sides are never each other's counterpart.
        for c in &truth.lookalikes {
            assert!(
                !matches!(bucket[&c.out], TruthBucket::Transfer { counterpart: Some(x) } if x.0 == c.into)
            );
        }
    }
}

#[test]
fn every_pattern_is_well_represented() {
    let f = files();
    let m: Manifest = serde_json::from_slice(&f["manifest.json"]).unwrap();
    let looks: usize = m
        .lookalikes_by_pattern
        .iter()
        .filter(|(p, _)| p.as_str() != "incidental")
        .map(|(_, n)| n)
        .sum();
    let pairs: usize = m.pairs_by_pattern.values().sum();
    assert!(looks >= 200, "{looks} look-alike candidates");
    assert!(pairs >= 50, "{pairs} true pairs");
    for (pattern, min) in [
        ("card_refund_vs_bill", 20),
        ("deposit_vs_savings_outflow", 10),
        ("bill_vs_savings_move", 20),
        ("zelle_out_vs_zelle_in", 20),
        ("balance_transfer_vs_payments", 8),
        ("rent_split_repayment", 10),
        ("payroll_vs_equal_transfer", 10),
        ("card_charge_vs_deposit", 20),
        ("equal_recurring_crossed", 20),
        ("pmt_bill_vs_savings_move", 20),
    ] {
        let n = m.lookalikes_by_pattern.get(pattern).copied().unwrap_or(0);
        assert!(n >= min, "{pattern}: {n} look-alikes");
    }
    for (pattern, min) in [
        ("card_payment", 30),
        ("savings_move", 30),
        ("equal_recurring", 20),
        ("weak_one_sided", 6),
        ("weak_none", 6),
        ("long_weekend", 6),
        ("balance_transfer", 2),
    ] {
        let n = m.pairs_by_pattern.get(pattern).copied().unwrap_or(0);
        assert!(n >= min, "{pattern}: {n} pairs");
    }
    let all = households(&f);
    let lags: BTreeSet<i32> = all
        .iter()
        .flat_map(|(_, t)| {
            t.transfer_pairs
                .iter()
                .filter(|p| p.pattern == "long_weekend")
                .map(|p| p.lag_days)
        })
        .collect();
    assert_eq!(
        lags,
        BTreeSet::from([4, 5]),
        "long-weekend landings 4 and 5 days later"
    );
    for (_, t) in &all {
        for c in &t.lookalikes {
            if c.pattern == "zelle_out_vs_zelle_in" {
                assert!(c.flags.contains(&"p2p".to_string()));
            }
            if c.pattern == "card_charge_vs_deposit" {
                assert!(c.flags.contains(&"leaves_card".to_string()));
            }
        }
    }
}

/// How hard the corpus is: a matcher that pairs on amount and date alone (nearest date
/// first, each transaction once) gets a poor precision here. Reported, and held below a
/// ceiling so the corpus keeps its teeth.
#[test]
fn amount_and_date_alone_is_not_enough() {
    let (mut right, mut wrong, mut total_true) = (0usize, 0usize, 0usize);
    for (_, truth) in households(&files()) {
        let truths: BTreeSet<(i64, i64)> = truth
            .transfer_pairs
            .iter()
            .map(|p| (p.out, p.into))
            .collect();
        total_true += truths.len();
        let mut candidates: Vec<(i32, i64, i64)> = truth
            .transfer_pairs
            .iter()
            .map(|p| (p.lag_days, p.out, p.into))
            .chain(truth.lookalikes.iter().map(|c| (c.lag_days, c.out, c.into)))
            .collect();
        candidates.sort_unstable();
        let mut used = BTreeSet::new();
        for (_, o, i) in candidates {
            if used.contains(&o) || used.contains(&i) {
                continue;
            }
            used.insert(o);
            used.insert(i);
            if truths.contains(&(o, i)) {
                right += 1;
            } else {
                wrong += 1;
            }
        }
    }
    let precision = right as f64 / (right + wrong) as f64;
    println!(
        "amount+date only: {right} right, {wrong} wrong of {total_true} true pairs, precision {precision:.3}"
    );
    assert!(
        wrong >= 40,
        "look-alikes should fool an amount-and-date matcher ({wrong} wrong)"
    );
}
