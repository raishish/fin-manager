//! Checks on the question set: it is fresh, well formed, and its answers agree with
//! TD.1's ledger when recomputed independently of the generator's own code.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use fm_core::model::{Date, Ledger, Transaction, TransactionStatus};
use fm_questions::{Answer, QuestionSet, Rule, build, render};
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

#[test]
fn one_hundred_questions_across_every_type() {
    let s = set();
    assert_eq!(s.questions.len(), 100);
    let ids: BTreeSet<&str> = s.questions.iter().map(|q| q.id.as_str()).collect();
    let texts: BTreeSet<&str> = s.questions.iter().map(|q| q.question.as_str()).collect();
    assert_eq!(ids.len(), 100);
    assert_eq!(texts.len(), 100, "no repeated questions");
    let mut types: BTreeMap<&str, usize> = BTreeMap::new();
    for q in &s.questions {
        *types.entry(q.kind.as_str()).or_default() += 1;
        assert!(!q.answers.is_empty(), "{} has no answer", q.id);
        assert!(
            q.expected_query.is_some() != q.null_reason.is_some(),
            "{}: a query or a reason for none",
            q.id
        );
    }
    for t in [
        "totals",
        "comparisons",
        "merchants",
        "recurring",
        "time_ranges",
        "hard",
    ] {
        assert!(types.get(t).copied().unwrap_or(0) >= 15, "{t}: {types:?}");
    }
    let tags: BTreeSet<&str> = s
        .questions
        .iter()
        .flat_map(|q| q.tags.iter().map(String::as_str))
        .collect();
    for t in [
        "refunds",
        "transfers",
        "multi_currency",
        "relative_date",
        "pending",
        "one_off",
        "income",
        "interest",
    ] {
        assert!(tags.contains(t), "no question tagged {t}");
    }
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
    assert!(checked >= 110, "only {checked} answers carry evidence");
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
    let year = spending_between(&ledger, &truth, ymd(2025, 1, 1), ymd(2025, 12, 31));
    assert_eq!(minor(answer(&s, "q001", "total")), year);
    let months: i64 = (1..=12)
        .map(|m| {
            spending_between(
                &ledger,
                &truth,
                ymd(2025, m, 1),
                fm_synth::cal::day_of_month(2025, m, 31),
            )
        })
        .sum();
    assert_eq!(months, year, "months add up to the year");
    let last_month = spending_between(&ledger, &truth, ymd(2026, 9, 1), ymd(2026, 9, 30));
    assert_eq!(
        minor(answer(&s, "q066", "total")),
        last_month,
        "last month is September 2026"
    );
    assert_eq!(minor(answer(&s, "q022", "september_2026")), last_month);
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
        let pending_question = q.tags.iter().any(|t| t == "pending");
        let transfer_question = q.tags.iter().any(|t| t == "transfers");
        for a in &q.answers {
            let Answer::Money { evidence, .. } = a else {
                continue;
            };
            for id in &evidence.transaction_ids {
                let (t, bucket) = x[id];
                if !pending_question {
                    assert_eq!(
                        t.status,
                        TransactionStatus::Posted,
                        "{}: a pending item in a total",
                        q.id
                    );
                }
                if !transfer_question && evidence.rule == Rule::NegatedSum {
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
fn groups_are_the_sum_of_their_categories() {
    let (ledger, truth) = td1();
    let s = set();
    let food: i64 = ledger
        .transactions
        .iter()
        .zip(&truth.transactions)
        .filter(|(t, x)| {
            counts_in(t, fm_synth::cal::ymd(2025, 1, 1), fm_synth::cal::ymd(2025, 12, 31))
                && matches!(&x.bucket, TruthBucket::Category { key } | TruthBucket::Refund { key, .. } if key.starts_with("food."))
        })
        .map(|(t, _)| -t.amount.minor())
        .sum();
    assert_eq!(minor(answer(&s, "q014", "total")), food);
    let parts: i64 = ["q002", "q004", "q020"]
        .iter()
        .map(|id| minor(answer(&s, id, "total")))
        .sum();
    assert!(parts < food, "groceries, coffee and bars are part of food");
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
    assert!(seen >= 8, "{seen} comparisons");
}
