//! Constraint 3: models pick from a closed list. The model chooses only from the user's
//! categories, Income or "uncategorised" (SPEC decision 45), enforced at generation time
//! by a grammar. This check makes sure nothing outside that list stands as a model
//! decision: no transfer, and nothing that can't be what the model was offered (income
//! or a refund for money going out).

use super::{Check, Constraint, LedgerIndex, Subject, Violation};
use crate::model::{Assignment, Bucket, DecisionSource, DecisionState};

pub struct ClosedList;

impl Check for ClosedList {
    fn constraint(&self) -> Constraint {
        Constraint::ClosedList
    }

    fn check(&self, ix: &LedgerIndex<'_>, out: &mut Vec<Violation>) {
        for d in &ix.ledger.decisions {
            if d.state != DecisionState::Applied || !matches!(d.source, DecisionSource::Model(_)) {
                continue;
            }
            let Assignment::Bucket(bucket) = d.assignment else {
                continue;
            };
            let money_out = ix
                .transaction(d.transaction_id)
                .is_some_and(|t| t.amount.minor() < 0);
            let violation = match bucket {
                Bucket::Transfer { .. } => Some((
                    "model_chose_transfer",
                    "a model decision files a transfer, which only transfer detection, rules and the user may do",
                )),
                Bucket::Income | Bucket::Refund { .. } if money_out => Some((
                    "model_bucket_does_not_fit",
                    "a model decision files money going out as income or a refund",
                )),
                _ => None,
            };
            if let Some((code, message)) = violation {
                out.push(Violation {
                    constraint: Constraint::ClosedList,
                    subject: Subject::Decision(d.id),
                    code,
                    message,
                });
            }
        }
    }
}
