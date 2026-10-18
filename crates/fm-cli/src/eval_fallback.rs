//! T2.5's measurement: the categorisation fallback, end to end, on TD.1.
//!
//! For each household: the starting state (see [`crate::td1`]) is saved to a throwaway
//! encrypted ledger, and [`fm_core::categorize::run_fallback`] runs on it with the model
//! behind [`SessionChooser`], exactly as the app would: the planner decides what may
//! change, and the writes go through one savepoint. The ledger is then loaded back,
//! checked against the contract, and scored against the truth.
//!
//! The correction rate is what the user would have to fix: of the answers the fallback
//! applied (at or above the threshold), the share that is wrong.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

use fm_core::categorize::{FALLBACK_THRESHOLD_BP, FallbackOptions, Scope, run_fallback};
use fm_core::contract;
use fm_core::model::{Assignment, Bucket, DecisionSource, DecisionState, TransactionId};
use fm_llm::Model;
use serde_json::{Value, json};

use crate::fallback::SessionChooser;
use crate::td1::{self, Score};

pub struct Settings<'a> {
    pub run_dir: &'a Path,
    /// Transactions in review per household, for quick runs; `None` for all.
    pub limit: Option<usize>,
    pub threads: Option<u32>,
    pub answer_lead: &'static str,
}

/// The model's own applied answers, scored.
#[derive(Default, Clone, Copy)]
struct Applied {
    correct: usize,
    wrong: usize,
}

impl Applied {
    fn total(self) -> usize {
        self.correct + self.wrong
    }

    fn json(self) -> Value {
        json!({
            "applied": self.total(), "correct": self.correct, "wrong": self.wrong,
            "correction_rate": ratio(self.wrong, self.total()),
        })
    }
}

fn ratio(n: usize, d: usize) -> Option<f64> {
    (d > 0).then(|| n as f64 / d as f64)
}

fn score_json(s: Score) -> Value {
    json!({"correct": s.correct, "wrong": s.wrong, "needs_review": s.needs_review})
}

/// Runs the measurement for one loaded model. Returns the summary written to
/// `fallback-meta.json`.
pub fn run(model: &Model, s: &Settings<'_>) -> Value {
    fs::create_dir_all(s.run_dir).expect("create the run directory");
    let scratch = std::env::temp_dir();
    let mut households = Vec::new();
    let (mut start_total, mut end_total) = (Score::default(), Score::default());
    let mut applied_total = Applied::default();
    let (mut asked_total, mut violations, mut errors) = (0usize, 0usize, 0usize);
    let mut seconds = 0.0;
    for persona in td1::PERSONAS {
        let h = td1::starting_state(persona, &scratch);
        let start = td1::score(&h);
        let before = td1::applied_buckets(&h.ledger);
        let mut review: Vec<TransactionId> = h
            .ledger
            .transactions
            .iter()
            .filter(|t| matches!(before.get(&t.id), None | Some(Bucket::NeedsReview)))
            .map(|t| t.id)
            .collect();
        if let Some(limit) = s.limit {
            review.truncate(limit);
        }

        let dir = scratch.join(format!("t2.5-{persona}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the scratch directory");
        let mut u = fm_core::storage::Vault::at(&dir.join("ledger"))
            .create(b"t2.5 measurement ledger")
            .expect("create the scratch ledger");
        u.save_ledger(&h.ledger).expect("save the starting state");

        let mut chooser = SessionChooser::new(model, s.answer_lead, s.threads);
        let options = FallbackOptions {
            threshold_bp: FALLBACK_THRESHOLD_BP,
            scope: if s.limit.is_some() {
                Scope::Transactions(&review)
            } else {
                Scope::All
            },
        };
        let t0 = Instant::now();
        let result = run_fallback(&mut u, &mut chooser, &options, td1::NOW);
        let elapsed = t0.elapsed().as_secs_f64();
        seconds += elapsed;
        let ledger = u.load_ledger().expect("load the ledger back");
        drop(u);
        let _ = fs::remove_dir_all(&dir);

        let report = match result {
            Ok(r) => r,
            Err(e) => {
                errors += 1;
                eprintln!("{persona}: the fallback failed: {e}");
                households.push(json!({"persona": persona, "error": e.to_string()}));
                continue;
            }
        };
        let check = contract::check(&ledger);
        violations += check.violations.len();
        if !check.is_ok() {
            eprintln!("{persona}: {check}");
        }

        let mut end = Score::default();
        let after = td1::applied_buckets(&ledger);
        for t in &ledger.transactions {
            end.count(after.get(&t.id).copied(), h.truth[&t.id]);
        }
        let by_model: HashMap<TransactionId, Bucket> = ledger
            .decisions
            .iter()
            .filter(|d| {
                d.state == DecisionState::Applied && matches!(d.source, DecisionSource::Model(_))
            })
            .filter_map(|d| match d.assignment {
                Assignment::Bucket(b) if b != Bucket::NeedsReview => Some((d.transaction_id, b)),
                _ => None,
            })
            .collect();
        let mut applied = Applied::default();
        for (id, b) in &by_model {
            if td1::same(*b, h.truth[id]) {
                applied.correct += 1;
            } else {
                applied.wrong += 1;
            }
        }
        eprintln!(
            "{persona}: asked {} in {elapsed:.0} s; applied {} ({} wrong), {} kept in review",
            report.asked,
            applied.total(),
            applied.wrong,
            report.needs_review
        );
        asked_total += report.asked;
        applied_total.correct += applied.correct;
        applied_total.wrong += applied.wrong;
        for (total, s) in [(&mut start_total, start), (&mut end_total, end)] {
            total.correct += s.correct;
            total.wrong += s.wrong;
            total.needs_review += s.needs_review;
        }
        households.push(json!({
            "persona": persona,
            "asked": report.asked, "categorised": report.categorised,
            "needs_review": report.needs_review,
            "planner": {
                "suggestions": report.run.suggestions,
                "kept_user_decision": report.run.kept_user_decision,
                "kept_dismissed": report.run.kept_dismissed,
            },
            "model_applied": applied.json(),
            "start": score_json(start), "end": score_json(end),
            "seconds": elapsed,
            "contract_violations": check.violations.len(),
        }));
    }
    json!({
        "threshold_bp": FALLBACK_THRESHOLD_BP,
        "prompt_version": fm_core::categorize::llm_fallback::PROMPT_VERSION,
        "limit": s.limit,
        "asked": asked_total,
        "model_applied": applied_total.json(),
        "start": score_json(start_total),
        "end": score_json(end_total),
        "seconds": seconds,
        "seconds_per_transaction": (asked_total > 0).then(|| seconds / asked_total as f64),
        "errors": errors,
        "contract_violations": violations,
        "households": households,
    })
}
