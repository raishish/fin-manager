//! Session tests against an in-memory ledger and a scripted Keychain. The real storage
//! is exercised by `storage::tests`.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use zeroize::Zeroizing;

use super::*;
use crate::storage::{StorageError, VaultState};

const STRONG: &str = "velvet anchor mosaic tundra";
const STRONG_2: &str = "purple monkey dishwasher";
const STRONG_3: &str = "blue-castle-river-42";
const MIN: Duration = Duration::from_secs(60);
const DAY: Duration = Duration::from_secs(24 * 3600);

// ---- fakes ---------------------------------------------------------------------------

#[derive(Default)]
struct Disk {
    keyfile: bool,
    database: bool,
    password: Vec<u8>,
    recovery: Option<Vec<u8>>,
    data_key: u8,
    settings: HashMap<String, String>,
    rotations: u32,
}

#[derive(Clone, Default)]
struct FakeStore(Rc<RefCell<Disk>>);

struct FakeOpen(Rc<RefCell<Disk>>);

impl FakeStore {
    fn disk(&self) -> std::cell::RefMut<'_, Disk> {
        self.0.borrow_mut()
    }
}

impl LedgerStore for FakeStore {
    type Open = FakeOpen;

    fn state(&self) -> VaultState {
        let d = self.0.borrow();
        match (d.keyfile, d.database) {
            (false, false) => VaultState::Missing,
            (true, true) => VaultState::Ready,
            (true, false) => VaultState::Incomplete,
            (false, true) => VaultState::Orphaned,
        }
    }

    fn create(&self, password: &[u8]) -> Result<FakeOpen, StorageError> {
        let mut d = self.disk();
        if d.keyfile {
            return Err(StorageError::AlreadyExists);
        }
        d.keyfile = true;
        d.database = true;
        d.password = password.to_vec();
        d.data_key = 1;
        Ok(FakeOpen(self.0.clone()))
    }

    fn finish_create(&self, password: &[u8]) -> Result<FakeOpen, StorageError> {
        let mut d = self.disk();
        if d.password != password {
            return Err(StorageError::WrongSecret);
        }
        d.database = true;
        Ok(FakeOpen(self.0.clone()))
    }

    fn unlock(&self, password: &[u8]) -> Result<FakeOpen, StorageError> {
        if self.0.borrow().password != password {
            return Err(StorageError::WrongSecret);
        }
        Ok(FakeOpen(self.0.clone()))
    }

    fn unlock_with_recovery_key(&self, recovery_key: &[u8]) -> Result<FakeOpen, StorageError> {
        if self.0.borrow().recovery.as_deref() != Some(recovery_key) {
            return Err(StorageError::WrongSecret);
        }
        Ok(FakeOpen(self.0.clone()))
    }

    fn unlock_with_data_key(&self, key: &[u8; 32]) -> Result<FakeOpen, StorageError> {
        if *key != [self.0.borrow().data_key; 32] {
            return Err(StorageError::DatabaseKeyRejected);
        }
        Ok(FakeOpen(self.0.clone()))
    }
}

impl OpenLedger for FakeOpen {
    fn has_recovery_key(&self) -> bool {
        self.0.borrow().recovery.is_some()
    }

    fn set_password(&mut self, current: &[u8], new_password: &[u8]) -> Result<(), StorageError> {
        let mut d = self.0.borrow_mut();
        if d.password != current {
            return Err(StorageError::WrongSecret);
        }
        d.password = new_password.to_vec();
        Ok(())
    }

    fn reset_password(&mut self, recovery: &[u8], new_password: &[u8]) -> Result<(), StorageError> {
        let mut d = self.0.borrow_mut();
        if d.recovery.as_deref() != Some(recovery) {
            return Err(StorageError::WrongSecret);
        }
        d.password = new_password.to_vec();
        Ok(())
    }

    fn set_recovery_key(&mut self, current: &[u8], key: &[u8]) -> Result<(), StorageError> {
        let mut d = self.0.borrow_mut();
        if d.password != current {
            return Err(StorageError::WrongSecret);
        }
        d.recovery = Some(key.to_vec());
        Ok(())
    }

    fn rotate_data_key(
        &mut self,
        password: &[u8],
        new_recovery_key: Option<&[u8]>,
    ) -> Result<(), StorageError> {
        let mut d = self.0.borrow_mut();
        if d.password != password {
            return Err(StorageError::WrongSecret);
        }
        d.data_key += 1;
        d.rotations += 1;
        d.recovery = new_recovery_key.map(<[u8]>::to_vec);
        Ok(())
    }

    fn data_key_copy(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new([self.0.borrow().data_key; 32])
    }

    fn setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        Ok(self.0.borrow().settings.get(key).cloned())
    }

    fn set_setting(&self, key: &str, value: Option<&str>) -> Result<(), StorageError> {
        let mut d = self.0.borrow_mut();
        match value {
            Some(v) => d.settings.insert(key.to_string(), v.to_string()),
            None => d.settings.remove(key),
        };
        Ok(())
    }
}

/// The Keychain and Touch ID hardware, as a test sees them: whether Touch ID exists,
/// which finger set is enrolled, the stored item, and what the next prompts return.
#[derive(Default)]
struct KeychainCtl {
    unavailable: bool,
    enrolment: u32,
    /// The stored data key and the enrolment it is bound to.
    item: Option<([u8; 32], u32)>,
    prompts: VecDeque<Result<(), TouchIdError>>,
    prompts_shown: u32,
    deletes: u32,
}

#[derive(Clone, Default)]
struct FakeKeychain(Rc<RefCell<KeychainCtl>>);

impl FakeKeychain {
    fn ctl(&self) -> std::cell::RefMut<'_, KeychainCtl> {
        self.0.borrow_mut()
    }

    fn stored_key(&self) -> Option<u8> {
        self.0.borrow().item.map(|(k, _)| k[0])
    }
}

impl TouchIdKeychain for FakeKeychain {
    fn available(&self) -> bool {
        !self.0.borrow().unavailable
    }

    fn store(&self, data_key: &[u8; 32]) -> Result<(), TouchIdError> {
        let mut ctl = self.ctl();
        if ctl.unavailable {
            return Err(TouchIdError::Unavailable);
        }
        ctl.item = Some((*data_key, ctl.enrolment));
        Ok(())
    }

    fn load(&self, _reason: &str) -> Result<Zeroizing<[u8; 32]>, TouchIdError> {
        let mut ctl = self.ctl();
        ctl.prompts_shown += 1;
        let (key, enrolment) = ctl.item.ok_or(TouchIdError::Unavailable)?;
        if enrolment != ctl.enrolment {
            return Err(TouchIdError::Invalidated);
        }
        ctl.prompts.pop_front().unwrap_or(Ok(()))?;
        Ok(Zeroizing::new(key))
    }

    fn delete(&self) -> Result<(), TouchIdError> {
        let mut ctl = self.ctl();
        ctl.item = None;
        ctl.deletes += 1;
        Ok(())
    }
}

type TestSession = Session<FakeStore, FakeKeychain>;

struct World {
    store: FakeStore,
    keychain: FakeKeychain,
    t0: Now,
}

impl World {
    fn new() -> Self {
        Self {
            store: FakeStore::default(),
            keychain: FakeKeychain::default(),
            t0: Now::current(),
        }
    }

    /// A fresh run of the app over the same disk and machine.
    fn start(&self) -> TestSession {
        Session::new(self.store.clone(), self.keychain.clone(), self.t0)
    }

    /// A ledger created with [`STRONG`] and its recovery key confirmed. Returns the
    /// session and the recovery key's text.
    fn created(&self) -> (TestSession, String) {
        let mut s = self.start();
        let shown = s.create(STRONG, self.t0).unwrap();
        let text = join(&shown);
        confirm(&mut s, &shown);
        (s, text)
    }
}

fn join(shown: &RecoveryToConfirm) -> String {
    shown
        .groups
        .iter()
        .map(|g| g.as_str())
        .collect::<Vec<_>>()
        .join("-")
}

fn answers(shown: &RecoveryToConfirm) -> Vec<String> {
    shown
        .challenge
        .iter()
        .map(|&i| shown.groups[i].to_string())
        .collect()
}

fn confirm(s: &mut TestSession, shown: &RecoveryToConfirm) {
    let a = answers(shown);
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    s.confirm_recovery_key(&a).unwrap();
}

// ---- master password -------------------------------------------------------------

#[test]
fn a_weak_password_cannot_create_a_ledger() {
    let w = World::new();
    let mut s = w.start();
    let err = s.create("Summer2024!!!", w.t0).unwrap_err();
    assert!(matches!(err, UnlockError::WeakPassword(ref st) if !st.acceptable));
    assert_eq!(s.status(), Status::NoLedger);
}

#[test]
fn a_weak_password_cannot_replace_the_current_one() {
    let w = World::new();
    let (mut s, _) = w.created();
    let err = s.change_password(STRONG, "short", false, w.t0).unwrap_err();
    assert!(matches!(err, UnlockError::WeakPassword(_)));
    s.lock(w.t0);
    s.unlock_with_password(STRONG, w.t0).unwrap();
}

#[test]
fn the_password_unlocks_and_a_wrong_one_does_not() {
    let w = World::new();
    let (mut s, _) = w.created();
    s.lock(w.t0);
    assert!(matches!(
        s.unlock_with_password(STRONG_2, w.t0),
        Err(UnlockError::WrongSecret)
    ));
    assert_eq!(s.status(), Status::Locked);
    assert!(s.unlock_with_password(STRONG, w.t0).unwrap().is_none());
    assert_eq!(s.status(), Status::Unlocked);
}

#[test]
fn passwords_are_normalised_before_use() {
    let w = World::new();
    let composed = "caf\u{e9} cr\u{e8}me br\u{fb}l\u{e9}e tundra";
    let decomposed = "cafe\u{301} cre\u{300}me bru\u{302}le\u{301}e tundra";
    let mut s = w.start();
    let shown = s.create(composed, w.t0).unwrap();
    confirm(&mut s, &shown);
    s.lock(w.t0);
    s.unlock_with_password(decomposed, w.t0).unwrap();
    assert_eq!(s.status(), Status::Unlocked);
}

#[test]
fn a_new_password_replaces_the_old_one() {
    let w = World::new();
    let (mut s, _) = w.created();
    assert!(
        s.change_password(STRONG, STRONG_2, false, w.t0)
            .unwrap()
            .is_none()
    );
    assert_eq!(w.store.disk().rotations, 0);
    s.lock(w.t0);
    assert!(s.unlock_with_password(STRONG, w.t0).is_err());
    s.unlock_with_password(STRONG_2, w.t0).unwrap();
}

#[test]
fn a_compromised_password_also_rotates_the_data_key_and_recovery_key() {
    let w = World::new();
    let (mut s, old_recovery) = w.created();
    let shown = s
        .change_password(STRONG, STRONG_2, true, w.t0)
        .unwrap()
        .expect("a new recovery key to confirm");
    assert_eq!(w.store.disk().rotations, 1);
    assert_eq!(s.status(), Status::ConfirmRecoveryKey);
    confirm(&mut s, &shown);
    s.lock(w.t0);
    assert!(s.unlock_with_recovery_key(&old_recovery, w.t0).is_err());
    s.unlock_with_recovery_key(&join(&shown), w.t0).unwrap();
}

// ---- recovery key ----------------------------------------------------------------

#[test]
fn the_ledger_stays_closed_until_the_recovery_key_is_confirmed() {
    let w = World::new();
    let mut s = w.start();
    let shown = s.create(STRONG, w.t0).unwrap();
    assert_eq!(s.status(), Status::ConfirmRecoveryKey);
    assert!(s.ledger().is_none());
    assert_eq!(shown.groups.len(), 14);
    assert!(matches!(
        s.confirm_recovery_key(&["0000", "0000", "0000"]),
        Err(UnlockError::ConfirmationMismatch)
    ));
    confirm(&mut s, &shown);
    assert_eq!(s.status(), Status::Unlocked);
    assert!(s.ledger().is_some());
    assert_eq!(s.has_recovery_key(), Some(true));
}

#[test]
fn an_unconfirmed_recovery_key_is_replaced_at_the_next_unlock() {
    let w = World::new();
    let mut s = w.start();
    let first = s.create(STRONG, w.t0).unwrap();
    drop(s); // the app quits before the user confirms

    let mut s = w.start();
    let second = s
        .unlock_with_password(STRONG, w.t0)
        .unwrap()
        .expect("a new key, since nobody confirmed the first");
    assert_ne!(join(&first), join(&second));
    confirm(&mut s, &second);
    s.lock(w.t0);
    assert!(s.unlock_with_recovery_key(&join(&first), w.t0).is_err());
    assert!(
        s.unlock_with_password(STRONG, w.t0).unwrap().is_none(),
        "confirmed now"
    );
}

#[test]
fn the_recovery_key_unlocks_but_a_new_password_comes_first() {
    let w = World::new();
    let (mut s, recovery) = w.created();
    s.lock(w.t0);
    assert!(matches!(
        s.unlock_with_recovery_key("not-a-key", w.t0),
        Err(UnlockError::RecoveryKey(_))
    ));
    s.unlock_with_recovery_key(&recovery.to_lowercase().replace('-', " "), w.t0)
        .unwrap();
    assert_eq!(s.status(), Status::NeedsNewPassword);
    assert!(s.ledger().is_none());
    assert!(matches!(
        s.set_password_after_recovery("weak", false, w.t0),
        Err(UnlockError::WeakPassword(_))
    ));

    let shown = s
        .set_password_after_recovery(STRONG_3, false, w.t0)
        .unwrap();
    assert_eq!(w.store.disk().rotations, 0);
    confirm(&mut s, &shown);
    assert_eq!(s.status(), Status::Unlocked);
    s.lock(w.t0);
    // Old password and the used recovery key no longer work; the new ones do.
    assert!(s.unlock_with_password(STRONG, w.t0).is_err());
    assert!(s.unlock_with_recovery_key(&recovery, w.t0).is_err());
    s.unlock_with_password(STRONG_3, w.t0).unwrap();
    s.lock(w.t0);
    s.unlock_with_recovery_key(&join(&shown), w.t0).unwrap();
}

#[test]
fn recovery_after_a_compromise_rotates_the_data_key() {
    let w = World::new();
    let (mut s, recovery) = w.created();
    s.lock(w.t0);
    s.unlock_with_recovery_key(&recovery, w.t0).unwrap();
    let shown = s.set_password_after_recovery(STRONG_3, true, w.t0).unwrap();
    assert_eq!(w.store.disk().rotations, 1);
    confirm(&mut s, &shown);
}

#[test]
fn an_interrupted_create_is_finished_with_the_password() {
    let w = World::new();
    {
        let mut d = w.store.disk();
        d.keyfile = true;
        d.password = normalize(STRONG).as_bytes().to_vec();
    }
    let mut s = w.start();
    assert_eq!(s.status(), Status::Incomplete);
    let shown = s
        .finish_create(STRONG, w.t0)
        .unwrap()
        .expect("no recovery key yet");
    confirm(&mut s, &shown);
    assert_eq!(s.status(), Status::Unlocked);
}

// ---- lock and auto-lock ----------------------------------------------------------

#[test]
fn locking_closes_the_ledger() {
    let w = World::new();
    let (mut s, _) = w.created();
    s.lock(w.t0);
    assert_eq!(s.status(), Status::Locked);
    assert!(s.ledger().is_none());
    assert!(s.has_recovery_key().is_none());
}

#[test]
fn the_ledger_locks_after_the_idle_timeout() {
    let w = World::new();
    let (mut s, _) = w.created();
    assert_eq!(s.autolock_minutes(), DEFAULT_MINUTES);
    assert!(!s.tick(w.t0.plus(9 * MIN)));
    s.activity(w.t0.plus(9 * MIN));
    assert!(!s.tick(w.t0.plus(18 * MIN)));
    assert!(s.tick(w.t0.plus(19 * MIN)));
    assert_eq!(s.status(), Status::Locked);
}

#[test]
fn the_idle_timeout_is_a_per_ledger_setting() {
    let w = World::new();
    let (mut s, _) = w.created();
    s.set_autolock_minutes(1, w.t0).unwrap();
    s.lock(w.t0);
    s.unlock_with_password(STRONG, w.t0).unwrap();
    assert_eq!(s.autolock_minutes(), 1);
    assert!(s.tick(w.t0.plus(MIN)));
}

#[test]
fn sleep_counts_towards_the_idle_timeout() {
    let w = World::new();
    let (mut s, _) = w.created();
    let woke = Now {
        wall: w.t0.wall + 3600 * MIN,
        mono: w.t0.mono,
    };
    assert!(s.tick(woke));
}

// ---- Touch ID --------------------------------------------------------------------

fn locked_with_touch_id(w: &World) -> TestSession {
    let (mut s, _) = w.created();
    s.lock(w.t0);
    assert_eq!(s.touch_id_available(w.t0), Ok(()));
    s
}

#[test]
fn touch_id_reopens_a_ledger_locked_in_this_run() {
    let w = World::new();
    let mut s = locked_with_touch_id(&w);
    s.unlock_with_touch_id(w.t0.plus(MIN)).unwrap();
    assert_eq!(s.status(), Status::Unlocked);
    // It can be used again after the next lock.
    s.lock(w.t0.plus(2 * MIN));
    s.unlock_with_touch_id(w.t0.plus(3 * MIN)).unwrap();
}

#[test]
fn the_password_is_needed_after_a_restart() {
    let w = World::new();
    let s = locked_with_touch_id(&w);
    drop(s);
    let mut s = w.start();
    assert_eq!(
        s.touch_id_available(w.t0),
        Err(TouchIdBlocked::Password(PasswordRequired::NotSinceStart))
    );
    assert!(matches!(
        s.unlock_with_touch_id(w.t0),
        Err(UnlockError::TouchIdBlocked(_))
    ));
    assert_eq!(w.keychain.0.borrow().prompts_shown, 0);
}

#[test]
fn the_password_is_needed_after_five_failed_scans() {
    let w = World::new();
    let mut s = locked_with_touch_id(&w);
    for _ in 0..MAX_FAILED_SCANS {
        w.keychain
            .ctl()
            .prompts
            .push_back(Err(TouchIdError::Failed));
        assert!(matches!(
            s.unlock_with_touch_id(w.t0),
            Err(UnlockError::TouchId(TouchIdError::Failed))
        ));
    }
    assert_eq!(
        s.touch_id_available(w.t0),
        Err(TouchIdBlocked::Password(
            PasswordRequired::TooManyFailedScans
        ))
    );
    let shown = w.keychain.0.borrow().prompts_shown;
    assert!(s.unlock_with_touch_id(w.t0).is_err());
    assert_eq!(
        w.keychain.0.borrow().prompts_shown,
        shown,
        "no sixth prompt"
    );
    s.unlock_with_password(STRONG, w.t0).unwrap();
    s.lock(w.t0);
    assert_eq!(s.touch_id_available(w.t0), Ok(()));
}

#[test]
fn cancelling_the_prompt_is_not_a_failed_scan() {
    let w = World::new();
    let mut s = locked_with_touch_id(&w);
    for _ in 0..10 {
        w.keychain
            .ctl()
            .prompts
            .push_back(Err(TouchIdError::Cancelled));
        assert!(s.unlock_with_touch_id(w.t0).is_err());
    }
    assert_eq!(s.touch_id_available(w.t0), Ok(()));
}

#[test]
fn the_password_is_needed_every_seven_days() {
    let w = World::new();
    let mut s = locked_with_touch_id(&w);
    // Touch ID unlocks don't restart the week; only the password does.
    s.unlock_with_touch_id(w.t0.plus(6 * DAY)).unwrap();
    s.lock(w.t0.plus(6 * DAY));
    assert_eq!(
        s.touch_id_available(w.t0.plus(7 * DAY)),
        Err(TouchIdBlocked::Password(PasswordRequired::Weekly))
    );
    s.unlock_with_password(STRONG, w.t0.plus(7 * DAY)).unwrap();
    s.lock(w.t0.plus(7 * DAY));
    assert_eq!(s.touch_id_available(w.t0.plus(13 * DAY)), Ok(()));
}

#[test]
fn the_keychain_copy_is_deleted_when_the_week_runs_out() {
    let w = World::new();
    let mut s = locked_with_touch_id(&w);
    assert!(w.keychain.stored_key().is_some());
    s.tick(w.t0.plus(6 * DAY));
    assert!(w.keychain.stored_key().is_some());
    s.tick(w.t0.plus(7 * DAY));
    assert_eq!(w.keychain.stored_key(), None);
    // Also when the week ends while unlocked.
    let w = World::new();
    let (mut s, _) = w.created();
    s.lock(w.t0.plus(8 * DAY));
    assert_eq!(w.keychain.stored_key(), None);
}

#[test]
fn the_keychain_copy_is_deleted_after_five_failed_scans() {
    let w = World::new();
    let mut s = locked_with_touch_id(&w);
    for _ in 0..MAX_FAILED_SCANS {
        w.keychain
            .ctl()
            .prompts
            .push_back(Err(TouchIdError::Failed));
        let _ = s.unlock_with_touch_id(w.t0);
    }
    assert_eq!(w.keychain.stored_key(), None);
}

#[test]
fn the_keychain_holds_the_current_data_key() {
    let w = World::new();
    let (mut s, _) = w.created();
    assert_eq!(w.keychain.stored_key(), Some(1));
    // A plain password change: deleted and stored again (same data key).
    let deletes = w.keychain.0.borrow().deletes;
    s.change_password(STRONG, STRONG_2, false, w.t0).unwrap();
    assert!(w.keychain.0.borrow().deletes > deletes);
    assert_eq!(w.keychain.stored_key(), Some(1));
    // Rotation: the old key is deleted and the new one stored.
    let shown = s
        .change_password(STRONG_2, STRONG_3, true, w.t0)
        .unwrap()
        .unwrap();
    confirm(&mut s, &shown);
    assert_eq!(w.store.disk().data_key, 2);
    assert_eq!(w.keychain.stored_key(), Some(2));
    s.lock(w.t0);
    s.unlock_with_touch_id(w.t0).unwrap();
}

#[test]
fn forgetting_touch_id_deletes_the_keychain_copy() {
    let w = World::new();
    let mut s = locked_with_touch_id(&w);
    s.forget_touch_id();
    assert_eq!(w.keychain.stored_key(), None);
    assert_eq!(s.touch_id_available(w.t0), Err(TouchIdBlocked::Unavailable));
}

#[test]
fn an_item_left_by_an_earlier_run_is_not_used() {
    let w = World::new();
    let s = locked_with_touch_id(&w);
    drop(s);
    let mut s = w.start();
    assert!(w.keychain.stored_key().is_some(), "still in the Keychain");
    assert!(s.unlock_with_touch_id(w.t0).is_err());
    assert_eq!(w.keychain.0.borrow().prompts_shown, 0);
    s.unlock_with_password(STRONG, w.t0).unwrap();
    s.lock(w.t0);
    s.unlock_with_touch_id(w.t0).unwrap();
}

#[test]
fn enrolling_a_new_finger_invalidates_touch_id() {
    let w = World::new();
    let mut s = locked_with_touch_id(&w);
    w.keychain.ctl().enrolment += 1;
    assert!(matches!(
        s.unlock_with_touch_id(w.t0),
        Err(UnlockError::TouchId(TouchIdError::Invalidated))
    ));
    assert_eq!(s.touch_id_available(w.t0), Err(TouchIdBlocked::Unavailable));
    assert_eq!(w.keychain.stored_key(), None);
    // The password brings it back, bound to the new set of fingers.
    s.unlock_with_password(STRONG, w.t0).unwrap();
    s.lock(w.t0);
    s.unlock_with_touch_id(w.t0).unwrap();
}

#[test]
fn macs_without_touch_id_use_the_password() {
    let w = World::new();
    w.keychain.ctl().unavailable = true;
    let (mut s, _) = w.created();
    s.lock(w.t0);
    assert_eq!(s.touch_id_available(w.t0), Err(TouchIdBlocked::Unavailable));

    let mut linux: Session<FakeStore, NoTouchId> = Session::new(w.store.clone(), NoTouchId, w.t0);
    linux.unlock_with_password(STRONG, w.t0).unwrap();
    linux.lock(w.t0);
    assert_eq!(
        linux.touch_id_available(w.t0),
        Err(TouchIdBlocked::Unavailable)
    );
}

#[test]
fn touch_id_is_not_set_up_while_a_recovery_key_is_unconfirmed() {
    let w = World::new();
    let mut s = w.start();
    s.create(STRONG, w.t0).unwrap();
    s.lock(w.t0);
    assert_eq!(s.touch_id_available(w.t0), Err(TouchIdBlocked::Unavailable));
}

#[test]
fn touch_id_is_not_set_up_by_a_recovery_key_unlock() {
    let w = World::new();
    let (s, recovery) = w.created();
    drop(s);
    let mut s = w.start();
    s.unlock_with_recovery_key(&recovery, w.t0).unwrap();
    s.lock(w.t0);
    assert!(s.touch_id_available(w.t0).is_err());
}

#[test]
fn a_sealed_key_that_no_longer_fits_disables_touch_id() {
    let w = World::new();
    let mut s = locked_with_touch_id(&w);
    w.store.disk().data_key = 9; // e.g. the ledger was restored from a backup
    assert!(s.unlock_with_touch_id(w.t0).is_err());
    assert_eq!(s.touch_id_available(w.t0), Err(TouchIdBlocked::Unavailable));
}

#[test]
fn calls_out_of_order_are_refused() {
    let w = World::new();
    let mut s = w.start();
    assert!(matches!(
        s.unlock_with_password(STRONG, w.t0),
        Err(UnlockError::WrongState(Status::NoLedger))
    ));
    let (mut s, _) = w.created();
    assert!(matches!(
        s.create(STRONG, w.t0),
        Err(UnlockError::WrongState(Status::Unlocked))
    ));
    assert!(matches!(
        s.confirm_recovery_key(&["a", "b", "c"]),
        Err(UnlockError::WrongState(Status::Unlocked))
    ));
}
