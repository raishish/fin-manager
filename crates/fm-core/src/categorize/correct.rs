//! User corrections. A correction is the user's decision about one transaction; it
//! becomes a rule (SPEC §2, feature 3) that re-applies to every matching transaction.

use std::collections::HashMap;

use super::apply::{CategorizeError, Loaded, apply_to_ledger, write};
use super::merchant;
use super::normalize::normalize;
use super::plan::{Kind, LedgerView, Plan, Planner, RunReport, needs_review_source};
use super::rules::matcher_matches;
use crate::model::{
    Assignment, Bucket, Decision, DecisionId, DecisionSource, DecisionState, Ledger, Rule,
    RuleAction, RuleId, RuleMatcher, RuleOrigin, TextMatch, Timestamp, TransactionId,
};
use crate::storage::Unlocked;

/// What the user says about one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Correction {
    /// Where its money belongs. `Transfer { counterpart: Some(x) }` pairs it with `x`,
    /// and `x` with it.
    Bucket(Bucket),
    /// Its merchant's name.
    Merchant(String),
}

/// Whether and how the correction becomes a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Learn {
    /// A rule matching [`learned_matcher`] of the description. If the description
    /// names no merchant (a paper check), no rule is made.
    Default,
    /// A rule with this matcher, e.g. one the user edited after a [`preview`].
    Matcher(RuleMatcher),
    /// This transaction only.
    No,
}

/// What a correction did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectionReport {
    /// The user decision now in effect for the transaction.
    pub decision: DecisionId,
    /// The rule learned from it, if any.
    pub rule: Option<RuleId>,
    /// `Learn::Default` found no merchant to match on, so no rule was made.
    pub no_pattern: bool,
    /// Transactions whose transfer pair the correction broke. Each is now in Needs
    /// review, or an unpaired transfer if the user had paired it.
    pub unpaired: Vec<TransactionId>,
    /// What re-applying the rules to the transactions the new rule matches did.
    pub history: RunReport,
}

/// The default matcher learned from a description: `contains` the merchant's first
/// words, up to and including the second significant one (`BLUE BOTTLE` from
/// `SQ *BLUE BOTTLE COFF SAN FRANCISCO CA`, `THE HOME DEPOT`, `CITY OF COLUMBUS`). Short
/// enough to match the merchant's other description variants, long enough not to match
/// other merchants. `None` when the description names no merchant.
pub fn learned_matcher(description: &str) -> Option<RuleMatcher> {
    const INSIGNIFICANT: &[&str] = &["THE", "OF", "AND", "A", "AN", "AT", "FOR", "TO"];
    let cleaned = merchant::clean(description)?;
    let mut words = Vec::new();
    let mut significant = 0;
    for word in cleaned.core.split(' ') {
        words.push(word);
        if !INSIGNIFICANT.contains(&word) {
            significant += 1;
            if significant == 2 {
                break;
            }
        }
    }
    let pattern = normalize(&words.join(" "));
    if pattern.is_empty() {
        return None;
    }
    Some(RuleMatcher {
        description: TextMatch::Contains(pattern),
        account_id: None,
        amount: None,
    })
}

/// How many transactions `matcher` would match, for the UI to show before saving.
pub fn preview(view: LedgerView<'_>, matcher: &RuleMatcher) -> usize {
    view.transactions
        .iter()
        .filter(|t| matcher_matches(matcher, t))
        .count()
}

/// Applies a correction to the stored ledger, learns its rule and re-applies the rules
/// to every transaction, all in one savepoint.
pub fn correct(
    unlocked: &mut Unlocked,
    transaction: TransactionId,
    correction: Correction,
    learn: Learn,
    now: Timestamp,
) -> Result<CorrectionReport, CategorizeError> {
    let data = Loaded::from(unlocked)?;
    let planned = plan_correction(data.view(), transaction, &correction, &learn, now)?;
    write(unlocked, &planned.plan, &planned.rules)?;
    Ok(planned.report)
}

/// [`correct`] on an in-memory ledger.
pub fn correct_ledger(
    ledger: &mut Ledger,
    transaction: TransactionId,
    correction: Correction,
    learn: Learn,
    now: Timestamp,
) -> Result<CorrectionReport, CategorizeError> {
    let planned = plan_correction(
        LedgerView::from(&*ledger),
        transaction,
        &correction,
        &learn,
        now,
    )?;
    apply_to_ledger(ledger, &planned.plan);
    ledger.rules.extend(planned.rules);
    Ok(planned.report)
}

/// A correction's writes: the plan, the rules to insert, and the report.
#[derive(Debug, Clone)]
pub struct PlannedCorrection {
    pub plan: Plan,
    pub rules: Vec<Rule>,
    pub report: CorrectionReport,
}

/// Plans a correction without writing it:
///
/// 1. A User decision on the transaction, in effect even over a decision the user has
///    seen.
/// 2. Transfer pairs: pairing with `x` also gives `x` a User decision pointing back.
///    A pair the correction breaks is undone on the other side: Needs review if the
///    transfer matcher made it, an unpaired transfer if the user did.
/// 3. The learned rule (see [`Learn`]), with no counterpart or original: a rule can't
///    know the other side of a future transaction.
/// 4. The rules re-applied to every transaction the new rule matches (never over User
///    decisions). A rule can't change anything it doesn't match.
pub fn plan_correction(
    view: LedgerView<'_>,
    transaction: TransactionId,
    correction: &Correction,
    learn: &Learn,
    now: Timestamp,
) -> Result<PlannedCorrection, CategorizeError> {
    let by_id: HashMap<TransactionId, &crate::model::Transaction> =
        view.transactions.iter().map(|t| (t.id, t)).collect();
    let t = *by_id
        .get(&transaction)
        .ok_or(CategorizeError::UnknownTransaction)?;
    let assignment = match correction {
        Correction::Bucket(bucket) => {
            check_bucket(view, &by_id, t, bucket)?;
            Assignment::Bucket(*bucket)
        }
        Correction::Merchant(name) => Assignment::Merchant(name.trim().to_string()),
    };

    let mut planner = Planner::new(view, now);
    let mut unpaired = Vec::new();

    // 2. Pairs broken on this side, and the new pair's other side.
    if let Assignment::Bucket(new_bucket) = assignment {
        if let Some(old) = paired_with(planner.current().applied(t.id, Kind::Bucket))
            && new_bucket
                != (Bucket::Transfer {
                    counterpart: Some(old),
                })
        {
            unpair(&mut planner, old, t.id, &mut unpaired);
        }
        if let Bucket::Transfer {
            counterpart: Some(x),
        } = new_bucket
        {
            if let Some(y) = paired_with(planner.current().applied(x, Kind::Bucket))
                && y != t.id
            {
                unpair(&mut planner, y, x, &mut unpaired);
            }
            planner.propose(
                x,
                DecisionSource::User,
                Assignment::Bucket(Bucket::Transfer {
                    counterpart: Some(t.id),
                }),
            );
        }
    }

    // 1. The user's decision.
    planner.propose(t.id, DecisionSource::User, assignment.clone());
    let decision = planner
        .current()
        .applied(t.id, kind_of(&assignment))
        .map(|d| d.id)
        .expect("a user decision is always in effect after a correction");

    // 3. The rule.
    let learnable = !matches!(assignment, Assignment::Bucket(Bucket::NeedsReview));
    let matcher = match learn {
        Learn::No => None,
        _ if !learnable => None,
        Learn::Default => learned_matcher(&t.description),
        Learn::Matcher(m) => Some(m.clone()),
    };
    let no_pattern = matches!(learn, Learn::Default) && learnable && matcher.is_none();
    let rule = matcher
        .map(|matcher| Rule {
            id: RuleId(view.rules.iter().map(|r| r.id.0).max().unwrap_or(0) + 1),
            matcher,
            action: match &assignment {
                Assignment::Bucket(b) => RuleAction {
                    bucket: Some(general(*b)),
                    merchant: None,
                },
                Assignment::Merchant(m) => RuleAction {
                    bucket: None,
                    merchant: Some(m.clone()),
                },
            },
            origin: RuleOrigin::Correction {
                transaction_id: t.id,
                decision_id: decision,
            },
            priority: 0,
            enabled: true,
            created_at: now,
        })
        .filter(|r| r.validate().is_ok());

    // 4. The rules over the whole ledger, seeing the decisions above as current.
    let first = planner.finish();
    let mut rules: Vec<Rule> = view.rules.to_vec();
    rules.extend(rule.iter().cloned());
    let with_rule = LedgerView {
        rules: &rules,
        ..view
    };
    // Adding one rule can only change what it matches, so only those are re-planned.
    let mut history = Planner::with_prior(with_rule, now, &first);
    if let Some(rule) = &rule {
        for tx in view
            .transactions
            .iter()
            .filter(|tx| super::rules::rule_matches(rule, tx))
        {
            history.transaction(tx);
        }
    }
    let second = history.finish();
    let report = CorrectionReport {
        decision,
        rule: rule.as_ref().map(|r| r.id),
        no_pattern,
        unpaired,
        history: second.report,
    };
    Ok(PlannedCorrection {
        plan: merge(first, second),
        rules: rule.into_iter().collect(),
        report,
    })
}

fn kind_of(assignment: &Assignment) -> Kind {
    super::plan::kind(assignment)
}

/// The counterpart of a transfer-pair decision.
fn paired_with(decision: Option<&Decision>) -> Option<TransactionId> {
    match decision?.assignment {
        Assignment::Bucket(Bucket::Transfer { counterpart }) => counterpart,
        _ => None,
    }
}

/// Undoes the pair on `other`'s side if it points at `this`.
fn unpair(
    planner: &mut Planner<'_>,
    other: TransactionId,
    this: TransactionId,
    unpaired: &mut Vec<TransactionId>,
) {
    let Some(current) = planner.current().applied(other, Kind::Bucket).cloned() else {
        return;
    };
    if paired_with(Some(&current)) != Some(this) {
        return;
    }
    let (source, bucket) = if current.source == DecisionSource::User {
        (DecisionSource::User, Bucket::Transfer { counterpart: None })
    } else {
        (needs_review_source(), Bucket::NeedsReview)
    };
    planner.force(other, source, Assignment::Bucket(bucket));
    unpaired.push(other);
}

/// A corrected bucket as a rule's action: without the counterpart or original that
/// only this transaction has.
fn general(bucket: Bucket) -> Bucket {
    match bucket {
        Bucket::Transfer { .. } => Bucket::Transfer { counterpart: None },
        Bucket::Refund { category_id, .. } => Bucket::Refund {
            category_id,
            original: None,
        },
        other => other,
    }
}

/// Rejects a bucket that would break the ledger: income or a refund for money going
/// out, an unknown category, or a pair that can't be one.
fn check_bucket(
    view: LedgerView<'_>,
    by_id: &HashMap<TransactionId, &crate::model::Transaction>,
    t: &crate::model::Transaction,
    bucket: &Bucket,
) -> Result<(), CategorizeError> {
    let money_out = t.amount.minor() < 0;
    match *bucket {
        Bucket::Income | Bucket::Refund { .. } if money_out => {
            return Err(CategorizeError::DoesNotFit);
        }
        Bucket::Category { category_id } | Bucket::Refund { category_id, .. } => {
            if view.taxonomy.category(category_id).is_none() {
                return Err(CategorizeError::UnknownCategory);
            }
        }
        Bucket::Transfer {
            counterpart: Some(x),
        } => {
            let other = by_id.get(&x).ok_or(CategorizeError::InvalidPair)?;
            let nets_to_zero = other.amount.currency() == t.amount.currency()
                && other.amount.minor().checked_add(t.amount.minor()) == Some(0);
            if x == t.id || other.account_id == t.account_id || !nets_to_zero {
                return Err(CategorizeError::InvalidPair);
            }
        }
        Bucket::Transfer { counterpart: None } | Bucket::Income | Bucket::NeedsReview => {}
    }
    Ok(())
}

/// Joins a correction's two plans. The second sees the first's decisions as stored, so
/// a first-plan decision it replaces is marked superseded in place.
fn merge(mut first: Plan, second: Plan) -> Plan {
    for id in second.superseded {
        if let Some(d) = first.new_decisions.iter_mut().find(|d| d.id == id) {
            d.state = DecisionState::Superseded;
        } else if !first.superseded.contains(&id) {
            first.superseded.push(id);
        }
    }
    first.new_decisions.extend(second.new_decisions);
    first.report.add(second.report);
    first
}
