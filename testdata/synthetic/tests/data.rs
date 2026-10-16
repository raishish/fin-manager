//! Checks on the generated data: determinism, the fm-core JSON form, and that the
//! ground truth is consistent with the ledger and with itself.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use fm_core::model::{AccountKind, Ledger, Transaction, TransactionId, TransactionStatus};
use fm_synth::catalog::FAKE_SURNAMES;
use fm_synth::truth::{AmountTruth, PairKind, PersonaTruth, TruthBucket};
use fm_synth::{DEFAULT_SEED, Generated, Manifest, generate, render};

fn generated() -> Vec<Generated> {
    generate(DEFAULT_SEED)
}

fn by_id(ledger: &Ledger) -> BTreeMap<TransactionId, &Transaction> {
    ledger.transactions.iter().map(|t| (t.id, t)).collect()
}

#[test]
fn same_seed_same_bytes_and_other_seeds_differ() {
    let a = render(DEFAULT_SEED);
    let b = render(DEFAULT_SEED);
    assert_eq!(a, b);
    let c = render(DEFAULT_SEED + 1);
    assert_eq!(a.keys().collect::<Vec<_>>(), c.keys().collect::<Vec<_>>());
    for (path, bytes) in &a {
        if path != "manifest.json" {
            assert_ne!(bytes, &c[path], "{path} ignores the seed");
        }
    }
}

#[test]
fn committed_data_matches_the_generator() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    for (path, bytes) in render(DEFAULT_SEED) {
        let on_disk = std::fs::read(dir.join(&path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(
            on_disk == bytes,
            "data/{path} is out of date: run `cargo run --release`"
        );
    }
}

#[test]
fn files_round_trip_through_fm_core_types() {
    let files = render(DEFAULT_SEED);
    let manifest: Manifest = serde_json::from_slice(&files["manifest.json"]).unwrap();
    assert_eq!(manifest.personas.len(), 4);
    for g in generated() {
        let ledger: Ledger =
            serde_json::from_slice(&files[&format!("{}/ledger.json", g.key)]).unwrap();
        assert_eq!(ledger, g.ledger);
        let truth: PersonaTruth =
            serde_json::from_slice(&files[&format!("{}/truth.json", g.key)]).unwrap();
        assert_eq!(truth, g.truth);
        let entry = manifest.personas.iter().find(|p| p.key == g.key).unwrap();
        assert_eq!(entry.transactions, ledger.transactions.len());
        assert_eq!(
            entry.ledger.sha256,
            fm_core::model::Sha256Hash::of(&files[&entry.ledger.path])
        );
    }
}

#[test]
fn ledgers_are_well_formed() {
    for g in generated() {
        let l = &g.ledger;
        l.taxonomy.validate().unwrap();
        assert_eq!(
            l.taxonomy,
            fm_synth::taxonomy(),
            "{}: uses the shipped taxonomy",
            g.key
        );
        assert!(
            l.decisions.is_empty() && l.rules.is_empty(),
            "{}: the ledger carries no answers",
            g.key
        );
        let accounts: BTreeMap<_, _> = l.accounts.iter().map(|a| (a.id, a)).collect();
        assert!(l.accounts.iter().all(|a| a.own_account));
        let imports: BTreeSet<_> = l.imports.iter().map(|i| i.id).collect();
        for (n, t) in l.transactions.iter().enumerate() {
            assert_eq!(
                t.id,
                TransactionId(n as i64 + 1),
                "{}: ids are 1..n in order",
                g.key
            );
            let acct = accounts[&t.account_id];
            assert_eq!(t.amount.currency(), acct.currency);
            assert!(imports.contains(&t.source.import_id));
            assert!(!t.amount.is_zero(), "{}: zero amount on {}", g.key, t.id.0);
            assert!(!t.description.trim().is_empty());
            assert!(t.posted_date >= fm_synth::start() && t.posted_date <= fm_synth::end());
            if let Some(o) = t.original_amount {
                assert_ne!(o.currency(), t.amount.currency());
                assert_eq!(o.is_negative(), t.amount.is_negative());
            }
            if t.status == TransactionStatus::Pending {
                assert!(
                    fm_synth::end().days_since(t.posted_date) <= 3,
                    "{}: old pending {}",
                    g.key,
                    t.id.0
                );
            }
        }
        // Per-account source indexes are 0..n.
        let mut per_import: BTreeMap<_, Vec<u32>> = BTreeMap::new();
        for t in &l.transactions {
            per_import
                .entry(t.source.import_id)
                .or_default()
                .push(t.source.index);
        }
        for idx in per_import.values() {
            assert!(idx.iter().enumerate().all(|(i, &x)| x as usize == i));
        }
        // Every one of the 24 months is populated.
        let mut per_month: BTreeMap<_, usize> = BTreeMap::new();
        for t in &l.transactions {
            *per_month.entry(t.posted_date.year_month()).or_default() += 1;
        }
        assert_eq!(per_month.len(), 24, "{}", g.key);
        assert!(
            per_month.values().all(|&n| n >= 25),
            "{}: thin month {per_month:?}",
            g.key
        );
    }
}

#[test]
fn truth_covers_every_transaction_with_known_keys() {
    let keys: BTreeSet<String> = fm_synth::taxonomy()
        .categories
        .into_iter()
        .map(|c| c.key)
        .collect();
    for g in generated() {
        assert_eq!(g.truth.transactions.len(), g.ledger.transactions.len());
        for (t, x) in g.ledger.transactions.iter().zip(&g.truth.transactions) {
            assert_eq!(t.id, x.id);
            match &x.bucket {
                TruthBucket::Category { key } => {
                    assert!(keys.contains(key), "{key}");
                    assert!(
                        t.amount.is_negative(),
                        "{}: spending {} is money out",
                        g.key,
                        t.id.0
                    );
                }
                TruthBucket::Refund { key, .. } => {
                    assert!(keys.contains(key), "{key}");
                    assert!(
                        t.amount.is_positive(),
                        "{}: refund {} is money in",
                        g.key,
                        t.id.0
                    );
                }
                TruthBucket::Income => {
                    assert!(t.amount.is_positive(), "{}: income {}", g.key, t.id.0)
                }
                TruthBucket::Transfer { .. } => {}
            }
            assert_eq!(
                x.tags.contains(&"pending".to_string()),
                t.status == TransactionStatus::Pending
            );
        }
    }
}

#[test]
fn transfer_pairs_net_to_zero_and_agree_with_buckets() {
    for g in generated() {
        let tx = by_id(&g.ledger);
        let kinds: BTreeMap<_, _> = g.ledger.accounts.iter().map(|a| (a.id, a.kind)).collect();
        let truth: BTreeMap<_, _> = g.truth.transactions.iter().map(|x| (x.id, x)).collect();
        let mut paired = BTreeSet::new();
        for p in &g.truth.transfer_pairs {
            let (o, i) = (tx[&p.out], tx[&p.into]);
            assert!(o.amount.is_negative() && i.amount.is_positive());
            assert_eq!(
                o.amount.checked_add(i.amount).unwrap().minor(),
                0,
                "{}: pair {} nets to zero",
                g.key,
                p.out.0
            );
            assert_ne!(o.account_id, i.account_id);
            assert!(i.posted_date >= o.posted_date && i.posted_date.days_since(o.posted_date) <= 5);
            assert_eq!(
                truth[&p.out].bucket,
                TruthBucket::Transfer {
                    counterpart: Some(p.into)
                }
            );
            assert_eq!(
                truth[&p.into].bucket,
                TruthBucket::Transfer {
                    counterpart: Some(p.out)
                }
            );
            assert_eq!(
                p.kind == PairKind::CardPayment,
                kinds[&i.account_id] == AccountKind::CreditCard
            );
            assert!(
                paired.insert(p.out) && paired.insert(p.into),
                "{}: a transaction in two pairs",
                g.key
            );
        }
        for x in &g.truth.transactions {
            if let TruthBucket::Transfer {
                counterpart: Some(_),
            } = x.bucket
            {
                assert!(
                    paired.contains(&x.id),
                    "{}: {} links outside a pair",
                    g.key,
                    x.id.0
                );
            }
        }
    }
}

#[test]
fn refunds_point_at_earlier_purchases_in_the_same_category() {
    for g in generated() {
        let tx = by_id(&g.ledger);
        let truth: BTreeMap<_, _> = g.truth.transactions.iter().map(|x| (x.id, x)).collect();
        for x in &g.truth.transactions {
            if let TruthBucket::Refund {
                key,
                original: Some(o),
            } = &x.bucket
            {
                let (r, p) = (tx[&x.id], tx[o]);
                assert_eq!(r.account_id, p.account_id);
                assert!(p.posted_date <= r.posted_date);
                assert!(r.amount.minor() <= -p.amount.minor());
                assert_eq!(truth[o].bucket, TruthBucket::Category { key: key.clone() });
            }
        }
    }
}

#[test]
fn recurring_series_match_their_transactions() {
    for g in generated() {
        let tx = by_id(&g.ledger);
        let mut tagged: BTreeMap<&str, BTreeSet<TransactionId>> = BTreeMap::new();
        for x in &g.truth.transactions {
            if let Some(id) = &x.recurring {
                tagged.entry(id).or_default().insert(x.id);
            }
        }
        assert_eq!(tagged.len(), g.truth.recurring.len(), "{}", g.key);
        for s in &g.truth.recurring {
            let ids: BTreeSet<_> = s.transaction_ids.iter().copied().collect();
            assert_eq!(&ids, &tagged[s.id.as_str()], "{}", s.id);
            for id in &s.transaction_ids {
                let t = tx[id];
                assert_eq!(t.account_id, s.account_id, "{}", s.id);
                match &s.amount {
                    AmountTruth::Fixed { steps } => {
                        let date = t.transaction_date.unwrap_or(t.posted_date);
                        let step = steps
                            .iter()
                            .rev()
                            .find(|st| st.from <= date)
                            .unwrap_or(&steps[0]);
                        assert_eq!(t.amount, step.amount, "{} on {}", s.id, date);
                    }
                    AmountTruth::Variable { min, max } => {
                        assert!(t.amount.minor() >= min.minor() && t.amount.minor() <= max.minor());
                    }
                }
            }
            assert!(s.first <= s.last);
        }
    }
}

#[test]
fn people_are_clearly_fake() {
    for g in generated() {
        for (t, x) in g.ledger.transactions.iter().zip(&g.truth.transactions) {
            if !x.tags.contains(&"p2p".to_string()) {
                continue;
            }
            let name = x.merchant.as_deref().unwrap();
            assert!(
                FAKE_SURNAMES.iter().any(|s| name.ends_with(s)),
                "{}: transaction {} pays someone not on the fake list",
                g.key,
                t.id.0
            );
            if !x.tags.contains(&"no_counterparty_name".to_string()) {
                let d = t.description.to_uppercase();
                let surname = name.rsplit(' ').next().unwrap().to_uppercase();
                let shown = d.contains(&surname) || d.contains(&surname[..surname.len().min(8)]);
                assert!(
                    shown,
                    "{}: transaction {} doesn't show the payee",
                    g.key, t.id.0
                );
            }
        }
    }
}

#[test]
fn hard_cases_are_present() {
    let all = generated();
    let tags: BTreeSet<String> = all
        .iter()
        .flat_map(|g| g.truth.transactions.iter().flat_map(|x| x.tags.clone()))
        .collect();
    for expected in [
        "pending",
        "tip_adjusted",
        "pump_hold",
        "refund",
        "partial_refund",
        "card_payment",
        "partial_payment",
        "card_interest",
        "foreign_currency",
        "foreign_fee",
        "lagged_transfer",
        "untracked_counterpart",
        "near_miss_transfer",
        "ambiguous_descriptor",
        "check",
        "p2p",
        "no_counterparty_name",
        "reimbursement",
        "atm",
    ] {
        assert!(tags.contains(expected), "no {expected} anywhere");
    }
    for g in &all {
        let t = &g.truth;
        assert!(
            t.transactions.iter().any(|x| x.one_off),
            "{}: no one-offs",
            g.key
        );
        assert!(
            t.transfer_pairs
                .iter()
                .any(|p| p.kind == PairKind::CardPayment),
            "{}",
            g.key
        );
        assert!(
            t.transfer_pairs
                .iter()
                .any(|p| p.kind == PairKind::OwnTransfer),
            "{}",
            g.key
        );
        assert!(
            t.recurring
                .iter()
                .any(|s| matches!(&s.amount, AmountTruth::Fixed { steps } if steps.len() > 1)),
            "{}: no price change",
            g.key
        );
        assert!(t.recurring.iter().any(|s| s.kind == "income"), "{}", g.key);
    }
    assert!(
        all.iter()
            .any(|g| g.truth.recurring.iter().any(|s| s.ended)),
        "no series ends"
    );
    let used: BTreeSet<String> = all
        .iter()
        .flat_map(|g| g.truth.transactions.iter())
        .filter_map(|x| match &x.bucket {
            TruthBucket::Category { key } | TruthBucket::Refund { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect();
    let unused: Vec<String> = fm_synth::taxonomy()
        .categories
        .into_iter()
        .map(|c| c.key)
        .filter(|k| !used.contains(k))
        .collect();
    assert_eq!(
        unused,
        ["other.misc"],
        "every specific category appears somewhere"
    );
    let currencies: BTreeSet<&str> = all
        .iter()
        .flat_map(|g| {
            g.ledger
                .transactions
                .iter()
                .filter_map(|t| t.original_amount.map(|o| o.currency().code()))
        })
        .collect();
    assert_eq!(
        currencies,
        BTreeSet::from(["EUR", "JPY"]),
        "a 2-decimal and a 0-decimal currency"
    );
}
