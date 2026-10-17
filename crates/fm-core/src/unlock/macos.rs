//! The Touch ID copy of the data key on macOS (SPEC §7, decided 2026-10-16).
//!
//! One generic-password item in the data-protection Keychain, per ledger:
//! - access control `BiometryCurrentSet`: reading it needs a fingerprint match against
//!   the fingers enrolled when it was stored, enforced by the Secure Enclave; enrolling
//!   or removing a finger makes it unreadable for good;
//! - `WhenPasscodeSetThisDeviceOnly`: never synced or restored to another Mac, and
//!   deleted by the system if the Mac's login password is removed;
//! - not synchronizable (no iCloud Keychain).
//!
//! The data-protection Keychain needs the `keychain-access-groups` entitlement, which
//! macOS only grants to apps signed with an Apple Developer ID and provisioning
//! profile. Unsigned development builds get `errSecMissingEntitlement`; Touch ID is
//! then reported as unavailable and the password is used.
//!
//! The Touch ID prompt shows the system's own wording: the safe API doesn't expose
//! `kSecUseOperationPrompt`, so `reason` is unused for now.
//!
//! Known gap: reading the item returns the key in a Core Foundation `CFData` that
//! `security-framework` copies into a `Vec` and releases. Our copy is wiped; the
//! `CFData`'s memory is freed without being wiped, which needs `unsafe` to fix.
//! Reaching it means reading process memory while unlocking, which SPEC §7 already
//! lists as not protected against (malware while unlocked).

use std::path::Path;

use security_framework::access_control::{ProtectionMode, SecAccessControl};
use security_framework::base::Error as SecError;
use security_framework::key::{GenerateKeyOptions, KeyType, SecKey, Token};
use security_framework::passwords::{
    delete_generic_password_options, generic_password, set_generic_password_options,
};
use security_framework::passwords_options::PasswordOptions;
use security_framework_sys::access_control::{
    kSecAccessControlBiometryCurrentSet, kSecAccessControlPrivateKeyUsage,
};
use zeroize::{Zeroize, Zeroizing};

use super::store::{KeychainDeletion, TouchIdError, TouchIdKeychain};

const SERVICE: &str = "fin-manager.touch-id";

const ERR_USER_CANCELED: i32 = -128;
const ERR_AUTH_FAILED: i32 = -25293;
const ERR_ITEM_NOT_FOUND: i32 = -25300;
const ERR_MISSING_ENTITLEMENT: i32 = -34018;

/// The Keychain item for one ledger.
#[derive(Debug, Clone)]
pub struct Keychain {
    account: String,
}

impl Keychain {
    /// The item for the ledger stored at `ledger_dir`.
    pub fn for_ledger(ledger_dir: &Path) -> Self {
        Self {
            account: ledger_dir.to_string_lossy().into_owned(),
        }
    }

    fn options(&self) -> PasswordOptions {
        let mut options = PasswordOptions::new_generic_password(SERVICE, &self.account);
        options.use_protected_keychain();
        options
    }
}

impl TouchIdKeychain for Keychain {
    /// True if this Mac has Touch ID with at least one finger enrolled. Checked by
    /// making a throwaway Secure Enclave key that requires biometry: it is never
    /// stored and shows no prompt.
    fn available(&self) -> bool {
        let Ok(access) = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleWhenPasscodeSetThisDeviceOnly),
            kSecAccessControlPrivateKeyUsage | kSecAccessControlBiometryCurrentSet,
        ) else {
            return false;
        };
        let mut options = GenerateKeyOptions::default();
        options
            .set_key_type(KeyType::ec_sec_prime_random())
            .set_size_in_bits(256)
            .set_token(Token::SecureEnclave)
            .set_access_control(access);
        SecKey::new(&options).is_ok()
    }

    fn store(&self, data_key: &[u8; 32]) -> Result<(), TouchIdError> {
        self.delete()?;
        let access = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleWhenPasscodeSetThisDeviceOnly),
            kSecAccessControlBiometryCurrentSet,
        )
        .map_err(map_error)?;
        let mut options = self.options();
        options.set_access_control(access);
        options.set_access_synchronized(Some(false));
        options.set_label("fin-manager Touch ID unlock");
        set_generic_password_options(data_key, options).map_err(map_error)
    }

    fn load(&self, _reason: &str) -> Result<Zeroizing<[u8; 32]>, TouchIdError> {
        let mut bytes = generic_password(self.options()).map_err(|e| match e.code() {
            // We stored it in this run; if it is gone, the fingerprints changed.
            ERR_ITEM_NOT_FOUND => TouchIdError::Invalidated,
            _ => map_error(e),
        })?;
        let key = <[u8; 32]>::try_from(bytes.as_slice())
            .map(Zeroizing::new)
            .map_err(|_| TouchIdError::Other("stored key has the wrong length".into()));
        bytes.zeroize();
        key
    }

    fn delete(&self) -> Result<(), TouchIdError> {
        match delete_generic_password_options(self.options()) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_ITEM_NOT_FOUND => Ok(()),
            // This build has no entitlement, so it can never have stored an item; the
            // background retry would only loop. (An item a signed build stored can't
            // be reached from here: see `delete_verified`.)
            Err(e) if e.code() == ERR_MISSING_ENTITLEMENT => Ok(()),
            Err(e) => Err(map_error(e)),
        }
    }

    fn delete_verified(&self) -> Result<KeychainDeletion, TouchIdError> {
        match delete_generic_password_options(self.options()) {
            Ok(()) => Ok(KeychainDeletion::Done),
            Err(e) if e.code() == ERR_ITEM_NOT_FOUND => Ok(KeychainDeletion::Done),
            // A signed build of the app may have stored one that this build can't see.
            Err(e) if e.code() == ERR_MISSING_ENTITLEMENT => Ok(KeychainDeletion::Unverifiable),
            Err(e) => Err(map_error(e)),
        }
    }
}

/// Errors other than a cancel or a failed match (biometry locked out after too many
/// attempts, interaction not allowed, ...) become `Other`. The session counts every
/// unsuccessful prompt as a failed scan, so none of them can be retried freely.
fn map_error(e: SecError) -> TouchIdError {
    match e.code() {
        ERR_USER_CANCELED => TouchIdError::Cancelled,
        ERR_AUTH_FAILED => TouchIdError::Failed,
        ERR_MISSING_ENTITLEMENT => TouchIdError::Unavailable,
        code => TouchIdError::Other(format!("Keychain error {code}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On an unsigned test binary the data-protection Keychain refuses access, so
    /// storing fails cleanly as "unavailable" and nothing is left behind.
    #[test]
    fn unsigned_builds_report_touch_id_unavailable() {
        let keychain = Keychain::for_ledger(Path::new("/nonexistent/fm-test-ledger"));
        match keychain.store(&[7; 32]) {
            Err(TouchIdError::Unavailable) => {}
            // A signed test runner would get this far; then clean up.
            Ok(()) => keychain.delete().unwrap(),
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Deleting must succeed on an unsigned build: nothing can be stored there, and
    /// "delete everything" (T5.3) refuses to go on while a delete is failing.
    #[test]
    fn deleting_on_an_unsigned_build_finds_nothing_to_delete() {
        let keychain = Keychain::for_ledger(Path::new("/nonexistent/fm-test-ledger"));
        assert_eq!(keychain.delete(), Ok(()));
        // But it can't vouch for an item a signed build may have stored.
        match keychain.delete_verified() {
            Ok(KeychainDeletion::Unverifiable) => {}
            // A signed test runner can check.
            Ok(KeychainDeletion::Done) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}
