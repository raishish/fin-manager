//! T4.1's done-when: every expected query runs through `fm_core::query` and gives its
//! question's answers.
//!
//! The ledger is TD.1's freelancer with one applied decision per truth bucket and
//! merchant: the ledger as it would look once everything were labelled correctly.
//! Recurring questions name their series through a labeler that reads TD.1's truth, a
//! stand-in for the model behind T3.2's `Labeler` hook.
//!
//! Every answer must match: amounts, counts, texts, dates and percents exactly, and, for
//! sums and counts, the transaction ids behind them. Failure messages name the question
//! and the answer, never an amount or a merchant.

use std::collections::HashMap;

use fm_core::model::{
    Assignment, Bucket, CategoryId, Decision, DecisionId, DecisionSource, DecisionState, Ledger,
    Money, Timestamp, TransactionId,
};
use fm_core::query::{self, Context, Value};
use fm_core::recurring::{Label, LabelKind, Labeler, SeriesSketch};
use fm_questions::{Answer, QuestionSet, Rule, build};
use fm_synth::truth::{PersonaTruth, TruthBucket};

/// Questions whose query is right but doesn't give the answer yet, and what it gives
/// today: (question, answer, today's value). A change in what these give fails here, so
/// a gap can't close or widen unnoticed.
///
/// Empty since T3.4 taught recurring detection the US estimated-tax calendar, which made
/// q062 and q065 exact.
const KNOWN_GAPS: &[(&str, &str, i64)] = &[];

const NULLS: &[&str] = &["q089", "q093"];

fn labelled() -> (Ledger, PersonaTruth) {
    let (mut ledger, truth) = fm_questions::facts::load(&fm_questions::facts::synthetic_data_dir());
    let by_key: HashMap<String, CategoryId> = ledger
        .taxonomy
        .categories
        .iter()
        .map(|c| (c.key.clone(), c.id))
        .collect();
    let mut decisions = Vec::new();
    for t in &truth.transactions {
        let bucket = match &t.bucket {
            TruthBucket::Category { key } => Bucket::Category {
                category_id: by_key[key],
            },
            TruthBucket::Refund { key, original } => Bucket::Refund {
                category_id: by_key[key],
                original: *original,
            },
            TruthBucket::Transfer { counterpart } => Bucket::Transfer {
                counterpart: *counterpart,
            },
            TruthBucket::Income => Bucket::Income,
        };
        let mut assignments = vec![Assignment::Bucket(bucket)];
        if let Some(m) = &t.merchant {
            assignments.push(Assignment::Merchant(m.clone()));
        }
        for assignment in assignments {
            decisions.push(Decision {
                id: DecisionId(decisions.len() as i64 + 1),
                transaction_id: t.id,
                assignment,
                source: DecisionSource::User,
                taxonomy_version: ledger.taxonomy.version.clone(),
                decided_at: Timestamp::from_unix_seconds(1_760_000_000),
                state: DecisionState::Applied,
                seen_at: None,
                replaces: None,
            });
        }
    }
    ledger.decisions = decisions;
    (ledger, truth)
}

/// Names a series the way TD.1's truth does, by the bank descriptions of its members.
/// Series truth doesn't know (a coincidence, an untracked account) stay unlabelled.
struct TruthLabeler {
    by_description: HashMap<String, Label>,
}

impl TruthLabeler {
    fn new(ledger: &Ledger, truth: &PersonaTruth) -> TruthLabeler {
        let description: HashMap<TransactionId, &str> = ledger
            .transactions
            .iter()
            .map(|t| (t.id, t.description.as_str()))
            .collect();
        let mut by_description = HashMap::new();
        for s in &truth.recurring {
            let kind = match s.kind.as_str() {
                "subscription" => LabelKind::Subscription,
                "bill" => LabelKind::Bill,
                "income" => LabelKind::Income,
                "interest" => LabelKind::Interest,
                "transfer" => LabelKind::Transfer,
                _ => LabelKind::Other,
            };
            for id in &s.transaction_ids {
                by_description.insert(
                    description[id].to_owned(),
                    Label {
                        name: s.merchant.clone(),
                        kind,
                    },
                );
            }
        }
        TruthLabeler { by_description }
    }
}

impl Labeler for TruthLabeler {
    fn label(&self, series: &SeriesSketch<'_>) -> Option<Label> {
        series
            .descriptions
            .iter()
            .find_map(|d| self.by_description.get(d).cloned())
    }
}

fn name(a: &Answer) -> &str {
    match a {
        Answer::Money { name, .. }
        | Answer::Count { name, .. }
        | Answer::Percent { name, .. }
        | Answer::Date { name, .. }
        | Answer::Text { name, .. }
        | Answer::Texts { name, .. } => name,
    }
}

fn ids(v: &[i64]) -> Vec<TransactionId> {
    let mut ids: Vec<TransactionId> = v.iter().copied().map(TransactionId).collect();
    ids.sort_unstable();
    ids
}

/// Whether `got` is the expected answer: the value, and the evidence ids for sums and
/// counts.
fn matches(expected: &Answer, got: &Value) -> bool {
    let traced = |rule: Rule| rule != Rule::Derived;
    match (expected, got) {
        (
            Answer::Money {
                amount, evidence, ..
            },
            Value::Money {
                amount: a,
                transaction_ids,
            },
        ) => {
            amount == a
                && (!traced(evidence.rule) || ids(&evidence.transaction_ids) == *transaction_ids)
        }
        (
            Answer::Count {
                value, evidence, ..
            },
            Value::Count {
                value: v,
                transaction_ids,
            },
        ) => {
            value == v
                && (!traced(evidence.rule) || ids(&evidence.transaction_ids) == *transaction_ids)
        }
        (Answer::Percent { basis_points, .. }, Value::Percent { basis_points: b }) => {
            basis_points == b
        }
        (Answer::Date { value, .. }, Value::Date { value: v }) => value == v,
        (Answer::Text { value, .. }, Value::Text { value: v }) => value == v,
        (Answer::Texts { values, .. }, Value::Texts { values: v }) => values == v,
        _ => false,
    }
}

/// The number a known gap gives today.
fn number(v: &Value) -> i64 {
    match v {
        Value::Money { amount, .. } => amount.minor(),
        Value::Count { value, .. } => *value,
        _ => panic!("a known gap is a number"),
    }
}

#[test]
fn every_expected_query_gives_its_answers() {
    let set: QuestionSet = build();
    let (ledger, truth) = labelled();
    let labeler = TruthLabeler::new(&ledger, &truth);
    let ctx = Context {
        as_of: set.today,
        labeler: Some(&labeler),
    };

    let (mut queries, mut exact, mut proxies, mut gaps) = (0, 0, 0, 0);
    for q in &set.questions {
        let Some(query) = &q.expected_query else {
            assert!(NULLS.contains(&q.id.as_str()), "{}: no query", q.id);
            assert!(q.null_reason.is_some(), "{}: a null needs a reason", q.id);
            continue;
        };
        queries += 1;
        assert!(
            q.null_reason.is_none(),
            "{}: a query and a null reason",
            q.id
        );
        let got = query::run(&ledger, query, &ctx)
            .unwrap_or_else(|e| panic!("{}: the query fails: {e}", q.id));
        assert!(
            got.len() == q.answers.len(),
            "{}: {} answers for {} expected",
            q.id,
            got.len(),
            q.answers.len()
        );
        let mut all_match = true;
        for (expected, answer) in q.answers.iter().zip(&got) {
            assert!(
                name(expected) == answer.name,
                "{}: answer {:?} comes back as {:?}",
                q.id,
                name(expected),
                answer.name
            );
            let gap = KNOWN_GAPS
                .iter()
                .find(|(id, a, _)| *id == q.id && *a == answer.name);
            match gap {
                Some((_, _, today)) => {
                    assert!(
                        number(&answer.value) == *today,
                        "{} {}: the known gap changed. If T3.4 landed, delete this entry from KNOWN_GAPS",
                        q.id,
                        answer.name
                    );
                    assert!(
                        !matches(expected, &answer.value),
                        "{} {}: now exact. Delete this entry from KNOWN_GAPS",
                        q.id,
                        answer.name
                    );
                    all_match = false;
                }
                None => assert!(
                    matches(expected, &answer.value),
                    "{} {}: wrong answer",
                    q.id,
                    answer.name
                ),
            }
        }
        if all_match {
            exact += 1;
        } else {
            gaps += 1;
            assert!(q.known_gap.is_some(), "{}: a gap without a reason", q.id);
        }
        if q.proxy.is_some() {
            proxies += 1;
        }
    }
    println!(
        "T4.1-TD3 queries={queries} exact={exact} known_gaps={gaps} nulls={} proxies={proxies}",
        NULLS.len()
    );
    assert_eq!((queries, exact, gaps), (98, 98, 0));
}

#[test]
fn expected_queries_round_trip_through_json() {
    for q in build().questions {
        if let Some(query) = q.expected_query {
            let json = serde_json::to_string(&query).unwrap();
            let back: query::Query = serde_json::from_str(&json).unwrap();
            assert!(back == query, "{}: JSON round trip", q.id);
        }
    }
}

#[test]
fn money_answers_are_in_the_expected_currency() {
    // Guards the EUR questions: euros come back in EUR, everything else in USD.
    let set = build();
    for id in ["q088", "q097"] {
        let q = set.questions.iter().find(|q| q.id == id).unwrap();
        let euros = q.answers.iter().find(|a| name(a) == "euros").unwrap();
        let Answer::Money { amount, .. } = euros else {
            panic!("{id}: euros is money")
        };
        assert_eq!(amount.currency().code(), "EUR");
        let _: Money = *amount;
    }
}
