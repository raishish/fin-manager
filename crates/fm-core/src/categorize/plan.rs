//! The decision planner: what the rules and the merchant clean-up decide for a set of
//! transactions, as new decisions and state changes. Pure: it reads a ledger and returns
//! a [`Plan`]; [`super::apply`] writes plans to a [`Ledger`] or to the database.

use std::collections::HashMap;

use super::merchant;
use super::rules::{self, Fit};
use crate::model::{
    Assignment, Bucket, BuiltinLogic, Decision, DecisionId, DecisionSource, DecisionState, Ledger,
    Rule, Taxonomy, Timestamp, Transaction, TransactionId,
};

/// Version of the import pipeline's Needs-review placeholder, which the planner also
/// writes when a transaction has no bucket at all (SPEC constraint 4).
pub const NEEDS_REVIEW_PLACEHOLDER_VERSION: u32 = 1;

/// What the planner reads: a ledger's transactions, decisions, rules and category list.
/// Raw imports aren't needed.
#[derive(Debug, Clone, Copy)]
pub struct LedgerView<'a> {
    pub transactions: &'a [Transaction],
    pub decisions: &'a [Decision],
    pub rules: &'a [Rule],
    pub taxonomy: &'a Taxonomy,
}

impl<'a> From<&'a Ledger> for LedgerView<'a> {
    fn from(ledger: &'a Ledger) -> Self {
        LedgerView {
            transactions: &ledger.transactions,
            decisions: &ledger.decisions,
            rules: &ledger.rules,
            taxonomy: &ledger.taxonomy,
        }
    }
}

/// Which transactions to (re)decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope<'a> {
    All,
    /// For example the transactions an import inserted or settled.
    Transactions(&'a [TransactionId]),
}

/// New decisions to insert and decisions to mark superseded. Apply the state changes
/// first, then insert, in one transaction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub superseded: Vec<DecisionId>,
    pub new_decisions: Vec<Decision>,
    pub report: RunReport,
}

/// Counts of what a run did, for the UI and the evals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunReport {
    /// Transactions looked at.
    pub transactions: usize,
    /// Buckets set from a rule.
    pub rule_buckets: usize,
    /// Merchant names set from a rule.
    pub rule_merchants: usize,
    /// Merchant names set by the clean-up.
    pub cleaned_merchants: usize,
    /// Needs-review placeholders written for transactions with no bucket.
    pub needs_review_added: usize,
    /// Changes written as suggestions because the user had seen the decision.
    pub suggestions: usize,
    /// A matching rule's bucket didn't fit the transaction (income on money out), so
    /// the transaction was left as it is. The UI shows "a rule didn't fit".
    pub rule_did_not_fit: usize,
    /// A rule matched but the user's own decision stands (constraint 2).
    pub kept_user_decision: usize,
    /// A rule matched but a transfer pair stands: rules never break a pair.
    pub kept_transfer_pair: usize,
    /// Fixed logic would have suggested or applied what the user dismissed for that
    /// transaction, so it didn't.
    pub kept_dismissed: usize,
}

impl RunReport {
    pub(crate) fn add(&mut self, other: RunReport) {
        self.transactions += other.transactions;
        self.rule_buckets += other.rule_buckets;
        self.rule_merchants += other.rule_merchants;
        self.cleaned_merchants += other.cleaned_merchants;
        self.needs_review_added += other.needs_review_added;
        self.suggestions += other.suggestions;
        self.rule_did_not_fit += other.rule_did_not_fit;
        self.kept_user_decision += other.kept_user_decision;
        self.kept_transfer_pair += other.kept_transfer_pair;
        self.kept_dismissed += other.kept_dismissed;
    }
}

/// How strongly a source's decision holds (SPEC constraint 2): the user, then the
/// user's rules, then fixed logic, then the model, then the Needs-review placeholder.
/// A source never replaces a decision from a stronger one.
pub fn rank(source: &DecisionSource) -> u8 {
    match source {
        DecisionSource::User => 4,
        DecisionSource::Rule { .. } => 3,
        DecisionSource::Builtin {
            logic: BuiltinLogic::TransferMatcher | BuiltinLogic::MerchantCleaner,
            ..
        } => 2,
        DecisionSource::Model(_) => 1,
        DecisionSource::Builtin {
            logic: BuiltinLogic::ImportPipeline,
            ..
        } => 0,
    }
}

/// A matched transfer pair refining a rule's or the model's "transfer to an untracked
/// account": the same bucket, with the other side now known. Not an override.
pub fn refines_transfer(
    current: &Decision,
    candidate_source: &DecisionSource,
    candidate: &Assignment,
) -> bool {
    matches!(
        candidate_source,
        DecisionSource::Builtin {
            logic: BuiltinLogic::TransferMatcher,
            ..
        }
    ) && matches!(
        candidate,
        Assignment::Bucket(Bucket::Transfer {
            counterpart: Some(_)
        })
    ) && matches!(
        current.assignment,
        Assignment::Bucket(Bucket::Transfer { counterpart: None })
    ) && !matches!(current.source, DecisionSource::User)
}

/// What to do with a candidate decision, given the one in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to write: the same decision is already in effect, or a stronger source
    /// decided (see [`Held`]).
    Keep(Option<Held>),
    /// Write it as applied, replacing the current decision (if any).
    Apply,
    /// Write it as a suggestion: the user has seen the current decision, and the
    /// candidate isn't the user's own doing (constraint 7).
    Suggest,
}

/// Why a candidate was turned down, when it is worth reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    UserDecision,
    TransferPair,
}

/// The precedence (constraint 2) and no-silent-changes (constraint 7) policy for one
/// assignment kind of one transaction. Shared with the model fallback (T2.5).
///
/// 1. No current decision: apply.
/// 2. The same assignment from the same source: keep.
/// 3. A User decision is replaced only by a newer User decision.
/// 4. A transfer pair with a counterpart is replaced only by the user; the transfer
///    matcher's pair refines an unpaired transfer from a rule or the model.
/// 5. A weaker source than the current one: keep.
/// 6. The user has seen the current decision and the candidate isn't a user input
///    (User or Rule): suggest. Otherwise apply.
pub fn decide(
    current: Option<&Decision>,
    source: &DecisionSource,
    assignment: &Assignment,
) -> Outcome {
    let Some(current) = current else {
        return Outcome::Apply;
    };
    if current.source == *source && current.assignment == *assignment {
        return Outcome::Keep(None);
    }
    let candidate_is_user = matches!(source, DecisionSource::User);
    if matches!(current.source, DecisionSource::User) && !candidate_is_user {
        return Outcome::Keep(Some(Held::UserDecision));
    }
    let is_pair = matches!(
        current.assignment,
        Assignment::Bucket(Bucket::Transfer {
            counterpart: Some(_)
        })
    );
    if is_pair && !candidate_is_user {
        return Outcome::Keep(Some(Held::TransferPair));
    }
    if rank(source) < rank(&current.source) && !refines_transfer(current, source, assignment) {
        return Outcome::Keep(None);
    }
    if current.seen_at.is_some() && !source.is_user_input() {
        Outcome::Suggest
    } else {
        Outcome::Apply
    }
}

/// Plans what rules and the merchant clean-up decide for the transactions in `scope`:
///
/// - **Bucket:** the winning bucket rule ([`rules::winners`]), fitted to the
///   transaction's sign ([`rules::fit`]); a transaction with no bucket at all gets the
///   Needs-review placeholder (source `builtin/import_pipeline`).
/// - **Merchant:** the winning merchant rule, else the clean-up ([`merchant::clean`]).
///
/// Each candidate goes through [`decide`]. Replacing a decision supersedes it and any
/// open suggestion for the same assignment kind.
pub fn plan(view: LedgerView<'_>, scope: Scope<'_>, now: Timestamp) -> Plan {
    let mut planner = Planner::new(view, now);
    let selected: Vec<&Transaction> = match scope {
        Scope::All => view.transactions.iter().collect(),
        Scope::Transactions(ids) => {
            let by_id: HashMap<TransactionId, &Transaction> =
                view.transactions.iter().map(|t| (t.id, t)).collect();
            let mut seen = std::collections::HashSet::new();
            ids.iter()
                .filter(|id| seen.insert(**id))
                .filter_map(|id| by_id.get(id).copied())
                .collect()
        }
    };
    for transaction in selected {
        planner.transaction(transaction);
    }
    planner.finish()
}

/// Which assignment a decision is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Bucket,
    Merchant,
}

pub(crate) fn kind(assignment: &Assignment) -> Kind {
    match assignment {
        Assignment::Bucket(_) => Kind::Bucket,
        Assignment::Merchant(_) => Kind::Merchant,
    }
}

/// The applied decision and open suggestions per transaction and kind.
#[derive(Debug, Default)]
pub(crate) struct Current {
    applied: HashMap<(TransactionId, Kind), Decision>,
    suggested: HashMap<(TransactionId, Kind), Vec<Decision>>,
    /// Assignments the user dismissed (a suggestion) or rejected (a decision), per
    /// transaction and kind: both end `dismissed`.
    dismissed: HashMap<(TransactionId, Kind), Vec<Assignment>>,
}

impl Current {
    pub(crate) fn of(decisions: &[Decision]) -> Current {
        let mut current = Current::default();
        for d in decisions {
            let key = (d.transaction_id, kind(&d.assignment));
            match d.state {
                DecisionState::Applied => {
                    // Constraint 4 allows one; if a broken ledger has several, the
                    // newest is taken as current.
                    let newer = current.applied.get(&key).is_none_or(|c| c.id.0 < d.id.0);
                    if newer {
                        current.applied.insert(key, d.clone());
                    }
                }
                DecisionState::Suggested => {
                    current.suggested.entry(key).or_default().push(d.clone())
                }
                DecisionState::Dismissed => current
                    .dismissed
                    .entry(key)
                    .or_default()
                    .push(d.assignment.clone()),
                DecisionState::Superseded => {}
            }
        }
        current
    }

    pub(crate) fn applied(&self, t: TransactionId, k: Kind) -> Option<&Decision> {
        self.applied.get(&(t, k))
    }
}

/// Builds a plan, keeping track of what it has already changed so several steps can
/// apply to the same transaction.
pub(crate) struct Planner<'a> {
    view: LedgerView<'a>,
    rules: rules::RuleSet<'a>,
    now: Timestamp,
    next_id: i64,
    current: Current,
    plan: Plan,
}

impl<'a> Planner<'a> {
    pub(crate) fn new(view: LedgerView<'a>, now: Timestamp) -> Planner<'a> {
        Planner {
            view,
            rules: rules::RuleSet::new(view.rules),
            now,
            next_id: view.decisions.iter().map(|d| d.id.0).max().unwrap_or(0) + 1,
            current: Current::of(view.decisions),
            plan: Plan::default(),
        }
    }

    /// Continues after decisions already planned elsewhere (a correction), which are
    /// treated as current.
    pub(crate) fn with_prior(view: LedgerView<'a>, now: Timestamp, prior: &Plan) -> Planner<'a> {
        let mut all: Vec<Decision> = view
            .decisions
            .iter()
            .filter(|d| !prior.superseded.contains(&d.id))
            .cloned()
            .collect();
        all.extend(prior.new_decisions.iter().cloned());
        let mut planner = Planner::new(view, now);
        // Ids are counted over every stored decision, superseded ones included: they
        // keep their rows.
        planner.next_id = view
            .decisions
            .iter()
            .chain(&prior.new_decisions)
            .map(|d| d.id.0)
            .max()
            .unwrap_or(0)
            + 1;
        planner.current = Current::of(&all);
        planner
    }

    pub(crate) fn finish(self) -> Plan {
        self.plan
    }

    pub(crate) fn current(&self) -> &Current {
        &self.current
    }

    pub(crate) fn transaction(&mut self, t: &Transaction) {
        self.plan.report.transactions += 1;
        let won = self.rules.winners(t);

        // Bucket.
        let mut placed = false;
        if let Some(rule) = won.bucket {
            let bucket = rule
                .action
                .bucket
                .expect("a bucket winner has a bucket action");
            match rules::fit(bucket, t) {
                Fit::Apply(bucket) => {
                    let source = DecisionSource::Rule { rule_id: rule.id };
                    if self.propose(t.id, source, Assignment::Bucket(bucket)) {
                        self.plan.report.rule_buckets += 1;
                    }
                    placed = true;
                }
                Fit::DoesNotFit => self.plan.report.rule_did_not_fit += 1,
            }
        }
        if !placed && self.current.applied(t.id, Kind::Bucket).is_none() {
            let source = needs_review_source();
            if self.propose(t.id, source, Assignment::Bucket(Bucket::NeedsReview)) {
                self.plan.report.needs_review_added += 1;
            }
        }

        // Merchant.
        if let Some(rule) = won.merchant {
            let name = rule
                .action
                .merchant
                .clone()
                .expect("a merchant winner has a merchant action");
            let source = DecisionSource::Rule { rule_id: rule.id };
            if self.propose(t.id, source, Assignment::Merchant(name)) {
                self.plan.report.rule_merchants += 1;
            }
        } else if let Some(cleaned) = merchant::clean(&t.description) {
            let source = DecisionSource::Builtin {
                logic: BuiltinLogic::MerchantCleaner,
                version: merchant::VERSION,
            };
            if self.propose(t.id, source, Assignment::Merchant(cleaned.name)) {
                self.plan.report.cleaned_merchants += 1;
            }
        }
    }

    /// Runs `assignment` from `source` through [`decide`] and records the result.
    /// Returns whether anything was written.
    pub(crate) fn propose(
        &mut self,
        transaction: TransactionId,
        source: DecisionSource,
        assignment: Assignment,
    ) -> bool {
        let k = kind(&assignment);
        let key = (transaction, k);
        let current = self.current.applied.get(&key).cloned();
        let outcome = decide(current.as_ref(), &source, &assignment);
        // A dismissal sticks: fixed logic and the model never bring the same assignment
        // back. The user's own rules still can, and the Needs-review placeholder is
        // always allowed (every transaction needs a bucket).
        if !matches!(outcome, Outcome::Keep(_))
            && !source.is_user_input()
            && assignment != Assignment::Bucket(Bucket::NeedsReview)
            && self
                .current
                .dismissed
                .get(&key)
                .is_some_and(|d| d.contains(&assignment))
        {
            self.plan.report.kept_dismissed += 1;
            return false;
        }
        let state = match outcome {
            Outcome::Keep(held) => {
                match held {
                    Some(Held::UserDecision) => self.plan.report.kept_user_decision += 1,
                    Some(Held::TransferPair) => self.plan.report.kept_transfer_pair += 1,
                    None => {}
                }
                return false;
            }
            Outcome::Apply => DecisionState::Applied,
            Outcome::Suggest => {
                let already = self.current.suggested.get(&key).is_some_and(|s| {
                    s.iter()
                        .any(|d| d.source == source && d.assignment == assignment)
                });
                if already {
                    return false;
                }
                DecisionState::Suggested
            }
        };
        self.record(transaction, current.as_ref(), source, assignment, state);
        true
    }

    /// Applies `assignment` whatever the precedence. Only for a correction's own
    /// consequences: undoing the other side of a transfer pair the user broke.
    pub(crate) fn force(
        &mut self,
        transaction: TransactionId,
        source: DecisionSource,
        assignment: Assignment,
    ) {
        let key = (transaction, kind(&assignment));
        let current = self.current.applied.get(&key).cloned();
        self.record(
            transaction,
            current.as_ref(),
            source,
            assignment,
            DecisionState::Applied,
        );
    }

    fn record(
        &mut self,
        transaction: TransactionId,
        current: Option<&Decision>,
        source: DecisionSource,
        assignment: Assignment,
        state: DecisionState,
    ) {
        let key = (transaction, kind(&assignment));
        // A decision this plan created and now replaces is dropped, and the new one
        // takes over what it replaced: nothing that was never in effect is stored.
        let replaces = match current {
            Some(c) => match self.take_planned(c.id) {
                Some(planned) => planned.replaces,
                None => Some(c.id),
            },
            None => None,
        };
        let decision = Decision {
            id: DecisionId(self.next_id),
            transaction_id: transaction,
            assignment,
            source,
            taxonomy_version: self.view.taxonomy.version.clone(),
            decided_at: self.now,
            state,
            seen_at: None,
            replaces: if state == DecisionState::Applied {
                replaces
            } else {
                current.map(|c| c.id)
            },
        };
        self.next_id += 1;

        // A new suggestion replaces older open ones; an applied change replaces the
        // current decision and makes every open suggestion stale.
        for stale in self.current.suggested.remove(&key).unwrap_or_default() {
            self.close_stale(&stale);
            // The other half of a pair suggestion goes too: half a pair can't be
            // accepted.
            if let Assignment::Bucket(Bucket::Transfer {
                counterpart: Some(other),
            }) = stale.assignment
            {
                let back = Assignment::Bucket(Bucket::Transfer {
                    counterpart: Some(stale.transaction_id),
                });
                let other_key = (other, Kind::Bucket);
                let halves = self
                    .current
                    .suggested
                    .remove(&other_key)
                    .unwrap_or_default();
                let (partners, rest): (Vec<_>, Vec<_>) = halves
                    .into_iter()
                    .partition(|d| d.assignment == back && d.source == stale.source);
                for partner in &partners {
                    self.close_stale(partner);
                }
                if !rest.is_empty() {
                    self.current.suggested.insert(other_key, rest);
                }
            }
        }
        if state == DecisionState::Applied {
            if let Some(c) = current
                && replaces == Some(c.id)
            {
                self.supersede(c.id);
            }
            self.current.applied.insert(key, decision.clone());
        } else {
            self.current
                .suggested
                .entry(key)
                .or_default()
                .push(decision.clone());
            self.plan.report.suggestions += 1;
        }
        self.plan.new_decisions.push(decision);
    }

    /// A suggestion no longer current: dropped if this plan created it, else superseded.
    fn close_stale(&mut self, stale: &Decision) {
        if self.take_planned(stale.id).is_none() {
            self.supersede(stale.id);
        }
    }

    /// Removes a decision this plan created, if `id` is one.
    fn take_planned(&mut self, id: DecisionId) -> Option<Decision> {
        let i = self.plan.new_decisions.iter().position(|d| d.id == id)?;
        Some(self.plan.new_decisions.remove(i))
    }

    /// Records that a stored decision is superseded.
    fn supersede(&mut self, id: DecisionId) {
        if !self.plan.superseded.contains(&id) {
            self.plan.superseded.push(id);
        }
    }
}

/// The source of the Needs-review placeholder: the import pipeline's (T1.3), also used
/// when the planner finds a transaction with no bucket.
pub fn needs_review_source() -> DecisionSource {
    DecisionSource::Builtin {
        logic: BuiltinLogic::ImportPipeline,
        version: NEEDS_REVIEW_PLACEHOLDER_VERSION,
    }
}
