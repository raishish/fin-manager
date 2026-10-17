//! The unlock session's storage seam, implemented by the real encrypted ledger.
//!
//! Unlock settings live in the ledger's `meta` table under `unlock.*` keys, written
//! with plain row statements (the connection's authorizer refuses PRAGMAs and DDL).

use rusqlite::OptionalExtension;
use zeroize::Zeroizing;

use super::store::{LedgerStore, OpenLedger};
use crate::storage::{KdfParams, StorageError, Unlocked, Vault, VaultState};

/// A ledger on disk, as the unlock session sees it.
#[derive(Debug, Clone)]
pub struct VaultStore {
    vault: Vault,
    kdf: KdfParams,
}

impl VaultStore {
    pub fn new(vault: Vault) -> Self {
        Self {
            vault,
            kdf: KdfParams::DEFAULT,
        }
    }

    /// Cheap key derivation, so tests don't spend seconds in Argon2.
    #[cfg(test)]
    pub(crate) fn insecure_for_tests(vault: Vault) -> Self {
        Self {
            vault,
            kdf: KdfParams::INSECURE_TEST,
        }
    }

    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    fn open(&self, unlocked: Unlocked) -> VaultLedger {
        VaultLedger {
            unlocked,
            kdf: self.kdf,
        }
    }
}

/// An open ledger. Dropping it locks.
#[derive(Debug)]
pub struct VaultLedger {
    unlocked: Unlocked,
    kdf: KdfParams,
}

impl VaultLedger {
    /// The ledger itself, for the rest of the app (repository functions).
    pub fn ledger(&self) -> &Unlocked {
        &self.unlocked
    }

    pub fn ledger_mut(&mut self) -> &mut Unlocked {
        &mut self.unlocked
    }
}

impl LedgerStore for VaultStore {
    type Open = VaultLedger;

    fn state(&self) -> VaultState {
        self.vault.state()
    }

    fn create(&self, password: &[u8]) -> Result<VaultLedger, StorageError> {
        let unlocked = self.vault.create_with_params(password, self.kdf)?;
        Ok(self.open(unlocked))
    }

    fn finish_create(&self, password: &[u8]) -> Result<VaultLedger, StorageError> {
        Ok(self.open(self.vault.finish_create(password)?))
    }

    fn unlock(&self, password: &[u8]) -> Result<VaultLedger, StorageError> {
        Ok(self.open(self.vault.unlock(password)?))
    }

    fn unlock_with_recovery_key(&self, recovery_key: &[u8]) -> Result<VaultLedger, StorageError> {
        Ok(self.open(self.vault.unlock_with_recovery_key(recovery_key)?))
    }

    fn unlock_with_data_key(&self, key: &[u8; 32]) -> Result<VaultLedger, StorageError> {
        Ok(self.open(self.vault.unlock_with_data_key(key)?))
    }
}

impl OpenLedger for VaultLedger {
    fn has_recovery_key(&self) -> bool {
        self.unlocked.has_recovery_key()
    }

    fn set_password(&mut self, current: &[u8], new_password: &[u8]) -> Result<(), StorageError> {
        self.unlocked
            .set_password_with_params(current, new_password, self.kdf)
    }

    fn reset_password(
        &mut self,
        recovery_key: &[u8],
        new_password: &[u8],
    ) -> Result<(), StorageError> {
        self.unlocked
            .reset_password_with_params(recovery_key, new_password, self.kdf)
    }

    fn set_recovery_key(
        &mut self,
        current_password: &[u8],
        recovery_key: &[u8],
    ) -> Result<(), StorageError> {
        self.unlocked
            .set_recovery_key_with_params(current_password, recovery_key, self.kdf)
    }

    fn rotate_data_key(
        &mut self,
        password: &[u8],
        new_recovery_key: Option<&[u8]>,
    ) -> Result<(), StorageError> {
        self.unlocked
            .rotate_data_key_with_params(password, new_recovery_key, self.kdf)
    }

    fn data_key_copy(&self) -> Zeroizing<[u8; 32]> {
        self.unlocked.data_key_copy()
    }

    fn setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        Ok(self
            .unlocked
            .connection()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    fn set_setting(&self, key: &str, value: Option<&str>) -> Result<(), StorageError> {
        let conn = self.unlocked.connection();
        match value {
            Some(value) => conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2) \
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                [key, value],
            ),
            None => conn.execute("DELETE FROM meta WHERE key = ?1", [key]),
        }?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! The session over the real encrypted ledger, end to end.

    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::unlock::{Now, RecoveryToConfirm, Session, Status, TouchIdError, TouchIdKeychain};

    const STRONG: &str = "velvet anchor mosaic tundra";
    const STRONG_2: &str = "purple monkey dishwasher";

    /// An in-memory Keychain that always recognises the finger.
    #[derive(Clone, Default)]
    struct Keychain(Rc<RefCell<Option<[u8; 32]>>>);

    impl TouchIdKeychain for Keychain {
        fn available(&self) -> bool {
            true
        }
        fn store(&self, key: &[u8; 32]) -> Result<(), TouchIdError> {
            *self.0.borrow_mut() = Some(*key);
            Ok(())
        }
        fn load(&self, _: &str) -> Result<Zeroizing<[u8; 32]>, TouchIdError> {
            self.0
                .borrow()
                .map(Zeroizing::new)
                .ok_or(TouchIdError::Unavailable)
        }
        fn delete(&self) -> Result<(), TouchIdError> {
            *self.0.borrow_mut() = None;
            Ok(())
        }
    }

    fn session(dir: &tempfile::TempDir, keychain: &Keychain) -> Session<VaultStore, Keychain> {
        let vault = Vault::at(&dir.path().join("ledger"));
        Session::new(
            VaultStore::insecure_for_tests(vault),
            keychain.clone(),
            Now::current(),
        )
    }

    fn confirm(s: &mut Session<VaultStore, Keychain>, shown: &RecoveryToConfirm) {
        let answers: Vec<String> = shown
            .challenge
            .iter()
            .map(|&i| shown.groups[i].to_string())
            .collect();
        let answers: Vec<&str> = answers.iter().map(String::as_str).collect();
        s.confirm_recovery_key(&answers).unwrap();
    }

    fn join(shown: &RecoveryToConfirm) -> String {
        let groups: Vec<&str> = shown.groups.iter().map(|g| g.as_str()).collect();
        groups.join("-")
    }

    #[test]
    fn the_whole_lifecycle_on_a_real_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let keychain = Keychain::default();
        let now = Now::current();
        let mut s = session(&dir, &keychain);
        assert_eq!(s.status(), Status::NoLedger);

        let shown = s.create(STRONG, now).unwrap();
        confirm(&mut s, &shown);
        assert_eq!(s.status(), Status::Unlocked);
        assert!(s.ledger().unwrap().ledger().accounts().unwrap().is_empty());
        s.set_autolock_minutes(30, now).unwrap();

        // Touch ID: the Keychain copy opens the real database.
        s.lock(now);
        assert_eq!(s.status(), Status::Locked);
        s.unlock_with_touch_id(now).unwrap();
        assert_eq!(s.autolock_minutes(), 30);

        // A compromised-password change rotates the data key; the Keychain copy follows.
        let before = *keychain.0.borrow();
        let new_recovery = s
            .change_password(STRONG, STRONG_2, true, now)
            .unwrap()
            .unwrap();
        assert_ne!(
            *keychain.0.borrow(),
            before,
            "Keychain refreshed after rotation"
        );
        confirm(&mut s, &new_recovery);
        s.lock(now);
        s.unlock_with_touch_id(now).unwrap();
        s.lock(now);

        // A new run: the password is needed; the old one fails, the new one works.
        drop(s);
        let mut s = session(&dir, &keychain);
        assert!(s.unlock_with_touch_id(now).is_err());
        assert!(s.unlock_with_password(STRONG, now).is_err());
        assert!(s.unlock_with_password(STRONG_2, now).unwrap().is_none());
        s.lock(now);

        // Forgotten password: recovery key, then a new password and a new key.
        s.unlock_with_recovery_key(&join(&new_recovery), now)
            .unwrap();
        assert_eq!(s.status(), Status::NeedsNewPassword);
        let third = s.set_password_after_recovery(STRONG, false, now).unwrap();
        confirm(&mut s, &third);
        s.lock(now);
        assert!(
            s.unlock_with_recovery_key(&join(&new_recovery), now)
                .is_err()
        );
        s.unlock_with_password(STRONG, now).unwrap();
        assert_eq!(s.autolock_minutes(), 30, "settings survive rotation");
    }

    #[test]
    fn an_unconfirmed_key_is_remembered_in_the_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let keychain = Keychain::default();
        let now = Now::current();
        let mut s = session(&dir, &keychain);
        let first = s.create(STRONG, now).unwrap();
        drop(s);
        let mut s = session(&dir, &keychain);
        let second = s
            .unlock_with_password(STRONG, now)
            .unwrap()
            .expect("new key");
        assert_ne!(join(&first), join(&second));
        confirm(&mut s, &second);
        s.lock(now);
        assert!(s.unlock_with_password(STRONG, now).unwrap().is_none());
    }

    #[test]
    fn a_stale_keychain_copy_is_refused_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let keychain = Keychain::default();
        let now = Now::current();
        let mut s = session(&dir, &keychain);
        let shown = s.create(STRONG, now).unwrap();
        confirm(&mut s, &shown);
        s.lock(now);
        *keychain.0.borrow_mut() = Some([9; 32]);
        assert!(s.unlock_with_touch_id(now).is_err());
        assert!(keychain.0.borrow().is_none());
        assert_eq!(s.status(), Status::Locked);
    }
}
