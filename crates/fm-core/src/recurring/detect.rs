//! Finding series (see README.md for the reasoning behind each rule).

use std::collections::{BTreeMap, HashMap, HashSet};

use super::key::merchant_keys;
use super::{Cadence, Direction, Series, SeriesAmount, SeriesKind};
use crate::model::{
    Account, AccountId, AccountKind, Assignment, Bucket, Date, Decision, DecisionState, Money,
    Transaction, TransactionId,
};
use crate::storage::{StorageError, Unlocked};

/// The tolerances, gathered so the tests can measure how much each one matters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Params {
    /// A silence longer than this ends a series (a later restart is a new series).
    /// Shorter pauses (a summer, a winter) keep it going once it is established.
    pub(crate) split_days: i32,
    /// The most a variable-amount series' largest amount may be of its smallest.
    pub(crate) variable_band: f64,
    /// The most amounts may differ in a semiannual or annual series (a price change
    /// between occurrences).
    pub(crate) step_band: f64,
    /// The share of a series' gaps that must be one regular period (the rest are pauses).
    pub(crate) regular_share: f64,
    /// The most a price step may change the amount (as a ratio).
    pub(crate) step_ratio: f64,
    /// The least share of a merchant's rows (of the same amount, for an exact series)
    /// over a series' span that the series must hold. A shop visited every few days
    /// has a visit near every monthly date; the visits around it are what show it is
    /// a habit, not a series.
    pub(crate) dominance: f64,
}

pub(crate) const PARAMS: Params = Params {
    split_days: 200,
    variable_band: 4.0,
    step_band: 1.10,
    regular_share: 0.75,
    step_ratio: 1.5,
    dominance: 0.75,
};

/// One cadence the search tries, shortest first: rows taken by a shorter cadence
/// aren't offered to a longer one, so a biweekly series isn't read as two monthly ones.
struct Pass {
    /// The gaps, in days, that count as one period.
    band: (i32, i32),
    nominal: i32,
    /// Biweekly and semimonthly share a pass; the days of the month tell them apart.
    cadence: Cadence,
    /// The fewest members of an exact-amount series; `None`: no exact pass.
    min_exact: Option<usize>,
    /// The fewest members of a variable-amount series; `None`: exact amounts only.
    min_variable: Option<usize>,
}

const PASSES: [Pass; 6] = [
    Pass {
        band: (5, 9),
        nominal: 7,
        cadence: Cadence::Weekly,
        min_exact: Some(4),
        min_variable: None,
    },
    Pass {
        band: (11, 18),
        nominal: 14,
        cadence: Cadence::Biweekly,
        min_exact: Some(4),
        min_variable: None,
    },
    Pass {
        band: (25, 35),
        nominal: 30,
        cadence: Cadence::Monthly,
        min_exact: Some(3),
        min_variable: Some(6),
    },
    Pass {
        band: (80, 100),
        nominal: 91,
        cadence: Cadence::Quarterly,
        min_exact: Some(3),
        min_variable: Some(4),
    },
    Pass {
        band: (140, 225),
        nominal: 182,
        cadence: Cadence::Semiannual,
        min_exact: Some(3),
        min_variable: Some(3),
    },
    Pass {
        band: (350, 380),
        nominal: 365,
        cadence: Cadence::Annual,
        min_exact: Some(2),
        min_variable: Some(2),
    },
];

/// A transaction as the search sees it.
struct Row<'a> {
    id: TransactionId,
    account: AccountId,
    direction: Direction,
    /// Purchase date when the bank gives one, otherwise posted (SPEC decision 42).
    date: Date,
    amount: Money,
    /// The merchant key it is grouped by, and a finer one that tells a merchant's
    /// subscription from its shop (see [`merchant_keys`]).
    key: String,
    fine_key: String,
    description: &'a str,
    /// Money out to one of the user's own accounts (an applied transfer decision).
    own_transfer: bool,
}

/// A found series, as indexes into the rows, before it is described.
struct Chain {
    rows: Vec<usize>,
    pass: usize,
    variable: bool,
}

/// The rows that can take part: everything except card payments (both sides), the
/// receiving side of a transfer between own accounts, and zero amounts. Those are one
/// movement of money already counted on its sending side, or not money at all.
fn rows<'a>(
    accounts: &[Account],
    transactions: &'a [Transaction],
    decisions: &[Decision],
) -> Vec<Row<'a>> {
    let cards: HashSet<AccountId> = accounts
        .iter()
        .filter(|a| a.kind == AccountKind::CreditCard)
        .map(|a| a.id)
        .collect();
    let account_of: HashMap<TransactionId, AccountId> =
        transactions.iter().map(|t| (t.id, t.account_id)).collect();
    let applied: HashMap<TransactionId, &Bucket> = decisions
        .iter()
        .filter(|d| d.state == DecisionState::Applied)
        .filter_map(|d| match &d.assignment {
            Assignment::Bucket(b) => Some((d.transaction_id, b)),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for t in transactions {
        if t.amount.minor() == 0 {
            continue;
        }
        let transfer = match applied.get(&t.id) {
            Some(Bucket::Transfer { counterpart }) => Some(*counterpart),
            _ => None,
        };
        if let Some(Some(counterpart)) = transfer {
            let card_payment = cards.contains(&t.account_id)
                || account_of
                    .get(&counterpart)
                    .is_some_and(|a| cards.contains(a));
            if card_payment || t.amount.is_positive() {
                continue;
            }
        }
        let (key, fine_key) = merchant_keys(&t.description);
        out.push(Row {
            id: t.id,
            account: t.account_id,
            direction: if t.amount.is_negative() {
                Direction::Out
            } else {
                Direction::In
            },
            date: t.transaction_date.unwrap_or(t.posted_date),
            amount: t.amount,
            key,
            fine_key,
            description: &t.description,
            own_transfer: transfer.is_some() && t.amount.is_negative(),
        });
    }
    out.sort_by_key(|r| (r.date, r.id));
    out
}

fn ratio(a: Money, b: Money) -> f64 {
    let (a, b) = (
        a.minor().unsigned_abs() as f64,
        b.minor().unsigned_abs() as f64,
    );
    a.max(b) / a.min(b).max(1.0)
}

/// How a row would follow a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Fit {
    /// One period after the last member.
    Regular,
    /// Early or late, but two periods after the member before last: a check that
    /// cleared late followed by one that cleared early.
    Jitter,
    /// After a pause no longer than `split_days`, once the chain is established.
    Pause,
}

fn fit(
    rows: &[Row<'_>],
    chain: &[usize],
    date: Date,
    pass: &Pass,
    min: usize,
    params: Params,
) -> Option<Fit> {
    let (lo, hi) = pass.band;
    let last = rows[*chain.last().expect("chains are never empty")].date;
    let gap = date.days_since(last);
    if (lo..=hi).contains(&gap) {
        return Some(Fit::Regular);
    }
    if gap >= lo / 2 && chain.len() >= 2 {
        let before = rows[chain[chain.len() - 2]].date;
        if (2 * lo..=2 * hi).contains(&date.days_since(before)) {
            return Some(Fit::Jitter);
        }
    }
    if gap > hi && gap <= params.split_days && chain.len() >= min {
        return Some(Fit::Pause);
    }
    None
}

/// How a row's amount must relate to a chain's.
#[derive(Debug, Clone, Copy)]
enum AmountRule {
    /// Exact-amount chains: every row already has the same amount.
    Same,
    /// Within this ratio of the chain's last amount: utilities drift with the seasons,
    /// a one-off spike doesn't.
    NearLast(f64),
    /// Within this ratio of every amount in the chain: long cadences, where a series
    /// changes price at most a little between occurrences.
    NearAll(f64),
}

/// Builds chains from `members` (in date order) one period apart: each row joins the
/// chain it follows best (see [`Fit`]; then the same merchant key; then the amount
/// nearest the chain's; then the gap nearest the usual one), or starts a new one.
fn build(
    rows: &[Row<'_>],
    members: &[usize],
    pass: &Pass,
    min: usize,
    params: Params,
    amounts: AmountRule,
) -> Vec<Vec<usize>> {
    let mut chains: Vec<Vec<usize>> = Vec::new();
    for &r in members {
        let row = &rows[r];
        let mut best: Option<((Fit, bool, u64, i32), usize)> = None;
        for (ci, chain) in chains.iter().enumerate() {
            let Some(how) = fit(rows, chain, row.date, pass, min, params) else {
                continue;
            };
            let last = &rows[*chain.last().expect("chains are never empty")];
            let fits = match amounts {
                AmountRule::Same => true,
                AmountRule::NearLast(band) => ratio(last.amount, row.amount) <= band,
                AmountRule::NearAll(band) => chain
                    .iter()
                    .all(|&m| ratio(rows[m].amount, row.amount) <= band),
            };
            if !fits {
                continue;
            }
            let other_key = rows[chain[0]].key != row.key;
            // In thousandths, so the score stays an integer.
            let amount_distance = (ratio(last.amount, row.amount) * 1000.0) as u64;
            let gap = row.date.days_since(last.date);
            let score = (how, other_key, amount_distance, (gap - pass.nominal).abs());
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, ci));
            }
        }
        match best {
            Some((_, ci)) => chains[ci].push(r),
            None => chains.push(vec![r]),
        }
    }
    chains
}

/// Whether a chain's members fall on about the same day of the month (within a week,
/// counting the last days of a month as just before the 1st). Bills due every few
/// months are scheduled on a day of the month, moved a few days at most by weekends and
/// holidays; the same meal bought every three months or so isn't.
fn same_day_of_month(rows: &[Row<'_>], chain: &[usize]) -> bool {
    let days: Vec<i32> = chain
        .iter()
        .map(|&i| i32::from(rows[i].date.day()))
        .collect();
    // Try both readings of the month's end, and keep the tighter one.
    let spread = |shift: bool| {
        let v: Vec<i32> = days
            .iter()
            .map(|&d| if shift && d >= 20 { d - 31 } else { d })
            .collect();
        v.iter().max().unwrap_or(&0) - v.iter().min().unwrap_or(&0)
    };
    spread(false).min(spread(true)) <= 6
}

/// Whether enough of a chain's steps are regular (or jitter) rather than pauses.
fn is_regular(rows: &[Row<'_>], chain: &[usize], pass: &Pass, params: Params) -> bool {
    if chain.len() < 2 {
        return false;
    }
    let regular = (1..chain.len())
        .filter(|&i| {
            matches!(
                fit(
                    rows,
                    &chain[..i],
                    rows[chain[i]].date,
                    pass,
                    usize::MAX,
                    params
                ),
                Some(Fit::Regular | Fit::Jitter)
            )
        })
        .count();
    let long = matches!(
        pass.cadence,
        Cadence::Quarterly | Cadence::Semiannual | Cadence::Annual
    );
    regular as f64 / (chain.len() - 1) as f64 >= params.regular_share
        && (!long || same_day_of_month(rows, chain))
}

/// The first and last dates of a chain.
fn span(rows: &[Row<'_>], chain: &[usize]) -> (Date, Date) {
    let dates = chain.iter().map(|&i| rows[i].date);
    (
        dates.clone().min().expect("chains are never empty"),
        dates.max().expect("chains are never empty"),
    )
}

/// Keeps the candidate chains that explain their merchant's activity.
///
/// Candidates on one account and in one direction that share a fine merchant key and
/// overlap in time are judged together: a merchant can have a few series side by side
/// (two Apple subscriptions, checks for church and for piano lessons). They are kept
/// when, between them, they hold at least `dominance` of the merchant's rows over their
/// span (any amount; rows `explained` by series already kept don't count), no amount
/// has more than two of them side by side, and at most one varies in amount. A shop visited every few days fails
/// both ways: its visits make many parallel chains, and leave most rows unexplained.
fn accept(
    rows: &[Row<'_>],
    candidates: Vec<Chain>,
    explained: &HashSet<usize>,
    params: Params,
) -> Vec<Chain> {
    let n = candidates.len();
    let spans: Vec<(Date, Date)> = candidates.iter().map(|c| span(rows, &c.rows)).collect();
    let fine: Vec<HashSet<&str>> = candidates
        .iter()
        .map(|c| c.rows.iter().map(|&i| rows[i].fine_key.as_str()).collect())
        .collect();
    // Union-find over candidates that are judged together.
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for a in 0..n {
        for b in a + 1..n {
            let (ra, rb) = (&rows[candidates[a].rows[0]], &rows[candidates[b].rows[0]]);
            if ra.account == rb.account
                && ra.direction == rb.direction
                && !fine[a].is_disjoint(&fine[b])
                && spans[a].0 <= spans[b].1
                && spans[b].0 <= spans[a].1
            {
                let (x, y) = (root(&mut parent, a), root(&mut parent, b));
                parent[x] = y;
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut keep = vec![false; n];
    for members in groups.values() {
        let sample = &rows[candidates[members[0]].rows[0]];
        let first = members
            .iter()
            .map(|&i| spans[i].0)
            .min()
            .expect("non-empty");
        let last = members
            .iter()
            .map(|&i| spans[i].1)
            .max()
            .expect("non-empty");
        let keys: HashSet<&str> = members
            .iter()
            .flat_map(|&i| fine[i].iter().copied())
            .collect();
        let held: usize = members.iter().map(|&i| candidates[i].rows.len()).sum();
        let pool = rows
            .iter()
            .enumerate()
            .filter(|(i, r)| {
                !explained.contains(i)
                    && r.account == sample.account
                    && r.direction == sample.direction
                    && keys.contains(r.fine_key.as_str())
                    && r.date >= first
                    && r.date <= last
            })
            .count();
        let mut per_amount: HashMap<i64, usize> = HashMap::new();
        for &i in members {
            if !candidates[i].variable {
                *per_amount
                    .entry(rows[candidates[i].rows[0]].amount.minor())
                    .or_default() += 1;
            }
        }
        let variable = members.iter().filter(|&&i| candidates[i].variable).count();
        let crowded = per_amount.values().any(|&k| k > 2) || variable > 1;
        if !crowded && held as f64 >= params.dominance * pool as f64 {
            for &i in members {
                keep[i] = true;
            }
        }
    }
    candidates
        .into_iter()
        .zip(keep)
        .filter_map(|(c, k)| k.then_some(c))
        .collect()
}

/// Drops the members whose merchant key occurs only once in a chain of several keys.
/// Two names that alternate (HBO MAX and MAX.COM) both recur; a different merchant
/// that happened to charge the same amount on the right day doesn't.
fn drop_stray_keys(rows: &[Row<'_>], chain: &mut Vec<usize>) {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for &i in chain.iter() {
        *counts.entry(rows[i].key.as_str()).or_default() += 1;
    }
    if counts.len() > 1 {
        chain.retain(|&i| counts[rows[i].key.as_str()] > 1);
    }
}

fn exact_chains(rows: &[Row<'_>], params: Params) -> Vec<Chain> {
    let mut clusters: BTreeMap<(AccountId, Direction, &'static str, i64), Vec<usize>> =
        BTreeMap::new();
    for (i, r) in rows.iter().enumerate() {
        clusters
            .entry((
                r.account,
                r.direction,
                r.amount.currency().code(),
                r.amount.minor(),
            ))
            .or_default()
            .push(i);
    }
    let mut out = Vec::new();
    for members in clusters.values() {
        let mut free: Vec<usize> = members.clone();
        for (p, pass) in PASSES.iter().enumerate() {
            let Some(min) = pass.min_exact else {
                continue;
            };
            let mut taken = HashSet::new();
            for mut chain in build(rows, &free, pass, min, params, AmountRule::Same) {
                drop_stray_keys(rows, &mut chain);
                if chain.len() >= min && is_regular(rows, &chain, pass, params) {
                    taken.extend(chain.iter().copied());
                    out.push(Chain {
                        rows: chain,
                        pass: p,
                        variable: false,
                    });
                }
            }
            free.retain(|i| !taken.contains(i));
        }
    }
    out
}

fn keys(rows: &[Row<'_>], chain: &Chain) -> HashSet<String> {
    chain.rows.iter().map(|&i| rows[i].key.clone()).collect()
}

/// Joins exact-amount chains that continue each other: the same merchant key and
/// cadence, the second starting about one period after the first ends. On one account
/// that is a price change; with the same amount on another account, a change of card.
fn merge_steps(rows: &[Row<'_>], mut chains: Vec<Chain>, params: Params) -> Vec<Chain> {
    loop {
        chains.sort_by_key(|c| (rows[c.rows[0]].date, rows[c.rows[0]].id));
        let mut join = None;
        'search: for (a, first) in chains.iter().enumerate() {
            let pass = &PASSES[first.pass];
            let end = &rows[*first.rows.last().expect("chains are never empty")];
            let first_keys = keys(rows, first);
            for (b, second) in chains.iter().enumerate().skip(a + 1) {
                let start = &rows[second.rows[0]];
                let gap = start.date.days_since(end.date);
                let continues = (pass.band.0..=pass.band.1).contains(&gap)
                    || (gap > pass.band.1 && gap <= params.split_days);
                let same_account = end.account == start.account;
                let same_amount = end.amount == start.amount;
                if second.pass == first.pass
                    && end.direction == start.direction
                    && continues
                    && (same_account || same_amount)
                    && ratio(end.amount, start.amount) <= params.step_ratio
                    && !first_keys.is_disjoint(&keys(rows, second))
                {
                    join = Some((a, b));
                    break 'search;
                }
            }
        }
        let Some((a, b)) = join else {
            return chains;
        };
        let second = chains.remove(b);
        chains[a].rows.extend(second.rows);
    }
}

/// Series whose amount changes every time (utilities, interest, estimated tax), and
/// long-cadence series with a price change, from the rows no exact-amount series took,
/// one fine merchant key at a time (so a membership isn't read among the same shop's
/// purchases).
fn variable_chains(rows: &[Row<'_>], used: &HashSet<usize>, params: Params) -> Vec<Chain> {
    let mut groups: BTreeMap<(AccountId, Direction, &str), Vec<usize>> = BTreeMap::new();
    for (i, r) in rows.iter().enumerate() {
        if !used.contains(&i) && !r.key.is_empty() {
            groups
                .entry((r.account, r.direction, r.fine_key.as_str()))
                .or_default()
                .push(i);
        }
    }
    let mut out = Vec::new();
    for members in groups.values() {
        let mut free = members.clone();
        for (p, pass) in PASSES.iter().enumerate() {
            let Some(min) = pass.min_variable else {
                continue;
            };
            let amounts = match pass.cadence {
                Cadence::Semiannual | Cadence::Annual => AmountRule::NearAll(params.step_band),
                _ => AmountRule::NearLast(params.variable_band),
            };
            let mut taken = HashSet::new();
            for chain in build(rows, &free, pass, min, params, amounts) {
                if chain.len() >= min && is_regular(rows, &chain, pass, params) {
                    taken.extend(chain.iter().copied());
                    out.push(Chain {
                        rows: chain,
                        pass: p,
                        variable: true,
                    });
                }
            }
            free.retain(|i| !taken.contains(i));
        }
    }
    out
}

/// Folds exact-amount chains into a variable one of the same merchant that spans them:
/// a few months of identical interest are part of the interest series.
fn absorb(rows: &[Row<'_>], exact: Vec<Chain>, variable: &mut [Chain]) -> Vec<Chain> {
    let mut kept = Vec::new();
    'chains: for e in exact {
        let (e_first, e_last) = (
            rows[e.rows[0]].date,
            rows[*e.rows.last().expect("chains are never empty")].date,
        );
        let e_row = &rows[e.rows[0]];
        let e_keys = keys(rows, &e);
        for v in variable.iter_mut() {
            let v_row = &rows[v.rows[0]];
            let slack = PASSES[v.pass].band.1;
            let v_first = rows[v.rows.iter().copied().min().expect("non-empty")].date;
            let v_last = rows[v.rows.iter().copied().max().expect("non-empty")].date;
            if v_row.account == e_row.account
                && v_row.direction == e_row.direction
                && e_keys.contains(&v_row.key)
                && e_first.days_since(v_first) >= -slack
                && v_last.days_since(e_last) >= -slack
            {
                v.rows.extend(e.rows);
                continue 'chains;
            }
        }
        kept.push(e);
    }
    kept
}

/// Adds to each series the rows of its merchant that no series took and that sit in
/// one of its empty slots (at least half a period from every member, and within a
/// period of its ends), when the amount is within a price step of the nearest member.
/// A series of interest that repeats each amount for a few months is found as
/// exact-amount chains; the months in between belong to it too.
fn fill_gaps(rows: &[Row<'_>], chains: &mut [Chain], params: Params) {
    let mut used: HashSet<usize> = chains.iter().flat_map(|c| c.rows.iter().copied()).collect();
    for chain in chains.iter_mut() {
        let pass = &PASSES[chain.pass];
        let sample = &rows[chain.rows[0]];
        let chain_keys = keys(rows, chain);
        let candidates: Vec<usize> = (0..rows.len())
            .filter(|i| !used.contains(i))
            .filter(|&i| {
                let r = &rows[i];
                r.account == sample.account
                    && r.direction == sample.direction
                    && chain_keys.contains(&r.key)
            })
            .collect();
        for i in candidates {
            let r = &rows[i];
            let mut nearest: Option<(i32, usize)> = None;
            for &m in &chain.rows {
                let distance = r.date.days_since(rows[m].date).abs();
                if nearest.is_none_or(|(d, _)| distance < d) {
                    nearest = Some((distance, m));
                }
            }
            let Some((distance, m)) = nearest else {
                continue;
            };
            let first = chain
                .rows
                .iter()
                .map(|&m| rows[m].date)
                .min()
                .expect("non-empty");
            let last = chain
                .rows
                .iter()
                .map(|&m| rows[m].date)
                .max()
                .expect("non-empty");
            let inside =
                r.date.days_since(first) >= -pass.band.1 && last.days_since(r.date) >= -pass.band.1;
            if inside
                && distance >= pass.band.0
                && ratio(rows[m].amount, r.amount) <= params.step_ratio
            {
                chain.rows.push(i);
                used.insert(i);
            }
        }
    }
}

/// Twice-monthly rows on about the same two days of the month are semimonthly;
/// anything else in that pass is biweekly.
fn twice_monthly(rows: &[Row<'_>], chain: &[usize]) -> Cadence {
    // Days late in the month count as just before the 1st.
    let days: Vec<i32> = chain
        .iter()
        .map(|&i| {
            let d = i32::from(rows[i].date.day());
            if d >= 26 { d - 31 } else { d }
        })
        .collect();
    let (early, mid): (Vec<i32>, Vec<i32>) = days.iter().partition(|&&d| d <= 8);
    let tight = |v: &[i32]| {
        !v.is_empty() && v.iter().max().unwrap_or(&0) - v.iter().min().unwrap_or(&0) <= 6
    };
    if tight(&early) && tight(&mid) {
        Cadence::Semimonthly
    } else {
        Cadence::Biweekly
    }
}

fn next_after(date: Date, cadence: Cadence) -> Option<Date> {
    match cadence {
        Cadence::Monthly => date.checked_add_months(1),
        Cadence::Quarterly => date.checked_add_months(3),
        Cadence::Semiannual => date.checked_add_months(6),
        Cadence::Annual => date.checked_add_months(12),
        _ => date.checked_add_days(cadence.nominal_days()),
    }
}

fn describe(rows: &[Row<'_>], mut chain: Chain, as_of: Date) -> Series {
    chain.rows.sort_by_key(|&i| (rows[i].date, rows[i].id));
    let members: Vec<&Row<'_>> = chain.rows.iter().map(|&i| &rows[i]).collect();
    let first = members[0];
    let last = *members.last().expect("chains are never empty");
    let cadence = match PASSES[chain.pass].cadence {
        Cadence::Biweekly => twice_monthly(rows, &chain.rows),
        c => c,
    };
    let distinct: HashSet<i64> = members.iter().map(|r| r.amount.minor()).collect();
    // An "exact" series with an amount change every few occurrences varies.
    let variable = chain.variable || distinct.len() > (members.len() / 4).max(3);
    let amount = if variable {
        let by_size = |r: &&&Row<'_>| r.amount.minor().unsigned_abs();
        SeriesAmount::Variable {
            min: members
                .iter()
                .min_by_key(by_size)
                .expect("non-empty")
                .amount,
            max: members
                .iter()
                .max_by_key(by_size)
                .expect("non-empty")
                .amount,
            latest: last.amount,
        }
    } else {
        let mut steps: Vec<(Date, Money)> = Vec::new();
        for r in &members {
            if steps.last().is_none_or(|s| s.1 != r.amount) {
                steps.push((r.date, r.amount));
            }
        }
        SeriesAmount::Fixed { steps }
    };
    let mut accounts = Vec::new();
    let mut descriptions: Vec<String> = Vec::new();
    let mut key_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &members {
        if !accounts.contains(&r.account) {
            accounts.push(r.account);
        }
        if descriptions.len() < 3 && !descriptions.iter().any(|d| d == r.description) {
            descriptions.push(r.description.to_owned());
        }
        *key_counts.entry(r.key.as_str()).or_default() += 1;
    }
    let merchant_key = key_counts
        .iter()
        .max_by_key(|&(k, n)| (*n, std::cmp::Reverse(*k)))
        .map(|(k, _)| (*k).to_owned())
        .unwrap_or_default();
    let kind = match first.direction {
        Direction::In => {
            let interest = members.iter().any(|r| {
                let d = r.description.to_uppercase();
                d.contains("INTEREST") || d.contains("DIVIDEND")
            });
            if interest {
                SeriesKind::Interest
            } else {
                SeriesKind::Income
            }
        }
        Direction::Out if members.iter().any(|r| r.own_transfer) => SeriesKind::Transfer,
        Direction::Out => SeriesKind::Charge,
    };
    let ended = f64::from(as_of.days_since(last.date)) > 1.5 * f64::from(cadence.nominal_days());
    Series {
        accounts,
        direction: first.direction,
        cadence,
        amount,
        members: members.iter().map(|r| r.id).collect(),
        first: first.date,
        last: last.date,
        next_expected: if ended {
            None
        } else {
            next_after(last.date, cadence)
        },
        ended,
        kind,
        merchant_key,
        descriptions,
        label: None,
    }
}

/// Finds recurring series among `transactions`, as of the date `as_of` (which decides
/// whether a series has ended). `decisions` tell it which rows are transfers (T1.4):
/// card payments and the receiving side of own-account transfers are left out.
///
/// Series come sorted by first date. Labels are empty; see [`label_all`](super::label_all).
pub fn detect(
    accounts: &[Account],
    transactions: &[Transaction],
    decisions: &[Decision],
    as_of: Date,
) -> Vec<Series> {
    detect_with(PARAMS, accounts, transactions, decisions, as_of)
}

/// [`detect`] with other tolerances, for measuring how much each one matters.
pub(crate) fn detect_with(
    params: Params,
    accounts: &[Account],
    transactions: &[Transaction],
    decisions: &[Decision],
    as_of: Date,
) -> Vec<Series> {
    let rows = rows(accounts, transactions, decisions);
    let exact = accept(&rows, exact_chains(&rows, params), &HashSet::new(), params);
    let exact = merge_steps(&rows, exact, params);
    let used: HashSet<usize> = exact.iter().flat_map(|c| c.rows.iter().copied()).collect();
    let mut variable = accept(&rows, variable_chains(&rows, &used, params), &used, params);
    let exact = absorb(&rows, exact, &mut variable);
    let mut chains: Vec<Chain> = exact.into_iter().chain(variable).collect();
    fill_gaps(&rows, &mut chains, params);
    let mut series: Vec<Series> = chains
        .into_iter()
        .map(|c| describe(&rows, c, as_of))
        .collect();
    series.sort_by(|a, b| {
        (a.first, &a.accounts, &a.merchant_key, &a.members).cmp(&(
            b.first,
            &b.accounts,
            &b.merchant_key,
            &b.members,
        ))
    });
    series
}

/// [`detect`] on a ledger.
pub fn detect_in(ledger: &Unlocked, as_of: Date) -> Result<Vec<Series>, StorageError> {
    Ok(detect(
        &ledger.accounts()?,
        &ledger.transactions()?,
        &ledger.decisions()?,
        as_of,
    ))
}
