//! The user's actions on rules and suggestions (T2.6, for the rule and review screens):
//! turning a rule on or off, deleting it, undoing what it decided, and accepting or
//! dismissing a suggestion. Each runs in one savepoint.

use std::collections::HashSet;

use super::apply::{CategorizeError, Loaded, in_savepoint, write_plan};
use super::plan::{
    Kind, LedgerView, Plan, Planner, RunReport, Scope, kind, needs_review_source, plan,
};
use crate::model::{
    Assignment, Bucket, Decision, DecisionId, DecisionSource, DecisionState, RuleId, Timestamp,
    TransactionId,
};
use crate::storage::{StorageError, Unlocked};

/// Turns a rule on or off. What it already decided stays (see [`undo_rule_decisions`]);
/// a disabled rule decides nothing new.
pub fn set_rule_enabled(
    unlocked: &mut Unlocked,
    rule: RuleId,
    enabled: bool,
) -> Result<(), CategorizeError> {
    in_savepoint(unlocked, |u| {
        u.set_rule_enabled(rule, enabled)
            .map_err(|e| not_found(e, CategorizeError::UnknownRule(rule)))
    })
}

/// Deletes a rule. The decisions it made go with it (the storage cascade), and the
/// transactions they were about are planned again without it: another rule, the
/// merchant clean-up, or the Needs-review placeholder, dated `now`. User decisions are
/// untouched.
pub fn delete_rule(
    unlocked: &mut Unlocked,
    rule: RuleId,
    now: Timestamp,
) -> Result<(), CategorizeError> {
    in_savepoint(unlocked, |u| {
        let affected: Vec<TransactionId> = decided_by(&u.decisions()?, rule, false)
            .map(|d| d.transaction_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        u.delete_rule(rule)
            .map_err(|e| not_found(e, CategorizeError::UnknownRule(rule)))?;
        let data = Loaded::from(u)?;
        let planned = plan(data.view(), Scope::Transactions(&affected), now);
        write_plan(u, &planned, &[])
    })
}

/// Supersedes every applied decision `rule` made and plans those transactions again
/// without it: another rule, the merchant clean-up, or the Needs-review placeholder.
/// The rule itself is kept; turn it off first, or the next run applies it again. User
/// decisions are never touched (the rule never replaced one).
pub fn undo_rule_decisions(
    unlocked: &mut Unlocked,
    rule: RuleId,
    now: Timestamp,
) -> Result<RunReport, CategorizeError> {
    in_savepoint(unlocked, |u| {
        let data = Loaded::from(u)?;
        if !data.rules.iter().any(|r| r.id == rule) {
            return Err(CategorizeError::UnknownRule(rule));
        }
        let undone: Vec<&Decision> = decided_by(&data.decisions, rule, true).collect();
        let prior = Plan {
            superseded: undone.iter().map(|d| d.id).collect(),
            ..Plan::default()
        };
        let without: Vec<_> = data
            .rules
            .iter()
            .filter(|r| r.id != rule)
            .cloned()
            .collect();
        let view = LedgerView {
            rules: &without,
            ..data.view()
        };
        let affected: HashSet<TransactionId> = undone.iter().map(|d| d.transaction_id).collect();
        let mut planner = Planner::with_prior(view, now, &prior);
        for t in data
            .transactions
            .iter()
            .filter(|t| affected.contains(&t.id))
        {
            planner.transaction(t);
        }
        let mut planned = planner.finish();
        planned.superseded.splice(0..0, prior.superseded);
        write_plan(u, &planned, &[])?;
        Ok(planned.report)
    })
}

/// Puts a suggestion into effect: it becomes applied, the decision it replaces is
/// superseded, and any other open suggestion for the same transaction and kind goes
/// too (with the other half of a pair suggestion among them). For one side of a
/// transfer pair, the other side's matching suggestion is accepted with it; if that
/// half is gone the pair can't be accepted (dismiss it instead). The user has now seen
/// the decisions (`seen_at` = `now`), so a later model change to them can only be a
/// suggestion. Returns the decisions now applied.
pub fn accept_suggestion(
    unlocked: &mut Unlocked,
    suggestion: DecisionId,
    now: Timestamp,
) -> Result<Vec<DecisionId>, CategorizeError> {
    in_savepoint(unlocked, |u| {
        let decisions = u.decisions()?;
        let group = suggestion_group(&decisions, suggestion, true)?;
        for s in &group {
            let current = applied(&decisions, s.transaction_id, kind(&s.assignment));
            if current.map(|c| c.id) != s.replaces || current.is_some_and(is_pair) {
                return Err(CategorizeError::StaleSuggestion(s.id));
            }
        }
        let accepted: HashSet<DecisionId> = group.iter().map(|s| s.id).collect();
        let slots: Vec<_> = group
            .iter()
            .map(|s| (s.transaction_id, kind(&s.assignment)))
            .collect();
        close_suggestions(u, &decisions, &slots, &accepted, DecisionState::Superseded)?;
        for s in &group {
            if let Some(old) = s.replaces {
                u.set_decision_state(old, DecisionState::Superseded)?;
            }
            u.set_decision_state(s.id, DecisionState::Applied)?;
            if s.seen_at.is_none() {
                u.mark_decision_seen(s.id, now)?;
            }
        }
        Ok(group.iter().map(|s| s.id).collect())
    })
}

/// Turns a suggestion down: it becomes dismissed, with the other side of a transfer
/// pair if that half is still open. It sticks: the rules engine's fixed logic and the
/// model never suggest or apply the same assignment for that transaction again (the
/// user's own rules still can). Returns the decisions dismissed.
pub fn dismiss_suggestion(
    unlocked: &mut Unlocked,
    suggestion: DecisionId,
    now: Timestamp,
) -> Result<Vec<DecisionId>, CategorizeError> {
    in_savepoint(unlocked, |u| {
        let decisions = u.decisions()?;
        let group = suggestion_group(&decisions, suggestion, false)?;
        for s in &group {
            u.set_decision_state(s.id, DecisionState::Dismissed)?;
            if s.seen_at.is_none() {
                u.mark_decision_seen(s.id, now)?;
            }
        }
        Ok(group.iter().map(|s| s.id).collect())
    })
}

/// Confirms decisions the user has looked at and agrees with (SPEC decision 46): each
/// stays as it is and is marked seen (`seen_at` = `now`), so a later model or
/// fixed-logic change to it can only be a suggestion (constraint 7). Skipped: decisions
/// already seen, and the user's own decisions and rules' (already user input). The
/// Needs-review placeholder decides nothing and can't be confirmed (NotConfirmable).
/// All or none: one refused id refuses the whole batch. Returns how many decisions
/// were newly marked seen.
pub fn confirm_decisions(
    unlocked: &mut Unlocked,
    ids: &[DecisionId],
    now: Timestamp,
) -> Result<usize, CategorizeError> {
    in_savepoint(unlocked, |u| {
        let decisions = u.decisions()?;
        let mut marked = 0;
        for id in unique(ids) {
            let d = in_effect(&decisions, id)?;
            if d.source == needs_review_source() {
                return Err(CategorizeError::NotConfirmable(id));
            }
            if d.seen_at.is_none() && !d.source.is_user_input() {
                u.mark_decision_seen(id, now)?;
                marked += 1;
            }
        }
        Ok(marked)
    })
}

/// Rejects buckets the user disagrees with but doesn't correct (SPEC decision 46): each
/// rejected decision is marked seen and dismissed, open suggestions for its bucket go
/// (with their pair halves), and the transaction is planned again, dated `now`: a user
/// rule that matches applies, otherwise the Needs-review placeholder. The rejection
/// sticks: fixed logic and the model never bring that assignment back for the
/// transaction. Rejecting one side of a transfer pair rejects both. Only a bucket from
/// the model or fixed logic can be rejected (not the placeholder); one the user has
/// already seen can be, and isn't marked seen twice; a model's "uncategorised" is only
/// marked seen. All or none. Returns how many decisions changed.
pub fn reject_decisions(
    unlocked: &mut Unlocked,
    ids: &[DecisionId],
    now: Timestamp,
) -> Result<usize, CategorizeError> {
    in_savepoint(unlocked, |u| {
        let decisions = u.decisions()?;
        let mut batch: Vec<&Decision> = Vec::new();
        for id in unique(ids) {
            let d = in_effect(&decisions, id)?;
            if !rejectable(d) {
                return Err(CategorizeError::NotRejectable(id));
            }
            batch.push(d);
            // The other side of a pair goes with it, if it is one: pointing back here,
            // from the model or fixed logic.
            if let Assignment::Bucket(Bucket::Transfer {
                counterpart: Some(other),
            }) = d.assignment
            {
                let back = Assignment::Bucket(Bucket::Transfer {
                    counterpart: Some(d.transaction_id),
                });
                match applied(&decisions, other, Kind::Bucket) {
                    Some(p) if p.assignment == back && rejectable(p) => batch.push(p),
                    _ => return Err(CategorizeError::NotRejectable(id)),
                }
            }
        }
        let mut unique_ids = HashSet::new();
        batch.retain(|d| unique_ids.insert(d.id));

        let mut changed = 0;
        let mut affected = Vec::new();
        for d in &batch {
            if d.seen_at.is_none() {
                u.mark_decision_seen(d.id, now)?;
            }
            if d.assignment == Assignment::Bucket(Bucket::NeedsReview) {
                // The model's "uncategorised": already in review; only record that the
                // user acted on it.
                if d.seen_at.is_none() {
                    changed += 1;
                }
                continue;
            }
            u.set_decision_state(d.id, DecisionState::Dismissed)?;
            affected.push(d.transaction_id);
            changed += 1;
        }
        let slots: Vec<_> = affected.iter().map(|t| (*t, Kind::Bucket)).collect();
        close_suggestions(
            u,
            &decisions,
            &slots,
            &HashSet::new(),
            DecisionState::Superseded,
        )?;
        let data = Loaded::from(u)?;
        let planned = plan(data.view(), Scope::Transactions(&affected), now);
        write_plan(u, &planned, &[])?;
        Ok(changed)
    })
}

/// A bucket from the model or fixed logic other than the Needs-review placeholder.
fn rejectable(d: &Decision) -> bool {
    matches!(d.assignment, Assignment::Bucket(_))
        && d.source != needs_review_source()
        && matches!(
            d.source,
            DecisionSource::Model(_) | DecisionSource::Builtin { .. }
        )
}

/// Closes (supersedes or dismisses) every open suggestion for these transactions and
/// kinds, except those in `keep`, together with the other half of any pair suggestion
/// among them, so no half is left open on its own.
fn close_suggestions(
    u: &Unlocked,
    decisions: &[Decision],
    slots: &[(TransactionId, Kind)],
    keep: &HashSet<DecisionId>,
    state: DecisionState,
) -> Result<(), CategorizeError> {
    let mut closing: Vec<&Decision> = decisions
        .iter()
        .filter(|d| {
            d.state == DecisionState::Suggested
                && !keep.contains(&d.id)
                && slots.contains(&(d.transaction_id, kind(&d.assignment)))
        })
        .collect();
    let partners: Vec<&Decision> = closing
        .iter()
        .filter_map(|s| partner_of(decisions, s))
        .filter(|p| !keep.contains(&p.id))
        .collect();
    closing.extend(partners);
    let mut done = HashSet::new();
    for d in closing {
        if done.insert(d.id) {
            u.set_decision_state(d.id, state)?;
        }
    }
    Ok(())
}

/// The other half of a pair suggestion: the open suggestion on the counterpart pointing
/// back, from the same source.
fn partner_of<'d>(decisions: &'d [Decision], s: &Decision) -> Option<&'d Decision> {
    let Assignment::Bucket(Bucket::Transfer {
        counterpart: Some(other),
    }) = s.assignment
    else {
        return None;
    };
    let back = Assignment::Bucket(Bucket::Transfer {
        counterpart: Some(s.transaction_id),
    });
    decisions.iter().find(|d| {
        d.transaction_id == other
            && d.state == DecisionState::Suggested
            && d.assignment == back
            && d.source == s.source
    })
}

/// `ids` without repeats, in order.
fn unique(ids: &[DecisionId]) -> Vec<DecisionId> {
    let mut seen = HashSet::new();
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}

/// The decision, which must exist and be in effect.
fn in_effect(decisions: &[Decision], id: DecisionId) -> Result<&Decision, CategorizeError> {
    let d = decisions
        .iter()
        .find(|d| d.id == id)
        .ok_or(CategorizeError::UnknownDecision(id))?;
    if d.state != DecisionState::Applied {
        return Err(CategorizeError::NotApplied(id));
    }
    Ok(d)
}

/// Decisions `rule` made; with `applied_only`, only those in effect.
fn decided_by(
    decisions: &[Decision],
    rule: RuleId,
    applied_only: bool,
) -> impl Iterator<Item = &Decision> {
    decisions.iter().filter(move |d| {
        d.source == DecisionSource::Rule { rule_id: rule }
            && (!applied_only || d.state == DecisionState::Applied)
    })
}

fn applied(decisions: &[Decision], t: TransactionId, k: Kind) -> Option<&Decision> {
    decisions
        .iter()
        .filter(|d| {
            d.transaction_id == t && d.state == DecisionState::Applied && kind(&d.assignment) == k
        })
        .max_by_key(|d| d.id)
}

fn is_pair(d: &Decision) -> bool {
    matches!(
        d.assignment,
        Assignment::Bucket(Bucket::Transfer {
            counterpart: Some(_)
        })
    )
}

/// The suggestion and, for one side of a transfer pair, the other side's open
/// suggestion. With `partner_required`, a missing other half makes it stale.
fn suggestion_group(
    decisions: &[Decision],
    id: DecisionId,
    partner_required: bool,
) -> Result<Vec<Decision>, CategorizeError> {
    let s = decisions
        .iter()
        .find(|d| d.id == id)
        .ok_or(CategorizeError::UnknownDecision(id))?;
    if s.state != DecisionState::Suggested {
        return Err(CategorizeError::NotASuggestion(id));
    }
    let mut group = vec![s.clone()];
    match partner_of(decisions, s) {
        Some(partner) => group.push(partner.clone()),
        None if partner_required && is_pair(s) => {
            return Err(CategorizeError::StaleSuggestion(id));
        }
        None => {}
    }
    Ok(group)
}

fn not_found(e: StorageError, instead: CategorizeError) -> CategorizeError {
    match e {
        StorageError::NotFound => instead,
        e => CategorizeError::Storage(e),
    }
}
