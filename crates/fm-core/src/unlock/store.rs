//! What the unlock session needs from storage and from the Keychain, as two small
//! seams. [`crate::storage`] implements the storage side; the Keychain side is
//! [`super::macos::Keychain`] on macOS and [`NoTouchId`] elsewhere. Tests use
//! in-memory fakes.

use std::fmt;

use zeroize::Zeroizing;

use crate::storage::{StorageError, VaultState};

/// A ledger at rest: the ways to open it.
pub trait LedgerStore {
    type Open: OpenLedger;

    fn state(&self) -> VaultState;
    fn create(&self, password: &[u8]) -> Result<Self::Open, StorageError>;
    fn finish_create(&self, password: &[u8]) -> Result<Self::Open, StorageError>;
    fn unlock(&self, password: &[u8]) -> Result<Self::Open, StorageError>;
    fn unlock_with_recovery_key(&self, recovery_key: &[u8]) -> Result<Self::Open, StorageError>;
    /// Opens with the raw data key (the Touch ID path).
    fn unlock_with_data_key(&self, key: &[u8; 32]) -> Result<Self::Open, StorageError>;
}

/// An open ledger: key management plus a small settings table. Dropping it locks.
pub trait OpenLedger {
    fn has_recovery_key(&self) -> bool;
    fn set_password(&mut self, current: &[u8], new_password: &[u8]) -> Result<(), StorageError>;
    /// Replaces the password using the recovery key as proof (forgotten password).
    fn reset_password(
        &mut self,
        recovery_key: &[u8],
        new_password: &[u8],
    ) -> Result<(), StorageError>;
    fn set_recovery_key(
        &mut self,
        current_password: &[u8],
        recovery_key: &[u8],
    ) -> Result<(), StorageError>;
    fn rotate_data_key(
        &mut self,
        password: &[u8],
        new_recovery_key: Option<&[u8]>,
    ) -> Result<(), StorageError>;
    fn data_key_copy(&self) -> Zeroizing<[u8; 32]>;
    fn setting(&self, key: &str) -> Result<Option<String>, StorageError>;
    fn set_setting(&self, key: &str, value: Option<&str>) -> Result<(), StorageError>;
}

/// Why a Touch ID operation didn't succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TouchIdError {
    /// No Touch ID on this machine, none enrolled, or no stored key.
    Unavailable,
    /// The user dismissed the Touch ID prompt.
    Cancelled,
    /// The fingerprint didn't match.
    Failed,
    /// The enrolled fingerprints changed, so the stored key can never be read again.
    Invalidated,
    Other(String),
}

impl fmt::Display for TouchIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("Touch ID isn't available"),
            Self::Cancelled => f.write_str("Touch ID was cancelled"),
            Self::Failed => f.write_str("fingerprint not recognised"),
            Self::Invalidated => f.write_str("fingerprints changed; enter your password"),
            Self::Other(why) => write!(f, "Touch ID failed: {why}"),
        }
    }
}

impl std::error::Error for TouchIdError {}

/// The copy of the data key that Touch ID unlocks (SPEC §7): one Keychain item that
/// can only be read after a fingerprint match against the fingers enrolled when it
/// was stored.
pub trait TouchIdKeychain {
    /// Whether Touch ID can be used on this machine at all.
    fn available(&self) -> bool;
    /// Stores `data_key`, replacing any earlier item. No prompt.
    fn store(&self, data_key: &[u8; 32]) -> Result<(), TouchIdError>;
    /// Reads the key back; shows the Touch ID prompt with `reason`.
    fn load(&self, reason: &str) -> Result<Zeroizing<[u8; 32]>, TouchIdError>;
    /// Deletes the item. Deleting a missing item succeeds.
    fn delete(&self) -> Result<(), TouchIdError>;
}

/// No Touch ID (Linux, or a Mac without it): the password is always used.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTouchId;

impl TouchIdKeychain for NoTouchId {
    fn available(&self) -> bool {
        false
    }
    fn store(&self, _: &[u8; 32]) -> Result<(), TouchIdError> {
        Err(TouchIdError::Unavailable)
    }
    fn load(&self, _: &str) -> Result<Zeroizing<[u8; 32]>, TouchIdError> {
        Err(TouchIdError::Unavailable)
    }
    fn delete(&self) -> Result<(), TouchIdError> {
        Ok(())
    }
}
