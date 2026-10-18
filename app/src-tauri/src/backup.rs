//! Backups in the app (T5.2): the settings screen's commands, restore on a fresh
//! install, and the daily backup run from the auto-lock timer. The logic is
//! `fm_core::backup`; `app/src/routes/backup/api.ts` mirrors these types.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::SystemTime;

use fm_core::backup::{self, BackupError, BackupInfo, Made};
use fm_core::storage::Unlocked;
use fm_core::unlock::Status;
use serde::Serialize;

use crate::unlock::AppUnlock;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupView {
    file_name: String,
    /// False for backups of another ledger sharing the folder (shown, never touched).
    mine: bool,
    /// Unix seconds, UTC.
    created_at: i64,
    bytes: u64,
}

impl BackupView {
    fn new(b: BackupInfo, my_id: &str) -> Self {
        Self {
            mine: b.ledger_id == my_id,
            file_name: b.file_name,
            created_at: b.created_unix,
            bytes: b.bytes,
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatus {
    pub(crate) folder: Option<String>,
    pub(crate) last_at: Option<i64>,
    pub(crate) last_error: Option<String>,
    /// The last backup was made, but something after it went wrong (e.g. an old backup
    /// couldn't be deleted).
    last_warning: Option<String>,
    /// Newest first, this ledger's and others'. Empty if no folder is set or it can't
    /// be read.
    backups: Vec<BackupView>,
    /// Set when the folder can't be read right now (e.g. the drive is unplugged).
    folder_error: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupErrorView {
    code: &'static str,
    message: String,
}

impl From<BackupError> for BackupErrorView {
    fn from(e: BackupError) -> Self {
        let code = match e {
            BackupError::NoFolder => "NoFolder",
            BackupError::FolderMissing(_) => "FolderMissing",
            BackupError::FolderNotAllowed(_) => "FolderNotAllowed",
            BackupError::NotABackup => "NotABackup",
            BackupError::UnsupportedVersion(_) => "UnsupportedVersion",
            BackupError::Truncated => "Truncated",
            BackupError::Corrupt => "Corrupt",
            BackupError::LedgerExists => "LedgerExists",
            BackupError::Interrupted => "Interrupted",
            _ => "Backup",
        };
        Self {
            code,
            message: e.to_string(),
        }
    }
}

type Result<T> = std::result::Result<T, BackupErrorView>;

fn made_view(made: Made) -> BackupView {
    let id = made.info.ledger_id.clone();
    BackupView::new(made.info, &id)
}

fn locked() -> BackupErrorView {
    BackupErrorView {
        code: "Locked",
        message: "unlock first".into(),
    }
}

impl AppUnlock {
    pub fn backup_status(&self) -> Result<BackupStatus> {
        self.with_ledger(|open| {
            let ledger = open.ledger();
            self.adopt_restored(ledger)?;
            let settings = backup::Settings::load(ledger)?;
            let id = backup::ledger_id(ledger)?;
            let (backups, folder_error) = match &settings.folder {
                None => (Vec::new(), None),
                Some(folder) => match backup::list(folder) {
                    Ok(list) => (
                        list.into_iter().map(|b| BackupView::new(b, &id)).collect(),
                        None,
                    ),
                    Err(_) => (
                        Vec::new(),
                        Some(BackupError::FolderMissing(folder.clone()).to_string()),
                    ),
                },
            };
            Ok(BackupStatus {
                folder: settings.folder.map(|f| f.to_string_lossy().into_owned()),
                last_at: settings.last_at,
                last_error: settings.last_error,
                last_warning: settings.last_warning,
                backups,
                folder_error,
            })
        })
        .unwrap_or_else(|| Err(locked()))
    }

    pub fn set_backup_folder(&self, folder: &Path) -> Result<()> {
        self.with_ledger(|open| backup::set_folder(open.ledger(), folder))
            .ok_or_else(locked)?
            .map_err(Into::into)
    }

    pub fn back_up_now(&self) -> Result<BackupView> {
        self.with_ledger(|open| {
            self.adopt_restored(open.ledger())?;
            let made = backup::back_up_now(open.ledger(), SystemTime::now())?;
            Ok(made_view(made))
        })
        .unwrap_or_else(|| Err(locked()))
    }

    /// The daily backup, run by the timer while the ledger is open. It stops at once if
    /// the app starts locking (it holds the session meanwhile). Failures are recorded
    /// in the settings for the screen to show, and back off; they never stop the app.
    pub fn daily_backup(&self) -> Option<std::result::Result<Made, BackupError>> {
        let cancel = || self.interrupt.load(Ordering::SeqCst);
        self.with_ledger(|open| {
            if let Err(e) = self.adopt_restored(open.ledger()) {
                return Some(Err(e));
            }
            backup::daily(open.ledger(), SystemTime::now(), &cancel)
        })
        .flatten()
    }

    /// After a restore in this run: the ledger takes a new backup id, so its backups and
    /// those of the install it came from (which may still be running and backing up to
    /// the same folder) never rotate each other away.
    fn adopt_restored(&self, ledger: &Unlocked) -> std::result::Result<(), BackupError> {
        if self.restored.swap(false, Ordering::SeqCst)
            && let Err(e) = backup::reassign_ledger_id(ledger)
        {
            self.restored.store(true, Ordering::SeqCst);
            return Err(e);
        }
        Ok(())
    }

    /// Restores `file` as this install's ledger, only while there is none (status
    /// `NoLedger`); otherwise refused, and nothing on disk is touched. Afterwards the
    /// ledger is locked and opens with the password or recovery key that were current
    /// when the backup was made.
    pub fn restore_backup(&self, file: &Path) -> Result<()> {
        self.change(|session| {
            if session.status() != Status::NoLedger {
                return Err(BackupError::LedgerExists.into());
            }
            backup::restore(file, session.store().vault())?;
            self.restored.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

/// Tauri commands behind `app/src/routes/backup/api.ts`. All but `restore_backup` need
/// an open ledger (the unlock gate refuses them otherwise).
pub mod commands {
    use tauri::State;

    use super::{BackupStatus, BackupView, Result, as_path};
    use crate::unlock::AppUnlock;

    #[tauri::command]
    pub fn backup_status(unlock: State<'_, AppUnlock>) -> Result<BackupStatus> {
        unlock.backup_status()
    }

    #[tauri::command]
    pub fn set_backup_folder(unlock: State<'_, AppUnlock>, folder: String) -> Result<()> {
        unlock.set_backup_folder(&as_path(&folder))
    }

    #[tauri::command(async)]
    pub fn backup_now(unlock: State<'_, AppUnlock>) -> Result<BackupView> {
        unlock.back_up_now()
    }

    #[tauri::command(async)]
    pub fn restore_backup(unlock: State<'_, AppUnlock>, file: String) -> Result<()> {
        unlock.restore_backup(&as_path(&file))
    }
}

/// A path typed by the user: surrounding spaces and quotes (from "Copy as path") are
/// removed, and `~/` means the home folder.
fn as_path(typed: &str) -> PathBuf {
    let trimmed = typed.trim().trim_matches(|c| c == '"' || c == '\'');
    match (trimmed.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(trimmed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_paths_are_tidied() {
        assert_eq!(
            as_path("  /Volumes/Backup Drive  "),
            PathBuf::from("/Volumes/Backup Drive")
        );
        assert_eq!(as_path("\"/tmp/a b\""), PathBuf::from("/tmp/a b"));
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(as_path("~/Backups"), PathBuf::from(home).join("Backups"));
        }
        assert_eq!(as_path("relative"), PathBuf::from("relative"));
    }
}
