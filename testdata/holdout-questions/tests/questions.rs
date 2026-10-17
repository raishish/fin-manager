//! Checks on the held-out set, as TD.3's: it is fresh, well formed, apart from TD.3, and
//! its answers agree with TD.1's ledger when recomputed independently of the generator.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use fm_core::model::{Date, Ledger, Transaction, TransactionStatus};
use fm_holdout_questions::{build, render};
use fm_questions::{Answer, QuestionSet, Rule};
use fm_synth::truth::{PersonaTruth, TruthBucket};

fn set() -> QuestionSet {
    build()
}

fn td1() -> (Ledger, PersonaTruth) {
    fm_questions::facts::load(&fm_questions::facts::synthetic_data_dir())
}

fn answer<'a>(s: &'a QuestionSet, id: &str, name: &str) -> &'a Answer {
    let q = s
        .questions
        .iter()
        .find(|q| q.id == id)
        .unwrap_or_else(|| panic!("no {id}"));
    q.answers
        .iter()
        .find(|a| match a {
            Answer::Money { name: n, .. }
            | Answer::Count { name: n, .. }
            | Answer::Percent { name: n, .. }
            | Answer::Date { name: n, .. }
            | Answer::Text { name: n, .. }
            | Answer::Texts { name: n, .. } => n == name,
        })
        .unwrap_or_else(|| panic!("{id} has no {name}"))
}

fn minor(a: &Answer) -> i64 {
    match a {
        Answer::Money { amount, .. } => amount.minor(),
        _ => panic!("not money"),
    }
}

#[test]
fn committed_file_is_fresh_and_deterministic() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/questions.json");
    let on_disk = std::fs::read(&path).expect("data/questions.json exists");
    let fresh = render();
    assert!(
        on_disk == fresh,
        "data/questions.json is out of date: run `cargo run --release`"
    );
    assert!(render() == fresh, "rendering is deterministic");
    let parsed: QuestionSet = serde_json::from_slice(&on_disk).unwrap();
    assert_eq!(parsed, set());
}

/// About 25 questions in TD.3's proportions (a fifth totals, a fifth hard, the rest
/// even), every one with a query, none a repeat of a TD.3 question.
#[test]
fn twenty_five_questions_in_td3_proportions_and_apart_from_it() {
    let s = set();
    assert_eq!(s.questions.len(), 25);
    let ids: BTreeSet<&str> = s.questions.iter().map(|q| q.id.as_str()).collect();
    let texts: BTreeSet<String> = s
        .questions
        .iter()
        .map(|q| q.question.to_lowercase())
        .collect();
    assert_eq!(ids.len(), 25);
    assert_eq!(texts.len(), 25, "no repeated questions");
    let td3: BTreeSet<String> = fm_questions::build()
        .questions
        .iter()
        .map(|q| q.question.to_lowercase())
        .collect();
    for t in &texts {
        assert!(!td3.contains(t), "a TD.3 question: {t}");
    }
    let mut types: BTreeMap<&str, usize> = BTreeMap::new();
    for q in &s.questions {
        *types.entry(q.kind.as_str()).or_default() += 1;
        assert!(!q.answers.is_empty(), "{} has no answer", q.id);
        assert!(q.expected_query.is_some(), "{}: no query", q.id);
        assert!(q.null_reason.is_none() && q.known_gap.is_none(), "{}", q.id);
    }
    assert_eq!(
        types,
        BTreeMap::from([
            ("comparisons", 4),
            ("hard", 4),
            ("merchants", 4),
            ("recurring", 4),
            ("time_ranges", 4),
            ("totals", 5),
        ])
    );
    let tags: BTreeSet<&str> = s
        .questions
        .iter()
        .flat_map(|q| q.tags.iter().map(String::as_str))
        .collect();
    for t in [
        "refunds",
        "transfers",
        "relative_date",
        "income",
        "percent",
        "price_change",
    ] {
        assert!(tags.contains(t), "no question tagged {t}");
    }
    assert_eq!(
        s.conventions,
        fm_questions::build().conventions,
        "TD.3's rules"
    );
    assert_eq!(s.today, fm_questions::facts::today());
}

/// Re-adds every answer's evidence straight from TD.1's ledger.
#[test]
fn every_evidence_list_adds_up_to_its_answer() {
    let (ledger, _) = td1();
    let tx: BTreeMap<i64, &Transaction> = ledger.transactions.iter().map(|t| (t.id.0, t)).collect();
    let s = set();
    let mut checked = 0;
    for q in &s.questions {
        for a in &q.answers {
            let (value, evidence) = match a {
                Answer::Money {
                    amount, evidence, ..
                } => (amount.minor(), evidence),
                Answer::Count {
                    value, evidence, ..
                } => (*value, evidence),
                _ => continue,
            };
            let ids = &evidence.transaction_ids;
            assert_eq!(
                ids.iter().collect::<BTreeSet<_>>().len(),
                ids.len(),
                "{}: repeated evidence",
                q.id
            );
            let sum = |f: &dyn Fn(&Transaction) -> i64| ids.iter().map(|i| f(tx[i])).sum::<i64>();
            let expected = match evidence.rule {
                Rule::NegatedSum => -sum(&|t| t.amount.minor()),
                Rule::Sum => sum(&|t| t.amount.minor()),
                Rule::NegatedOriginalSum => {
                    -sum(&|t| t.original_amount.expect("original amount").minor())
                }
                Rule::Count => ids.len() as i64,
                Rule::Derived => continue,
            };
            assert_eq!(value, expected, "{}: evidence doesn't add up", q.id);
            checked += 1;
        }
    }
    assert!(checked >= 25, "only {checked} answers carry evidence");
}

/// The date a transaction counts on and whether totals include it, written out again
/// from the conventions rather than taken from the generator.
fn counts_in(t: &Transaction, start: Date, end: Date) -> bool {
    let d = t.transaction_date.unwrap_or(t.posted_date);
    t.status == TransactionStatus::Posted && d >= start && d <= end
}

fn spending_between(ledger: &Ledger, truth: &PersonaTruth, start: Date, end: Date) -> i64 {
    ledger
        .transactions
        .iter()
        .zip(&truth.transactions)
        .filter(|(t, x)| {
            counts_in(t, start, end)
                && matches!(
                    x.bucket,
                    TruthBucket::Category { .. } | TruthBucket::Refund { .. }
                )
        })
        .map(|(t, _)| -t.amount.minor())
        .sum()
}

#[test]
fn spending_totals_agree_with_an_independent_count() {
    let (ledger, truth) = td1();
    let ymd = fm_synth::cal::ymd;
    let s = set();
    let summer = spending_between(&ledger, &truth, ymd(2025, 6, 1), ymd(2025, 8, 31));
    assert_eq!(minor(answer(&s, "h19", "total")), summer);
    let last_30 = spending_between(&ledger, &truth, ymd(2026, 9, 1), ymd(2026, 9, 30));
    assert_eq!(
        minor(answer(&s, "h18", "total")),
        last_30,
        "the 30 days before 2026-10-01"
    );
    let year = spending_between(&ledger, &truth, ymd(2025, 1, 1), ymd(2025, 12, 31));
    assert_eq!(minor(answer(&s, "h09", "total_spending")), year);
}

#[test]
fn transfers_and_pending_never_count_as_spending() {
    let (ledger, truth) = td1();
    let x: BTreeMap<i64, (&Transaction, &TruthBucket)> = ledger
        .transactions
        .iter()
        .zip(&truth.transactions)
        .map(|(t, x)| (t.id.0, (t, &x.bucket)))
        .collect();
    let s = set();
    for q in &s.questions {
        let transfer_question = q.tags.iter().any(|t| t == "transfers");
        for a in &q.answers {
            let Answer::Money { evidence, .. } = a else {
                continue;
            };
            for id in &evidence.transaction_ids {
                let (t, bucket) = x[id];
                assert_eq!(
                    t.status,
                    TransactionStatus::Posted,
                    "{}: a pending item in a total",
                    q.id
                );
                if transfer_question {
                    assert!(
                        matches!(bucket, TruthBucket::Transfer { .. }),
                        "{}: a transfer question counts a non-transfer",
                        q.id
                    );
                } else if evidence.rule == Rule::NegatedSum {
                    assert!(
                        matches!(
                            bucket,
                            TruthBucket::Category { .. } | TruthBucket::Refund { .. }
                        ),
                        "{}: a transfer or income item counted as spending",
                        q.id
                    );
                }
            }
        }
    }
}

#[test]
fn differences_are_first_minus_second() {
    let s = set();
    let mut seen = 0;
    for q in &s.questions {
        let money: Vec<(&str, i64)> = q
            .answers
            .iter()
            .filter_map(|a| match a {
                Answer::Money { name, amount, .. } => Some((name.as_str(), amount.minor())),
                _ => None,
            })
            .collect();
        if let Some(&(_, diff)) = money.iter().find(|(n, _)| *n == "difference") {
            assert_eq!(diff, money[0].1 - money[1].1, "{}", q.id);
            seen += 1;
        }
    }
    assert!(seen >= 1, "{seen} comparisons");
}
