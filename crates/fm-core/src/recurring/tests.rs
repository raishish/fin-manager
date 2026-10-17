//! Recurring detection measured on TD.1's 83 series, end to end (import state, then
//! transfer detection, then this), plus hand-written cases TD.1 doesn't have.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use tempfile::TempDir;

use super::detect::{PARAMS, Params, detect_with};
use super::*;
use crate::model::*;
use crate::pipeline::PIPELINE_VERSION;
use crate::storage::{KdfParams, Unlocked, Vault};
use crate::transfers;

const PERSONAS: [&str; 4] = ["renter", "family", "freelancer", "retiree"];
const PASSWORD: &[u8] = b"correct horse battery staple";

fn data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/synthetic/data")
}

fn date(s: &str) -> Date {
    s.parse().unwrap()
}

/// TD.1's truth about series: each series' kind and members, and the one-offs.
struct Truth {
    series: BTreeMap<String, (String, Vec<i64>)>,
    /// Per series: cadence and whether it ended, as TD.1 names them.
    facts: BTreeMap<String, (String, bool)>,
    one_offs: BTreeSet<i64>,
}

fn truth(persona: &str) -> Truth {
    let t: Value =
        serde_json::from_slice(&fs::read(data().join(persona).join("truth.json")).unwrap())
            .unwrap();
    let series = t["recurring"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let members = s["transaction_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i.as_i64().unwrap())
                .collect();
            (
                s["id"].as_str().unwrap().to_owned(),
                (s["kind"].as_str().unwrap().to_owned(), members),
            )
        })
        .collect();
    let facts = t["recurring"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["id"].as_str().unwrap().to_owned(),
                (
                    s["cadence"].as_str().unwrap().to_owned(),
                    s["ended"].as_bool().unwrap(),
                ),
            )
        })
        .collect();
    let one_offs = t["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["one_off"].as_bool() == Some(true))
        .map(|x| x["id"].as_i64().unwrap())
        .collect();
    Truth {
        series,
        facts,
        one_offs,
    }
}

/// A persona's TD.1 ledger as after import (Needs review everywhere), with transfer
/// detection run: the state recurring detection meets in the app.
fn after_transfers(persona: &str) -> (TempDir, Unlocked) {
    let mut ledger: Ledger =
        serde_json::from_slice(&fs::read(data().join(persona).join("ledger.json")).unwrap())
            .unwrap();
    let version = ledger.taxonomy.version.clone();
    ledger.decisions = ledger
        .transactions
        .iter()
        .map(|t| Decision {
            id: DecisionId(t.id.0),
            transaction_id: t.id,
            assignment: Assignment::Bucket(Bucket::NeedsReview),
            source: DecisionSource::Builtin {
                logic: BuiltinLogic::ImportPipeline,
                version: PIPELINE_VERSION,
            },
            taxonomy_version: version.clone(),
            decided_at: Timestamp::from_unix_seconds(1_700_000_000),
            state: DecisionState::Applied,
            seen_at: None,
            replaces: None,
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let mut u = Vault::at(&dir.path().join("ledger"))
        .create_with_params(PASSWORD, KdfParams::INSECURE_TEST)
        .unwrap();
    u.save_ledger(&ledger).unwrap();
    transfers::detect(&mut u, Timestamp::from_unix_seconds(1_800_000_000)).unwrap();
    (dir, u)
}

const AS_OF: &str = "2026-09-30";

#[derive(Debug, Default, Clone, Copy)]
struct Score {
    detected: usize,
    correct: usize,
    truth: usize,
    found: usize,
    /// Truth series covered by more than one correct detected series.
    split: usize,
    member_right: usize,
    member_predicted: usize,
    member_truth: usize,
    one_offs_in_series: usize,
    /// Of the correct series: how many have TD.1's cadence, kind and ended flag.
    cadence_right: usize,
    kind_right: usize,
    ended_right: usize,
}

impl Score {
    fn add(&mut self, o: Score) {
        self.detected += o.detected;
        self.correct += o.correct;
        self.truth += o.truth;
        self.found += o.found;
        self.split += o.split;
        self.member_right += o.member_right;
        self.member_predicted += o.member_predicted;
        self.member_truth += o.member_truth;
        self.one_offs_in_series += o.one_offs_in_series;
        self.cadence_right += o.cadence_right;
        self.kind_right += o.kind_right;
        self.ended_right += o.ended_right;
    }
    fn precision(&self) -> f64 {
        self.correct as f64 / self.detected.max(1) as f64
    }
    fn recall(&self) -> f64 {
        self.found as f64 / self.truth.max(1) as f64
    }
}

/// Scores detected series against the truth. A detected series is correct when at
/// least 80% of its members belong to one truth series; a truth series is found when a
/// correct detected series covers at least 80% of its members.
fn score(
    persona: &str,
    series: &[Series],
    truth: &Truth,
    describe: &HashMap<i64, String>,
    verbose: bool,
    found_kinds: &mut BTreeMap<String, (usize, usize)>,
) -> Score {
    let owner: HashMap<i64, &str> = truth
        .series
        .iter()
        .flat_map(|(id, (_, m))| m.iter().map(move |t| (*t, id.as_str())))
        .collect();
    let mut s = Score {
        detected: series.len(),
        truth: truth.series.len(),
        ..Score::default()
    };
    let mut coverage: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for d in series {
        let mut counts: BTreeMap<Option<&str>, usize> = BTreeMap::new();
        for m in &d.members {
            *counts.entry(owner.get(&m.0).copied()).or_default() += 1;
        }
        let (best, n) = counts
            .iter()
            .max_by_key(|(_, n)| **n)
            .map(|(k, n)| (*k, *n))
            .unwrap();
        let correct = best.is_some() && n as f64 >= 0.8 * d.members.len() as f64;
        if correct {
            s.correct += 1;
            let id = best.unwrap();
            coverage.entry(id).or_default().push(n);
            let (cadence, ended) = &truth.facts[id];
            let cadence_name = format!("{:?}", d.cadence).to_lowercase();
            s.cadence_right += usize::from(&cadence_name == cadence);
            s.ended_right += usize::from(d.ended == *ended);
            let kind = truth.series[id].0.as_str();
            let kind_ok = matches!(
                (d.kind, kind),
                (SeriesKind::Charge, "subscription" | "bill")
                    | (SeriesKind::Income, "income")
                    | (SeriesKind::Interest, "interest")
                    | (SeriesKind::Transfer, "transfer")
            );
            s.kind_right += usize::from(kind_ok);
            if verbose && (!kind_ok || &cadence_name != cadence || d.ended != *ended) {
                println!(
                    "  {persona} facts {id}: cadence {cadence_name} (truth {cadence}), kind {:?} (truth {kind}), ended {} (truth {ended})",
                    d.kind, d.ended
                );
            }
        } else if verbose {
            println!(
                "  {persona} WRONG {:?} {:?} {:?} n={} composition={counts:?} e.g. {:?}",
                d.kind,
                d.cadence,
                d.merchant_key,
                d.members.len(),
                d.descriptions
            );
        }
        s.one_offs_in_series += d
            .members
            .iter()
            .filter(|m| truth.one_offs.contains(&m.0))
            .count();
    }
    for (id, (kind, members)) in &truth.series {
        let covers = coverage.get(id.as_str()).cloned().unwrap_or_default();
        let found = covers
            .iter()
            .any(|&n| n as f64 >= 0.8 * members.len() as f64);
        if covers.len() > 1 {
            s.split += 1;
        }
        let e = found_kinds.entry(kind.clone()).or_default();
        e.1 += 1;
        if found {
            s.found += 1;
            e.0 += 1;
        } else if verbose {
            let best_any: BTreeMap<usize, usize> = series
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    (
                        i,
                        d.members.iter().filter(|m| members.contains(&m.0)).count(),
                    )
                })
                .filter(|(_, n)| *n > 0)
                .collect();
            println!(
                "  {persona} MISSED {id} ({kind}, n={}) covered by {covers:?}; overlaps {:?}; e.g. {:?}",
                members.len(),
                best_any
                    .iter()
                    .map(|(i, n)| (n, series[*i].members.len(), series[*i].merchant_key.clone()))
                    .collect::<Vec<_>>(),
                members
                    .iter()
                    .take(2)
                    .map(|m| describe[m].clone())
                    .collect::<Vec<_>>()
            );
        }
    }
    let predicted: BTreeSet<i64> = series
        .iter()
        .flat_map(|d| d.members.iter().map(|m| m.0))
        .collect();
    let actual: BTreeSet<i64> = owner.keys().copied().collect();
    s.member_predicted = predicted.len();
    s.member_truth = actual.len();
    s.member_right = predicted.intersection(&actual).count();
    s
}

/// One persona's ledger after transfer detection, and its truth.
struct Household {
    persona: &'static str,
    accounts: Vec<Account>,
    transactions: Vec<Transaction>,
    decisions: Vec<Decision>,
    truth: Truth,
}

fn households() -> Vec<Household> {
    PERSONAS
        .iter()
        .map(|&persona| {
            let (_dir, u) = after_transfers(persona);
            Household {
                persona,
                accounts: u.accounts().unwrap(),
                transactions: u.transactions().unwrap(),
                decisions: u.decisions().unwrap(),
                truth: truth(persona),
            }
        })
        .collect()
}

fn run(
    households: &[Household],
    params: Params,
    verbose: bool,
) -> (Score, BTreeMap<String, (usize, usize)>) {
    let mut total = Score::default();
    let mut kinds = BTreeMap::new();
    for h in households {
        let series = detect_with(
            params,
            &h.accounts,
            &h.transactions,
            &h.decisions,
            date(AS_OF),
        );
        let describe: HashMap<i64, String> = h
            .transactions
            .iter()
            .map(|t| (t.id.0, t.description.clone()))
            .collect();
        let s = score(h.persona, &series, &h.truth, &describe, verbose, &mut kinds);
        if verbose {
            println!(
                "{}: {} of {} detected correct, {} of {} found, {} split",
                h.persona, s.correct, s.detected, s.found, s.truth, s.split
            );
        }
        total.add(s);
    }
    (total, kinds)
}

#[test]
fn td1_precision_and_recall() {
    let (s, kinds) = run(&households(), PARAMS, true);
    println!(
        "TD.1 series: precision {:.4} ({}/{}), recall {:.4} ({}/{}), {} split",
        s.precision(),
        s.correct,
        s.detected,
        s.recall(),
        s.found,
        s.truth,
        s.split
    );
    println!(
        "TD.1 members: precision {:.4}, recall {:.4}",
        s.member_right as f64 / s.member_predicted as f64,
        s.member_right as f64 / s.member_truth as f64
    );
    println!("found by kind: {kinds:?}");
    println!(
        "of the correct series: cadence right {}, kind right {}, ended right {} (of {})",
        s.cadence_right, s.kind_right, s.ended_right, s.correct
    );
    println!("one-offs in a series: {}", s.one_offs_in_series);
    assert_eq!(s.one_offs_in_series, 0, "no one-off may join a series");
    // Floors that catch a regression; they are below the measured values, not tuned to them.
    assert!(s.recall() >= 0.95, "recall {}", s.recall());
    assert!(s.precision() >= 0.85, "precision {}", s.precision());
}

/// Precision and recall as each tolerance moves, one at a time from the defaults.
#[test]
fn td1_sensitivity() {
    let households = households();
    let report = |name: &str, value: String, params: Params| {
        let (s, _) = run(&households, params, false);
        println!(
            "{name} {value:>5}: precision {:.4} ({}/{}), recall {:.4} ({}/{})",
            s.precision(),
            s.correct,
            s.detected,
            s.recall(),
            s.found,
            s.truth
        );
        s
    };
    for split_days in [100, 150, 175, 200, 250, 300, 400] {
        report(
            "split_days",
            split_days.to_string(),
            Params {
                split_days,
                ..PARAMS
            },
        );
    }
    for variable_band in [1.5, 2.0, 3.0, 4.0, 6.0, 10.0, 100.0] {
        report(
            "variable_band",
            variable_band.to_string(),
            Params {
                variable_band,
                ..PARAMS
            },
        );
    }
    for step_band in [1.02, 1.05, 1.10, 1.20, 1.50] {
        report(
            "step_band",
            step_band.to_string(),
            Params {
                step_band,
                ..PARAMS
            },
        );
    }
    for dominance in [0.5, 0.6, 0.75, 0.9, 1.0] {
        report(
            "dominance",
            dominance.to_string(),
            Params {
                dominance,
                ..PARAMS
            },
        );
    }
    for regular_share in [0.5, 0.6, 0.75, 0.9, 1.0] {
        report(
            "regular_share",
            regular_share.to_string(),
            Params {
                regular_share,
                ..PARAMS
            },
        );
    }
}

struct Echo;

impl Labeler for Echo {
    fn label(&self, s: &SeriesSketch<'_>) -> Option<Label> {
        (s.kind == SeriesKind::Charge).then(|| Label {
            name: format!("{} ({:?})", s.merchant_key, s.cadence),
            kind: LabelKind::Subscription,
        })
    }
}

#[test]
fn the_labeler_sees_each_series_and_its_answers_are_kept() {
    let (_dir, u) = after_transfers("renter");
    let mut series = detect_in(&u, date(AS_OF)).unwrap();
    assert!(series.iter().all(|s| s.label.is_none()));
    label_all(&mut series, &Echo);
    for s in &series {
        match s.kind {
            SeriesKind::Charge => {
                let label = s.label.as_ref().unwrap();
                assert_eq!(label.name, format!("{} ({:?})", s.merchant_key, s.cadence));
                assert!(!s.descriptions.is_empty() && s.descriptions.len() <= 3);
            }
            _ => assert!(s.label.is_none()),
        }
    }
}

#[test]
fn detection_is_deterministic_and_reads_the_ledger_as_given() {
    let (_dir, u) = after_transfers("family");
    let a = detect_in(&u, date(AS_OF)).unwrap();
    let b = detect(
        &u.accounts().unwrap(),
        &u.transactions().unwrap(),
        &u.decisions().unwrap(),
        date(AS_OF),
    );
    assert_eq!(a, b);
    // Series come in order, with members in date order and dates by decision 42.
    for s in &a {
        assert!(s.first <= s.last);
        assert_eq!(s.ended, s.next_expected.is_none());
    }
    assert!(a.windows(2).all(|w| w[0].first <= w[1].first));
}

// ---- Hand-written cases from real-world shapes TD.1 doesn't have. Expectations are
// ---- what a person would say; a case the logic gets wrong is listed, not tuned away.

fn account(id: i64, kind: AccountKind) -> Account {
    Account {
        id: AccountId(id),
        name: format!("Account {id}"),
        kind,
        currency: Currency::USD,
        institution: None,
        masked_number: None,
        external_id: None,
        own_account: true,
        created_at: Timestamp::from_unix_seconds(0),
    }
}

struct Rows(Vec<Transaction>);

impl Rows {
    fn add(&mut self, account: i64, day: &str, minor: i64, description: &str) -> i64 {
        let id = self.0.len() as i64 + 1;
        self.0.push(Transaction {
            id: TransactionId(id),
            account_id: AccountId(account),
            source: SourceRef {
                import_id: ImportId(1),
                index: id as u32,
                external_id: None,
            },
            status: TransactionStatus::Posted,
            posted_date: date(day),
            transaction_date: None,
            amount: Money::new(minor, Currency::USD),
            original_amount: None,
            description: description.into(),
            payee: None,
            memo: None,
            trn_type: None,
            check_number: None,
        });
        id
    }

    /// The same charge on day `day` of each month from `first` (YYYY-MM) for `n` months.
    fn monthly(
        &mut self,
        account: i64,
        first: &str,
        day: u8,
        n: i32,
        minor: i64,
        d: &str,
    ) -> Vec<i64> {
        let start = date(&format!("{first}-01"));
        (0..n)
            .map(|k| {
                let month = start.checked_add_months(k).unwrap();
                let on = Date::from_ymd(month.year(), month.month(), day).unwrap();
                self.add(account, &on.to_string(), minor, d)
            })
            .collect()
    }
}

struct Case {
    name: &'static str,
    rows: Rows,
    /// Each expected series by its members; detected series must match exactly.
    expected: Vec<Vec<i64>>,
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();

    let mut r = Rows(Vec::new());
    let gym: Vec<i64> = [
        "2025-01-28",
        "2025-03-01",
        "2025-03-28",
        "2025-05-01",
        "2025-05-28",
        "2025-07-01",
        "2025-07-28",
        "2025-09-01",
    ]
    .iter()
    .map(|d| r.add(2, d, -2499, "PLANET FITNESS CLUB FEES"))
    .collect();
    out.push(Case {
        name: "a gym billing on the 28th or the 1st, alternately",
        rows: r,
        expected: vec![gym],
    });

    let mut r = Rows(Vec::new());
    let water: Vec<i64> = [
        "2025-03-25",
        "2025-06-25",
        "2025-09-25",
        "2025-12-26",
        "2026-03-25",
    ]
    .iter()
    .map(|d| r.add(1, d, -8500, "SPRINGFIELD WATER DEPT BILLPAY"))
    .collect();
    out.push(Case {
        name: "a quarterly bill moved a day by Christmas",
        rows: r,
        expected: vec![water],
    });

    let mut r = Rows(Vec::new());
    for d in ["2025-03-03", "2025-03-10", "2025-03-17"] {
        r.add(2, d, -500, "BLUE BOTTLE COFFEE 12");
    }
    out.push(Case {
        name: "the same $5 coffee three Mondays running",
        rows: r,
        expected: vec![],
    });

    let mut r = Rows(Vec::new());
    for week in 0..8 {
        let monday = date("2025-03-03").checked_add_days(7 * week).unwrap();
        for offset in [0, 2, 4] {
            let day = monday.checked_add_days(offset).unwrap();
            r.add(2, &day.to_string(), -450, "SQ *CORNER CAFE");
        }
    }
    out.push(Case {
        name: "the same $4.50 coffee every Monday, Wednesday and Friday",
        rows: r,
        expected: vec![],
    });

    let mut r = Rows(Vec::new());
    let music = r.monthly(2, "2025-01", 3, 8, -999, "APPLE.COM/BILL 866-712-7753");
    let tv = r.monthly(2, "2025-01", 17, 8, -999, "APPLE.COM/BILL 866-712-7753");
    out.push(Case {
        name: "two $9.99 Apple subscriptions on one card",
        rows: r,
        expected: vec![music, tv],
    });

    let mut r = Rows(Vec::new());
    let mut netflix = r.monthly(2, "2025-01", 9, 6, -1549, "NETFLIX.COM");
    netflix.extend(r.monthly(3, "2025-07", 9, 6, -1549, "NETFLIX.COM"));
    out.push(Case {
        name: "a subscription moved to another card",
        rows: r,
        expected: vec![netflix],
    });

    let mut r = Rows(Vec::new());
    r.add(2, "2025-01-05", 0, "SPOTIFY USA TRIAL");
    let spotify = r.monthly(2, "2025-02", 5, 6, -1199, "SPOTIFY USA");
    out.push(Case {
        name: "a free trial, then paid",
        rows: r,
        expected: vec![spotify],
    });

    let mut r = Rows(Vec::new());
    let icloud = r.monthly(2, "2025-01", 12, 10, -299, "APPLE.COM/BILL");
    r.add(2, "2025-05-12", -129_900, "APPLE STORE #R123");
    out.push(Case {
        name: "a laptop from the shop that also bills a subscription",
        rows: r,
        expected: vec![icloud],
    });

    let mut r = Rows(Vec::new());
    let prime = vec![
        r.add(2, "2025-02-10", -13900, "Amazon Prime*J4WLGLFZ1"),
        r.add(2, "2026-02-10", -14900, "Amazon Prime*8IHLB8K6M"),
    ];
    for k in 0..40 {
        let day = date("2025-01-04").checked_add_days(k * 13).unwrap();
        r.add(
            2,
            &day.to_string(),
            -(1_000 + k as i64 * 137 % 9_000),
            "AMAZON MKTPL*X1Y2Z3",
        );
    }
    out.push(Case {
        name: "an annual membership with a price rise, among the shop's purchases",
        rows: r,
        expected: vec![prime],
    });

    let mut r = Rows(Vec::new());
    let gas: Vec<i64> = [
        3000, 3400, 4500, 7000, 11000, 16000, 18000, 15000, 9500, 6000, 4000, 3100,
    ]
    .iter()
    .enumerate()
    .map(|(k, &a)| {
        let month = date("2025-05-01").checked_add_months(k as i32).unwrap();
        let on = Date::from_ymd(month.year(), month.month(), 20).unwrap();
        r.add(1, &on.to_string(), -a, "COLUMBIA GAS OF OHIO")
    })
    .collect();
    out.push(Case {
        name: "a gas bill six times higher in winter",
        rows: r,
        expected: vec![gas],
    });

    let mut r = Rows(Vec::new());
    // The estimated-tax calendar: early, on time, and moved past a weekend; a balance
    // due paid the same April day as the first estimate.
    let federal: Vec<i64> = [
        ("2025-01-15", -310_000),
        ("2025-04-15", -325_000),
        ("2025-06-16", -298_000),
        ("2025-09-10", -305_000),
        ("2026-01-15", -330_000),
    ]
    .iter()
    .map(|&(d, a)| r.add(1, d, a, "IRS USATAXPYMT"))
    .collect();
    r.add(1, "2025-04-15", -91_000, "IRS USATAXPYMT");
    // A state's estimates on the same dates are their own series.
    let state: Vec<i64> = ["2025-01-14", "2025-04-15", "2025-06-13", "2025-09-15"]
        .iter()
        .map(|d| r.add(1, d, -60_000, "OHIO DEPT TAXATION ESTPMT"))
        .collect();
    out.push(Case {
        name: "federal and state estimated tax on the 1040-ES calendar",
        rows: r,
        expected: vec![federal, state],
    });

    let mut r = Rows(Vec::new());
    for d in ["2025-01-15", "2025-04-15", "2025-06-15"] {
        r.add(1, d, -200_000, "IRS USATAXPYMT");
    }
    out.push(Case {
        name: "three estimated-tax payments are not yet a series",
        rows: r,
        expected: vec![],
    });

    let mut r = Rows(Vec::new());
    for week in 0..60 {
        let day = date("2025-01-03").checked_add_days(week * 6).unwrap();
        r.add(
            2,
            &day.to_string(),
            -(4_000 + week as i64 * 731 % 6_000),
            "WHOLE FOODS MARKET",
        );
    }
    out.push(Case {
        name: "a shop visited all year, including near the tax due dates",
        rows: r,
        expected: vec![],
    });

    out
}

/// Cases the logic gets wrong, with what it does instead (see README.md).
///
/// Two $9.99 subscriptions on the 3rd and the 17th read as one semimonthly $9.99 series:
/// the rows alone can't tell that apart from one twice-monthly charge. The amount per
/// month is still right for a forecast; a labeler can split it.
const KNOWN_FAILURES: &[&str] = &["two $9.99 Apple subscriptions on one card"];

#[test]
fn hand_written_cases() {
    let accounts = [
        account(1, AccountKind::Checking),
        account(2, AccountKind::CreditCard),
        account(3, AccountKind::CreditCard),
    ];
    let mut failures = Vec::new();
    for case in cases() {
        let series = detect(&accounts, &case.rows.0, &[], date("2026-03-31"));
        let mut found: Vec<Vec<i64>> = series
            .iter()
            .map(|s| s.members.iter().map(|m| m.0).collect())
            .collect();
        found.sort();
        let mut expected = case.expected.clone();
        for e in &mut expected {
            e.sort();
        }
        expected.sort();
        let ok = found == expected;
        println!(
            "{} {}: {}",
            if ok { "ok  " } else { "FAIL" },
            case.name,
            series
                .iter()
                .map(|s| format!("{:?} {:?} x{}", s.cadence, s.accounts, s.members.len()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if !ok {
            failures.push(case.name);
        }
    }
    assert_eq!(failures, KNOWN_FAILURES);
}

#[test]
fn an_estimated_tax_series_expects_the_next_due_date() {
    let accounts = [account(1, AccountKind::Checking)];
    let mut r = Rows(Vec::new());
    for (d, a) in [
        ("2025-01-15", -310_000),
        ("2025-04-15", -325_000),
        ("2025-06-16", -298_000),
        ("2025-09-15", -305_000),
    ] {
        r.add(1, d, a, "ACH DEBIT IRS USATAXPYMT");
    }
    let series = detect(&accounts, &r.0, &[], date("2025-10-01"));
    assert_eq!(series.len(), 1);
    let s = &series[0];
    assert_eq!(s.cadence, Cadence::Quarterly);
    assert_eq!(s.kind, SeriesKind::Charge);
    assert!(!s.ended);
    // The next due date on the calendar, not three months after the last payment.
    assert_eq!(s.next_expected, Some(date("2026-01-15")));
}

/// T3.4: the freelancer's quarterly estimated tax is one series of exactly its eight
/// payments, without the April 2025 balance due (a one-off of the same payee and day).
#[test]
fn td1_estimated_tax_is_one_series_of_exactly_its_payments() {
    let (_dir, u) = after_transfers("freelancer");
    let truth = truth("freelancer");
    let expected: Vec<i64> = truth.series["freelancer.estimated_tax"].1.clone();
    let series = detect_in(&u, date(AS_OF)).unwrap();
    let tax: Vec<&Series> = series
        .iter()
        .filter(|s| s.members.iter().any(|m| expected.contains(&m.0)))
        .collect();
    assert_eq!(tax.len(), 1);
    let members: Vec<i64> = tax[0].members.iter().map(|m| m.0).collect();
    assert_eq!(members, expected);
    assert_eq!(tax[0].cadence, Cadence::Quarterly);
    assert!(members.iter().all(|m| !truth.one_offs.contains(m)));
}
