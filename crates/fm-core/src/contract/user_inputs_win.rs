//! Constraint 2: user inputs always win. Corrections, rules and account designations are
//! never overridden by a model re-run.
//!
//! Sources rank user > rule > fixed logic > model > the Needs-review placeholder (see
//! `categorize`). This check looks for a weaker source standing where a user input
//! should:
//! - an applied decision that replaced a decision from a stronger user input;
//! - a user's correction no longer in effect, without a newer correction replacing it;
//! - an applied model decision where one of the user's rules matches.
//!
//! The transfer matcher's pair replacing a rule's unpaired transfer is a refinement (the
//! same bucket, with the other side found), not an override. Account designations
//! (`own_account`) are plain fields that no model writes, so there is nothing to check.

use std::collections::HashMap;

use super::{Check, Constraint, LedgerIndex, Subject, Violation};
use crate::categorize::plan::{rank, refines_transfer};
use crate::categorize::rules::{Fit, fit, winners};
use crate::model::{Assignment, DecisionSource, DecisionState, TransactionId};

pub struct UserInputsWin;

fn violation(subject: Subject, code: &'static str, message: &'static str) -> Violation {
    Violation {
        constraint: Constraint::UserInputsWin,
        subject,
        code,
        message,
    }
}

/// 0 for the bucket, 1 for the merchant name.
fn kind(assignment: &Assignment) -> u8 {
    match assignment {
        Assignment::Bucket(_) => 0,
        Assignment::Merchant(_) => 1,
    }
}

impl Check for UserInputsWin {
    fn constraint(&self) -> Constraint {
        Constraint::UserInputsWin
    }

    fn check(&self, ix: &LedgerIndex<'_>, out: &mut Vec<Violation>) {
        let ledger = ix.ledger;

        // An applied decision that replaced a stronger user input.
        for d in ledger
            .decisions
            .iter()
            .filter(|d| d.state == DecisionState::Applied)
        {
            let Some(replaced) = d.replaces.and_then(|id| ix.decision(id)) else {
                continue;
            };
            if replaced.source.is_user_input()
                && rank(&d.source) < rank(&replaced.source)
                && !refines_transfer(replaced, &d.source, &d.assignment)
            {
                out.push(violation(
                    Subject::Decision(d.id),
                    "user_input_overridden",
                    "the decision replaced a user's correction or rule with a weaker source",
                ));
            }
        }

        // The newest correction per transaction and kind must still be in effect, or be
        // replaced by a newer correction that is.
        let mut newest_user: HashMap<(TransactionId, u8), &crate::model::Decision> = HashMap::new();
        let mut applied: HashMap<(TransactionId, u8), &crate::model::Decision> = HashMap::new();
        for d in &ledger.decisions {
            let key = (d.transaction_id, kind(&d.assignment));
            if d.source == DecisionSource::User
                && newest_user.get(&key).is_none_or(|u| u.id.0 < d.id.0)
            {
                newest_user.insert(key, d);
            }
            // With several applied (a constraint 4 violation), the newest counts, so the
            // report doesn't depend on row order.
            if d.state == DecisionState::Applied
                && applied.get(&key).is_none_or(|a| a.id.0 < d.id.0)
            {
                applied.insert(key, d);
            }
        }
        for (key, user) in &newest_user {
            if user.state == DecisionState::Applied {
                continue;
            }
            let replaced_by_newer_user = applied
                .get(key)
                .is_some_and(|a| a.source == DecisionSource::User && a.id.0 > user.id.0);
            if !replaced_by_newer_user {
                out.push(violation(
                    Subject::Transaction(key.0),
                    "user_choice_not_in_effect",
                    "the user's latest correction is not in effect and no newer correction replaced it",
                ));
            }
        }

        // A model decision where one of the user's rules matches.
        for d in ledger.decisions.iter().filter(|d| {
            d.state == DecisionState::Applied && matches!(d.source, DecisionSource::Model(_))
        }) {
            let Some(t) = ix.transaction(d.transaction_id) else {
                continue;
            };
            let won = winners(&ledger.rules, t);
            let rule_applies = match d.assignment {
                Assignment::Bucket(_) => won
                    .bucket
                    .and_then(|r| r.action.bucket)
                    .is_some_and(|b| matches!(fit(b, t), Fit::Apply(_))),
                Assignment::Merchant(_) => won.merchant.is_some(),
            };
            if rule_applies {
                out.push(violation(
                    Subject::Decision(d.id),
                    "model_over_matching_rule",
                    "a model decision is in effect where one of the user's rules matches",
                ));
            }
        }
    }
}
