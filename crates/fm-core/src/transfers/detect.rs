//! Running transfer detection on a ledger: finding pairs among the transactions nothing
//! has decided yet, and recording them as transfer decisions.

use std::collections::HashMap;
use std::fmt;

use super::pairing::{Pair, find_pairs};
use crate::model::{
    Assignment, Bucket, BuiltinLogic, Decision, DecisionId, DecisionSource, DecisionState,
    Timestamp, TransactionId,
};
use crate::storage::{StorageError, Table, Unlocked};

/// The version recorded on the decisions this logic makes.
pub const MATCHER_VERSION: u32 = 1;

/// What a run of [`detect`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferReport {
    /// Pairs recorded as applied transfer decisions on both sides.
    pub applied: Vec<Pair>,
    /// Pairs recorded as suggestions on both sides, because the user had already seen
    /// one side's earlier decision: a bucket the user has looked at doesn't change
    /// under them.
    pub suggested: Vec<Pair>,
}

/// Why [`detect`] failed. Nothing was written.
#[derive(Debug)]
#[non_exhaustive]
pub enum DetectError {
    /// The ledger connection is already inside a transaction.
    TransactionOpen,
    Storage(StorageError),
}

impl fmt::Display for DetectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TransactionOpen => f.write_str("another change to the ledger is in progress"),
            Self::Storage(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DetectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(e) => Some(e),
            Self::TransactionOpen => None,
        }
    }
}

impl From<StorageError> for DetectError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

impl From<rusqlite::Error> for DetectError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e.into())
    }
}

/// Undoes everything since [`Savepoint::begin`] unless released.
pub(super) struct Savepoint<'c> {
    conn: &'c rusqlite::Connection,
    open: bool,
}

impl<'c> Savepoint<'c> {
    const NAME: &'static str = "transfer_detect";

    pub(super) fn begin(conn: &'c rusqlite::Connection) -> Result<Savepoint<'c>, DetectError> {
        if !conn.is_autocommit() {
            return Err(DetectError::TransactionOpen);
        }
        conn.execute_batch(&format!("SAVEPOINT {}", Self::NAME))?;
        Ok(Savepoint { conn, open: true })
    }

    fn release(mut self) -> Result<(), DetectError> {
        self.conn
            .execute_batch(&format!("RELEASE {}", Self::NAME))?;
        self.open = false;
        Ok(())
    }
}

impl Drop for Savepoint<'_> {
    fn drop(&mut self) {
        if self.open {
            // Nothing to report from here; the caller already has the original error.
            let _ = self
                .conn
                .execute_batch(&format!("ROLLBACK TO {0}; RELEASE {0}", Self::NAME));
        }
    }
}

/// Where a transaction stands before detection.
enum Standing<'d> {
    /// Open to pairing; the applied bucket decision it replaces, if any.
    Open(Option<&'d Decision>),
    /// Decided by a user or a rule, or already seen by this matcher: left alone.
    Closed,
}

fn is_bucket(d: &Decision) -> bool {
    matches!(d.assignment, Assignment::Bucket(_))
}

/// A transaction is open to pairing when its applied bucket came from the import
/// pipeline's Needs review or from a model, or it has none. A user's or a rule's
/// decision is never overridden, and a transaction this matcher has decided before, in
/// any state, isn't decided again: a dismissed pair stays dismissed.
fn standings(decisions: &[Decision]) -> HashMap<TransactionId, Standing<'_>> {
    let mut out: HashMap<TransactionId, Standing> = HashMap::new();
    for d in decisions.iter().filter(|d| is_bucket(d)) {
        let entry = out.entry(d.transaction_id).or_insert(Standing::Open(None));
        if matches!(entry, Standing::Closed) {
            continue;
        }
        let matcher = matches!(
            d.source,
            DecisionSource::Builtin {
                logic: BuiltinLogic::TransferMatcher,
                ..
            }
        );
        if matcher {
            *entry = Standing::Closed;
        } else if d.state == DecisionState::Applied {
            let replaceable = matches!(
                d.source,
                DecisionSource::Model(_)
                    | DecisionSource::Builtin {
                        logic: BuiltinLogic::ImportPipeline,
                        ..
                    }
            );
            *entry = if replaceable {
                Standing::Open(Some(d))
            } else {
                Standing::Closed
            };
        }
    }
    out
}

/// Finds transfers among the transactions nothing has decided yet (see [`standings`])
/// and records each pair as a transfer decision on both sides, pointing at each other,
/// with source `builtin/transfer_matcher` and `replaces` set to the decision it takes
/// over. The replaced decisions are superseded.
///
/// When the user has already seen either side's earlier decision, both sides get a
/// suggestion instead and the earlier decisions stay applied.
///
/// Runs in one savepoint: all of it is recorded or none. Running it again changes
/// nothing, since paired transactions are no longer open.
pub fn detect(ledger: &mut Unlocked, now: Timestamp) -> Result<TransferReport, DetectError> {
    let savepoint = Savepoint::begin(ledger.connection())?;
    let report = record_pairs(ledger, now)?;
    savepoint.release()?;
    Ok(report)
}

fn record_pairs(ledger: &Unlocked, now: Timestamp) -> Result<TransferReport, DetectError> {
    let accounts = ledger.accounts()?;
    let transactions = ledger.transactions()?;
    let decisions = ledger.decisions()?;
    let standings = standings(&decisions);
    let open = |id: TransactionId| !matches!(standings.get(&id), Some(Standing::Closed));
    let replaced = |id: TransactionId| match standings.get(&id) {
        Some(Standing::Open(Some(d))) => Some(d),
        _ => None,
    };

    let taxonomy_version = ledger.taxonomy()?.version;
    let mut next_id = ledger.next_id(Table::Decisions)?;
    let mut report = TransferReport::default();
    for pair in find_pairs(&accounts, &transactions, open) {
        let sides = [(pair.out, pair.inflow), (pair.inflow, pair.out)];
        let seen = sides
            .iter()
            .any(|&(id, _)| replaced(id).is_some_and(|d| d.seen_at.is_some()));
        let state = if seen {
            DecisionState::Suggested
        } else {
            DecisionState::Applied
        };
        for (id, counterpart) in sides {
            let old = replaced(id);
            if state == DecisionState::Applied
                && let Some(old) = old
            {
                ledger.set_decision_state(old.id, DecisionState::Superseded)?;
            }
            ledger.insert_decision(&Decision {
                id: DecisionId(next_id),
                transaction_id: id,
                assignment: Assignment::Bucket(Bucket::Transfer {
                    counterpart: Some(counterpart),
                }),
                source: DecisionSource::Builtin {
                    logic: BuiltinLogic::TransferMatcher,
                    version: MATCHER_VERSION,
                },
                taxonomy_version: taxonomy_version.clone(),
                decided_at: now,
                state,
                seen_at: None,
                replaces: old.map(|d| d.id),
            })?;
            next_id += 1;
        }
        if seen {
            report.suggested.push(pair);
        } else {
            report.applied.push(pair);
        }
    }
    Ok(report)
}
