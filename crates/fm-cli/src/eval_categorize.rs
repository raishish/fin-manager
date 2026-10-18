//! Part (a): each model categorises what the rules left in Needs review.
//!
//! For one model, every household in the starting state (see [`crate::td1`]) and every
//! choice-set [`Variant`]: one [`fm_llm::Session`] per household, one greedy constrained
//! answer per Needs-review transaction. Results go to `eval/runs/<run>/`: one JSON line
//! per transaction and a `meta.json` with timings, starting scores and checks.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use fm_core::contract;
use fm_core::model::{
    Assignment, Bucket, Confidence, Decision, DecisionId, DecisionSource, DecisionState,
    ModelProvenance, Sha256Hash, TransactionId,
};
use fm_llm::{GenerateOptions, Model, SessionOptions, Timings};
use serde_json::{Value, json};

use crate::prompt::{PROMPT_VERSION, Prompt, Variant};
use crate::td1::{self, Household, Score};

pub struct Settings<'a> {
    pub run_dir: &'a Path,
    pub variants: &'a [Variant],
    /// Transactions per household, for quick runs; `None` for all.
    pub limit: Option<usize>,
    pub threads: Option<u32>,
    pub answer_lead: &'static str,
    /// Check every answer against the whole prompt evaluated at once, not just the
    /// first few per household.
    pub check_whole: bool,
    /// Check none (each check evaluates the whole prompt again: slow on a CPU).
    pub no_check: bool,
}

const MAX_ANSWER_TOKENS: u32 = 48;

/// The confidence an answer needs to be applied (T2.5's fallback threshold).
const THRESHOLD_BP: u16 = 8000;

/// Session answers against the whole prompt evaluated at once.
#[derive(Default)]
struct WholeCheck {
    checked: usize,
    same_key: usize,
    /// Answers with the same key whose confidences differ, and the largest difference.
    confidence_differs: usize,
    max_diff_bp: u16,
    /// Answers on different sides of the threshold.
    crosses_threshold: usize,
}

impl WholeCheck {
    fn add(&mut self, split: &fm_llm::Choice, whole: &fm_llm::Choice) {
        self.checked += 1;
        if split.key == whole.key {
            self.same_key += 1;
            let diff = split.confidence_bp.abs_diff(whole.confidence_bp);
            if diff > 0 {
                self.confidence_differs += 1;
                self.max_diff_bp = self.max_diff_bp.max(diff);
            }
        }
        if (split.confidence_bp >= THRESHOLD_BP) != (whole.confidence_bp >= THRESHOLD_BP) {
            self.crosses_threshold += 1;
        }
    }

    fn json(&self) -> Value {
        json!({
            "checked": self.checked, "same_key": self.same_key,
            "confidence_differs": self.confidence_differs, "max_diff_bp": self.max_diff_bp,
            "crosses_threshold": self.crosses_threshold,
        })
    }
}

/// Runs part (a) for one loaded model. Returns the summary written to `meta.json`.
pub fn run(model: &Model, households: &[Household], s: &Settings<'_>) -> Value {
    fs::create_dir_all(s.run_dir).expect("create the run directory");
    let mut variants_meta = Vec::new();
    for &variant in s.variants {
        let mut lines = fs::File::create(s.run_dir.join(format!("{}.jsonl", variant.name())))
            .expect("create results file");
        let mut per_household = Vec::new();
        let mut total = Timings::default();
        let mut answered = 0usize;
        let mut errors = 0usize;
        let mut violations = 0usize;
        let mut self_check = WholeCheck::default();
        let mut check_ms = 0.0;
        let started = Instant::now();
        for h in households {
            let examples: Vec<(TransactionId, Bucket)> =
                h.corrected.iter().map(|id| (*id, h.truth[id])).collect();
            let prompt = Prompt::new(model, variant, &h.ledger, &examples, s.answer_lead);
            let choices = variant.choices(&h.ledger.taxonomy);
            let accounts: HashMap<_, _> = h.ledger.accounts.iter().map(|a| (a.id, a)).collect();
            let applied = td1::applied_buckets(&h.ledger);
            let mut review: Vec<_> = h
                .ledger
                .transactions
                .iter()
                .filter(|t| matches!(applied.get(&t.id), None | Some(Bucket::NeedsReview)))
                .collect();
            if let Some(limit) = s.limit {
                review.truncate(limit);
            }

            let t0 = Instant::now();
            let mut session = model
                .session(
                    &prompt.prefix,
                    &SessionOptions {
                        max_suffix_tokens: 256,
                        max_answer_tokens: MAX_ANSWER_TOKENS,
                        threads: s.threads,
                    },
                )
                .expect("start a session");
            let prefix_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let prefix_tokens = session.prefix_tokens();
            session.reset_timings();
            let options = GenerateOptions {
                max_tokens: MAX_ANSWER_TOKENS,
                threads: s.threads,
                ..GenerateOptions::default()
            };

            let mut ledger = h.ledger.clone();
            let mut next_id = ledger.decisions.iter().map(|d| d.id.0).max().unwrap_or(0) + 1;
            let calls_started = Instant::now();
            for (i, t) in review.iter().enumerate() {
                let suffix = prompt.suffix(t, accounts.get(&t.account_id).copied());
                let call = Instant::now();
                let result = session.choose(&suffix, &choices, &options);
                let ms = call.elapsed().as_secs_f64() * 1000.0;
                let truth = h.truth[&t.id];
                let record = match result {
                    Ok(choice) => {
                        answered += 1;
                        // Answers are checked against the whole prompt in a fresh context:
                        // the first few, or all of them.
                        let mut whole = None;
                        if s.check_whole || (!s.no_check && i < 3 && variant == s.variants[0]) {
                            let check = Instant::now();
                            let prompt = format!("{}{}", prompt.prefix, suffix);
                            if let Ok(fresh) = model.choose(&prompt, &choices, &options) {
                                self_check.add(&choice, &fresh);
                                whole = Some(fresh);
                            }
                            check_ms += check.elapsed().as_secs_f64() * 1000.0;
                        }
                        let bucket = variant.bucket(&choice.key, &ledger.taxonomy, t);
                        if bucket != Bucket::NeedsReview {
                            record_decision(
                                &mut ledger,
                                &mut next_id,
                                t.id,
                                bucket,
                                model,
                                choice.confidence_bp,
                            );
                        }
                        let outcome = match bucket {
                            Bucket::NeedsReview => "review",
                            b if td1::same(b, truth) => "correct",
                            _ => "wrong",
                        };
                        let mut record = json!({
                            "persona": h.persona, "id": t.id.0, "key": choice.key,
                            "confidence_bp": choice.confidence_bp, "outcome": outcome,
                            "truth": truth_label(truth, &ledger), "ms": ms,
                        });
                        if let Some(whole) = whole {
                            record["whole"] = json!({
                                "key": whole.key, "confidence_bp": whole.confidence_bp,
                            });
                        }
                        record
                    }
                    Err(e) => {
                        errors += 1;
                        json!({"persona": h.persona, "id": t.id.0, "error": e.to_string(), "ms": ms})
                    }
                };
                writeln!(lines, "{record}").expect("write a result");
            }
            // Without the fresh-context checks, which re-evaluate the whole prompt.
            let calls_ms = calls_started.elapsed().as_secs_f64() * 1000.0 - check_ms;
            check_ms = 0.0;
            let timings = session.timings();
            total.prompt_tokens += timings.prompt_tokens;
            total.prompt_ms += timings.prompt_ms;
            total.generated_tokens += timings.generated_tokens;
            total.generated_ms += timings.generated_ms;

            let report = contract::check(&ledger);
            violations += report.violations.len();
            if !report.is_ok() {
                eprintln!("{}: {report}", h.persona);
            }
            let start = td1::score(h);
            per_household.push(json!({
                "persona": h.persona,
                "start": {"correct": start.correct, "wrong": start.wrong, "needs_review": start.needs_review},
                "in_review": review.len(),
                "prefix_tokens": prefix_tokens, "prefix_ms": prefix_ms,
                "calls_ms": calls_ms,
            }));
            eprintln!(
                "{} {}: {} transactions in {:.1} s (prefix {} tokens in {:.0} ms)",
                variant.name(),
                h.persona,
                review.len(),
                calls_ms / 1000.0,
                prefix_tokens,
                prefix_ms
            );
        }
        variants_meta.push(json!({
            "variant": variant.name(),
            "answered": answered, "errors": errors,
            "contract_violations": violations,
            "fresh_context_agreement": [self_check.same_key, self_check.checked],
            "whole_prompt_check": self_check.json(),
            "wall_ms": started.elapsed().as_secs_f64() * 1000.0,
            "timings": {
                "prompt_tokens": total.prompt_tokens, "prompt_ms": total.prompt_ms,
                "generated_tokens": total.generated_tokens, "generated_ms": total.generated_ms,
            },
            "households": per_household,
        }));
    }
    json!({"prompt_version": PROMPT_VERSION, "variants": variants_meta})
}

/// Files a model answer as an applied decision replacing the Needs-review placeholder,
/// so the contract can be checked on the result.
fn record_decision(
    ledger: &mut fm_core::model::Ledger,
    next_id: &mut i64,
    t: TransactionId,
    bucket: Bucket,
    model: &Model,
    confidence_bp: u16,
) {
    let current = ledger.decisions.iter_mut().find(|d| {
        d.transaction_id == t
            && d.state == DecisionState::Applied
            && matches!(d.assignment, Assignment::Bucket(_))
    });
    let replaces = current.map(|d| {
        d.state = DecisionState::Superseded;
        d.id
    });
    let version = ledger.taxonomy.version.clone();
    ledger.decisions.push(Decision {
        id: DecisionId(*next_id),
        transaction_id: t,
        assignment: Assignment::Bucket(bucket),
        source: DecisionSource::Model(ModelProvenance {
            model_id: model.id().to_string(),
            gguf_sha256: Sha256Hash::from_bytes(model.gguf_sha256()),
            prompt_version: PROMPT_VERSION.to_string(),
            confidence: Confidence::from_basis_points(confidence_bp).expect("0..=10000"),
        }),
        taxonomy_version: version,
        decided_at: td1::NOW,
        state: DecisionState::Applied,
        seen_at: None,
        replaces,
    });
    *next_id += 1;
}

fn truth_label(b: Bucket, ledger: &fm_core::model::Ledger) -> String {
    let key = |id| {
        ledger
            .taxonomy
            .categories
            .iter()
            .find(|c| c.id == id)
            .map_or_else(String::new, |c| c.key.clone())
    };
    match b {
        Bucket::Category { category_id } => key(category_id),
        Bucket::Refund { category_id, .. } => format!("refund:{}", key(category_id)),
        Bucket::Income => "income".into(),
        Bucket::Transfer { counterpart: None } => "transfer".into(),
        Bucket::Transfer { .. } => "transfer_pair".into(),
        Bucket::NeedsReview => "needs_review".into(),
    }
}

/// Whole-ledger scores if model answers with confidence at or above `threshold` are
/// applied and the rest stay in review.
pub fn at_threshold(results: &[Value], start: Score, threshold: u16) -> Score {
    let mut s = start;
    for r in results {
        let Some(outcome) = r["outcome"].as_str() else {
            continue;
        };
        let confident = r["confidence_bp"].as_u64().unwrap_or(0) >= u64::from(threshold);
        if !confident || outcome == "review" {
            continue;
        }
        s.needs_review -= 1;
        match outcome {
            "correct" => s.correct += 1,
            _ => s.wrong += 1,
        }
    }
    s
}
