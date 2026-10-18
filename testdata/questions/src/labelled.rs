//! The freelancer's ledger as it would look once everything were labelled correctly,
//! and a labeler that names recurring series the way TD.1's truth does. For running
//! queries against TD.3's answers (`nl-eval`).
//!
//! `tests/answers.rs` keeps its own copy of the same setup, so its known-gap lines
//! (edited by T3.4) stay the only thing that changes there.

use std::collections::HashMap;

use fm_core::model::{
    Assignment, Bucket, CategoryId, Decision, DecisionId, DecisionSource, DecisionState, Ledger,
    Timestamp, TransactionId,
};
use fm_core::recurring::{Label, LabelKind, Labeler, SeriesSketch};
use fm_synth::truth::{PersonaTruth, TruthBucket};

/// TD.1's freelancer with one applied user decision per truth bucket and merchant.
pub fn ledger() -> (Ledger, PersonaTruth) {
    ledger_for(crate::facts::PERSONA)
}

/// Any TD.1 persona, labelled the same way.
pub fn ledger_for(persona: &str) -> (Ledger, PersonaTruth) {
    let (mut ledger, truth) =
        crate::facts::load_persona(&crate::facts::synthetic_data_dir(), persona);
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
/// Series truth doesn't know stay unlabelled. A stand-in for the model behind T3.2's
/// labeler hook.
pub struct TruthLabeler {
    by_description: HashMap<String, Label>,
}

impl TruthLabeler {
    pub fn new(ledger: &Ledger, truth: &PersonaTruth) -> TruthLabeler {
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
