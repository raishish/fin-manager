//! Constraint 5: admit uncertainty. Below the confidence threshold
//! ([`FALLBACK_THRESHOLD_BP`]), a model decision is Needs review, never a guess shown as
//! fact.

use super::{Check, Constraint, LedgerIndex, Subject, Violation};
use crate::categorize::FALLBACK_THRESHOLD_BP;
use crate::model::{Assignment, Bucket, DecisionSource, DecisionState};

pub struct AdmitUncertainty;

impl Check for AdmitUncertainty {
    fn constraint(&self) -> Constraint {
        Constraint::AdmitUncertainty
    }

    fn check(&self, ix: &LedgerIndex<'_>, out: &mut Vec<Violation>) {
        for d in &ix.ledger.decisions {
            let DecisionSource::Model(p) = &d.source else {
                continue;
            };
            if d.state == DecisionState::Applied
                && d.assignment != Assignment::Bucket(Bucket::NeedsReview)
                && matches!(d.assignment, Assignment::Bucket(_))
                && p.confidence.basis_points() < FALLBACK_THRESHOLD_BP
            {
                out.push(Violation {
                    constraint: Constraint::AdmitUncertainty,
                    subject: Subject::Decision(d.id),
                    code: "below_threshold_applied",
                    message: "a model decision below the confidence threshold is in effect instead of Needs review",
                });
            }
        }
    }
}
