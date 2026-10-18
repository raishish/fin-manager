//! Constraint 7: no silent changes. A decision the user has seen (`seen_at`) is never
//! replaced behind their back: a re-run by fixed logic or the model that disagrees is a
//! suggestion, which only the user puts into effect (and accepting marks it seen).
//!
//! Not a silent change: the user's own corrections and rules (user input), and going
//! back to Needs review, which puts the transaction in front of the user (undoing or
//! deleting a rule, a correction breaking a transfer pair).

use super::{Check, Constraint, LedgerIndex, Subject, Violation};
use crate::model::{Assignment, Bucket, DecisionState};

pub struct NoSilentChanges;

impl Check for NoSilentChanges {
    fn constraint(&self) -> Constraint {
        Constraint::NoSilentChanges
    }

    fn check(&self, ix: &LedgerIndex<'_>, out: &mut Vec<Violation>) {
        for d in &ix.ledger.decisions {
            if d.state != DecisionState::Applied
                || d.source.is_user_input()
                || d.seen_at.is_some()
                || d.assignment == Assignment::Bucket(Bucket::NeedsReview)
            {
                continue;
            }
            let replaced_a_seen_one = d
                .replaces
                .and_then(|id| ix.decision(id))
                .is_some_and(|r| r.seen_at.is_some());
            if replaced_a_seen_one {
                out.push(Violation {
                    constraint: Constraint::NoSilentChanges,
                    subject: Subject::Decision(d.id),
                    code: "silent_change",
                    message: "a decision the user had seen was replaced without the user acting on it",
                });
            }
        }
    }
}
