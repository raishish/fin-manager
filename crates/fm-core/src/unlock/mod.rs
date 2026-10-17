//! Unlocking the ledger (SPEC §7, T5.1): master password rules, the recovery key,
//! auto-lock and Touch ID. Storage itself (keys, slots, encryption) is
//! [`crate::storage`]; this module decides who may unlock it and when.
//!
//! See `README.md` for how each SPEC §7 rule is enforced and tested.

mod autolock;
mod clock;
#[cfg(target_os = "macos")]
mod macos;
mod password;
mod recovery;
mod session;
mod store;
mod touch_id;
mod vault;

#[cfg(test)]
mod tests;

pub use autolock::{AutoLock, CHOICES_MINUTES, DEFAULT_MINUTES};
pub use clock::Now;
#[cfg(target_os = "macos")]
pub use macos::Keychain;
pub use password::{MIN_CHARS, MIN_SCORE, Strength, normalize, strength};
pub use recovery::{CHALLENGE_GROUPS, Challenge, GROUPS, RecoveryKey, RecoveryKeyError};
pub use session::{RecoveryToConfirm, Session, Status, TouchIdBlocked, UnlockError};
pub use store::{LedgerStore, NoTouchId, OpenLedger, TouchIdError, TouchIdKeychain};
pub use touch_id::{MAX_FAILED_SCANS, PASSWORD_EVERY, PasswordRequired, TouchIdPolicy};
pub use vault::{VaultLedger, VaultStore};
