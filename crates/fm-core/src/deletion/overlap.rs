//! Warning when a new file covers a period the user deleted earlier (decision 44).
//!
//! Each deletion records what it covered: at most an account and a date range, never
//! anything about the transactions. An import whose statement period overlaps a
//! recorded scope is still imported in full; the import summary shows
//! [`OVERLAP_WARNING`].
//!
//! TODO(decision 44): refine, e.g. skip overlapping rows by default with a "bring them
//! back" choice. For the MVP the rule is simply "import everything and warn".

use rusqlite::params;

use super::{DeletionError, Target};
use crate::model::{AccountId, Date, ImportId};
use crate::storage::Unlocked;

/// Shown in the import summary when the file overlaps a deleted period.
pub const OVERLAP_WARNING: &str = "This file covers a period you deleted earlier";

const KEY_PREFIX: &str = "deletion.scope.";

/// What one deletion covered. `None` means "any": any account, or no bound on that
/// side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletedScope {
    pub account: Option<AccountId>,
    pub from: Option<Date>,
    pub to: Option<Date>,
}

impl DeletedScope {
    fn encode(&self) -> String {
        let account = self.account.map(|a| a.0.to_string()).unwrap_or_default();
        let date = |d: Option<Date>| d.map(|d| d.to_string()).unwrap_or_default();
        format!("{account}|{}|{}", date(self.from), date(self.to))
    }

    fn decode(s: &str) -> Option<Self> {
        let mut parts = s.split('|');
        let (account, from, to) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        let date = |p: &str| -> Option<Option<Date>> {
            if p.is_empty() {
                Some(None)
            } else {
                p.parse().ok().map(Some)
            }
        };
        Some(Self {
            account: if account.is_empty() {
                None
            } else {
                Some(AccountId(account.parse().ok()?))
            },
            from: date(from)?,
            to: date(to)?,
        })
    }

    /// True if a file for `account` (`None`: unknown) covering `from..=to` touches
    /// this scope.
    fn overlaps(&self, account: Option<AccountId>, from: Date, to: Date) -> bool {
        let same_account = match (self.account, account) {
            (Some(deleted), Some(file)) => deleted == file,
            _ => true,
        };
        same_account && self.from.is_none_or(|f| to >= f) && self.to.is_none_or(|t| from <= t)
    }
}

/// The scope to record for `target`, read before the deletion removes the rows it is
/// computed from. `None` when the target holds no transactions.
///
/// A deleted account is recorded as "any account" over its date span: a file imported
/// again later creates a new account with a new id, and the warning is meant for
/// exactly that case. Over-warning beats missing it.
pub(super) fn scope_of(
    ledger: &Unlocked,
    target: Target,
) -> Result<Option<DeletedScope>, DeletionError> {
    let span = |filter: &str, id: i64| -> Result<Option<(Date, Date)>, DeletionError> {
        let (min, max): (Option<String>, Option<String>) = ledger.connection().query_row(
            &format!(
                "SELECT MIN(COALESCE(transaction_date, posted_date)),
                        MAX(COALESCE(transaction_date, posted_date))
                 FROM transactions WHERE {filter} = ?1"
            ),
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(match (min, max) {
            (Some(min), Some(max)) => min.parse().ok().zip(max.parse().ok()),
            _ => None,
        })
    };
    Ok(match target {
        Target::Range { account, from, to } => Some(DeletedScope {
            account,
            from: Some(from),
            to: Some(to),
        }),
        Target::Import(ImportId(id)) => {
            let account = ledger.raw_import(ImportId(id))?.and_then(|r| r.account_id);
            span("import_id", id)?.map(|(from, to)| DeletedScope {
                account,
                from: Some(from),
                to: Some(to),
            })
        }
        Target::Account(AccountId(id)) => span("account_id", id)?.map(|(from, to)| DeletedScope {
            account: None,
            from: Some(from),
            to: Some(to),
        }),
    })
}

/// Records a deleted scope in the ledger's `meta` table.
pub(super) fn record(ledger: &Unlocked, scope: &DeletedScope) -> Result<(), DeletionError> {
    let conn = ledger.connection();
    let mut stmt = conn.prepare("SELECT key FROM meta WHERE key LIKE ?1")?;
    let last = stmt
        .query_map([format!("{KEY_PREFIX}%")], |r| r.get::<_, String>(0))?
        .filter_map(Result::ok)
        .filter_map(|k| k.strip_prefix(KEY_PREFIX)?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)",
        params![format!("{KEY_PREFIX}{}", last + 1), scope.encode()],
    )?;
    Ok(())
}

/// Every deleted scope on record, oldest first. Unreadable rows are skipped.
pub fn deleted_scopes(ledger: &Unlocked) -> Result<Vec<DeletedScope>, DeletionError> {
    let conn = ledger.connection();
    let mut stmt = conn.prepare("SELECT key, value FROM meta WHERE key LIKE ?1")?;
    let mut rows: Vec<(u64, DeletedScope)> = stmt
        .query_map([format!("{KEY_PREFIX}%")], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .filter_map(Result::ok)
        .filter_map(|(key, value)| {
            let n = key.strip_prefix(KEY_PREFIX)?.parse().ok()?;
            Some((n, DeletedScope::decode(&value)?))
        })
        .collect();
    rows.sort_by_key(|(n, _)| *n);
    Ok(rows.into_iter().map(|(_, s)| s).collect())
}

/// True if a file for `account` (`None` if not known yet) whose statement covers
/// `from..=to` overlaps a period deleted earlier. The import goes ahead either way;
/// the summary shows [`OVERLAP_WARNING`] (decision 44).
pub fn overlaps_deleted(
    ledger: &Unlocked,
    account: Option<AccountId>,
    from: Date,
    to: Date,
) -> Result<bool, DeletionError> {
    let (from, to) = if from <= to { (from, to) } else { (to, from) };
    Ok(deleted_scopes(ledger)?
        .iter()
        .any(|s| s.overlaps(account, from, to)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }

    #[test]
    fn scopes_round_trip_through_their_text() {
        for scope in [
            DeletedScope {
                account: Some(AccountId(7)),
                from: Some(d("2026-03-01")),
                to: Some(d("2026-03-31")),
            },
            DeletedScope {
                account: None,
                from: Some(d("2025-01-01")),
                to: None,
            },
        ] {
            assert_eq!(DeletedScope::decode(&scope.encode()), Some(scope));
        }
        assert_eq!(
            DeletedScope {
                account: Some(AccountId(7)),
                from: Some(d("2026-03-01")),
                to: Some(d("2026-03-31")),
            }
            .encode(),
            "7|2026-03-01|2026-03-31",
            "ids and dates only"
        );
        for bad in [
            "",
            "7|2026-03-01",
            "x|2026-03-01|2026-03-31",
            "7|March|2026-03-31",
            "1|||",
        ] {
            assert_eq!(DeletedScope::decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn overlap_rules() {
        let march = DeletedScope {
            account: Some(AccountId(7)),
            from: Some(d("2026-03-01")),
            to: Some(d("2026-03-31")),
        };
        let o = |account, from: &str, to: &str| march.overlaps(account, d(from), d(to));
        assert!(o(Some(AccountId(7)), "2026-03-10", "2026-03-20"), "inside");
        assert!(
            o(Some(AccountId(7)), "2026-02-01", "2026-03-01"),
            "touches the start"
        );
        assert!(
            o(Some(AccountId(7)), "2026-03-31", "2026-04-30"),
            "touches the end"
        );
        assert!(
            o(Some(AccountId(7)), "2026-01-01", "2026-12-31"),
            "contains it"
        );
        assert!(!o(Some(AccountId(7)), "2026-04-01", "2026-04-30"), "after");
        assert!(!o(Some(AccountId(7)), "2026-01-01", "2026-02-28"), "before");
        assert!(
            !o(Some(AccountId(8)), "2026-03-10", "2026-03-20"),
            "another account"
        );
        assert!(o(None, "2026-03-10", "2026-03-20"), "account not known yet");
        let any = DeletedScope {
            account: None,
            ..march
        };
        assert!(any.overlaps(Some(AccountId(8)), d("2026-03-10"), d("2026-03-20")));
    }
}
