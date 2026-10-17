//! Constraint 4: everything adds up. Every transaction is in exactly one bucket (a
//! category, a transfer, income, a refund, or needs review). Category totals plus
//! transfers (plus what still needs review) equal total money out. Matched transfers
//! net to zero.

use std::collections::{BTreeMap, HashSet};

use super::{Check, Constraint, LedgerIndex, Subject, Violation};
use crate::model::{Assignment, Bucket, Transaction};

pub struct EverythingAddsUp;

fn violation(subject: Subject, code: &'static str, message: &'static str) -> Violation {
    Violation {
        constraint: Constraint::EverythingAddsUp,
        subject,
        code,
        message,
    }
}

impl EverythingAddsUp {
    fn check_transfer(
        ix: &LedgerIndex<'_>,
        t: &Transaction,
        counterpart: Option<crate::model::TransactionId>,
        out: &mut Vec<Violation>,
    ) {
        let subject = Subject::Transaction(t.id);
        if ix.account(t.account_id).is_some_and(|a| !a.own_account) {
            out.push(violation(
                subject,
                "transfer_from_foreign_account",
                "a transfer is on an account the user hasn't designated as their own",
            ));
        }
        let Some(c) = counterpart else {
            return; // To or from an own account outside the ledger.
        };
        if c == t.id {
            out.push(violation(
                subject,
                "self_counterpart",
                "a transfer names itself as its counterpart",
            ));
            return;
        }
        let Some(other) = ix.transaction(c) else {
            out.push(violation(
                subject,
                "unknown_counterpart",
                "a transfer's counterpart is not in the ledger",
            ));
            return;
        };
        if other.account_id == t.account_id {
            out.push(violation(
                subject,
                "same_account_transfer",
                "a transfer and its counterpart are on the same account",
            ));
        }
        if !matches!(ix.bucket(c), Some(Bucket::Transfer { counterpart: Some(back) }) if *back == t.id)
        {
            out.push(violation(
                subject,
                "counterpart_not_reciprocal",
                "the counterpart isn't a transfer pointing back at this transaction",
            ));
        }
        let nets_to_zero = t.amount.currency() == other.amount.currency()
            && t.amount
                .checked_add(other.amount)
                .is_ok_and(|sum| sum.is_zero());
        if !nets_to_zero {
            out.push(violation(
                subject,
                "transfer_not_net_zero",
                "a matched transfer and its counterpart don't add up to zero",
            ));
        }
    }
}

impl Check for EverythingAddsUp {
    fn constraint(&self) -> Constraint {
        Constraint::EverythingAddsUp
    }

    fn check(&self, ix: &LedgerIndex<'_>, out: &mut Vec<Violation>) {
        let ledger = ix.ledger;
        let mut seen = HashSet::new();
        // Per currency: all money out, money out that sits in a bucket that may hold it,
        // and the sum of matched transfers. i128, so these sums can't overflow.
        let mut money_out: BTreeMap<&'static str, i128> = BTreeMap::new();
        let mut bucketed_out: BTreeMap<&'static str, i128> = BTreeMap::new();
        let mut matched_transfers: BTreeMap<&'static str, i128> = BTreeMap::new();

        for t in &ledger.transactions {
            let subject = Subject::Transaction(t.id);
            if !seen.insert(t.id) {
                out.push(violation(
                    subject,
                    "duplicate_transaction_id",
                    "two transactions share this id, so one would be counted twice",
                ));
                continue;
            }
            let currency = t.amount.currency().code();
            let minor = i128::from(t.amount.minor());
            if t.amount.is_negative() {
                *money_out.entry(currency).or_default() += minor;
            }

            let buckets: Vec<_> = ix.applied_buckets(t.id).collect();
            let merchants = ix
                .applied(t.id)
                .iter()
                .filter(|d| matches!(d.assignment, Assignment::Merchant(_)))
                .count();
            if merchants > 1 {
                out.push(violation(
                    subject,
                    "several_merchants",
                    "more than one merchant name is applied to the transaction",
                ));
            }
            let bucket = match buckets.as_slice() {
                [] => {
                    out.push(violation(
                        subject,
                        "no_bucket",
                        "the transaction is in no bucket (not even needs review)",
                    ));
                    continue;
                }
                [(_, bucket)] => *bucket,
                _ => {
                    out.push(violation(
                        subject,
                        "several_buckets",
                        "the transaction is in more than one bucket",
                    ));
                    continue;
                }
            };

            match *bucket {
                Bucket::Category { category_id } => {
                    if ix.category(category_id).is_none() {
                        out.push(violation(
                            subject,
                            "unknown_category",
                            "the transaction's category is not in the category list",
                        ));
                    }
                }
                Bucket::Refund {
                    category_id,
                    original,
                } => {
                    if ix.category(category_id).is_none() {
                        out.push(violation(
                            subject,
                            "unknown_category",
                            "the refund's category is not in the category list",
                        ));
                    }
                    if let Some(o) = original
                        && (o == t.id || ix.transaction(o).is_none())
                    {
                        out.push(violation(
                            subject,
                            "unknown_refund_original",
                            "the purchase a refund reverses is not in the ledger",
                        ));
                    }
                }
                Bucket::Transfer { counterpart } => {
                    Self::check_transfer(ix, t, counterpart, out);
                    if counterpart.is_some() {
                        *matched_transfers.entry(currency).or_default() += minor;
                    }
                }
                Bucket::Income | Bucket::NeedsReview => {}
            }

            if t.amount.is_negative() {
                match bucket {
                    Bucket::Category { .. } | Bucket::Transfer { .. } | Bucket::NeedsReview => {
                        *bucketed_out.entry(currency).or_default() += minor;
                    }
                    Bucket::Income | Bucket::Refund { .. } => out.push(violation(
                        subject,
                        "outflow_not_spending_or_transfer",
                        "money out is filed as income or a refund, not as spending, a transfer or needs review",
                    )),
                }
            }
        }

        if money_out != bucketed_out {
            out.push(violation(
                Subject::Ledger,
                "outflows_dont_add_up",
                "category totals plus transfers plus needs review don't equal total money out",
            ));
        }
        if matched_transfers.values().any(|&sum| sum != 0) {
            out.push(violation(
                Subject::Ledger,
                "matched_transfers_not_net_zero",
                "matched transfers don't net to zero",
            ));
        }
    }
}
