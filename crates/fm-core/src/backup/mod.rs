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
/// After a failed attempt, automatic backups wait this long before trying again, so
/// a broken folder isn't hammered every few seconds.
pub const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// A temporary file or restore folder older than this was left by a run that never
/// finished.
const STALE: Duration = Duration::from_secs(10 * 60);

const SETTING_FOLDER: &str = "backup.folder";
const SETTING_LEDGER_ID: &str = "backup.ledger_id";
const SETTING_LAST_AT: &str = "backup.last_at";
const SETTING_LAST_ATTEMPT: &str = "backup.last_attempt";
const SETTING_LAST_ERROR: &str = "backup.last_error";
const SETTING_LAST_WARNING: &str = "backup.last_warning";

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
    /// Stopped because the ledger is being locked; nothing was left behind.
    Interrupted,
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
            Self::UnsupportedVersion(v) => write!(
                f,
                "this backup was made by a newer version of the app (format {v})"
            ),
            Self::Truncated => f.write_str("this backup is incomplete (the copy was cut short)"),
            Self::Corrupt => f.write_str("this backup is damaged"),
            Self::LedgerExists => {
                f.write_str("there is already data here; restore only works on a fresh install")
            }
            Self::Interrupted => f.write_str("the backup stopped because the app locked"),
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

/// One backup file in a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub file_name: String,
    /// Which ledger it belongs to (see `name.rs`).
    pub ledger_id: String,
    /// When it was made, from its name (Unix seconds, UTC).
    pub created_unix: i64,
    pub bytes: u64,
}

/// A backup that was made and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    pub info: BackupInfo,
    /// Something went wrong after the backup was safely made, e.g. an old backup
    /// couldn't be deleted by the rotation. Worth showing, not worth retrying.
    pub warning: Option<String>,
}

/// Never cancels.
fn never() -> bool {
    false
}

// ---- making backups -----------------------------------------------------------------

/// Backs up `ledger` into `folder` now, verifies the copy, and applies the rotation.
pub fn back_up(ledger: &Unlocked, folder: &Path, now: SystemTime) -> Result<Made, BackupError> {
    back_up_cancellable(ledger, folder, now, &never)
}

/// [`back_up`], stopping with [`BackupError::Interrupted`] (and leaving nothing behind)
/// as soon as `cancel` returns true, e.g. because the user locks the app.
///
/// The ledger is held still while it is copied: `BEGIN IMMEDIATE` blocks any write to
/// the database, and the keyfile can only change through `&mut Unlocked`, which can't
/// coexist with the `&Unlocked` held here. The copy is streamed, never held in memory.
/// A failure of the rotation afterwards doesn't fail the backup; it comes back as a
/// warning.
pub fn back_up_cancellable(
    ledger: &Unlocked,
    folder: &Path,
    now: SystemTime,
    cancel: &dyn Fn() -> bool,
) -> Result<Made, BackupError> {
    check_folder(ledger.vault(), folder)?;
    let id = ledger_id(ledger)?;
    remove_stale_tmp(folder, now);
    let created_unix = unix(now);

    let (tmp_path, final_path) = free_names(folder, &id, created_unix)?;
    if let Err(e) = write_backup(ledger, &tmp_path, created_unix, cancel) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    let moved = move_no_replace(&tmp_path, &final_path);
    if let Err(e) = moved {
        let _ = fs::remove_file(&tmp_path);
        return Err(e.into());
    }
    sync_dir(folder);

    // Read it back before it counts: cloud-synced folders can fail without an error.
    let bytes = fs::metadata(&final_path)?.len();
    let checked = File::open(&final_path)
        .map_err(BackupError::from)
        .and_then(|f| format::verify(&mut BufReader::new(f), bytes, cancel));
    if let Err(e) = checked {
        let _ = fs::remove_file(&final_path);
        return Err(e);
    }

    let warning = rotate(folder, &id)
        .err()
        .map(|e| format!("an old backup couldn't be removed: {e}"));
    Ok(Made {
        info: BackupInfo {
            file_name: file_name(&final_path),
            path: final_path,
            ledger_id: id,
            created_unix,
            bytes,
        },
        warning,
    })
}

fn write_backup(
    ledger: &Unlocked,
    tmp_path: &Path,
    created_unix: i64,
    cancel: &dyn Fn() -> bool,
) -> Result<(), BackupError> {
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
        format::write(&mut out, &header, &mut keyfile, &mut db, cancel)?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        Ok(())
    })();
    // A read-only transaction: nothing to commit.
    let _ = conn.execute_batch("ROLLBACK");
    copied
}

/// A backup name (and its temporary name) not yet used in `folder`.
fn free_names(
    folder: &Path,
    id: &str,
    created_unix: i64,
) -> Result<(PathBuf, PathBuf), BackupError> {
    for n in 0..100 {
        let tmp = folder.join(name::tmp_name(id, created_unix, n));
        let fin = folder.join(name::backup_name(id, created_unix, n));
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

/// This ledger's id for backup names, made on first use and kept in `meta`.
pub fn ledger_id(ledger: &Unlocked) -> Result<String, BackupError> {
    if let Some(id) = get(ledger, SETTING_LEDGER_ID)?
        && name::is_ledger_id(&id)
    {
        return Ok(id);
    }
    reassign_ledger_id(ledger)
}

/// Gives the ledger a new id, so its backups are kept apart from those of the copy it
/// was restored from (which may still be running elsewhere and backing up to the same
/// folder). Backups under the old id are left alone.
pub fn reassign_ledger_id(ledger: &Unlocked) -> Result<String, BackupError> {
    let mut bytes = [0u8; name::ID_LEN / 2];
    getrandom::fill(&mut bytes).map_err(StorageError::from)?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    set(ledger, SETTING_LEDGER_ID, Some(&id))?;
    Ok(id)
}

// ---- listing, rotation, deletion ------------------------------------------------------

/// Every backup in `folder`, of any ledger, newest first. Only files named like ours
/// count; temporary files and everything else are ignored.
pub fn list(folder: &Path) -> Result<Vec<BackupInfo>, BackupError> {
    let mut out = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let Some(parsed) = name::parse_backup_name(&file_name) else {
            continue;
        };
        // Not following symlinks: only real files count.
        let meta = entry.metadata()?;
        if !meta.is_file() {
            continue;
        }
        out.push(BackupInfo {
            path: entry.path(),
            file_name,
            ledger_id: parsed.ledger_id,
            created_unix: parsed.unix_secs,
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

/// The backups of ledger `id` in `folder`, newest first.
pub fn list_mine(folder: &Path, id: &str) -> Result<Vec<BackupInfo>, BackupError> {
    let mut all = list(folder)?;
    all.retain(|b| b.ledger_id == id);
    Ok(all)
}

/// Deletes the backups of ledger `id` that the rotation doesn't keep; returns what was
/// deleted. Backups of other ledgers are never touched. Keeps going past a file it
/// can't delete, then reports the first error.
pub fn rotate(folder: &Path, id: &str) -> Result<Vec<PathBuf>, BackupError> {
    let backups = list_mine(folder, id)?;
    let times: Vec<i64> = backups.iter().map(|b| b.created_unix).collect();
    let keep = rotation::keep(&times);
    let mut deleted = Vec::new();
    let mut first_error = None;
    for (i, backup) in backups.into_iter().enumerate() {
        if keep.contains(&i) {
            continue;
        }
        match remove_backup_file(&backup.path) {
            Ok(()) => deleted.push(backup.path),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    if !deleted.is_empty() {
        sync_dir(folder);
    }
    match first_error {
        Some(e) => Err(e.into()),
        None => Ok(deleted),
    }
}

/// Deletes every backup of ledger `id` in `folder` (decision 28: after the user deletes
/// data, offered so old backups don't keep it). Returns how many were deleted. Backups
/// of other ledgers and other files are left alone. The files are unlinked, not
/// overwritten: on SSDs and copy-on-write file systems an overwrite wouldn't reach the
/// old blocks anyway, and they are encrypted.
pub fn delete_all(folder: &Path, id: &str) -> Result<usize, BackupError> {
    let backups = list_mine(folder, id)?;
    for backup in &backups {
        remove_backup_file(&backup.path)?;
    }
    sync_dir(folder);
    Ok(backups.len())
}

#[cfg(test)]
thread_local! {
    /// Tests: make deleting a backup file fail, as on a share that allows creating but
    /// not deleting.
    static FAIL_REMOVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn remove_backup_file(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    if FAIL_REMOVE.with(std::cell::Cell::get) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "test: can't delete",
        ));
    }
    fs::remove_file(path)
}

/// Removes temporary files left by backups that never finished (the app was killed,
/// the drive was pulled).
fn remove_stale_tmp(folder: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        if name::is_tmp_name(&entry.file_name().to_string_lossy()) && is_stale(&entry, now) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn is_stale(entry: &fs::DirEntry, now: SystemTime) -> bool {
    entry
        .metadata()
        .and_then(|m| m.modified())
        .is_ok_and(|m| now.duration_since(m).is_ok_and(|age| age >= STALE))
}

// ---- restore --------------------------------------------------------------------------

/// Restores `backup` as the ledger of `vault`, which must not exist yet (a fresh
/// install). Afterwards the ledger opens as usual with the password or the recovery
/// key that were current when the backup was made.
///
/// 1. The backup is streamed into a private staging folder next to the ledger's and
///    checked against its hash.
/// 2. The ledger lock is taken, so no other instance can create or open a ledger
///    meanwhile, and the target is checked again while holding it.
/// 3. The database and then the keyfile are moved in without replacing anything.
///
/// A failed or interrupted restore leaves no ledger behind, and nothing that exists is
/// ever overwritten or deleted (including the lock file).
pub fn restore(backup: &Path, vault: &Vault) -> Result<(), BackupError> {
    if !is_fresh(vault)? {
        return Err(BackupError::LedgerExists);
    }
    let target = vault.dir();
    let parent = target.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let staging_prefix = format!(
        ".{}.restoring-",
        target
            .file_name()
            .map_or("ledger".into(), |n| n.to_string_lossy())
    );
    remove_stale_restoring(parent, &staging_prefix, SystemTime::now());

    let source = open_backup(backup)?;
    let staging = parent.join(format!("{staging_prefix}{}", unix(SystemTime::now())));
    create_private_dir(&staging)?;
    let result = restore_via(source, &staging, vault);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn restore_via(source: File, staging: &Path, vault: &Vault) -> Result<(), BackupError> {
    let keyfile_name = vault
        .keyfile_path()
        .file_name()
        .expect("keyfile has a name");
    let db_name = vault.db_path().file_name().expect("database has a name");
    let (staged_keyfile, staged_db) = (staging.join(keyfile_name), staging.join(db_name));

    let total = source.metadata()?.len();
    let mut keyfile = BufWriter::new(create_new_private(&staged_keyfile)?);
    let mut db = BufWriter::new(create_new_private(&staged_db)?);
    format::read(
        &mut BufReader::new(source),
        total,
        &mut keyfile,
        &mut db,
        &never,
    )?;
    for out in [keyfile, db] {
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    }

    let _lock = vault.lock_for_restore()?;
    if !is_fresh(vault)? {
        return Err(BackupError::LedgerExists);
    }
    // Database first, keyfile last: until the keyfile is in, there is no ledger.
    move_no_replace(&staged_db, vault.db_path())?;
    if let Err(e) = move_no_replace(&staged_keyfile, vault.keyfile_path()) {
        // Ours, and useless without its keyfile; we still hold the lock.
        let _ = fs::remove_file(vault.db_path());
        return Err(e.into());
    }
    sync_dir(vault.dir());
    Ok(())
}

/// No ledger yet: the folder is absent, empty, or holds only the advisory lock file.
fn is_fresh(vault: &Vault) -> Result<bool, BackupError> {
    if vault.state() != VaultState::Missing {
        return Ok(false);
    }
    match fs::read_dir(vault.dir()) {
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

/// Opens a backup for reading without hanging on a FIFO or a device: non-blocking,
/// and only a regular file is accepted.
fn open_backup(path: &Path) -> Result<File, BackupError> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags};
        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        File::from(fd)
    };
    #[cfg(not(unix))]
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(BackupError::NotABackup);
    }
    Ok(file)
}

/// Removes restore staging folders left by restores that never finished.
fn remove_stale_restoring(parent: &Path, prefix: &str, now: SystemTime) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let is_ours = entry.file_name().to_string_lossy().starts_with(prefix)
            && entry.file_type().is_ok_and(|t| t.is_dir());
        if is_ours && is_stale(&entry, now) {
            let _ = fs::remove_dir_all(entry.path());
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
    /// Unix seconds of the last attempt, successful or not.
    pub last_attempt: Option<i64>,
    pub last_error: Option<String>,
    pub last_warning: Option<String>,
}

impl Settings {
    pub fn load(ledger: &Unlocked) -> Result<Self, BackupError> {
        let number = |key| -> Result<Option<i64>, BackupError> {
            Ok(get(ledger, key)?.and_then(|s| s.parse().ok()))
        };
        Ok(Self {
            folder: get(ledger, SETTING_FOLDER)?.map(PathBuf::from),
            last_at: number(SETTING_LAST_AT)?,
            last_attempt: number(SETTING_LAST_ATTEMPT)?,
            last_error: get(ledger, SETTING_LAST_ERROR)?,
            last_warning: get(ledger, SETTING_LAST_WARNING)?,
        })
    }
}

/// Chooses the backup folder: an existing, absolute folder outside the ledger's own.
pub fn set_folder(ledger: &Unlocked, folder: &Path) -> Result<(), BackupError> {
    check_folder(ledger.vault(), folder)?;
    let folder = fs::canonicalize(folder)?;
    set(ledger, SETTING_FOLDER, Some(&folder.to_string_lossy()))?;
    // A new folder gets a fresh start rather than waiting out an old failure.
    set(ledger, SETTING_LAST_ATTEMPT, None)?;
    set(ledger, SETTING_LAST_ERROR, None)
}

/// True when an automatic backup is due: never made, or a day old, or the clock moved
/// back past the last one. After a failed attempt, not again for [`RETRY_AFTER`].
pub fn due(settings: &Settings, now: SystemTime) -> bool {
    let now = unix(now);
    if let Some(attempt) = settings.last_attempt {
        let failed = settings.last_at.is_none_or(|ok| attempt > ok);
        if failed && now >= attempt && now - attempt < secs(RETRY_AFTER) {
            return false;
        }
    }
    match settings.last_at {
        None => true,
        Some(last) => now < last || now - last >= secs(DAILY_INTERVAL),
    }
}

/// The daily backup: backs up if a folder is set and a backup is due. Records the
/// outcome in the settings. `None` if nothing was due.
pub fn daily(
    ledger: &Unlocked,
    now: SystemTime,
    cancel: &dyn Fn() -> bool,
) -> Option<Result<Made, BackupError>> {
    let settings = Settings::load(ledger).ok()?;
    settings.folder.as_ref()?;
    if !due(&settings, now) {
        return None;
    }
    Some(back_up_and_record(ledger, &settings, now, cancel))
}

/// The backup after an import (T1.3 calls this): always, if a folder is set.
pub fn after_import(ledger: &Unlocked, now: SystemTime) -> Option<Result<Made, BackupError>> {
    let settings = Settings::load(ledger).ok()?;
    settings.folder.as_ref()?;
    Some(back_up_and_record(ledger, &settings, now, &never))
}

/// "Back up now" from the settings screen.
pub fn back_up_now(ledger: &Unlocked, now: SystemTime) -> Result<Made, BackupError> {
    let settings = Settings::load(ledger)?;
    if settings.folder.is_none() {
        return Err(BackupError::NoFolder);
    }
    back_up_and_record(ledger, &settings, now, &never)
}

fn back_up_and_record(
    ledger: &Unlocked,
    settings: &Settings,
    now: SystemTime,
    cancel: &dyn Fn() -> bool,
) -> Result<Made, BackupError> {
    let folder = settings.folder.as_deref().ok_or(BackupError::NoFolder)?;
    let result = back_up_cancellable(ledger, folder, now, cancel);
    let attempt = unix(now).to_string();
    let recorded = match &result {
        Ok(made) => set(
            ledger,
            SETTING_LAST_AT,
            Some(&made.info.created_unix.to_string()),
        )
        .and_then(|()| set(ledger, SETTING_LAST_ATTEMPT, Some(&attempt)))
        .and_then(|()| set(ledger, SETTING_LAST_ERROR, None))
        .and_then(|()| set(ledger, SETTING_LAST_WARNING, made.warning.as_deref())),
        // Locking interrupted it: not a failure, try again after the next unlock.
        Err(BackupError::Interrupted) => Ok(()),
        Err(e) => set(ledger, SETTING_LAST_ATTEMPT, Some(&attempt))
            .and_then(|()| set(ledger, SETTING_LAST_ERROR, Some(&e.to_string()))),
    };
    result.and_then(|made| recorded.map(|()| made))
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

fn secs(d: Duration) -> i64 {
    i64::try_from(d.as_secs()).unwrap_or(i64::MAX)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Moves `from` to `to`, failing with `AlreadyExists` rather than replacing a file
/// that is already there. A hard link plus unlink is atomic and never replaces; on file
/// systems without hard links (exFAT drives) it falls back to checking first and
/// renaming, which leaves a tiny window but is the best they offer.
fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    match fs::hard_link(from, to) {
        Ok(()) => fs::remove_file(from),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => {
            if fs::symlink_metadata(to).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "the target already exists",
                ));
            }
            fs::rename(from, to)
        }
    }
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
