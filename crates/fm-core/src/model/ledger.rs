use serde::{Deserialize, Serialize};

use super::{Account, Decision, RawImport, Rule, Taxonomy, Transaction};

/// Everything in one user's ledger, in memory. Used by test fixtures, the synthetic
/// data generator's ground truth and the contract checker; storage keeps the same data
/// in tables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub accounts: Vec<Account>,
    pub imports: Vec<RawImport>,
    pub transactions: Vec<Transaction>,
    pub taxonomy: Taxonomy,
    /// Every decision, including superseded and dismissed ones.
    pub decisions: Vec<Decision>,
    pub rules: Vec<Rule>,
}
