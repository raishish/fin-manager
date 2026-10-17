//! A household's transactions as they are scripted, and how they become a ledger and
//! its truth.

use std::collections::{BTreeMap, BTreeSet};

use fm_core::model::{
    Account, AccountId, AccountKind, Date, ImportId, ImportSource, Ledger, Money, RawImport,
    SourceRef, Taxonomy, Timestamp, Transaction, TransactionId, TransactionStatus,
};
use fm_synth::describe::{AcctView, Style};
use fm_synth::truth::TruthBucket;

use crate::truth::{Candidate, HouseholdTruth, Pair, TxTruth};

/// Candidates are pairs whose money-in side posts this many days or fewer after the
/// money-out side (wider than any matcher's window, so near misses are listed too).
pub const CANDIDATE_WINDOW_DAYS: i32 = 7;

/// Where a transaction's money truly belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Category(&'static str),
    Refund(&'static str),
    Income,
    /// A transfer; `counterpart` filled in from the declared pairs.
    Transfer,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub account: usize,
    pub posted: Date,
    pub amount: i64,
    pub description: String,
    pub bucket: Bucket,
    /// The scenario this row was written for (`background` for everyday spending).
    pub pattern: &'static str,
    pub merchant: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AcctSpec {
    pub key: &'static str,
    pub kind: AccountKind,
    pub institution: &'static str,
    pub view: AcctView,
}

/// One household's script.
pub struct Book {
    pub key: &'static str,
    pub holder: &'static str,
    pub accounts: Vec<AcctSpec>,
    pub entries: Vec<Entry>,
    /// True transfers: (out, in, kind, pattern, wording).
    pub pairs: Vec<(usize, usize, &'static str, &'static str, &'static str)>,
    /// Rows written together for one look-alike scenario; a non-transfer candidate
    /// inside a group gets the group's pattern.
    pub groups: Vec<(&'static str, Vec<usize>)>,
}

impl Book {
    pub fn new(key: &'static str, holder: &'static str) -> Book {
        Book {
            key,
            holder,
            accounts: Vec::new(),
            entries: Vec::new(),
            pairs: Vec::new(),
            groups: Vec::new(),
        }
    }

    pub fn account(
        &mut self,
        key: &'static str,
        name: &str,
        kind: AccountKind,
        institution: &'static str,
        last4: &str,
        style: Style,
    ) -> usize {
        self.accounts.push(AcctSpec {
            key,
            kind,
            institution,
            view: AcctView {
                style,
                name: name.to_string(),
                last4: last4.to_string(),
                holder: self.holder.to_uppercase(),
            },
        });
        self.accounts.len() - 1
    }

    pub fn view(&self, a: usize) -> AcctView {
        self.accounts[a].view.clone()
    }

    pub fn is_card(&self, a: usize) -> bool {
        self.accounts[a].kind == AccountKind::CreditCard
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        account: usize,
        posted: Date,
        amount: i64,
        description: impl Into<String>,
        bucket: Bucket,
        pattern: &'static str,
        merchant: Option<&str>,
    ) -> usize {
        assert_ne!(amount, 0);
        self.entries.push(Entry {
            account,
            posted,
            amount,
            description: description.into(),
            bucket,
            pattern,
            merchant: merchant.map(str::to_string),
        });
        self.entries.len() - 1
    }

    pub fn pair(
        &mut self,
        out: usize,
        into: usize,
        kind: &'static str,
        pattern: &'static str,
        wording: &'static str,
    ) {
        let (o, i) = (&self.entries[out], &self.entries[into]);
        assert!(
            o.amount < 0 && i.amount == -o.amount && o.account != i.account,
            "{pattern}: not a pair"
        );
        assert!(
            o.bucket == Bucket::Transfer && i.bucket == Bucket::Transfer,
            "{pattern}: pair sides are transfers"
        );
        self.pairs.push((out, into, kind, pattern, wording));
    }

    pub fn group(&mut self, pattern: &'static str, rows: &[usize]) {
        self.groups.push((pattern, rows.to_vec()));
    }

    pub fn finish(
        self,
        description: &str,
        taxonomy: &Taxonomy,
        start: Date,
        end: Date,
    ) -> (Ledger, HouseholdTruth) {
        let usd = fm_synth::sim::usd();
        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        order.sort_by_key(|&i| (self.entries[i].posted, self.entries[i].account, i));
        let mut id = vec![0i64; self.entries.len()];
        for (pos, &i) in order.iter().enumerate() {
            id[i] = pos as i64 + 1;
        }
        let at = Timestamp::from_unix_seconds(
            i64::from(end.days_since_epoch() + 1) * 86_400 + 12 * 3600,
        );
        let mut ledger = Ledger {
            accounts: Vec::new(),
            imports: Vec::new(),
            transactions: Vec::new(),
            taxonomy: taxonomy.clone(),
            decisions: Vec::new(),
            rules: Vec::new(),
        };
        for (a, spec) in self.accounts.iter().enumerate() {
            let account_id = AccountId(a as i64 + 1);
            let len = if spec.kind == AccountKind::CreditCard {
                16
            } else {
                12
            };
            let last = &spec.view.last4;
            ledger.accounts.push(Account {
                id: account_id,
                name: spec.view.name.clone(),
                kind: spec.kind,
                currency: usd,
                institution: Some(spec.institution.to_string()),
                masked_number: Some(last.clone()),
                external_id: Some(format!("9999{}{last}", "0".repeat(len - 4 - last.len()))),
                own_account: true,
                created_at: at,
            });
            let bytes = format!(
                "fm-transfer-lookalikes household={} account={}\n",
                self.key, spec.key
            );
            let mut raw = RawImport::new(
                ImportId(a as i64 + 1),
                ImportSource::Manual,
                at,
                bytes.into_bytes(),
            );
            raw.file_name = Some(format!("transfer-lookalikes/{}/{}", self.key, spec.key));
            raw.account_id = Some(account_id);
            ledger.imports.push(raw);
        }
        let mut index = vec![0u32; self.accounts.len()];
        for &i in &order {
            let e = &self.entries[i];
            assert!(
                (start..=end).contains(&e.posted),
                "{}: entry outside the period",
                self.key
            );
            ledger.transactions.push(Transaction {
                id: TransactionId(id[i]),
                account_id: AccountId(e.account as i64 + 1),
                source: SourceRef {
                    import_id: ImportId(e.account as i64 + 1),
                    index: index[e.account],
                    external_id: None,
                },
                status: TransactionStatus::Posted,
                posted_date: e.posted,
                transaction_date: None,
                amount: Money::new(e.amount, usd),
                original_amount: None,
                description: e.description.clone(),
                payee: None,
                memo: None,
                trn_type: None,
                check_number: None,
            });
            index[e.account] += 1;
        }

        let counterpart: BTreeMap<usize, usize> = self
            .pairs
            .iter()
            .flat_map(|&(o, i, ..)| [(o, i), (i, o)])
            .collect();
        let keys: BTreeSet<&str> = taxonomy.categories.iter().map(|c| c.key.as_str()).collect();
        let check = |k: &'static str| {
            assert!(keys.contains(k), "unknown category key {k}");
            k.to_string()
        };
        let transactions = order
            .iter()
            .map(|&i| {
                let e = &self.entries[i];
                TxTruth {
                    id: id[i],
                    bucket: match e.bucket {
                        Bucket::Category(k) => TruthBucket::Category { key: check(k) },
                        Bucket::Refund(k) => TruthBucket::Refund {
                            key: check(k),
                            original: None,
                        },
                        Bucket::Income => TruthBucket::Income,
                        Bucket::Transfer => TruthBucket::Transfer {
                            counterpart: counterpart.get(&i).map(|&c| TransactionId(id[c])),
                        },
                    },
                    pattern: e.pattern.to_string(),
                    merchant: e.merchant.clone(),
                }
            })
            .collect();

        let lag = |o: usize, i: usize| self.entries[i].posted.days_since(self.entries[o].posted);
        let true_pairs: BTreeSet<(usize, usize)> =
            self.pairs.iter().map(|&(o, i, ..)| (o, i)).collect();
        let mut pairs: Vec<Pair> = self
            .pairs
            .iter()
            .map(|&(o, i, kind, pattern, wording)| Pair {
                out: id[o],
                into: id[i],
                kind: kind.to_string(),
                pattern: pattern.to_string(),
                wording: wording.to_string(),
                lag_days: lag(o, i),
            })
            .collect();
        pairs.sort_by_key(|p| (p.out, p.into));

        let group_of = |o: usize, i: usize| {
            self.groups
                .iter()
                .find(|(_, rows)| rows.contains(&o) && rows.contains(&i))
                .map(|(p, _)| *p)
        };
        let p2p = |e: &Entry| {
            let d = e.description.to_uppercase();
            ["ZELLE", "VENMO", "PAYPAL", "CASH APP"]
                .iter()
                .any(|w| d.contains(w))
        };
        let mut lookalikes = Vec::new();
        for o in 0..self.entries.len() {
            for i in 0..self.entries.len() {
                let (eo, ei) = (&self.entries[o], &self.entries[i]);
                if eo.amount >= 0 || ei.amount != -eo.amount || eo.account == ei.account {
                    continue;
                }
                let l = lag(o, i);
                if !(0..=CANDIDATE_WINDOW_DAYS).contains(&l) || true_pairs.contains(&(o, i)) {
                    continue;
                }
                let mut flags = Vec::new();
                if p2p(eo) || p2p(ei) {
                    flags.push("p2p".to_string());
                }
                if self.is_card(eo.account) {
                    flags.push("leaves_card".to_string());
                }
                lookalikes.push(Candidate {
                    out: id[o],
                    into: id[i],
                    pattern: group_of(o, i).unwrap_or("incidental").to_string(),
                    lag_days: l,
                    flags,
                });
            }
        }
        lookalikes.sort_by_key(|c| (c.out, c.into));
        for (pattern, rows) in &self.groups {
            assert!(
                lookalikes.iter().any(|c| c.pattern == *pattern
                    && rows.iter().any(|&r| id[r] == c.out || id[r] == c.into)),
                "{}: group {pattern} produced no look-alike",
                self.key
            );
        }

        let truth = HouseholdTruth {
            household: self.key.to_string(),
            description: description.to_string(),
            start,
            end,
            candidate_window_days: CANDIDATE_WINDOW_DAYS,
            transactions,
            transfer_pairs: pairs,
            lookalikes,
        };
        (ledger, truth)
    }
}
