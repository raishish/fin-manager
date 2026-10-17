//! Constraint 8: every decision shows its source: a rule, the user, or the model (with
//! model identity and confidence).
//!
//! The type guarantees every decision *has* a source; this check makes sure it can be
//! followed: the rule exists, the model is fully identified, the decision is about a
//! real transaction, the decision it replaced exists, and a correction-learned rule
//! points at the user's correction it came from.

use std::collections::HashSet;

use super::{Check, Constraint, LedgerIndex, Subject, Violation};
use crate::model::{DecisionSource, RuleOrigin};

pub struct SourceShown;

fn violation(subject: Subject, code: &'static str, message: &'static str) -> Violation {
    Violation {
        constraint: Constraint::SourceShown,
        subject,
        code,
        message,
    }
}

impl Check for SourceShown {
    fn constraint(&self) -> Constraint {
        Constraint::SourceShown
    }

    fn check(&self, ix: &LedgerIndex<'_>, out: &mut Vec<Violation>) {
        let ledger = ix.ledger;

        let mut seen = HashSet::new();
        for d in &ledger.decisions {
            let subject = Subject::Decision(d.id);
            if !seen.insert(d.id) {
                out.push(violation(
                    subject,
                    "duplicate_decision_id",
                    "two decisions share this id, so neither can be traced",
                ));
            }
            if ix.transaction(d.transaction_id).is_none() {
                out.push(violation(
                    subject,
                    "unknown_transaction",
                    "the decision is about a transaction that is not in the ledger",
                ));
            }
            if d.taxonomy_version.base.trim().is_empty() {
                out.push(violation(
                    subject,
                    "taxonomy_version_missing",
                    "the decision doesn't say which category list it was made against",
                ));
            }
            match &d.source {
                DecisionSource::User | DecisionSource::Builtin { .. } => {}
                DecisionSource::Rule { rule_id } => {
                    if ix.rule(*rule_id).is_none() {
                        out.push(violation(
                            subject,
                            "unknown_rule",
                            "the decision names a rule that is not in the ledger",
                        ));
                    }
                }
                DecisionSource::Model(p) => {
                    if p.model_id.trim().is_empty() {
                        out.push(violation(
                            subject,
                            "model_id_missing",
                            "a model decision doesn't name its model",
                        ));
                    }
                    if p.gguf_sha256.as_bytes().iter().all(|&b| b == 0) {
                        out.push(violation(
                            subject,
                            "model_hash_missing",
                            "a model decision doesn't give its model file's hash",
                        ));
                    }
                    if p.prompt_version.trim().is_empty() {
                        out.push(violation(
                            subject,
                            "prompt_version_missing",
                            "a model decision doesn't give its prompt version",
                        ));
                    }
                }
            }
            if let Some(replaced) = d.replaces {
                match ix.decision(replaced) {
                    _ if replaced == d.id => out.push(violation(
                        subject,
                        "replaces_itself",
                        "a decision says it replaced itself",
                    )),
                    None => out.push(violation(
                        subject,
                        "unknown_replaced_decision",
                        "the decision it replaced is not in the ledger",
                    )),
                    Some(r) if r.transaction_id != d.transaction_id => out.push(violation(
                        subject,
                        "replaces_other_transaction",
                        "the decision it replaced is about another transaction",
                    )),
                    Some(_) => {}
                }
            }
        }

        let mut seen = HashSet::new();
        for r in &ledger.rules {
            let subject = Subject::Rule(r.id);
            if !seen.insert(r.id) {
                out.push(violation(
                    subject,
                    "duplicate_rule_id",
                    "two rules share this id, so decisions naming it can't be traced",
                ));
            }
            if let RuleOrigin::Correction {
                transaction_id,
                decision_id,
            } = r.origin
            {
                if ix.transaction(transaction_id).is_none() {
                    out.push(violation(
                        subject,
                        "unknown_correction_transaction",
                        "a learned rule's source transaction is not in the ledger",
                    ));
                }
                match ix.decision(decision_id) {
                    None => out.push(violation(
                        subject,
                        "unknown_correction_decision",
                        "the correction a rule was learned from is not in the ledger",
                    )),
                    Some(c) if c.transaction_id != transaction_id => out.push(violation(
                        subject,
                        "correction_about_other_transaction",
                        "the correction a rule was learned from is about another transaction",
                    )),
                    Some(c) if c.source != DecisionSource::User => out.push(violation(
                        subject,
                        "correction_not_by_user",
                        "a rule was learned from a decision the user didn't make",
                    )),
                    Some(_) => {}
                }
            }
        }
    }
}
