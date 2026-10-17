//! Encrypted storage (SPEC decision 16): the ledger lives in a SQLCipher database whose
//! random data key is wrapped, in a separate keyfile, by a key derived from the master
//! password with Argon2id (and, once T5.1 sets one up, by the recovery key).
//!
//! A ledger is a directory holding `ledger.db` and `ledger.keys`. [`Vault`] is a locked
//! ledger; [`Vault::unlock`] gives an [`Unlocked`] one holding the open connection and
//! the data key; [`Unlocked::lock`] (or dropping it) closes the connection and wipes the
//! key. See `README.md` for the design and its known gaps.

mod db;
mod error;
mod keyfile;
mod repo;

use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use zeroize::Zeroize;

pub use db::{MIGRATIONS, Migration};
pub use error::StorageError;
pub use keyfile::{KdfParams, SlotKind};
pub use repo::{DeletedCounts, Table, default_taxonomy};

use keyfile::{DataKey, Keyfile};

const DB_FILE: &str = "ledger.db";
const KEYFILE: &str = "ledger.keys";
/// The new keyfile while the data key is being rotated (see [`Unlocked::rotate_data_key`]).
const NEXT_KEYFILE: &str = "ledger.keys.next";
/// Advisory lock held while the ledger is open, so two app instances can't both write.
const LOCK_FILE: &str = "ledger.lock";

/// Owner-only permissions (SPEC §7: other users on the same machine).
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;

/// What is on disk at a ledger's location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultState {
    /// Nothing yet: [`Vault::create`] can make one.
    Missing,
    /// A keyfile and a database: [`Vault::unlock`] opens it.
    Ready,
    /// A keyfile but no database (or an empty one): creation was interrupted.
    /// [`Vault::finish_create`] completes it.
    Incomplete,
    /// A database without its keyfile. It can never be opened again (this is also
    /// what [`Vault::shred_keys`] leaves behind).
    Orphaned,
}

/// A ledger on disk, locked. Holds only paths, never key material.
///
/// The raw-key hooks T5.1 uses for Touch ID stay inside fm-core:
///
/// ```compile_fail
/// let vault = fm_core::storage::Vault::at(std::path::Path::new("ledger"));
/// let _ = vault.unlock_with_data_key(&[0u8; 32]); // crate-private
/// ```
///
/// ```compile_fail
/// fn leak(u: &fm_core::storage::Unlocked) -> [u8; 32] {
///     *u.data_key_copy() // crate-private
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vault {
    dir: PathBuf,
    db_path: PathBuf,
    keyfile_path: PathBuf,
    next_keyfile_path: PathBuf,
    lock_path: PathBuf,
}

/// Holds the ledger's advisory lock until dropped (the OS releases it when the file
/// closes, including when the process dies).
#[derive(Debug)]
struct LedgerLock(#[allow(dead_code)] fs::File);

/// Holds the ledger's advisory lock until dropped (crate-internal handle), so a restore
/// can replace the ledger's files without another instance opening them meanwhile.
pub(crate) struct LedgerGuard(#[allow(dead_code, reason = "held for its Drop")] LedgerLock);

impl Vault {
    /// The ledger in `dir` (which may not exist yet).
    pub fn at(dir: &Path) -> Vault {
        Vault {
            dir: dir.to_owned(),
            db_path: dir.join(DB_FILE),
            keyfile_path: dir.join(KEYFILE),
            next_keyfile_path: dir.join(NEXT_KEYFILE),
            lock_path: dir.join(LOCK_FILE),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// The keyfile. Destroying it makes the database permanently unreadable
    /// (crypto-shred, T5.3); backups must include it (T5.2).
    pub fn keyfile_path(&self) -> &Path {
        &self.keyfile_path
    }

    pub fn state(&self) -> VaultState {
        let db_has_data = fs::metadata(&self.db_path).is_ok_and(|m| m.len() > 0);
        match (self.keyfile_path.exists(), db_has_data) {
            (true, true) => VaultState::Ready,
            (true, false) => VaultState::Incomplete,
            (false, true) => VaultState::Orphaned,
            (false, false) => VaultState::Missing,
        }
    }

    /// Creates a new ledger protected by `password`, with the default Argon2id
    /// parameters, and returns it unlocked at the latest schema version.
    pub fn create(&self, password: &[u8]) -> Result<Unlocked, StorageError> {
        self.create_with_params(password, KdfParams::DEFAULT)
    }

    pub(crate) fn create_with_params(
        &self,
        password: &[u8],
        kdf: KdfParams,
    ) -> Result<Unlocked, StorageError> {
        self.ensure_dir()?;
        let lock = self.lock()?;
        match self.state() {
            VaultState::Missing => {}
            VaultState::Incomplete => return Err(StorageError::IncompleteCreate),
            VaultState::Ready | VaultState::Orphaned => return Err(StorageError::AlreadyExists),
        }
        let key = DataKey::generate()?;
        let mut keyfile = Keyfile::new()?;
        keyfile.set_slot(SlotKind::Password, password, kdf, &key)?;
        // Keyfile first: a database without its keyfile could never be opened, while a
        // keyfile without a database can be finished (`finish_create`).
        keyfile.write_atomic(&self.keyfile_path)?;
        let opened = self.create_db(lock, keyfile, key);
        if opened.is_err() {
            let _ = fs::remove_file(&self.db_path);
            let _ = fs::remove_file(&self.keyfile_path);
        }
        opened
    }

    /// Completes a creation that was interrupted after the keyfile was written
    /// ([`VaultState::Incomplete`]), using the password it was created with.
    pub fn finish_create(&self, password: &[u8]) -> Result<Unlocked, StorageError> {
        let lock = self.lock()?;
        match self.state() {
            VaultState::Incomplete => {}
            VaultState::Missing | VaultState::Orphaned => return Err(StorageError::NotFound),
            VaultState::Ready => return Err(StorageError::AlreadyExists),
        }
        let keyfile = Keyfile::read(&self.keyfile_path)?;
        let key = keyfile.unwrap_key(SlotKind::Password, password)?;
        match fs::remove_file(&self.db_path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        let opened = self.create_db(lock, keyfile, key);
        if opened.is_err() {
            let _ = fs::remove_file(&self.db_path);
        }
        opened
    }

    /// Unlocks with the master password. A wrong password is
    /// [`StorageError::WrongSecret`], whatever the underlying cause.
    pub fn unlock(&self, password: &[u8]) -> Result<Unlocked, StorageError> {
        self.unlock_slot(SlotKind::Password, password)
    }

    /// Unlocks with the recovery key (once [`Unlocked::set_recovery_key`] has set one).
    pub fn unlock_with_recovery_key(&self, recovery_key: &[u8]) -> Result<Unlocked, StorageError> {
        self.unlock_slot(SlotKind::RecoveryKey, recovery_key)
    }

    /// Unlocks with the raw data key instead of a secret: the copy T5.1 keeps in the
    /// macOS Keychain behind Touch ID. Same lock, permission and migration steps as
    /// [`Vault::unlock`]. A key that doesn't open the database (stale after a rotation,
    /// or wrong) is [`StorageError::DatabaseKeyRejected`]; the caller falls back to the
    /// password, which also completes any interrupted rotation.
    #[allow(dead_code, reason = "used by T5.1 (Touch ID unlock)")]
    pub(crate) fn unlock_with_data_key(&self, key: &[u8; 32]) -> Result<Unlocked, StorageError> {
        match self.state() {
            VaultState::Ready => {}
            VaultState::Incomplete => return Err(StorageError::IncompleteCreate),
            VaultState::Missing | VaultState::Orphaned => return Err(StorageError::NotFound),
        }
        let lock = self.lock()?;
        let keyfile = Keyfile::read(&self.keyfile_path)?;
        let key = DataKey::from_bytes(key);
        let (conn, permits) = db::open_encrypted(&self.db_path, &key, false)?;
        self.finish_open(lock, conn, permits, keyfile, key)
    }

    /// Creates the ledger folder (0700) if missing, then takes the ledger's advisory
    /// lock, held until the guard drops. [`StorageError::InUse`] if another instance
    /// holds it. Used by `backup::restore` (T5.2).
    pub(crate) fn lock_for_restore(&self) -> Result<LedgerGuard, StorageError> {
        self.ensure_dir()?;
        self.lock().map(LedgerGuard)
    }

    /// Destroys the key material: every keyfile (current, mid-rotation and temporary)
    /// is overwritten with zeros, flushed and deleted. The database is left in place
    /// but can never be decrypted again. Fails with [`StorageError::InUse`] while the
    /// ledger is unlocked anywhere. The full "delete everything" flow is T5.3.
    ///
    /// Overwriting can't reach old copies on copy-on-write or wear-levelled storage, or in
    /// snapshots and backups (see README, "Threat model notes").
    pub fn shred_keys(&self) -> Result<(), StorageError> {
        let _lock = self.lock()?;
        for path in [
            &self.keyfile_path,
            &self.next_keyfile_path,
            &Keyfile::tmp_path(&self.keyfile_path),
            &Keyfile::tmp_path(&self.next_keyfile_path),
        ] {
            let len = match fs::metadata(path) {
                Ok(m) => m.len(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            let mut f = fs::OpenOptions::new().write(true).open(path)?;
            f.write_all(&vec![0u8; usize::try_from(len).unwrap_or(0)])?;
            f.sync_all()?;
            drop(f);
            fs::remove_file(path)?;
        }
        sync_dir(&self.dir);
        Ok(())
    }

    fn unlock_slot(&self, kind: SlotKind, secret: &[u8]) -> Result<Unlocked, StorageError> {
        match self.state() {
            VaultState::Ready => {}
            VaultState::Incomplete => return Err(StorageError::IncompleteCreate),
            VaultState::Missing | VaultState::Orphaned => return Err(StorageError::NotFound),
        }
        let lock = self.lock()?;
        let keyfile = Keyfile::read(&self.keyfile_path)?;
        let key = keyfile.unwrap_key(kind, secret)?;
        match db::open_encrypted(&self.db_path, &key, false) {
            Ok((conn, permits)) => {
                // A rotation that stopped before re-encrypting leaves a stale next
                // keyfile; the database still uses the current key.
                remove_if_present(&self.next_keyfile_path)?;
                self.finish_open(lock, conn, permits, keyfile, key)
            }
            Err(StorageError::DatabaseKeyRejected) if self.next_keyfile_path.exists() => {
                // A rotation re-encrypted the database but stopped before promoting the
                // new keyfile: finish it.
                let next = Keyfile::read(&self.next_keyfile_path)?;
                let next_key = next.unwrap_key(kind, secret)?;
                let (conn, permits) = db::open_encrypted(&self.db_path, &next_key, false)?;
                fs::rename(&self.next_keyfile_path, &self.keyfile_path)?;
                sync_dir(&self.dir);
                self.finish_open(lock, conn, permits, next, next_key)
            }
            Err(e) => Err(e),
        }
    }

    fn create_db(
        &self,
        lock: LedgerLock,
        keyfile: Keyfile,
        key: DataKey,
    ) -> Result<Unlocked, StorageError> {
        // Create the file owner-only before SQLite opens it; SQLite gives its journals
        // the same mode as the database.
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(false);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, FILE_MODE);
        drop(options.open(&self.db_path)?);
        let (conn, permits) = db::open_encrypted(&self.db_path, &key, true)?;
        self.finish_open(lock, conn, permits, keyfile, key)
    }

    fn finish_open(
        &self,
        lock: LedgerLock,
        mut conn: Connection,
        permits: std::sync::Arc<db::Permits>,
        keyfile: Keyfile,
        key: DataKey,
    ) -> Result<Unlocked, StorageError> {
        db::migrate(&mut conn, &permits, MIGRATIONS)?;
        repo::seed_default_taxonomy(&mut conn)?;
        self.restrict_permissions()?;
        Ok(Unlocked {
            vault: self.clone(),
            keyfile,
            key,
            conn: Some(conn),
            permits,
            _lock: lock,
        })
    }

    fn ensure_dir(&self) -> Result<(), StorageError> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, DIR_MODE);
        builder.create(&self.dir)?;
        Ok(())
    }

    /// Takes the ledger's advisory lock, or fails with [`StorageError::InUse`].
    fn lock(&self) -> Result<LedgerLock, StorageError> {
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, FILE_MODE);
        let file = match options.open(&self.lock_path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::NotFound);
            }
            Err(e) => return Err(e.into()),
        };
        match file.try_lock() {
            Ok(()) => Ok(LedgerLock(file)),
            Err(fs::TryLockError::WouldBlock) => Err(StorageError::InUse),
            Err(fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }

    /// Sets the folder to 0700 and the database and keyfile to 0600, repairing them if
    /// something loosened them. No-op outside Unix.
    fn restrict_permissions(&self) -> Result<(), StorageError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.dir, fs::Permissions::from_mode(DIR_MODE))?;
            for path in [&self.db_path, &self.keyfile_path] {
                fs::set_permissions(path, fs::Permissions::from_mode(FILE_MODE))?;
            }
        }
        Ok(())
    }
}

/// An unlocked ledger: the open database connection plus the data key, both held only in
/// memory, and the ledger's lock. [`Unlocked::lock`], or dropping it, closes the
/// connection, wipes the key and releases the lock.
pub struct Unlocked {
    vault: Vault,
    keyfile: Keyfile,
    key: DataKey,
    /// `None` only after [`Unlocked::wipe`], which runs as the value is consumed.
    conn: Option<Connection>,
    /// The switches behind `fm_permit()` and the authorizer on this connection (db.rs).
    permits: std::sync::Arc<db::Permits>,
    // Declared last, so it is released after the connection closes.
    _lock: LedgerLock,
}

/// Paths and slot kinds only; never key material.
impl fmt::Debug for Unlocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Unlocked")
            .field("vault", &self.vault)
            .field("keyfile", &self.keyfile)
            .finish_non_exhaustive()
    }
}

impl Unlocked {
    /// The connection, for the repository and the rest of fm-core. Not public: every
    /// write from outside fm-core goes through repository functions, and even inside it
    /// the authorizer and triggers refuse schema changes and forbidden edits.
    pub(crate) fn connection(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("connection is open until the ledger is locked")
    }

    /// Mutable access, for transactions.
    pub(crate) fn connection_mut(&mut self) -> &mut Connection {
        self.conn
            .as_mut()
            .expect("connection is open until the ledger is locked")
    }

    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    /// The schema version of the open database.
    pub fn schema_version(&self) -> Result<u32, StorageError> {
        db::schema_version(self.connection())
    }

    /// Replaces the master password, after checking the current one. Only the
    /// password's slot in the keyfile is rewritten; the database is untouched, so a copy
    /// of the old keyfile still opens it with the old password. After a suspected
    /// compromise, use [`Unlocked::rotate_data_key`] instead.
    pub fn set_password(
        &mut self,
        current: &[u8],
        new_password: &[u8],
    ) -> Result<(), StorageError> {
        self.set_password_with_params(current, new_password, KdfParams::DEFAULT)
    }

    pub(crate) fn set_password_with_params(
        &mut self,
        current: &[u8],
        new_password: &[u8],
        kdf: KdfParams,
    ) -> Result<(), StorageError> {
        self.check_password(current)?;
        self.rewrite_slot(SlotKind::Password, new_password, kdf)
    }

    /// Sets (or replaces) the recovery key that can also unlock the ledger, after
    /// checking the current password. Generating it and showing it to the user is T5.1.
    pub fn set_recovery_key(
        &mut self,
        current_password: &[u8],
        recovery_key: &[u8],
    ) -> Result<(), StorageError> {
        self.set_recovery_key_with_params(current_password, recovery_key, KdfParams::DEFAULT)
    }

    pub(crate) fn set_recovery_key_with_params(
        &mut self,
        current_password: &[u8],
        recovery_key: &[u8],
        kdf: KdfParams,
    ) -> Result<(), StorageError> {
        self.check_password(current_password)?;
        self.rewrite_slot(SlotKind::RecoveryKey, recovery_key, kdf)
    }

    /// Sets a new master password after the user unlocked with the recovery key and no
    /// longer knows the old password. Checks `recovery_key` against its slot (not the
    /// password's) and rewrites only the password slot, like [`Unlocked::set_password`].
    /// After a suspected compromise, follow with [`Unlocked::rotate_data_key`].
    pub fn reset_password(
        &mut self,
        recovery_key: &[u8],
        new_password: &[u8],
    ) -> Result<(), StorageError> {
        self.reset_password_with_params(recovery_key, new_password, KdfParams::DEFAULT)
    }

    pub(crate) fn reset_password_with_params(
        &mut self,
        recovery_key: &[u8],
        new_password: &[u8],
        kdf: KdfParams,
    ) -> Result<(), StorageError> {
        self.keyfile
            .unwrap_key(SlotKind::RecoveryKey, recovery_key)
            .map(drop)?;
        self.rewrite_slot(SlotKind::Password, new_password, kdf)
    }

    /// A copy of the raw data key, wiped when dropped, for T5.1 to store in the macOS
    /// Keychain behind Touch ID. Never leaves the crate.
    #[allow(dead_code, reason = "used by T5.1 (Touch ID unlock)")]
    pub(crate) fn data_key_copy(&self) -> zeroize::Zeroizing<[u8; 32]> {
        self.key.copy_bytes()
    }

    pub fn has_recovery_key(&self) -> bool {
        self.keyfile.has_slot(SlotKind::RecoveryKey)
    }

    /// Replaces the data key itself: the whole database is re-encrypted under a new
    /// random key (SQLCipher `PRAGMA rekey`) and a new keyfile (new id) holds it, wrapped
    /// by `password`. Old keyfile copies (snapshots, backups) and old passwords stop
    /// working on this database. Use it after a password change made because of a
    /// suspected compromise.
    ///
    /// The old recovery key stops working too. Pass `new_recovery_key` to set one in
    /// the same step; otherwise the ledger has none until [`Unlocked::set_recovery_key`].
    ///
    /// Crash-safe: the new keyfile is written to `ledger.keys.next` first, then the
    /// database is re-encrypted (atomically, by SQLCipher), then the new keyfile replaces
    /// the old one. [`Vault::unlock`] completes or discards an interrupted rotation.
    pub fn rotate_data_key(
        &mut self,
        password: &[u8],
        new_recovery_key: Option<&[u8]>,
    ) -> Result<(), StorageError> {
        self.rotate_data_key_with_params(password, new_recovery_key, KdfParams::DEFAULT)
    }

    pub(crate) fn rotate_data_key_with_params(
        &mut self,
        password: &[u8],
        new_recovery_key: Option<&[u8]>,
        kdf: KdfParams,
    ) -> Result<(), StorageError> {
        self.check_password(password)?;
        let new_key = DataKey::generate()?;
        let mut next = Keyfile::new()?;
        next.set_slot(SlotKind::Password, password, kdf, &new_key)?;
        if let Some(recovery_key) = new_recovery_key {
            next.set_slot(SlotKind::RecoveryKey, recovery_key, kdf, &new_key)?;
        }
        let next_path = &self.vault.next_keyfile_path;
        next.write_atomic(next_path)?;
        if let Err(e) = db::rekey(self.connection(), &self.permits, &new_key) {
            let _ = fs::remove_file(next_path);
            return Err(e);
        }
        fs::rename(next_path, &self.vault.keyfile_path)?;
        sync_dir(&self.vault.dir);
        // The old key is wiped as it is dropped here.
        self.key = new_key;
        self.keyfile = next;
        Ok(())
    }

    fn check_password(&self, password: &[u8]) -> Result<(), StorageError> {
        self.keyfile
            .unwrap_key(SlotKind::Password, password)
            .map(drop)
    }

    fn rewrite_slot(
        &mut self,
        kind: SlotKind,
        secret: &[u8],
        kdf: KdfParams,
    ) -> Result<(), StorageError> {
        let mut updated = self.keyfile.clone();
        updated.set_slot(kind, secret, kdf, &self.key)?;
        updated.write_atomic(&self.vault.keyfile_path)?;
        self.keyfile = updated;
        Ok(())
    }

    /// Closes the database, wipes the data key from memory and releases the lock;
    /// returns the locked ledger.
    pub fn lock(mut self) -> Vault {
        self.wipe();
        self.vault.clone()
    }

    /// Closes the connection (SQLCipher wipes its own copy of the key when the
    /// connection closes) and zeroes the data key.
    fn wipe(&mut self) {
        if let Some(conn) = self.conn.take() {
            // A failed close still releases the handle on drop; nothing to do about it
            // while locking.
            let _ = conn.close();
        }
        self.key.zeroize();
    }
}

impl Drop for Unlocked {
    fn drop(&mut self) {
        self.wipe();
    }
}

fn remove_if_present(path: &Path) -> Result<(), StorageError> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Makes renames and deletions in `dir` durable. Not supported everywhere: best effort.
fn sync_dir(dir: &Path) {
    let _ = fs::File::open(dir).and_then(|d| d.sync_all());
}

#[cfg(test)]
mod repo_tests;
#[cfg(test)]
mod tests;
