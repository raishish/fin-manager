//! Deleting data (SPEC §8, decision 28): by import, by account or by date range, with
//! everything derived from it, and "delete everything", which destroys the keys first.
//! After any deletion the user is told that older backups still hold the data and is
//! offered to delete them. See `README.md`.
//!
//! Storage does the cascade in one transaction (`Unlocked::delete_planned`, T0.10);
//! this module decides what goes: which original files are deleted whole, which are
//! only partly in scope, and what the user is shown before confirming.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::backup::{self, BackupError};
use crate::model::{AccountId, Date, ImportId};
use crate::storage::{DeletedCounts, DeletionPlan, DeletionScope, StorageError, Unlocked, Vault};
use crate::unlock::{KeychainDeletion, Now, Session, Status, TouchIdKeychain, VaultStore};

/// What the user asked to delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// One original file and everything that came from it.
    Import(ImportId),
    /// An account: its transactions, its own files and the account itself.
    Account(AccountId),
    /// Transactions dated `from..=to` (purchase date when known, else posted date:
    /// decision 42), in one account or all.
    Range {
        account: Option<AccountId>,
        from: Date,
        to: Date,
    },
}

/// What to do with original files that hold rows both inside and outside the
/// deletion (decision 28 keeps original files exactly as they came in, so a file is
/// only ever deleted whole). Pending the user's decision; see the README.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PartialFiles {
    /// Keep them; their out-of-scope transactions stay. The default.
    #[default]
    Keep,
    /// Delete them whole, with their out-of-scope transactions.
    DeleteWhole,
}

/// An original file only partly inside the deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialFile {
    pub import: ImportId,
    pub file_name: Option<String>,
    /// Its transactions inside the deletion.
    pub in_scope: u64,
    /// Its transactions outside it, which go too if the file is deleted whole.
    pub out_of_scope: u64,
    /// Deleting the account requires deleting this file whole (it belongs to the
    /// account), whatever [`PartialFiles`] says.
    pub forced: bool,
}

/// The rows a deletion takes, in the order storage removes them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Transactions in scope (with everything derived), if any.
    pub transactions: Option<Scope>,
    /// Original files deleted whole.
    pub imports: Vec<ImportId>,
    /// Accounts deleted, after the above.
    pub accounts: Vec<AccountId>,
}

/// Transactions of one account and/or a date range (both bounds inclusive).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scope {
    pub account: Option<AccountId>,
    pub from: Option<Date>,
    pub to: Option<Date>,
}

impl Plan {
    fn to_storage(&self) -> DeletionPlan {
        DeletionPlan {
            transactions: self.transactions.map(|s| DeletionScope {
                account: s.account,
                from: s.from,
                to: s.to,
            }),
            imports: self.imports.clone(),
            accounts: self.accounts.clone(),
        }
    }
}

/// What a deletion would take, shown before the user confirms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// Exactly what [`delete`] would remove with the same arguments: storage runs the
    /// deletion and rolls it back.
    pub counts: DeletedCounts,
    /// Original files only partly inside the deletion (decision 43).
    pub partial_files: Vec<PartialFile>,
}

/// What a deletion removed, and the offer to delete old backups that still hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deleted {
    pub counts: DeletedCounts,
    pub backups: Option<BackupOffer>,
}

/// The counts `target` would remove, without removing anything.
pub fn preview(
    ledger: &mut Unlocked,
    target: Target,
    partial: PartialFiles,
) -> Result<Preview, DeletionError> {
    let (plan, partial_files) = plan(ledger, target, partial)?;
    let counts = ledger.delete_planned(&plan.to_storage(), true)?;
    Ok(Preview {
        counts,
        partial_files,
    })
}

/// Deletes `target` and everything derived from it, in one transaction. Rows removed
/// from files that are kept are recorded, so they never come back from those files
/// (decision 43).
pub fn delete(
    ledger: &mut Unlocked,
    target: Target,
    partial: PartialFiles,
) -> Result<Deleted, DeletionError> {
    let (plan, _) = plan(ledger, target, partial)?;
    let counts = ledger.delete_planned(&plan.to_storage(), false)?;
    Ok(Deleted {
        counts,
        backups: backup_offer(ledger),
    })
}

/// The plan for `target`, and the original files only partly inside it.
pub fn plan(
    ledger: &Unlocked,
    target: Target,
    partial: PartialFiles,
) -> Result<(Plan, Vec<PartialFile>), DeletionError> {
    let scope = match target {
        Target::Import(id) => {
            if ledger.raw_import(id)?.is_none() {
                return Err(DeletionError::NotFound);
            }
            let plan = Plan {
                transactions: None,
                imports: vec![id],
                accounts: Vec::new(),
            };
            return Ok((plan, Vec::new()));
        }
        Target::Account(id) => {
            if ledger.account(id)?.is_none() {
                return Err(DeletionError::NotFound);
            }
            Scope {
                account: Some(id),
                from: None,
                to: None,
            }
        }
        Target::Range { account, from, to } => {
            if from > to {
                return Err(DeletionError::EmptyRange);
            }
            Scope {
                account,
                from: Some(from),
                to: Some(to),
            }
        }
    };
    let account_target = match target {
        Target::Account(id) => Some(id),
        _ => None,
    };

    let mut imports = Vec::new();
    let mut partial_files = Vec::new();
    for touched in touched_imports(ledger, scope, account_target)? {
        let whole = touched.out_of_scope == 0;
        let forced = !whole && touched.owned_by_target;
        if !whole {
            partial_files.push(PartialFile {
                import: touched.import,
                file_name: touched.file_name,
                in_scope: touched.in_scope,
                out_of_scope: touched.out_of_scope,
                forced,
            });
        }
        if whole || forced || partial == PartialFiles::DeleteWhole {
            imports.push(touched.import);
        }
    }
    let plan = Plan {
        transactions: Some(scope),
        imports,
        accounts: account_target.into_iter().collect(),
    };
    Ok((plan, partial_files))
}

struct Touched {
    import: ImportId,
    file_name: Option<String>,
    in_scope: u64,
    out_of_scope: u64,
    /// The file is recorded as the account's own (raw_imports.account_id).
    owned_by_target: bool,
}

/// Original files with at least one transaction in scope, plus, when deleting an
/// account, every file recorded as that account's (even ones with no transactions).
fn touched_imports(
    ledger: &Unlocked,
    scope: Scope,
    account_target: Option<AccountId>,
) -> Result<Vec<Touched>, DeletionError> {
    // Decision 42: a transaction's date is its purchase date when known.
    const IN_SCOPE: &str = "(?1 IS NULL OR t.account_id = ?1)
         AND (?2 IS NULL OR COALESCE(t.transaction_date, t.posted_date) >= ?2)
         AND (?3 IS NULL OR COALESCE(t.transaction_date, t.posted_date) <= ?3)";
    let sql = format!(
        "SELECT r.id, r.file_name, r.account_id,
                SUM(CASE WHEN t.id IS NOT NULL AND ({IN_SCOPE}) THEN 1 ELSE 0 END),
                SUM(CASE WHEN t.id IS NOT NULL AND NOT ({IN_SCOPE}) THEN 1 ELSE 0 END)
         FROM raw_imports r LEFT JOIN transactions t ON t.import_id = r.id
         GROUP BY r.id
         HAVING SUM(CASE WHEN t.id IS NOT NULL AND ({IN_SCOPE}) THEN 1 ELSE 0 END) > 0
             OR (?4 IS NOT NULL AND r.account_id = ?4)
         ORDER BY r.id"
    );
    let conn = ledger.connection();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            scope.account.map(|a| a.0),
            scope.from.map(|d| d.to_string()),
            scope.to.map(|d| d.to_string()),
            account_target.map(|a| a.0),
        ],
        |r| {
            let owner: Option<i64> = r.get(2)?;
            Ok(Touched {
                import: ImportId(r.get(0)?),
                file_name: r.get(1)?,
                owned_by_target: owner.is_some() && owner == account_target.map(|a| a.0),
                in_scope: r.get::<_, i64>(3)?.unsigned_abs(),
                out_of_scope: r.get::<_, i64>(4)?.unsigned_abs(),
            })
        },
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---- the backup offer (decision 28) -------------------------------------------------------

/// After a deletion: older backups in the user's folder still hold the deleted data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupOffer {
    pub folder: PathBuf,
    pub ledger_id: String,
    /// This ledger's backups in the folder.
    pub count: usize,
}

impl BackupOffer {
    /// The warning to show, in the words of decision 28.
    pub fn message(&self) -> String {
        format!(
            "Your older backups still contain the data you just deleted, and they open \
             with the password or recovery key that was current when they were made. \
             Delete the {} backup{} in {} too? Copies you moved or copied elsewhere \
             are outside the app's reach.",
            self.count,
            if self.count == 1 { "" } else { "s" },
            self.folder.display()
        )
    }
}

/// The offer for `ledger`, if a backup folder is set and holds backups of it.
pub fn backup_offer(ledger: &Unlocked) -> Option<BackupOffer> {
    let settings = backup::Settings::load(ledger).ok()?;
    let folder = settings.folder?;
    let ledger_id = backup::ledger_id(ledger).ok()?;
    let count = backup::list_mine(&folder, &ledger_id).ok()?.len();
    (count > 0).then_some(BackupOffer {
        folder,
        ledger_id,
        count,
    })
}

/// Accepting the offer: deletes this ledger's backups in the folder (and nothing
/// else). Returns how many were deleted.
pub fn delete_backups(offer: &BackupOffer) -> Result<usize, DeletionError> {
    Ok(backup::delete_all(&offer.folder, &offer.ledger_id)?)
}

// ---- delete everything --------------------------------------------------------------------

/// What "delete everything" leaves to say: the backup offer, read before the ledger
/// (which holds the backup settings) was destroyed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EverythingDeleted {
    pub backups: Option<BackupOffer>,
    /// The user went ahead although this build couldn't check the Keychain for a
    /// Touch ID copy of the key (see [`DeletionError::KeychainUnverifiable`]).
    pub keychain_unverified: bool,
}

/// Deletes the whole ledger, destroying the keys first (decision 28), so any copy left
/// behind (snapshots, Time Machine, synced folders) can never be decrypted:
/// 1. the Touch ID copy of the data key in the Keychain;
/// 2. the data key in memory (the ledger is locked);
/// 3. the keyfile, which holds the password and recovery-key slots: overwritten,
///    synced and deleted;
/// 4. then the database and the ledger folder.
///
/// The ledger must be open (the caller has confirmed). If the Keychain copy can't be
/// removed, nothing is destroyed: with that copy, leftovers could still be read. If
/// this build can't even check for one (an unsigned build can't see an item a signed
/// build stored), nothing is destroyed either, unless the user has read that and
/// chosen to go ahead (`allow_unverified_keychain`).
pub fn delete_everything<K: TouchIdKeychain>(
    session: &mut Session<VaultStore, K>,
    now: Now,
    allow_unverified_keychain: bool,
) -> Result<EverythingDeleted, DeletionError> {
    if session.status() != Status::Unlocked {
        return Err(DeletionError::NotUnlocked);
    }
    let backups = session
        .ledger()
        .and_then(|open| backup_offer(open.ledger()));

    step("forget_touch_id");
    let keychain_unverified = match session.forget_touch_id_verified() {
        Err(_) => return Err(DeletionError::KeychainNotCleared),
        Ok(KeychainDeletion::Done) => false,
        Ok(KeychainDeletion::Unverifiable) if allow_unverified_keychain => true,
        Ok(KeychainDeletion::Unverifiable) => return Err(DeletionError::KeychainUnverifiable),
    };
    step("lock");
    session.lock(now);
    let vault = session.store().vault().clone();
    step("shred_keys");
    vault.shred_keys()?;
    step("remove_files");
    remove_ledger_files(&vault)?;
    Ok(EverythingDeleted {
        backups,
        keychain_unverified,
    })
}

/// Removes what remains of a ledger whose keys are gone.
fn remove_ledger_files(vault: &Vault) -> Result<(), DeletionError> {
    let db = vault.db_path();
    let mut journal = db.as_os_str().to_owned();
    journal.push("-journal");
    for path in [db, Path::new(&journal)] {
        match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    match fs::remove_dir_all(vault.dir()) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: the order in which delete_everything did its steps.
    static STEPS: std::cell::RefCell<Vec<&'static str>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn step(_name: &'static str) {
    #[cfg(test)]
    STEPS.with(|s| s.borrow_mut().push(_name));
}

// ---- errors -----------------------------------------------------------------------------

/// Why a deletion didn't happen. Messages never include ledger contents.
#[derive(Debug)]
#[non_exhaustive]
pub enum DeletionError {
    /// The import or account doesn't exist.
    NotFound,
    /// The range ends before it starts.
    EmptyRange,
    /// The ledger must be open.
    NotUnlocked,
    /// The Touch ID copy of the key couldn't be removed from the Keychain, so nothing
    /// was destroyed. Try again.
    KeychainNotCleared,
    /// This build can't check the Keychain for a Touch ID copy of the key that a
    /// signed build of the app may have stored, so nothing was destroyed. The user can
    /// remove it by hand and go ahead.
    KeychainUnverifiable,
    Backup(BackupError),
    Storage(StorageError),
    Io(io::Error),
}

impl fmt::Display for DeletionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("there is nothing to delete with that"),
            Self::EmptyRange => f.write_str("the date range ends before it starts"),
            Self::NotUnlocked => f.write_str("unlock first"),
            Self::KeychainNotCleared => f.write_str(
                "the Touch ID copy of the key couldn't be removed from the Keychain, so \
                 nothing was deleted; try again",
            ),
            Self::KeychainUnverifiable => f.write_str(
                "This build can't check for a Touch ID key left by a signed build of the \
                 app. If you used Touch ID, delete the 'fin-manager' item in Keychain \
                 Access, or run Delete everything from the signed app.",
            ),
            Self::Backup(e) => e.fmt(f),
            Self::Storage(e) => e.fmt(f),
            Self::Io(e) => write!(f, "file error: {e}"),
        }
    }
}

impl std::error::Error for DeletionError {}

impl From<StorageError> for DeletionError {
    fn from(e: StorageError) -> Self {
        match e {
            StorageError::NotFound => Self::NotFound,
            e => Self::Storage(e),
        }
    }
}

impl From<rusqlite::Error> for DeletionError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(StorageError::Database(e))
    }
}

impl From<BackupError> for DeletionError {
    fn from(e: BackupError) -> Self {
        Self::Backup(e)
    }
}

impl From<io::Error> for DeletionError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests;
