use std::collections::HashMap;

use crate::model::{
    Account, AccountId, Assignment, Bucket, Category, CategoryId, Decision, DecisionId,
    DecisionState, Ledger, Rule, RuleId, Transaction, TransactionId,
};

/// A ledger with lookups the checks share, built once per run.
///
/// Lookups by id keep the *first* row with that id; duplicate ids are reported by the
/// checks themselves.
pub struct LedgerIndex<'a> {
    pub ledger: &'a Ledger,
    accounts: HashMap<AccountId, &'a Account>,
    transactions: HashMap<TransactionId, &'a Transaction>,
    decisions: HashMap<DecisionId, &'a Decision>,
    rules: HashMap<RuleId, &'a Rule>,
    categories: HashMap<CategoryId, &'a Category>,
    /// Applied decisions per transaction, in ledger order.
    applied: HashMap<TransactionId, Vec<&'a Decision>>,
}

fn first_by_id<K: std::hash::Hash + Eq, T>(rows: &[T], id: impl Fn(&T) -> K) -> HashMap<K, &T> {
    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        map.entry(id(row)).or_insert(row);
    }
    map
}

impl<'a> LedgerIndex<'a> {
    pub fn new(ledger: &'a Ledger) -> LedgerIndex<'a> {
        let mut applied: HashMap<TransactionId, Vec<&Decision>> = HashMap::new();
        for d in &ledger.decisions {
            if d.state == DecisionState::Applied {
                applied.entry(d.transaction_id).or_default().push(d);
            }
        }
        LedgerIndex {
            ledger,
            accounts: first_by_id(&ledger.accounts, |a| a.id),
            transactions: first_by_id(&ledger.transactions, |t| t.id),
            decisions: first_by_id(&ledger.decisions, |d| d.id),
            rules: first_by_id(&ledger.rules, |r| r.id),
            categories: first_by_id(&ledger.taxonomy.categories, |c| c.id),
            applied,
        }
    }

    pub fn account(&self, id: AccountId) -> Option<&'a Account> {
        self.accounts.get(&id).copied()
    }

    pub fn transaction(&self, id: TransactionId) -> Option<&'a Transaction> {
        self.transactions.get(&id).copied()
    }

    pub fn decision(&self, id: DecisionId) -> Option<&'a Decision> {
        self.decisions.get(&id).copied()
    }

    pub fn rule(&self, id: RuleId) -> Option<&'a Rule> {
        self.rules.get(&id).copied()
    }

    pub fn category(&self, id: CategoryId) -> Option<&'a Category> {
        self.categories.get(&id).copied()
    }

    /// The applied decisions about `transaction` (any assignment).
    pub fn applied(&self, transaction: TransactionId) -> &[&'a Decision] {
        self.applied.get(&transaction).map_or(&[], Vec::as_slice)
    }

    /// The applied bucket decisions about `transaction`. Exactly one is correct.
    pub fn applied_buckets(
        &self,
        transaction: TransactionId,
    ) -> impl Iterator<Item = (&'a Decision, &'a Bucket)> + '_ {
        self.applied(transaction)
            .iter()
            .filter_map(|d| match &d.assignment {
                Assignment::Bucket(b) => Some((*d, b)),
                Assignment::Merchant(_) => None,
            })
    }

    /// The transaction's bucket, if it has exactly one applied.
    pub fn bucket(&self, transaction: TransactionId) -> Option<&'a Bucket> {
        let mut buckets = self.applied_buckets(transaction);
        let (_, first) = buckets.next()?;
        buckets.next().is_none().then_some(first)
    }
}
