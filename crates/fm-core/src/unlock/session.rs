//! The unlock session: one ledger, its lock state, and the rules for opening it.
//!
//! The app holds one [`Session`] for the life of the process. Every call takes the
//! current time ([`Now`]) so the time rules can be tested without waiting.

use std::fmt;

use zeroize::Zeroizing;

use super::autolock::{AutoLock, DEFAULT_MINUTES};
use super::clock::Now;
use super::password::{Strength, normalize, strength};
use super::recovery::{CHALLENGE_GROUPS, Challenge, RecoveryKey, RecoveryKeyError};
use super::store::{LedgerStore, OpenLedger, TouchIdError, TouchIdKeychain};
use super::touch_id::{PasswordRequired, TouchIdPolicy};
use crate::storage::{StorageError, VaultState};

/// Settings kept in the (encrypted) ledger.
const SETTING_RECOVERY_UNCONFIRMED: &str = "unlock.recovery_unconfirmed";
const SETTING_AUTOLOCK_MINUTES: &str = "unlock.autolock_minutes";

/// Shown in the Touch ID prompt.
const TOUCH_ID_REASON: &str = "unlock your finances";

/// Where the session is, for the UI to pick a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// No ledger yet: create one.
    NoLedger,
    /// Creation was interrupted: enter the password to finish it.
    Incomplete,
    /// The keyfile is gone; the ledger can never be opened.
    Unrecoverable,
    Locked,
    /// Opened with the recovery key: a new password must be set before anything else.
    NeedsNewPassword,
    /// A new recovery key is waiting for the user to confirm they saved it.
    ConfirmRecoveryKey,
    Unlocked,
}

/// A recovery key to show once, and the groups the user must type back.
#[derive(Clone)]
pub struct RecoveryToConfirm {
    pub groups: Vec<Zeroizing<String>>,
    pub challenge: [usize; CHALLENGE_GROUPS],
}

impl fmt::Debug for RecoveryToConfirm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryToConfirm")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

/// Why Touch ID can't be offered right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchIdBlocked {
    /// No usable Touch ID on this machine, or no key stored for it.
    Unavailable,
    /// Not while the ledger is open, being created or waiting for a confirmation.
    NotLocked,
    Password(PasswordRequired),
}

#[derive(Debug)]
pub enum UnlockError {
    /// The new password is too weak; the strength says why.
    WeakPassword(Strength),
    /// Wrong password or recovery key.
    WrongSecret,
    RecoveryKey(RecoveryKeyError),
    /// The confirmation answers didn't match the recovery key.
    ConfirmationMismatch,
    /// Touch ID isn't possible; enter the password.
    TouchIdBlocked(TouchIdBlocked),
    TouchId(TouchIdError),
    /// The call doesn't fit the session's current [`Status`].
    WrongState(Status),
    Storage(StorageError),
}

impl fmt::Display for UnlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WeakPassword(s) => match &s.warning {
                Some(w) => write!(f, "password too weak: {w}"),
                None => f.write_str("password too weak"),
            },
            Self::WrongSecret => f.write_str("wrong password or recovery key"),
            Self::RecoveryKey(e) => e.fmt(f),
            Self::ConfirmationMismatch => f.write_str("those groups don't match your recovery key"),
            Self::TouchIdBlocked(TouchIdBlocked::Password(why)) => why.fmt(f),
            Self::TouchIdBlocked(_) => f.write_str("Touch ID isn't available; enter your password"),
            Self::TouchId(e) => e.fmt(f),
            Self::WrongState(status) => write!(f, "not possible while {status:?}"),
            Self::Storage(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for UnlockError {}

impl From<StorageError> for UnlockError {
    fn from(e: StorageError) -> Self {
        match e {
            StorageError::WrongSecret => Self::WrongSecret,
            e => Self::Storage(e),
        }
    }
}

impl From<RecoveryKeyError> for UnlockError {
    fn from(e: RecoveryKeyError) -> Self {
        Self::RecoveryKey(e)
    }
}

struct Pending {
    key: RecoveryKey,
    challenge: Challenge,
}

pub struct Session<S: LedgerStore, K: TouchIdKeychain> {
    store: S,
    keychain: K,
    open: Option<S::Open>,
    /// Set while open with the recovery key and no new password yet.
    recovery_unlock: Option<RecoveryKey>,
    pending: Option<Pending>,
    /// The Keychain holds a copy of the current data key, stored after a password
    /// unlock in this run of the app.
    touch_id_stored: bool,
    /// A Keychain delete failed; retried on every tick until it succeeds.
    delete_pending: bool,
    policy: TouchIdPolicy,
    autolock: AutoLock,
}

impl<S: LedgerStore, K: TouchIdKeychain> Session<S, K> {
    /// A session for a new run of the app. A Keychain item left by an earlier run
    /// (the app crashed, or was killed) is deleted: Touch ID needs the password in
    /// this run first.
    pub fn new(store: S, keychain: K, now: Now) -> Self {
        let mut session = Self {
            store,
            keychain,
            open: None,
            recovery_unlock: None,
            pending: None,
            touch_id_stored: false,
            delete_pending: false,
            policy: TouchIdPolicy::default(),
            autolock: AutoLock::new(DEFAULT_MINUTES, now),
        };
        session.forget_touch_id();
        session
    }

    pub fn status(&self) -> Status {
        if self.open.is_some() {
            return if self.recovery_unlock.is_some() {
                Status::NeedsNewPassword
            } else if self.pending.is_some() {
                Status::ConfirmRecoveryKey
            } else {
                Status::Unlocked
            };
        }
        match self.store.state() {
            VaultState::Missing => Status::NoLedger,
            VaultState::Incomplete => Status::Incomplete,
            VaultState::Orphaned => Status::Unrecoverable,
            VaultState::Ready => Status::Locked,
        }
    }

    /// The open ledger, only once it is fully unlocked (not while a new password or a
    /// recovery-key confirmation is outstanding).
    pub fn ledger(&self) -> Option<&S::Open> {
        (self.status() == Status::Unlocked)
            .then_some(self.open.as_ref())
            .flatten()
    }

    pub fn ledger_mut(&mut self) -> Option<&mut S::Open> {
        if self.status() == Status::Unlocked {
            self.open.as_mut()
        } else {
            None
        }
    }

    pub fn has_recovery_key(&self) -> Option<bool> {
        self.open.as_ref().map(OpenLedger::has_recovery_key)
    }

    /// The recovery key waiting for confirmation, to show again (e.g. after the
    /// window was reloaded). It is only ever held in memory.
    pub fn recovery_to_confirm(&self) -> Option<RecoveryToConfirm> {
        self.pending.as_ref().map(|p| RecoveryToConfirm {
            groups: p.key.groups(),
            challenge: p.challenge.groups,
        })
    }

    // ---- create and unlock -------------------------------------------------------

    /// Creates the ledger with `password` and a new recovery key, which the user must
    /// then confirm.
    pub fn create(&mut self, password: &str, now: Now) -> Result<RecoveryToConfirm, UnlockError> {
        self.expect_status(Status::NoLedger)?;
        let password = acceptable(password)?;
        let open = self.store.create(password.as_bytes())?;
        self.after_password(open, now);
        let result = self.new_recovery_key(&password);
        self.or_lock(result, now)
    }

    /// Completes a creation that was interrupted ([`Status::Incomplete`]).
    pub fn finish_create(
        &mut self,
        password: &str,
        now: Now,
    ) -> Result<Option<RecoveryToConfirm>, UnlockError> {
        self.expect_status(Status::Incomplete)?;
        let password = normalize(password);
        let open = self.store.finish_create(password.as_bytes())?;
        self.after_password(open, now);
        let result = self.recovery_check(&password);
        self.or_lock(result, now)
    }

    /// Unlocks with the master password. If the recovery key was never confirmed
    /// (the app closed first), a new one is made and returned for confirmation.
    pub fn unlock_with_password(
        &mut self,
        password: &str,
        now: Now,
    ) -> Result<Option<RecoveryToConfirm>, UnlockError> {
        self.expect_status(Status::Locked)?;
        let password = normalize(password);
        let open = self.store.unlock(password.as_bytes())?;
        self.after_password(open, now);
        let result = self.recovery_check(&password);
        self.or_lock(result, now)
    }

    /// Unlocks with the recovery key. Nothing else is possible until a new password is
    /// set with [`Session::set_password_after_recovery`].
    pub fn unlock_with_recovery_key(&mut self, text: &str, now: Now) -> Result<(), UnlockError> {
        self.expect_status(Status::Locked)?;
        let key = RecoveryKey::parse(text)?;
        let open = self.store.unlock_with_recovery_key(key.as_bytes())?;
        self.open = Some(open);
        self.recovery_unlock = Some(key);
        self.autolock = AutoLock::new(self.autolock_minutes(), now);
        Ok(())
    }

    /// After a recovery-key unlock: sets the new password and a new recovery key (the
    /// old one has been used). With `compromised`, the data key is replaced too, so
    /// old copies of the keyfile stop working.
    pub fn set_password_after_recovery(
        &mut self,
        new_password: &str,
        compromised: bool,
        now: Now,
    ) -> Result<RecoveryToConfirm, UnlockError> {
        self.expect_status(Status::NeedsNewPassword)?;
        let password = acceptable(new_password)?;
        // Cloned, not taken: until the new password is in place the session must stay
        // in NeedsNewPassword, even if something below fails.
        let recovery = self.recovery_unlock.clone().expect("NeedsNewPassword");
        let open = self.open.as_mut().expect("open");
        open.reset_password(recovery.as_bytes(), password.as_bytes())?;
        let open = self.open.take().expect("open");
        self.after_password(open, now);
        // The password changed; if the new recovery key can't be set up, lock rather
        // than leave the ledger open without one confirmed.
        let result = self.replace_recovery_key(&password, compromised);
        self.or_lock(result, now)
    }

    /// Unlocks with Touch ID, if the rules allow it now.
    ///
    /// Every prompt that doesn't end in a match counts as a failed scan, including a
    /// dismissed one: the system prompt can take several fingers before the user
    /// cancels, and those attempts aren't reported separately.
    pub fn unlock_with_touch_id(&mut self, now: Now) -> Result<(), UnlockError> {
        self.policy.observe(now);
        if let Err(blocked) = self.touch_id_available(now) {
            if matches!(blocked, TouchIdBlocked::Password(_)) {
                self.forget_touch_id();
            }
            return Err(UnlockError::TouchIdBlocked(blocked));
        }
        let data_key = match self.keychain.load(TOUCH_ID_REASON) {
            Ok(key) => key,
            Err(e) => {
                match e {
                    TouchIdError::Invalidated | TouchIdError::Unavailable => self.forget_touch_id(),
                    TouchIdError::Failed | TouchIdError::Cancelled | TouchIdError::Other(_) => {
                        self.policy.scan_failed();
                        if self.policy.check(now).is_err() {
                            self.forget_touch_id();
                        }
                    }
                }
                return Err(UnlockError::TouchId(e));
            }
        };
        let open = match self.store.unlock_with_data_key(&data_key) {
            Ok(open) => open,
            Err(e) => {
                // The stored key no longer opens the ledger.
                self.forget_touch_id();
                return Err(e.into());
            }
        };
        // A recovery key that was never confirmed can only be replaced with the
        // password, so Touch ID can't finish this unlock.
        if !matches!(open.setting(SETTING_RECOVERY_UNCONFIRMED), Ok(None)) {
            drop(open);
            self.forget_touch_id();
            return Err(UnlockError::TouchIdBlocked(TouchIdBlocked::Password(
                PasswordRequired::RecoveryKeyUnconfirmed,
            )));
        }
        self.policy.scan_succeeded();
        self.autolock = AutoLock::new(setting_minutes(&open), now);
        self.open = Some(open);
        Ok(())
    }

    /// `Ok` if the UI may offer Touch ID now.
    pub fn touch_id_available(&self, now: Now) -> Result<(), TouchIdBlocked> {
        if self.status() != Status::Locked {
            return Err(TouchIdBlocked::NotLocked);
        }
        self.policy.check(now).map_err(TouchIdBlocked::Password)?;
        if self.touch_id_stored {
            Ok(())
        } else {
            Err(TouchIdBlocked::Unavailable)
        }
    }

    /// Deletes the Keychain copy of the data key. Called when the Touch ID rules
    /// expire, and by anything that destroys or replaces the key (T5.3 shredding).
    /// If the delete fails, it is retried on every [`Session::tick`]; the item is never
    /// offered meanwhile.
    pub fn forget_touch_id(&mut self) {
        self.touch_id_stored = false;
        self.delete_pending = self.keychain.delete().is_err();
    }

    /// True while a Keychain delete is still waiting to be retried.
    pub fn touch_id_delete_pending(&self) -> bool {
        self.delete_pending
    }

    // ---- recovery key confirmation -----------------------------------------------

    /// Checks the groups typed back. On success the key is forgotten (it was shown
    /// once) and the ledger opens fully.
    pub fn confirm_recovery_key(&mut self, answers: &[&str]) -> Result<(), UnlockError> {
        self.expect_status(Status::ConfirmRecoveryKey)?;
        let pending = self.pending.as_ref().expect("ConfirmRecoveryKey");
        if !pending.challenge.check(&pending.key, answers) {
            return Err(UnlockError::ConfirmationMismatch);
        }
        let open = self.open.as_ref().expect("open");
        open.set_setting(SETTING_RECOVERY_UNCONFIRMED, None)?;
        self.pending = None;
        Ok(())
    }

    // ---- while unlocked ------------------------------------------------------------

    /// Changes the master password. With `compromised`, the data key is replaced as
    /// well and a new recovery key is returned for confirmation.
    pub fn change_password(
        &mut self,
        current: &str,
        new_password: &str,
        compromised: bool,
        now: Now,
    ) -> Result<Option<RecoveryToConfirm>, UnlockError> {
        self.expect_status(Status::Unlocked)?;
        let current = normalize(current);
        let password = acceptable(new_password)?;
        let open = self.open.as_mut().expect("open");
        open.set_password(current.as_bytes(), password.as_bytes())?;
        let open = self.open.take().expect("open");
        self.after_password(open, now);
        if compromised {
            let result = self.replace_recovery_key(&password, true).map(Some);
            self.or_lock(result, now)
        } else {
            Ok(None)
        }
    }

    pub fn autolock_minutes(&self) -> u64 {
        self.open.as_ref().map_or(DEFAULT_MINUTES, setting_minutes)
    }

    pub fn set_autolock_minutes(&mut self, minutes: u64, now: Now) -> Result<(), UnlockError> {
        self.expect_status(Status::Unlocked)?;
        self.autolock.set_minutes(minutes, now);
        let chosen = self.autolock.timeout().as_secs() / 60;
        let open = self.open.as_ref().expect("open");
        open.set_setting(SETTING_AUTOLOCK_MINUTES, Some(&chosen.to_string()))?;
        Ok(())
    }

    /// The user did something; restarts the idle timer.
    pub fn activity(&mut self, now: Now) {
        if self.open.is_some() {
            self.autolock.activity(now);
        }
    }

    /// Call periodically. Locks if the idle timeout has passed (returns true if it
    /// locked), and deletes the Touch ID copy once the rules no longer allow it.
    pub fn tick(&mut self, now: Now) -> bool {
        self.policy.observe(now);
        if self.delete_pending || (self.touch_id_stored && self.policy.check(now).is_err()) {
            self.forget_touch_id();
        }
        if self.open.is_some() && self.autolock.expired(now) {
            self.lock(now);
            true
        } else {
            false
        }
    }

    pub fn idle_remaining(&self, now: Now) -> Option<std::time::Duration> {
        self.open.as_ref().map(|_| self.autolock.remaining(now))
    }

    /// Locks: the database closes and the data key is wiped from memory. The Keychain
    /// copy stays for Touch ID unless the rules no longer allow it, or the ledger was
    /// opened with the recovery key, or a new recovery key is still unconfirmed (that
    /// needs the password again).
    pub fn lock(&mut self, now: Now) {
        self.policy.observe(now);
        let Some(open) = self.open.take() else {
            return;
        };
        if self.recovery_unlock.is_some()
            || self.pending.is_some()
            || self.policy.check(now).is_err()
        {
            self.forget_touch_id();
        }
        self.recovery_unlock = None;
        self.pending = None;
        drop(open);
    }

    // ---- internals -----------------------------------------------------------------

    /// Passes `result` through, locking first if it is an error: used for steps after
    /// the ledger was opened, so a failure never leaves it open half set up.
    fn or_lock<T>(&mut self, result: Result<T, UnlockError>, now: Now) -> Result<T, UnlockError> {
        if result.is_err() {
            self.lock(now);
        }
        result
    }

    fn expect_status(&self, expected: Status) -> Result<(), UnlockError> {
        match self.status() {
            s if s == expected => Ok(()),
            s => Err(UnlockError::WrongState(s)),
        }
    }

    /// Bookkeeping after the password itself was used: the Touch ID rules restart and
    /// the Keychain copy is replaced with the current data key.
    fn after_password(&mut self, open: S::Open, now: Now) {
        self.policy.password_unlocked(now);
        self.autolock = AutoLock::new(setting_minutes(&open), now);
        self.recovery_unlock = None;
        self.open = Some(open);
        self.refresh_touch_id();
    }

    /// Deletes the Keychain copy and stores the current data key in its place.
    fn refresh_touch_id(&mut self) {
        self.forget_touch_id();
        if !self.keychain.available() {
            return;
        }
        let open = self.open.as_ref().expect("open");
        self.touch_id_stored = self.keychain.store(&open.data_key_copy()).is_ok();
    }

    /// After a password unlock: makes a new recovery key if the last one was never
    /// confirmed or none exists.
    fn recovery_check(
        &mut self,
        password: &Zeroizing<String>,
    ) -> Result<Option<RecoveryToConfirm>, UnlockError> {
        let open = self.open.as_ref().expect("open");
        let unconfirmed = open.setting(SETTING_RECOVERY_UNCONFIRMED)?.is_some();
        if unconfirmed || !open.has_recovery_key() {
            self.new_recovery_key(password).map(Some)
        } else {
            Ok(None)
        }
    }

    fn new_recovery_key(
        &mut self,
        password: &Zeroizing<String>,
    ) -> Result<RecoveryToConfirm, UnlockError> {
        self.replace_recovery_key(password, false)
    }

    /// Sets a new recovery key (rotating the data key too if `rotate`), marks it
    /// unconfirmed, and returns it for display.
    fn replace_recovery_key(
        &mut self,
        password: &Zeroizing<String>,
        rotate: bool,
    ) -> Result<RecoveryToConfirm, UnlockError> {
        let key = RecoveryKey::generate()?;
        let challenge = Challenge::random()?;
        let open = self.open.as_mut().expect("open");
        // Flag first: if the app stops before the user confirms, the next unlock
        // makes yet another key rather than trusting one nobody saw.
        open.set_setting(SETTING_RECOVERY_UNCONFIRMED, Some("1"))?;
        if rotate {
            open.rotate_data_key(password.as_bytes(), Some(key.as_bytes()))?;
            // The Keychain held the old data key.
            self.refresh_touch_id();
        } else {
            open.set_recovery_key(password.as_bytes(), key.as_bytes())?;
        }
        let shown = RecoveryToConfirm {
            groups: key.groups(),
            challenge: challenge.groups,
        };
        self.pending = Some(Pending { key, challenge });
        Ok(shown)
    }
}

fn acceptable(password: &str) -> Result<Zeroizing<String>, UnlockError> {
    let s = strength(password);
    if s.acceptable {
        Ok(normalize(password))
    } else {
        Err(UnlockError::WeakPassword(s))
    }
}

fn setting_minutes<O: OpenLedger>(open: &O) -> u64 {
    open.setting(SETTING_AUTOLOCK_MINUTES)
        .ok()
        .flatten()
        .and_then(|m| m.parse().ok())
        .unwrap_or(DEFAULT_MINUTES)
}
