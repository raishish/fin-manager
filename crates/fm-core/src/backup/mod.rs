//! Backups (SPEC §8, decisions 17 and 28): encrypted copies of the ledger in a folder
//! the user chooses, made daily and after each import, kept 7 daily / 4 weekly /
//! 12 monthly, and restorable on a fresh install with the password or the recovery
//! key. See `README.md`.

mod format;
mod name;
mod rotation;

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;

use crate::storage::{StorageError, Unlocked, Vault, VaultState};

pub use rotation::{DAILY, MONTHLY, WEEKLY};

/// Automatic backups run when the last one is at least this old.
pub const DAILY_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// A temporary file older than this was left by a backup that never finished.
const STALE_TMP: Duration = Duration::from_secs(10 * 60);

const SETTING_FOLDER: &str = "backup.folder";
const SETTING_LAST_AT: &str = "backup.last_at";
const SETTING_LAST_ERROR: &str = "backup.last_error";

/// Why a backup or restore failed. Messages hold paths at most, never ledger contents.
#[derive(Debug)]
#[non_exhaustive]
pub enum BackupError {
    /// No backup folder has been chosen.
    NoFolder,
    /// The folder doesn't exist or isn't a folder (e.g. an external drive is unplugged).
    FolderMissing(PathBuf),
    /// The folder must be an absolute path outside the ledger's own folder.
    FolderNotAllowed(PathBuf),
    /// The file isn't a fin-manager backup.
    NotABackup,
    /// A backup from a newer version of the app.
    UnsupportedVersion(u16),
    /// The file is shorter than it says: an incomplete copy.
    Truncated,
    /// The file is damaged.
    Corrupt,
    /// Restoring is only possible where there is no ledger yet.
    LedgerExists,
    Storage(StorageError),
    Io(io::Error),
}

impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoFolder => f.write_str("no backup folder is set"),
            Self::FolderMissing(p) => {
                write!(f, "the backup folder {} isn't available", p.display())
            }
            Self::FolderNotAllowed(p) => write!(
                f,
                "{} can't be used: choose a full path outside the app's own data folder",
                p.display()
            ),
            Self::NotABackup => f.write_str("this file isn't a fin-manager backup"),
            Self::UnsupportedVersion(v) => {
                write!(
                    f,
                    "this backup was made by a newer version of the app (format {v})"
                )
            }
            Self::Truncated => f.write_str("this backup is incomplete (the copy was cut short)"),
            Self::Corrupt => f.write_str("this backup is damaged"),
            Self::LedgerExists => {
                f.write_str("there is already data here; restore only works on a fresh install")
            }
            Self::Storage(e) => e.fmt(f),
            Self::Io(e) => write!(f, "file error: {e}"),
        }
    }
}

impl std::error::Error for BackupError {}

impl From<io::Error> for BackupError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<StorageError> for BackupError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

impl From<rusqlite::Error> for BackupError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(StorageError::Database(e))
    }
}

/// One backup file in the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub file_name: String,
    /// When it was made, from its name (Unix seconds, UTC).
    pub created_unix: i64,
    pub bytes: u64,
}

// ---- making backups -----------------------------------------------------------------

/// Backs up `ledger` into `folder` now, verifies the copy, and applies the rotation.
///
/// The ledger is held still while it is copied: `BEGIN IMMEDIATE` blocks any write to
/// the database, and the keyfile can only change through `&mut Unlocked`, which can't
/// coexist with the `&Unlocked` held here. The copy is streamed, never held in memory.
pub fn back_up(
    ledger: &Unlocked,
    folder: &Path,
    now: SystemTime,
) -> Result<BackupInfo, BackupError> {
    check_folder(ledger.vault(), folder)?;
    remove_stale_tmp(folder, now);
    let created_unix = unix(now);

    let (tmp_path, final_path) = free_names(folder, created_unix)?;
    let written = write_backup(ledger, &tmp_path, created_unix);
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    fs::rename(&tmp_path, &final_path)?;
    sync_dir(folder);

    // Read it back before it counts: cloud-synced folders can fail without an error.
    let bytes = fs::metadata(&final_path)?.len();
    if let Err(e) = format::verify(&mut BufReader::new(File::open(&final_path)?), bytes) {
        let _ = fs::remove_file(&final_path);
        return Err(e);
    }
    rotate(folder)?;
    Ok(BackupInfo {
        file_name: file_name(&final_path),
        path: final_path,
        created_unix,
        bytes,
    })
}

fn write_backup(ledger: &Unlocked, tmp_path: &Path, created_unix: i64) -> Result<(), BackupError> {
    let vault = ledger.vault();
    let conn = ledger.connection();
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let copied = (|| {
        let mut keyfile = File::open(vault.keyfile_path())?;
        let mut db = File::open(vault.db_path())?;
        let header = format::Header {
            created_unix,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            keyfile_len: keyfile.metadata()?.len(),
            db_len: db.metadata()?.len(),
        };
        let file = create_new_private(tmp_path)?;
        let mut out = BufWriter::new(file);
        format::write(&mut out, &header, &mut keyfile, &mut db)?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        Ok(())
    })();
    // A read-only transaction: nothing to commit.
    let _ = conn.execute_batch("ROLLBACK");
    copied
}

/// A backup name (and its temporary name) not yet used in `folder`.
fn free_names(folder: &Path, created_unix: i64) -> Result<(PathBuf, PathBuf), BackupError> {
    for n in 0..100 {
        let tmp = folder.join(name::tmp_name(created_unix, n));
        let fin = folder.join(name::backup_name(created_unix, n));
        if !tmp.exists() && !fin.exists() {
            return Ok((tmp, fin));
        }
    }
    Err(BackupError::Io(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "too many backups in the same second",
    )))
}

fn check_folder(vault: &Vault, folder: &Path) -> Result<(), BackupError> {
    if !folder.is_absolute() {
        return Err(BackupError::FolderNotAllowed(folder.to_path_buf()));
    }
    if !folder.is_dir() {
        return Err(BackupError::FolderMissing(folder.to_path_buf()));
    }
    // Not inside the ledger's own folder (a backup next to the data it backs up is no
    // backup). A folder that merely contains it, like the home folder, is fine: only
    // files named like backups are ever touched.
    let folder = fs::canonicalize(folder)?;
    if let Ok(ledger_dir) = fs::canonicalize(vault.dir())
        && folder.starts_with(&ledger_dir)
    {
        return Err(BackupError::FolderNotAllowed(folder));
    }
    Ok(())
}

// ---- listing, rotation, deletion ------------------------------------------------------

/// The backups in `folder`, newest first. Only files named like ours count; temporary
/// files and everything else are ignored.
pub fn list(folder: &Path) -> Result<Vec<BackupInfo>, BackupError> {
    let mut out = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let Some(created_unix) = name::parse_backup_name(&file_name) else {
            continue;
        };
        let meta = entry.metadata()?;
        if !meta.is_file() {
            continue;
        }
        out.push(BackupInfo {
            path: entry.path(),
            file_name,
            created_unix,
            bytes: meta.len(),
        });
    }
    out.sort_by(|a, b| {
        b.created_unix
            .cmp(&a.created_unix)
            .then(b.file_name.cmp(&a.file_name))
    });
    Ok(out)
}

/// Deletes the backups the rotation doesn't keep; returns what was deleted.
pub fn rotate(folder: &Path) -> Result<Vec<PathBuf>, BackupError> {
    let backups = list(folder)?;
    let times: Vec<i64> = backups.iter().map(|b| b.created_unix).collect();
    let keep = rotation::keep(&times);
    let mut deleted = Vec::new();
    for (i, backup) in backups.into_iter().enumerate() {
        if !keep.contains(&i) {
            fs::remove_file(&backup.path)?;
            deleted.push(backup.path);
        }
    }
    if !deleted.is_empty() {
        sync_dir(folder);
    }
    Ok(deleted)
}

/// Deletes every backup of ours in `folder` (decision 28: after the user deletes data,
/// offered so old backups don't keep it). Returns how many were deleted. Other files
/// are left alone. The files are unlinked, not overwritten: on SSDs and copy-on-write
/// file systems an overwrite wouldn't reach the old blocks anyway, and they are
/// encrypted.
pub fn delete_all(folder: &Path) -> Result<usize, BackupError> {
    let backups = list(folder)?;
    for backup in &backups {
        fs::remove_file(&backup.path)?;
    }
    sync_dir(folder);
    Ok(backups.len())
}

/// Removes temporary files left by backups that never finished (the app was killed,
/// the drive was pulled).
fn remove_stale_tmp(folder: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        if !name::is_tmp_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|m| now.duration_since(m).is_ok_and(|age| age >= STALE_TMP));
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

// ---- restore --------------------------------------------------------------------------

/// Restores `backup` as the ledger of `vault`, which must not exist yet (a fresh
/// install). The files are written into a private staging folder next to it, checked
/// against the backup's hash, and the folder is renamed into place in one step, so an
/// interrupted restore leaves nothing half done and nothing existing is ever
/// overwritten. Afterwards the ledger opens as usual with the password or the
/// recovery key that were current when the backup was made.
pub fn restore(backup: &Path, vault: &Vault) -> Result<(), BackupError> {
    if vault.state() != VaultState::Missing || !dir_is_replaceable(vault.dir())? {
        return Err(BackupError::LedgerExists);
    }
    let target = vault.dir();
    let parent = target.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(
        ".{}.restoring-{}",
        target
            .file_name()
            .map_or("ledger".into(), |n| n.to_string_lossy()),
        unix(SystemTime::now())
    ));
    create_private_dir(&staging)?;
    let result = (|| {
        let keyfile_name = vault
            .keyfile_path()
            .file_name()
            .expect("keyfile has a name");
        let db_name = vault.db_path().file_name().expect("database has a name");
        let source = File::open(backup)?;
        let total = source.metadata()?.len();
        let mut keyfile = BufWriter::new(create_new_private(&staging.join(keyfile_name))?);
        let mut db = BufWriter::new(create_new_private(&staging.join(db_name))?);
        format::read(&mut BufReader::new(source), total, &mut keyfile, &mut db)?;
        for out in [keyfile, db] {
            out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        }
        sync_dir(&staging);
        // Only an empty folder (or one holding just the lock file) can be here.
        if target.exists() {
            let _ = fs::remove_file(target.join("ledger.lock"));
            fs::remove_dir(target)?;
        }
        fs::rename(&staging, target)?;
        sync_dir(parent);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// A ledger folder that is absent, empty, or holds only the advisory lock file.
fn dir_is_replaceable(dir: &Path) -> Result<bool, BackupError> {
    match fs::read_dir(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e.into()),
        Ok(entries) => {
            for entry in entries {
                if entry?.file_name() != "ledger.lock" {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}

// ---- settings and schedule ------------------------------------------------------------

/// Backup settings, stored in the ledger itself (`meta` table, `backup.*`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub folder: Option<PathBuf>,
    /// Unix seconds of the last successful backup.
    pub last_at: Option<i64>,
    pub last_error: Option<String>,
}

impl Settings {
    pub fn load(ledger: &Unlocked) -> Result<Self, BackupError> {
        Ok(Self {
            folder: get(ledger, SETTING_FOLDER)?.map(PathBuf::from),
            last_at: get(ledger, SETTING_LAST_AT)?.and_then(|s| s.parse().ok()),
            last_error: get(ledger, SETTING_LAST_ERROR)?,
        })
    }
}

/// Chooses the backup folder: an existing, absolute folder outside the ledger's own.
pub fn set_folder(ledger: &Unlocked, folder: &Path) -> Result<(), BackupError> {
    check_folder(ledger.vault(), folder)?;
    let folder = fs::canonicalize(folder)?;
    set(ledger, SETTING_FOLDER, Some(&folder.to_string_lossy()))
}

/// True when an automatic backup is due: never made, a day old, or the clock moved
/// back past the last one.
pub fn due(settings: &Settings, now: SystemTime) -> bool {
    match settings.last_at {
        None => true,
        Some(last) => {
            let now = unix(now);
            now < last || now - last >= DAILY_INTERVAL.as_secs() as i64
        }
    }
}

/// The daily backup: backs up if a folder is set and a backup is due. Records the
/// outcome in the settings. `None` if nothing was due.
pub fn daily(ledger: &Unlocked, now: SystemTime) -> Option<Result<BackupInfo, BackupError>> {
    let settings = Settings::load(ledger).ok()?;
    settings.folder.as_ref()?;
    if !due(&settings, now) {
        return None;
    }
    Some(back_up_and_record(ledger, &settings, now))
}

/// The backup after an import (T1.3 calls this): always, if a folder is set.
pub fn after_import(ledger: &Unlocked, now: SystemTime) -> Option<Result<BackupInfo, BackupError>> {
    let settings = Settings::load(ledger).ok()?;
    settings.folder.as_ref()?;
    Some(back_up_and_record(ledger, &settings, now))
}

/// "Back up now" from the settings screen.
pub fn back_up_now(ledger: &Unlocked, now: SystemTime) -> Result<BackupInfo, BackupError> {
    let settings = Settings::load(ledger)?;
    if settings.folder.is_none() {
        return Err(BackupError::NoFolder);
    }
    back_up_and_record(ledger, &settings, now)
}

fn back_up_and_record(
    ledger: &Unlocked,
    settings: &Settings,
    now: SystemTime,
) -> Result<BackupInfo, BackupError> {
    let folder = settings.folder.as_deref().ok_or(BackupError::NoFolder)?;
    let result = back_up(ledger, folder, now);
    let recorded = match &result {
        Ok(info) => set(
            ledger,
            SETTING_LAST_AT,
            Some(&info.created_unix.to_string()),
        )
        .and_then(|()| set(ledger, SETTING_LAST_ERROR, None)),
        Err(e) => set(ledger, SETTING_LAST_ERROR, Some(&e.to_string())),
    };
    result.and_then(|info| recorded.map(|()| info))
}

fn get(ledger: &Unlocked, key: &str) -> Result<Option<String>, BackupError> {
    Ok(ledger
        .connection()
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

fn set(ledger: &Unlocked, key: &str, value: Option<&str>) -> Result<(), BackupError> {
    let conn = ledger.connection();
    match value {
        Some(v) => conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [key, v],
        ),
        None => conn.execute("DELETE FROM meta WHERE key = ?1", [key]),
    }?;
    Ok(())
}

// ---- file helpers -----------------------------------------------------------------------

fn unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A new file only the owner can read (0600), failing if it exists.
fn create_new_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// A new folder only the owner can enter (0700).
fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Makes renames and deletions in `dir` durable. Best effort: not every file system
/// supports it.
fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(test)]
mod tests;
