//! Constraint 1: numbers are exact. Every figure comes from fixed logic over the stored
//! transactions, and the same ledger always gives the same totals.
//!
//! Amounts are integers by type ([`Money`]), so what can still go wrong is in the data:
//! an amount in a currency other than its account's (adding it would mix currencies),
//! a malformed foreign amount, or totals that overflow `i64` minor units. The check also
//! sums every account twice, in opposite orders, and requires the same result.

use std::collections::{BTreeMap, HashSet};

use super::{Check, Constraint, LedgerIndex, Subject, Violation};
use crate::model::{AccountId, Currency, Money, Transaction};

pub struct ExactNumbers;

fn violation(subject: Subject, code: &'static str, message: &'static str) -> Violation {
    Violation {
        constraint: Constraint::ExactNumbers,
        subject,
        code,
        message,
    }
}

/// Money in and money out per account, summed exactly. `None` once a sum overflows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Flow {
    inflow: Option<Money>,
    outflow: Option<Money>,
}

fn add(sum: Option<Money>, amount: Money) -> Option<Money> {
    sum?.checked_add(amount).ok()
}

fn account_flows<'a>(
    transactions: impl Iterator<Item = &'a Transaction>,
    currency_of: impl Fn(AccountId) -> Option<Currency>,
) -> BTreeMap<AccountId, Flow> {
    let mut flows: BTreeMap<AccountId, Flow> = BTreeMap::new();
    for t in transactions {
        // Amounts in the wrong currency are reported separately and left out here.
        let Some(currency) = currency_of(t.account_id) else {
            continue;
        };
        if t.amount.currency() != currency {
            continue;
        }
        let flow = flows.entry(t.account_id).or_insert(Flow {
            inflow: Some(Money::zero(currency)),
            outflow: Some(Money::zero(currency)),
        });
        if t.amount.is_negative() {
            flow.outflow = add(flow.outflow, t.amount);
        } else {
            flow.inflow = add(flow.inflow, t.amount);
        }
    }
    flows
}

impl Check for ExactNumbers {
    fn constraint(&self) -> Constraint {
        Constraint::ExactNumbers
    }

    fn check(&self, ix: &LedgerIndex<'_>, out: &mut Vec<Violation>) {
        let ledger = ix.ledger;

        let mut seen = HashSet::new();
        for a in &ledger.accounts {
            if !seen.insert(a.id) {
                out.push(violation(
                    Subject::Account(a.id),
                    "duplicate_account_id",
                    "two accounts share this id",
                ));
            }
        }

        for t in &ledger.transactions {
            let subject = Subject::Transaction(t.id);
            match ix.account(t.account_id) {
                None => out.push(violation(
                    subject,
                    "unknown_account",
                    "the transaction's account is not in the ledger",
                )),
                Some(a) if t.amount.currency() != a.currency => out.push(violation(
                    subject,
                    "currency_mismatch",
                    "the amount is not in its account's currency",
                )),
                Some(_) => {}
            }
            if let Some(original) = t.original_amount {
                if original.currency() == t.amount.currency() {
                    out.push(violation(
                        subject,
                        "original_same_currency",
                        "a foreign amount is in the account's own currency",
                    ));
                }
                if (original.is_negative() && t.amount.is_positive())
                    || (original.is_positive() && t.amount.is_negative())
                {
                    out.push(violation(
                        subject,
                        "original_sign_mismatch",
                        "the foreign amount and the amount have opposite signs",
                    ));
                }
            }
        }

        let currency_of = |id: AccountId| ix.account(id).map(|a| a.currency);
        let forward = account_flows(ledger.transactions.iter(), currency_of);
        let backward = account_flows(ledger.transactions.iter().rev(), currency_of);
        for (account, flow) in &forward {
            if flow.inflow.is_none() || flow.outflow.is_none() {
                out.push(violation(
                    Subject::Account(*account),
                    "total_overflow",
                    "the account's total doesn't fit in 64-bit minor units",
                ));
            }
        }
        if forward != backward {
            out.push(violation(
                Subject::Ledger,
                "totals_not_reproducible",
                "summing the same transactions in another order gave different totals",
            ));
        }

        // The ledger-wide total per currency must fit too: views add accounts together.
        let mut by_currency: BTreeMap<&'static str, (Option<Money>, Option<Money>)> =
            BTreeMap::new();
        for flow in forward.values() {
            let (Some(inflow), Some(outflow)) = (flow.inflow, flow.outflow) else {
                continue;
            };
            let entry = by_currency.entry(inflow.currency().code()).or_insert((
                Some(Money::zero(inflow.currency())),
                Some(Money::zero(outflow.currency())),
            ));
            entry.0 = add(entry.0, inflow);
            entry.1 = add(entry.1, outflow);
        }
        if by_currency
            .values()
            .any(|(i, o)| i.is_none() || o.is_none())
        {
            out.push(violation(
                Subject::Ledger,
                "total_overflow",
                "the ledger's total in one currency doesn't fit in 64-bit minor units",
            ));
        }
    }
}
