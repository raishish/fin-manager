//! Deciding what each record of a statement is, against the transactions already on its
//! account: a duplicate, the posted version of a pending transaction, a pending version
//! of something already posted, or new.
//!
//! Wrongly merging two real transactions makes one silently disappear (a duplicate gets
//! no decision of its own), which is worse than a duplicate the user can see. So every
//! match needs agreeing evidence (amount, dates of the same kind, description), and
//! anything ambiguous falls through to "new".
//!
//! - Matching is a multiset match: each existing transaction can be claimed by at most
//!   one record per import, so k identical purchases on one day stay k transactions.
//! - Rows of the import being matched never match each other by content. Rows of another
//!   statement in the same file match only by bank id, as exact repeats (OFX files can
//!   hold overlapping statements of one account), or as a pending row settling.
//! - The whole statement is classified at once, so which record claims which existing
//!   transaction doesn't depend on row order.

use std::collections::{HashMap, HashSet};

use crate::model::{Date, Transaction, TransactionId, TransactionStatus};

/// How far after a pending transaction its posted version may be dated.
pub(crate) const PENDING_WINDOW_DAYS: i32 = 7;

/// How far apart two rows with the same bank id may be dated and still be one
/// transaction (banks rarely reuse ids, but they do).
pub(crate) const SAME_ID_WINDOW_DAYS: i32 = 3;

/// How long after its purchase a transaction may post, for pairing a purchase-date-only
/// row with a posted-date-only one.
pub(crate) const POSTING_LAG_DAYS: i32 = 3;

/// The shortest normalised text a prefix match may start from. OFX cuts `NAME` at 32
/// characters, which normalises to roughly 24 or more; shorter texts ("POS PURCHASE")
/// must match exactly.
pub(crate) const MIN_PREFIX_LEN: usize = 24;

/// Which dates a row's source actually gives. A row's `posted_date` field always holds
/// a date, but for a purchase-date-only CSV layout it is the purchase date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DatesGiven {
    /// Only the posted date (OFX without `DTUSER`, a CSV with a posted-date column only).
    Posted,
    /// Only the purchase date (a CSV with a transaction-date column only), held in
    /// `posted_date`.
    Purchase,
    /// Both (OFX with `DTUSER`, a CSV with both columns). A row may still lack a
    /// transaction date if the cell was empty.
    Both,
    /// Not known (a CSV profile that no longer exists, a manual entry): dates are
    /// compared only as written, and never paired by posting lag.
    Unknown,
}

/// Normalised text for comparing descriptions: upper case, every run of
/// non-alphanumeric characters turned into one space, trimmed.
pub(crate) fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.extend(c.to_uppercase());
        } else {
            space = true;
        }
    }
    out
}

/// How well two texts agree; the stronger sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TextMatch {
    Exact,
    /// One is a prefix of the other, and the shorter is at least [`MIN_PREFIX_LEN`]
    /// characters (a name truncated by OFX against its full text).
    Prefix,
}

fn text_match(a: &str, b: &str) -> Option<TextMatch> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    if a == b {
        return Some(TextMatch::Exact);
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    (short.len() >= MIN_PREFIX_LEN && long.starts_with(short)).then_some(TextMatch::Prefix)
}

/// How two rows' dates agree; the stronger sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DateMatch {
    /// Both rows give both dates, and both agree.
    Both,
    /// Both give a posted date and they agree (and purchase dates agree if both give one).
    Posted,
    /// Both give a purchase date and they agree.
    Purchase,
    /// A purchase-date-only row and a posted-date-only row, the posting this many days
    /// after the purchase. Accepted only as a unique pairing (see `classify_all`).
    Lagged(i32),
}

/// What the matcher knows about a transaction, existing or incoming.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub status: TransactionStatus,
    pub dates_given: DatesGiven,
    /// The row's `posted_date` field (a purchase date when `dates_given` is `Purchase`).
    pub posted_date: Date,
    pub transaction_date: Option<Date>,
    pub amount_minor: i64,
    pub external_id: Option<String>,
    pub check_number: Option<String>,
    /// Normalised description (possibly empty).
    pub description: String,
    /// Normalised memo, if any.
    pub memo: Option<String>,
    /// Normalised description and memo together, when there is a memo: the text a CSV of
    /// the same bank may print as one description (PNC: NAME "POS PURCHASE" + MEMO
    /// "H-E-B #475" → "POS PURCHASE H-E-B #475").
    pub combined: Option<String>,
    /// A row of the import being matched (another statement of the same file).
    pub same_import: bool,
}

impl Candidate {
    pub fn of(t: &Transaction, dates_given: DatesGiven) -> Candidate {
        Candidate {
            status: t.status,
            dates_given,
            posted_date: t.posted_date,
            transaction_date: t.transaction_date,
            amount_minor: t.amount.minor(),
            external_id: t.source.external_id.clone().filter(|e| !e.is_empty()),
            check_number: t.check_number.clone().filter(|c| !c.is_empty()),
            description: normalise(&t.description),
            memo: t.memo.as_deref().map(normalise).filter(|m| !m.is_empty()),
            combined: t
                .memo
                .as_deref()
                .map(|m| normalise(&format!("{} {m}", t.description)))
                .filter(|c| !c.is_empty()),
            same_import: false,
        }
    }

    /// The posted date, if the source gives one.
    fn posted(&self) -> Option<Date> {
        (self.dates_given != DatesGiven::Purchase).then_some(self.posted_date)
    }

    /// The purchase date, if the source gives one.
    fn purchase(&self) -> Option<Date> {
        match self.dates_given {
            DatesGiven::Purchase => Some(self.posted_date),
            _ => self.transaction_date,
        }
    }

    /// Only a posted date, from a source known to give posted dates (eligible for a
    /// lagged pairing).
    fn posted_only(&self) -> bool {
        matches!(self.dates_given, DatesGiven::Posted | DatesGiven::Both)
            && self.purchase().is_none()
    }

    /// Only a purchase date (eligible for a lagged pairing).
    fn purchase_only(&self) -> bool {
        self.dates_given == DatesGiven::Purchase
    }

    /// Descriptions are compared with descriptions and memos with memos, never one with
    /// the other. Every field both rows have must agree, and at least one must be
    /// compared. A match is exact only if every compared field is exact. One more form:
    /// when only one row has a memo, its description and memo together may equal the
    /// other's description (the same bank's CSV printing them as one text).
    fn text_match(&self, other: &Candidate) -> Option<TextMatch> {
        self.field_match(other)
            .or_else(|| match (&self.combined, &other.combined) {
                (Some(c), None) => text_match(c, &other.description),
                (None, Some(c)) => text_match(&self.description, c),
                _ => None,
            })
    }

    fn field_match(&self, other: &Candidate) -> Option<TextMatch> {
        let description = (!self.description.is_empty() && !other.description.is_empty())
            .then(|| text_match(&self.description, &other.description));
        let memo = match (&self.memo, &other.memo) {
            (Some(a), Some(b)) => Some(text_match(a, b)),
            _ => None,
        };
        match (description, memo) {
            (None, None) => None,
            (Some(d), None) => d,
            (None, Some(m)) => m,
            (Some(d), Some(m)) => Some(d?.max(m?)),
        }
    }

    /// Check numbers, when both rows give one, must be equal.
    fn checks_agree(&self, other: &Candidate) -> bool {
        match (&self.check_number, &other.check_number) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
    }

    /// Dates of the same kind compared with each other: posted with posted, purchase with
    /// purchase. A purchase-date-only row against a posted-date-only one gives a lag
    /// (a candidate only; `classify_all` accepts it only when unique).
    fn date_match(&self, other: &Candidate) -> Option<DateMatch> {
        match (
            self.posted(),
            other.posted(),
            self.purchase(),
            other.purchase(),
        ) {
            (Some(p), Some(q), Some(a), Some(b)) => match (a == b, p == q) {
                (true, true) => Some(DateMatch::Both),
                (true, false) => Some(DateMatch::Purchase),
                (false, _) => None,
            },
            (Some(p), Some(q), _, _) => (p == q).then_some(DateMatch::Posted),
            (_, _, Some(a), Some(b)) => (a == b).then_some(DateMatch::Purchase),
            _ => self.lag(other).filter(|d| *d >= 0).map(DateMatch::Lagged),
        }
    }

    /// Days from the purchase-only row's purchase to the posted-only row's posting, for
    /// a purchase-only/posted-only pair within ±[`POSTING_LAG_DAYS`]. Negative means
    /// posted before bought: not a pairing, but it still makes a pairing ambiguous.
    fn lag(&self, other: &Candidate) -> Option<i32> {
        let (bought, posted) = if self.purchase_only() && other.posted_only() {
            (self.posted_date, other.posted_date)
        } else if self.posted_only() && other.purchase_only() {
            (other.posted_date, self.posted_date)
        } else {
            return None;
        };
        let d = posted.days_since(bought);
        (d.abs() <= POSTING_LAG_DAYS).then_some(d)
    }

    /// Same amount, agreeing check numbers and text: the non-date evidence of one
    /// transaction.
    fn same_money_and_text(&self, other: &Candidate) -> Option<TextMatch> {
        if self.amount_minor != other.amount_minor || !self.checks_agree(other) {
            return None;
        }
        self.text_match(other)
    }
}

/// The ranking key of a pending → posted pair: nearest date, then the stronger text
/// match, then the nearer amount.
type SettleKey = (i32, TextMatch, i128);

/// Whether `posted` can be the settled version of `pending`: same sign, dated 0 to 7
/// days later, the amount between 10% (fuel holds) and 135% (tips) of the pending one,
/// agreeing text and check numbers. Returns the pair's ranking key.
fn settles(pending: &Candidate, posted: &Candidate) -> Option<SettleKey> {
    let days = posted.posted_date.days_since(pending.posted_date);
    let (p, q) = (
        i128::from(pending.amount_minor),
        i128::from(posted.amount_minor),
    );
    let in_range = (0..=PENDING_WINDOW_DAYS).contains(&days)
        && p != 0
        && p.signum() == q.signum()
        && 10 * q.abs() >= p.abs()
        && 100 * q.abs() <= 135 * p.abs()
        && pending.checks_agree(posted);
    if !in_range {
        return None;
    }
    let text = pending.text_match(posted)?;
    Some((days, text, (p - q).abs()))
}

/// What a record turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Already in the ledger as this transaction.
    Duplicate(TransactionId),
    /// The posted version of this pending transaction, which it updates.
    Posts(TransactionId),
    /// A pending version of this transaction, which has already posted.
    AlreadyPosted(TransactionId),
    /// A pending version of the posted record at this (earlier) index of the same
    /// statement.
    PendingOf(usize),
    /// The posted version of the pending record at this (earlier) index of the same
    /// statement, which it settles.
    SettlesRow(usize),
    /// New.
    New,
}

impl Outcome {
    fn claims(self) -> Option<TransactionId> {
        match self {
            Self::Duplicate(id) | Self::Posts(id) | Self::AlreadyPosted(id) => Some(id),
            Self::PendingOf(_) | Self::SettlesRow(_) | Self::New => None,
        }
    }
}

/// Matches one statement's records against one account's existing transactions.
pub(crate) struct Matcher {
    existing: Vec<(TransactionId, Candidate)>,
}

impl Matcher {
    /// `existing` is the account's transactions with the dates each one's source gives.
    /// Those from `import` (other statements of the file being imported) are marked.
    pub fn new(
        existing: Vec<(Transaction, DatesGiven)>,
        import: crate::model::ImportId,
    ) -> Matcher {
        Matcher {
            existing: existing
                .into_iter()
                .map(|(t, given)| {
                    let mut c = Candidate::of(&t, given);
                    c.same_import = t.source.import_id == import;
                    (t.id, c)
                })
                .collect(),
        }
    }

    fn unclaimed<'a>(
        &'a self,
        claimed: &'a HashSet<TransactionId>,
    ) -> impl Iterator<Item = &'a (TransactionId, Candidate)> {
        self.existing
            .iter()
            .filter(move |(id, _)| !claimed.contains(id))
    }

    /// Classifies every record of a statement (in file order) at once.
    pub fn classify_all(&self, records: &[Candidate]) -> Vec<Outcome> {
        let mut claimed: HashSet<TransactionId> = HashSet::new();
        let mut outcomes: Vec<Option<Outcome>> = vec![None; records.len()];

        // 1. In file order: the bank's id.
        for (i, rec) in records.iter().enumerate() {
            if let Some(outcome) = self.same_bank_id(rec, &claimed) {
                claimed.extend(outcome.claims());
                outcomes[i] = Some(outcome);
            }
        }

        // 2. The same content with dates of the same kind, best pair first across the
        //    statement: (dates, text, record, id).
        let mut pairs = Vec::new();
        for (i, rec) in records.iter().enumerate() {
            if outcomes[i].is_some() {
                continue;
            }
            for (id, c) in self.unclaimed(&claimed) {
                if let Some((date, text)) = Self::same_content(rec, c)
                    && !matches!(date, DateMatch::Lagged(_))
                {
                    pairs.push(((date, text), i, *id, Self::content_outcome(rec, c, *id)));
                }
            }
        }
        pairs.sort_by_key(|&(key, i, id, _)| (key, i, id));
        for (_, i, id, outcome) in pairs {
            if outcomes[i].is_none() && !claimed.contains(&id) {
                outcomes[i] = Some(outcome);
                claimed.insert(id);
            }
        }

        // 3. A purchase-date-only row and a posted-date-only one, a few days apart: only
        //    when each is the other's unique candidate within ±3 days. Anything else is
        //    ambiguous (a daily fare) and stays new.
        let mut lagged: Vec<(usize, TransactionId, Outcome)> = Vec::new();
        let mut per_record: HashMap<usize, usize> = HashMap::new();
        let mut per_existing: HashMap<TransactionId, usize> = HashMap::new();
        for (i, rec) in records.iter().enumerate() {
            if outcomes[i].is_some() {
                continue;
            }
            for (id, c) in self.unclaimed(&claimed) {
                if !Self::content_allowed(rec, c) || rec.same_money_and_text(c).is_none() {
                    continue;
                }
                if let Some(d) = rec.lag(c) {
                    *per_record.entry(i).or_default() += 1;
                    *per_existing.entry(*id).or_default() += 1;
                    if d >= 0 {
                        lagged.push((i, *id, Self::content_outcome(rec, c, *id)));
                    }
                }
            }
        }
        for (i, id, outcome) in lagged {
            if per_record[&i] == 1 && per_existing[&id] == 1 {
                outcomes[i] = Some(outcome);
                claimed.insert(id);
            }
        }

        // 4. Posted records settling pending transactions, best pair first.
        let mut pairs = Vec::new();
        for (i, rec) in records.iter().enumerate() {
            if outcomes[i].is_some() || rec.status != TransactionStatus::Posted {
                continue;
            }
            for (id, c) in self.unclaimed(&claimed) {
                if c.status == TransactionStatus::Pending
                    && let Some(key) = settles(c, rec)
                {
                    pairs.push((key, i, *id));
                }
            }
        }
        assign(&mut pairs, &mut outcomes, &mut claimed, Outcome::Posts);

        // 5. Pending records whose transaction has already posted.
        let mut pairs = Vec::new();
        for (i, rec) in records.iter().enumerate() {
            if outcomes[i].is_some() || rec.status != TransactionStatus::Pending {
                continue;
            }
            for (id, c) in self.unclaimed(&claimed) {
                if c.status == TransactionStatus::Posted
                    && let Some(key) = settles(rec, c)
                {
                    pairs.push((key, i, *id));
                }
            }
        }
        assign(
            &mut pairs,
            &mut outcomes,
            &mut claimed,
            Outcome::AlreadyPosted,
        );

        // 6. A pending row and its posted version in this same statement are one
        //    transaction. As across files, the earlier row is the one written: a pending
        //    row first is inserted and the posted row settles it; a posted row first is
        //    inserted and the pending row points at it.
        let mut pairs = Vec::new();
        for (i, pending) in records.iter().enumerate() {
            if outcomes[i].is_some() || pending.status != TransactionStatus::Pending {
                continue;
            }
            for (j, posted) in records.iter().enumerate() {
                if outcomes[j].is_none()
                    && posted.status == TransactionStatus::Posted
                    && let Some(key) = settles(pending, posted)
                {
                    pairs.push((key, i, j));
                }
            }
        }
        pairs.sort();
        let mut used: HashSet<usize> = HashSet::new();
        for (_, i, j) in pairs {
            if outcomes[i].is_none() && !used.contains(&i) && !used.contains(&j) {
                if i < j {
                    outcomes[j] = Some(Outcome::SettlesRow(i));
                } else {
                    outcomes[i] = Some(Outcome::PendingOf(j));
                }
                used.insert(i);
                used.insert(j);
            }
        }

        outcomes
            .into_iter()
            .map(|o| o.unwrap_or(Outcome::New))
            .collect()
    }

    /// The same bank id is the same transaction only if the rest agrees: the same amount
    /// and check number, a date within a few days, and agreeing text or an exact date (a
    /// duplicate); or a pending one it settles. Otherwise the id was reused, and the
    /// record falls through to the other checks.
    fn same_bank_id(&self, rec: &Candidate, claimed: &HashSet<TransactionId>) -> Option<Outcome> {
        let ext = rec.external_id.as_ref()?;
        self.unclaimed(claimed)
            .filter(|(_, c)| c.external_id.as_ref() == Some(ext))
            .find_map(|(id, c)| match (c.status, rec.status) {
                // Another statement of this same file: only an exact repeat counts.
                _ if c.same_import => (c.status == rec.status
                    && c.amount_minor == rec.amount_minor
                    && c.posted_date == rec.posted_date
                    && c.checks_agree(rec))
                .then_some(Outcome::Duplicate(*id)),
                (TransactionStatus::Pending, TransactionStatus::Posted) => {
                    settles(c, rec).map(|_| Outcome::Posts(*id))
                }
                (TransactionStatus::Posted, TransactionStatus::Pending) => {
                    settles(rec, c).map(|_| Outcome::AlreadyPosted(*id))
                }
                _ => {
                    let exact_date = matches!(
                        c.date_match(rec),
                        Some(DateMatch::Both | DateMatch::Posted | DateMatch::Purchase)
                    );
                    let near = exact_date
                        || c.posted_date.days_since(rec.posted_date).abs() <= SAME_ID_WINDOW_DAYS;
                    let evidence = exact_date || c.text_match(rec).is_some();
                    (c.amount_minor == rec.amount_minor && c.checks_agree(rec) && near && evidence)
                        .then_some(Outcome::Duplicate(*id))
                }
            })
    }

    /// A posted record can match only posted transactions (settling a pending one is
    /// pass 4); a pending record matches either. Rows of this import never match by
    /// content.
    fn content_allowed(rec: &Candidate, c: &Candidate) -> bool {
        !c.same_import
            && (rec.status == TransactionStatus::Pending || c.status == TransactionStatus::Posted)
    }

    fn same_content(rec: &Candidate, c: &Candidate) -> Option<(DateMatch, TextMatch)> {
        if !Self::content_allowed(rec, c) {
            return None;
        }
        let text = rec.same_money_and_text(c)?;
        Some((c.date_match(rec)?, text))
    }

    fn content_outcome(rec: &Candidate, c: &Candidate, id: TransactionId) -> Outcome {
        if c.status == TransactionStatus::Posted && rec.status == TransactionStatus::Pending {
            Outcome::AlreadyPosted(id)
        } else {
            Outcome::Duplicate(id)
        }
    }
}

/// Greedy assignment, best key first (ties: earlier record, then lower id).
fn assign(
    pairs: &mut [(SettleKey, usize, TransactionId)],
    outcomes: &mut [Option<Outcome>],
    claimed: &mut HashSet<TransactionId>,
    outcome: fn(TransactionId) -> Outcome,
) {
    pairs.sort();
    for &(_, i, id) in pairs.iter() {
        if outcomes[i].is_none() && !claimed.contains(&id) {
            outcomes[i] = Some(outcome(id));
            claimed.insert(id);
        }
    }
}

#[cfg(test)]
mod tests;
