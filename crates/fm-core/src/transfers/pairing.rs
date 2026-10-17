//! Finding transfer pairs: money leaving one of the user's own accounts and the same
//! amount arriving in another a few days later.
//!
//! A pure function over accounts and transactions, so it can be tested and measured
//! without a database. A false pair hides real spending or income, so a pair needs
//! textual evidence; the amount and dates only make it a candidate.

use std::collections::{HashMap, HashSet};

use crate::model::{Account, AccountId, AccountKind, Transaction, TransactionId};

/// How far apart and how well evidenced a pair must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Params {
    /// The latest the receiving side may post after the sending side, in days.
    pub(crate) max_lag_days: i32,
    /// The least evidence a pair needs (see [`evidence`]).
    pub(crate) min_score: u32,
}

/// Five days covers ACH between banks over a long weekend (TD.1's lags are 0 to 3
/// days). Two points of evidence means both sides say "transfer", or one says it and
/// names the other account: one side's wording alone is never enough.
pub(crate) const PARAMS: Params = Params {
    max_lag_days: 5,
    min_score: 2,
};

/// What kind of transfer a pair is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairKind {
    /// Between two of the user's non-card accounts (checking, savings, ...).
    OwnTransfer,
    /// A payment from a bank account to a credit card.
    CardPayment,
}

/// One matched transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    /// The negative side (money leaving).
    pub out: TransactionId,
    /// The positive side (money arriving).
    pub inflow: TransactionId,
    pub kind: PairKind,
    /// The evidence score (see [`evidence`]).
    pub score: u32,
}

fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(' ');
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_uppercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// Words banks use for moving money between accounts and paying cards.
const TRANSFER_WORDS: &[&str] = &[
    " TRANSFER ",
    " XFER ",
    " TRANSFERS ",
    " PAYMENT THANK YOU ",
    " PAYMENT THANK ",
    " THANK YOU ",
    " AUTOPAY ",
    " AUTO PAY ",
    " AUTOMATIC PAYMENT ",
    " EPAY ",
    " ACH PMT ",
    " PMT ",
    " PYMT ",
    " CRD ",
    " CARD ENDING ",
    " ONLINE PAYMENT ",
    " MOBILE PAYMENT ",
];

/// Person-to-person payment services: money to a person, not to one's own account.
const P2P_WORDS: &[&str] = &[
    " ZELLE ",
    " VENMO ",
    " PAYPAL ",
    " CASH APP ",
    " SQC ",
    " APPLE CASH ",
];

fn has_any(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(w))
}

/// Whether `text` names `account`: its institution (or a word of it longer than three
/// letters), a distinctive word of its name, or its last four digits.
fn names_account(text: &str, account: &Account) -> bool {
    let words = |s: &str| -> Vec<String> {
        normalise(s)
            .split_whitespace()
            .filter(|w| w.len() >= 4 && !GENERIC_ACCOUNT_WORDS.contains(w))
            .map(|w| format!(" {w} "))
            .collect()
    };
    let institution = account
        .institution
        .as_deref()
        .map(|i| words(i).iter().any(|w| text.contains(w.as_str())) || text.contains(&normalise(i)))
        .unwrap_or(false);
    let name = words(&account.name)
        .iter()
        .any(|w| text.contains(w.as_str()));
    let last4 = account
        .masked_number
        .as_deref()
        .is_some_and(|m| m.len() >= 4 && text.contains(&format!(" {} ", &m[m.len() - 4..])));
    institution || name || last4
}

/// Words in account names that say nothing about which account it is.
const GENERIC_ACCOUNT_WORDS: &[&str] = &[
    "BANK",
    "CARD",
    "CHECKING",
    "SAVINGS",
    "ACCOUNT",
    "ONLINE",
    "CREDIT",
    "TOTAL",
    "EVERYDAY",
    "REGULAR",
    "SHARE",
    "DRAFT",
    "JOINT",
    "PREFERRED",
    "AMERICA",
    "FEDERAL",
    "UNION",
    "NATIONAL",
];

/// The source's transaction type, as a hint that adds to text evidence but is never
/// enough alone.
fn type_hint(t: &Transaction) -> bool {
    matches!(
        t.trn_type.as_deref(),
        Some("XFER" | "PAYMENT" | "DIRECTDEP" | "DIRECTDEBIT")
    )
}

/// How much the two rows' text says they are one transfer: a point for transfer wording
/// on each side, and a point for each side naming the other side's account. The source's
/// transaction type adds a point only when there is already text evidence.
pub(crate) fn evidence(
    out: &Transaction,
    out_account: &Account,
    inflow: &Transaction,
    in_account: &Account,
) -> u32 {
    let text = |t: &Transaction| {
        normalise(&format!(
            "{} {} {}",
            t.description,
            t.payee.as_deref().unwrap_or(""),
            t.memo.as_deref().unwrap_or("")
        ))
    };
    let (o, i) = (text(out), text(inflow));
    let mut score = u32::from(has_any(&o, TRANSFER_WORDS))
        + u32::from(has_any(&i, TRANSFER_WORDS))
        + u32::from(names_account(&o, in_account))
        + u32::from(names_account(&i, out_account));
    if score > 0 && (type_hint(out) || type_hint(inflow)) {
        score += 1;
    }
    score
}

fn is_card(kind: AccountKind) -> bool {
    kind == AccountKind::CreditCard
}

fn is_p2p(t: &Transaction) -> bool {
    has_any(&normalise(&t.description), P2P_WORDS)
}

/// A possible pair: opposite amounts on two own accounts within the window, with its
/// evidence score, before the score threshold and the one-use rule are applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) out: TransactionId,
    pub(crate) inflow: TransactionId,
    pub(crate) kind: PairKind,
    pub(crate) score: u32,
    /// Days from the sending side's posted date to the receiving side's.
    pub(crate) lag: i32,
}

/// Every candidate within `max_lag_days`, whatever its score (see [`find_pairs`]).
pub(crate) fn candidates(
    accounts: &[Account],
    transactions: &[Transaction],
    eligible: impl Fn(TransactionId) -> bool,
    max_lag_days: i32,
) -> Vec<Candidate> {
    let by_id: HashMap<AccountId, &Account> = accounts.iter().map(|a| (a.id, a)).collect();
    let own = |t: &Transaction| by_id.get(&t.account_id).filter(|a| a.own_account).copied();
    let usable = |t: &&Transaction| eligible(t.id) && own(t).is_some() && !is_p2p(t);

    // Positive rows by (currency, amount), for looking up each negative row's matches.
    let mut inflows: HashMap<(&'static str, i64), Vec<&Transaction>> = HashMap::new();
    for t in transactions
        .iter()
        .filter(usable)
        .filter(|t| t.amount.is_positive())
    {
        inflows
            .entry((t.amount.currency().code(), t.amount.minor()))
            .or_default()
            .push(t);
    }

    let mut out_candidates = Vec::new();
    for out in transactions
        .iter()
        .filter(usable)
        .filter(|t| t.amount.is_negative())
    {
        let Some(amount) = out.amount.minor().checked_neg() else {
            continue;
        };
        let Some(matches) = inflows.get(&(out.amount.currency().code(), amount)) else {
            continue;
        };
        let out_account = own(out).expect("usable rows are on own accounts");
        for inflow in matches {
            let in_account = own(inflow).expect("usable rows are on own accounts");
            let lag = inflow.posted_date.days_since(out.posted_date);
            if inflow.account_id == out.account_id || !(0..=max_lag_days).contains(&lag) {
                continue;
            }
            let kind = match (is_card(out_account.kind), is_card(in_account.kind)) {
                (false, true) => PairKind::CardPayment,
                (false, false) => PairKind::OwnTransfer,
                // Money leaving a card is a purchase or a cash advance, not a transfer
                // the matcher can tell apart.
                (true, _) => continue,
            };
            out_candidates.push(Candidate {
                out: out.id,
                inflow: inflow.id,
                kind,
                score: evidence(out, out_account, inflow, in_account),
                lag,
            });
        }
    }
    out_candidates
}

/// Finds transfer pairs among `transactions`, on `accounts`. Only transactions for which
/// `eligible` is true take part (the caller leaves out ones a user, a rule or a model
/// has decided, and ones already paired).
///
/// A candidate is a negative and a positive transaction of exactly opposite amount in
/// one currency, on two different accounts the user has marked as their own, the
/// positive one posted 0 to [`PARAMS`]`.max_lag_days` days later, and not a
/// person-to-person payment. Card payments go from a non-card account to a card;
/// anything else into a card isn't a transfer. Candidates with at least
/// [`PARAMS`]`.min_score` evidence are assigned best first: highest score, then the
/// nearest dates, then the lowest ids. Each transaction is used at most once.
pub fn find_pairs(
    accounts: &[Account],
    transactions: &[Transaction],
    eligible: impl Fn(TransactionId) -> bool,
) -> Vec<Pair> {
    find_pairs_with(PARAMS, accounts, transactions, eligible)
}

/// [`find_pairs`] with other parameters, for measuring how sensitive it is to them.
pub(crate) fn find_pairs_with(
    params: Params,
    accounts: &[Account],
    transactions: &[Transaction],
    eligible: impl Fn(TransactionId) -> bool,
) -> Vec<Pair> {
    let mut ranked: Vec<Candidate> =
        candidates(accounts, transactions, eligible, params.max_lag_days)
            .into_iter()
            .filter(|c| c.score >= params.min_score)
            .collect();
    ranked.sort_by_key(|c| (std::cmp::Reverse(c.score), c.lag, c.out, c.inflow));
    let mut used: HashSet<TransactionId> = HashSet::new();
    let mut pairs = Vec::new();
    for c in ranked {
        if !used.contains(&c.out) && !used.contains(&c.inflow) {
            used.insert(c.out);
            used.insert(c.inflow);
            pairs.push(Pair {
                out: c.out,
                inflow: c.inflow,
                kind: c.kind,
                score: c.score,
            });
        }
    }
    pairs.sort_by_key(|p| (p.out, p.inflow));
    pairs
}
