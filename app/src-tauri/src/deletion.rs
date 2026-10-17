//! Deleting data in the app (T5.3): what can be deleted, "delete everything", and the
//! offer to delete old backups afterwards. The rules are `fm_core::deletion`;
//! `app/src/routes/data/api.ts` mirrors these types.

use std::sync::PoisonError;

use fm_core::deletion::{self, BackupOffer, DeletionError, PartialFile, PartialFiles, Target};
use fm_core::model::{AccountId, Date, ImportId};
use fm_core::storage::DeletedCounts;
use fm_core::unlock::Now;
use serde::{Deserialize, Serialize};

use crate::unlock::AppUnlock;

/// Typed by the user to confirm "delete everything".
pub const CONFIRMATION: &str = "delete everything";

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountChoice {
    id: i64,
    name: String,
    institution: Option<String>,
    transactions: u64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileChoice {
    id: i64,
    file_name: Option<String>,
    /// Unix seconds.
    imported_at: i64,
    transactions: u64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DataChoices {
    accounts: Vec<AccountChoice>,
    files: Vec<FileChoice>,
}

/// What the page asks to delete (`Target` in api.ts).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TargetArg {
    Import {
        id: i64,
    },
    Account {
        id: i64,
    },
    Range {
        account: Option<i64>,
        from: String,
        to: String,
    },
}

impl TargetArg {
    fn parse(self) -> Result<Target> {
        let date = |s: &str| {
            s.parse::<Date>()
                .map_err(|_| error("InvalidDate", "dates are written YYYY-MM-DD"))
        };
        Ok(match self {
            Self::Import { id } => Target::Import(ImportId(id)),
            Self::Account { id } => Target::Account(AccountId(id)),
            Self::Range { account, from, to } => Target::Range {
                account: account.map(AccountId),
                from: date(&from)?,
                to: date(&to)?,
            },
        })
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CountsView {
    transactions: u64,
    decisions: u64,
    rules: u64,
    imports: u64,
    accounts: u64,
    recorded_rows: u64,
}

impl From<DeletedCounts> for CountsView {
    fn from(c: DeletedCounts) -> Self {
        Self {
            transactions: c.transactions,
            decisions: c.decisions,
            rules: c.rules,
            imports: c.imports,
            accounts: c.accounts,
            recorded_rows: c.recorded_rows,
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PartialFileView {
    id: i64,
    file_name: Option<String>,
    in_scope: u64,
    out_of_scope: u64,
    forced: bool,
}

impl From<PartialFile> for PartialFileView {
    fn from(p: PartialFile) -> Self {
        Self {
            id: p.import.0,
            file_name: p.file_name,
            in_scope: p.in_scope,
            out_of_scope: p.out_of_scope,
            forced: p.forced,
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewView {
    counts: CountsView,
    partial_files: Vec<PartialFileView>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeletedView {
    counts: CountsView,
    backup_offer: Option<OfferView>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OfferView {
    count: usize,
    folder: String,
    message: String,
}

impl From<&BackupOffer> for OfferView {
    fn from(o: &BackupOffer) -> Self {
        Self {
            count: o.count,
            folder: o.folder.to_string_lossy().into_owned(),
            message: o.message(),
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeletionErrorView {
    code: &'static str,
    message: String,
}

impl From<DeletionError> for DeletionErrorView {
    fn from(e: DeletionError) -> Self {
        let code = match e {
            DeletionError::NotFound => "NotFound",
            DeletionError::EmptyRange => "EmptyRange",
            DeletionError::NotUnlocked => "Locked",
            DeletionError::KeychainNotCleared => "KeychainNotCleared",
            DeletionError::KeychainUnverifiable => "KeychainUnverifiable",
            _ => "Deletion",
        };
        Self {
            code,
            message: e.to_string(),
        }
    }
}

fn error(code: &'static str, message: &str) -> DeletionErrorView {
    DeletionErrorView {
        code,
        message: message.into(),
    }
}

type Result<T> = std::result::Result<T, DeletionErrorView>;

fn partial(delete_whole: bool) -> PartialFiles {
    if delete_whole {
        PartialFiles::DeleteWhole
    } else {
        PartialFiles::Keep
    }
}

impl AppUnlock {
    /// The accounts and original files the user can choose to delete.
    pub fn data_choices(&self) -> Result<DataChoices> {
        self.with_ledger(|open| {
            let ledger = open.ledger();
            let transactions = ledger.transactions().map_err(DeletionError::from)?;
            let count = |f: &dyn Fn(&fm_core::model::Transaction) -> bool| {
                transactions.iter().filter(|t| f(t)).count() as u64
            };
            let accounts = ledger
                .accounts()
                .map_err(DeletionError::from)?
                .into_iter()
                .map(|a| AccountChoice {
                    transactions: count(&|t| t.account_id == a.id),
                    id: a.id.0,
                    name: a.name,
                    institution: a.institution,
                })
                .collect();
            let files = ledger
                .raw_imports()
                .map_err(DeletionError::from)?
                .into_iter()
                .map(|r| FileChoice {
                    transactions: count(&|t| t.source.import_id == r.id),
                    id: r.id.0,
                    file_name: r.file_name,
                    imported_at: r.imported_at.unix_seconds(),
                })
                .collect();
            Ok(DataChoices { accounts, files })
        })
        .unwrap_or_else(|| Err(error("Locked", "unlock first")))
    }

    /// Exactly what deleting `target` would remove, without removing anything.
    pub fn preview_deletion(&self, target: TargetArg, delete_partial: bool) -> Result<PreviewView> {
        let target = target.parse()?;
        let preview = self
            .write_ledger(|open| {
                deletion::preview(open.ledger_mut(), target, partial(delete_partial))
            })
            .ok_or_else(|| error("Locked", "unlock first"))??;
        Ok(PreviewView {
            counts: preview.counts.into(),
            partial_files: preview.partial_files.into_iter().map(Into::into).collect(),
        })
    }

    /// Deletes `target` and everything derived from it. Keeps the backup offer for
    /// [`Self::delete_old_backups`].
    pub fn delete_data(&self, target: TargetArg, delete_partial: bool) -> Result<DeletedView> {
        let target = target.parse()?;
        let deleted = self
            .write_ledger(|open| {
                deletion::delete(open.ledger_mut(), target, partial(delete_partial))
            })
            .ok_or_else(|| error("Locked", "unlock first"))??;
        let view = deleted.backups.as_ref().map(OfferView::from);
        *self
            .backup_offer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = deleted.backups;
        Ok(DeletedView {
            counts: deleted.counts.into(),
            backup_offer: view,
        })
    }

    /// Deletes the whole ledger, keys first (decision 28). `confirmation` must be the
    /// phrase the user typed. `allow_unverified_keychain`: the user was told this build
    /// can't check the Keychain and chose to go ahead. Returns the offer to delete old
    /// backups, if any.
    pub fn delete_everything(
        &self,
        confirmation: &str,
        allow_unverified_keychain: bool,
    ) -> Result<Option<OfferView>> {
        if !confirmation.trim().eq_ignore_ascii_case(CONFIRMATION) {
            return Err(error(
                "ConfirmationMismatch",
                "type the confirmation phrase exactly",
            ));
        }
        let done = self.change(|s| {
            deletion::delete_everything(s, Now::current(), allow_unverified_keychain)
        })?;
        let view = done.backups.as_ref().map(OfferView::from);
        *self
            .backup_offer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = done.backups;
        Ok(view)
    }

    /// Accepts the offer the app made after a deletion in this run: deletes this
    /// ledger's backups in that folder, and nothing else.
    pub fn delete_old_backups(&self) -> Result<usize> {
        let offer = self
            .backup_offer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| error("NoOffer", "there are no backups to delete"))?;
        Ok(deletion::delete_backups(&offer)?)
    }

    /// The pending offer, e.g. to show it again after the page reloads.
    pub fn backup_offer(&self) -> Option<OfferView> {
        self.backup_offer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(OfferView::from)
    }
}

/// Tauri commands behind `app/src/routes/data/api.ts`. All but `delete_old_backups`
/// need an open ledger (the unlock gate refuses them otherwise).
pub mod commands {
    use tauri::State;

    use super::{DataChoices, DeletedView, OfferView, PreviewView, Result, TargetArg};
    use crate::unlock::AppUnlock;

    #[tauri::command]
    pub fn data_choices(unlock: State<'_, AppUnlock>) -> Result<DataChoices> {
        unlock.data_choices()
    }

    #[tauri::command(async)]
    pub fn preview_deletion(
        unlock: State<'_, AppUnlock>,
        target: TargetArg,
        delete_partial_files: bool,
    ) -> Result<PreviewView> {
        unlock.preview_deletion(target, delete_partial_files)
    }

    #[tauri::command(async)]
    pub fn delete_data(
        unlock: State<'_, AppUnlock>,
        target: TargetArg,
        delete_partial_files: bool,
    ) -> Result<DeletedView> {
        unlock.delete_data(target, delete_partial_files)
    }

    #[tauri::command(async)]
    pub fn delete_everything(
        unlock: State<'_, AppUnlock>,
        confirmation: String,
        allow_unverified_keychain: bool,
    ) -> Result<Option<OfferView>> {
        unlock.delete_everything(&confirmation, allow_unverified_keychain)
    }

    #[tauri::command(async)]
    pub fn delete_old_backups(unlock: State<'_, AppUnlock>) -> Result<usize> {
        unlock.delete_old_backups()
    }

    #[tauri::command]
    pub fn pending_backup_offer(unlock: State<'_, AppUnlock>) -> Option<OfferView> {
        unlock.backup_offer()
    }
}
