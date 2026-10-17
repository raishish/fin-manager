//! Writing plans: into an in-memory [`Ledger`], or into the encrypted database in one
//! savepoint.

use std::fmt;

use rusqlite::Connection;

use super::plan::{LedgerView, Plan, RunReport, Scope, plan};
use crate::model::{DecisionState, Ledger, Rule, Timestamp};
use crate::storage::{StorageError, Unlocked};

/// Why categorisation couldn't be stored.
#[derive(Debug)]
pub enum CategorizeError {
    Storage(StorageError),
    /// Another transaction is open on the connection; categorisation is its own unit of
    /// work.
    TransactionOpen,
    /// The transaction to correct isn't in the ledger.
    UnknownTransaction,
    /// A transfer pair named by a correction can't be one: the same transaction, a
    /// missing one, the same account, or amounts that don't net to zero.
    InvalidPair,
    /// The correction's category isn't in the category list.
    UnknownCategory,
}

impl fmt::Display for CategorizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(e) => write!(f, "{e}"),
            Self::TransactionOpen => f.write_str("another database transaction is open"),
            Self::UnknownTransaction => f.write_str("the transaction isn't in the ledger"),
            Self::InvalidPair => f.write_str("those two transactions can't be a transfer pair"),
            Self::UnknownCategory => f.write_str("the category isn't in the category list"),
        }
    }
}

impl std::error::Error for CategorizeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(e) => Some(e),
            _ => None,
        }
    }
}

impl From<StorageError> for CategorizeError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

impl From<rusqlite::Error> for CategorizeError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e.into())
    }
}

/// Applies a plan to an in-memory ledger.
pub fn apply_to_ledger(ledger: &mut Ledger, plan: &Plan) {
    for d in &mut ledger.decisions {
        if plan.superseded.contains(&d.id) {
            d.state = DecisionState::Superseded;
        }
    }
    ledger.decisions.extend(plan.new_decisions.iter().cloned());
}

/// Plans and applies in memory: `plan` then [`apply_to_ledger`].
pub fn run_on_ledger(ledger: &mut Ledger, scope: Scope<'_>, now: Timestamp) -> RunReport {
    let plan = plan(LedgerView::from(&*ledger), scope, now);
    apply_to_ledger(ledger, &plan);
    plan.report
}

/// Runs the rules and the merchant clean-up over `scope` and stores the result in one
/// savepoint. After an import, call it with the transactions the import inserted or
/// settled, after transfer detection and before the model fallback.
pub fn run(
    unlocked: &mut Unlocked,
    scope: Scope<'_>,
    now: Timestamp,
) -> Result<RunReport, CategorizeError> {
    let data = Loaded::from(unlocked)?;
    let plan = plan(data.view(), scope, now);
    write(unlocked, &plan, &[])?;
    Ok(plan.report)
}

/// The parts of a stored ledger the planner reads.
pub(crate) struct Loaded {
    pub(crate) transactions: Vec<crate::model::Transaction>,
    pub(crate) decisions: Vec<crate::model::Decision>,
    pub(crate) rules: Vec<Rule>,
    pub(crate) taxonomy: crate::model::Taxonomy,
}

impl Loaded {
    pub(crate) fn from(unlocked: &Unlocked) -> Result<Loaded, StorageError> {
        Ok(Loaded {
            transactions: unlocked.transactions()?,
            decisions: unlocked.decisions()?,
            rules: unlocked.rules()?,
            taxonomy: unlocked.taxonomy()?,
        })
    }

    pub(crate) fn view(&self) -> LedgerView<'_> {
        LedgerView {
            transactions: &self.transactions,
            decisions: &self.decisions,
            rules: &self.rules,
            taxonomy: &self.taxonomy,
        }
    }
}

/// Stores `rules` and `plan` in one savepoint: the state changes, then the new
/// decisions and rules in id order, so every reference (a rule's correction decision,
/// a decision's rule or `replaces`) points at a row already written.
pub(crate) fn write(
    unlocked: &mut Unlocked,
    plan: &Plan,
    rules: &[Rule],
) -> Result<(), CategorizeError> {
    let savepoint = Savepoint::begin(unlocked.connection())?;
    for id in &plan.superseded {
        unlocked.set_decision_state(*id, DecisionState::Superseded)?;
    }
    // A correction's rule names the user decision it came from; decisions made by the
    // rule name the rule. Writing decisions up to that one, then the rules, then the
    // rest keeps every reference pointing backwards.
    let last_correction = rules
        .iter()
        .filter_map(|r| match r.origin {
            crate::model::RuleOrigin::Correction { decision_id, .. } => Some(decision_id),
            crate::model::RuleOrigin::User => None,
        })
        .max();
    let mut decisions: Vec<&crate::model::Decision> = plan.new_decisions.iter().collect();
    decisions.sort_by_key(|d| d.id);
    let split = last_correction.map_or(0, |id| decisions.partition_point(|d| d.id <= id));
    for d in &decisions[..split] {
        unlocked.insert_decision(d)?;
    }
    for r in rules {
        unlocked.insert_rule(r)?;
    }
    for d in &decisions[split..] {
        unlocked.insert_decision(d)?;
    }
    savepoint.release()?;
    Ok(())
}

/// Categorisation's savepoint: rolled back unless released, including on an early
/// return or a panic.
struct Savepoint<'c> {
    conn: &'c Connection,
    open: bool,
}

impl<'c> Savepoint<'c> {
    const NAME: &'static str = "categorize";

    fn begin(conn: &'c Connection) -> Result<Savepoint<'c>, CategorizeError> {
        if !conn.is_autocommit() {
            return Err(CategorizeError::TransactionOpen);
        }
        conn.execute_batch(&format!("SAVEPOINT {}", Self::NAME))?;
        Ok(Savepoint { conn, open: true })
    }

    fn release(mut self) -> Result<(), CategorizeError> {
        self.conn
            .execute_batch(&format!("RELEASE {}", Self::NAME))?;
        self.open = false;
        Ok(())
    }
}

impl Drop for Savepoint<'_> {
    fn drop(&mut self) {
        if self.open {
            // The caller already has the original error.
            let _ = self
                .conn
                .execute_batch(&format!("ROLLBACK TO {0}; RELEASE {0}", Self::NAME));
        }
    }
}
