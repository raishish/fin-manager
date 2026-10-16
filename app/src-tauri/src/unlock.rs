//! The unlock screens' side of the app (T5.1). The rules live in
//! `fm_core::unlock::Session`; this module turns its results into what the UI shows
//! (`app/src/routes/unlock/api.ts` mirrors these types) and runs the auto-lock timer.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use fm_core::crash::CrashReporter;
use fm_core::storage::{Unlocked, Vault};
use fm_core::unlock::{
    CHOICES_MINUTES, LedgerStore, MIN_CHARS, Now, RecoveryToConfirm, Session, Status, Strength,
    TouchIdBlocked, TouchIdKeychain, UnlockError, VaultStore,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use zeroize::Zeroizing;

/// Emitted when auto-lock closes the ledger (`LOCKED_EVENT` in api.ts).
pub const LOCKED_EVENT: &str = "unlock://locked";
/// How often the auto-lock timer is checked.
pub const TICK: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StateView {
    status: &'static str,
    touch_id_blocked: Option<String>,
    has_recovery_key: Option<bool>,
    autolock_minutes: u64,
    autolock_choices: Vec<u64>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StrengthView {
    score: u8,
    chars: usize,
    acceptable: bool,
    warning: Option<String>,
    suggestions: Vec<String>,
    min_chars: usize,
}

impl From<Strength> for StrengthView {
    fn from(s: Strength) -> Self {
        Self {
            score: s.score,
            chars: s.chars,
            acceptable: s.acceptable,
            warning: s.warning,
            suggestions: s.suggestions,
            min_chars: MIN_CHARS,
        }
    }
}

/// A recovery key on its way to the screen that shows it once.
#[derive(Serialize)]
pub struct RecoveryView {
    groups: Vec<String>,
    challenge: Vec<usize>,
}

impl From<RecoveryToConfirm> for RecoveryView {
    fn from(r: RecoveryToConfirm) -> Self {
        Self {
            groups: r.groups.iter().map(|g| g.to_string()).collect(),
            challenge: r.challenge.to_vec(),
        }
    }
}

impl std::fmt::Debug for RecoveryView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveryView").finish_non_exhaustive()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorView {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    strength: Option<StrengthView>,
}

impl From<UnlockError> for ErrorView {
    fn from(e: UnlockError) -> Self {
        let message = e.to_string();
        let (code, strength) = match e {
            UnlockError::WeakPassword(s) => ("WeakPassword", Some(s.into())),
            UnlockError::WrongSecret => ("WrongSecret", None),
            UnlockError::RecoveryKey(_) => ("RecoveryKey", None),
            UnlockError::ConfirmationMismatch => ("ConfirmationMismatch", None),
            UnlockError::TouchIdBlocked(_) => ("TouchIdBlocked", None),
            UnlockError::TouchId(_) => ("TouchId", None),
            UnlockError::WrongState(_) => ("WrongState", None),
            UnlockError::Storage(_) => ("Storage", None),
        };
        Self {
            code,
            message,
            strength,
        }
    }
}

type Result<T> = std::result::Result<T, ErrorView>;

/// The app's one unlock session, shared by the commands and the timer thread.
pub struct UnlockState<S: LedgerStore, K: TouchIdKeychain> {
    session: Mutex<Session<S, K>>,
}

impl<S: LedgerStore, K: TouchIdKeychain> UnlockState<S, K> {
    pub fn new(store: S, keychain: K) -> Self {
        Self {
            session: Mutex::new(Session::new(store, keychain, Now::current())),
        }
    }

    fn session(&self) -> MutexGuard<'_, Session<S, K>> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `f` on the open ledger, if it is fully unlocked.
    pub fn with_ledger<T>(&self, f: impl FnOnce(&S::Open) -> T) -> Option<T> {
        self.session().ledger().map(f)
    }

    pub fn view(&self) -> StateView {
        let s = self.session();
        let now = Now::current();
        StateView {
            status: status_name(s.status()),
            touch_id_blocked: match s.touch_id_available(now) {
                Ok(()) => None,
                Err(TouchIdBlocked::Password(why)) => Some(why.to_string()),
                Err(TouchIdBlocked::Unavailable) => Some("Touch ID isn't available".into()),
                Err(TouchIdBlocked::NotLocked) => Some("not locked".into()),
            },
            has_recovery_key: s.has_recovery_key(),
            autolock_minutes: s.autolock_minutes(),
            autolock_choices: CHOICES_MINUTES.to_vec(),
        }
    }

    pub fn create(&self, password: Zeroizing<String>) -> Result<RecoveryView> {
        Ok(self.session().create(&password, Now::current())?.into())
    }

    pub fn finish_create(&self, password: Zeroizing<String>) -> Result<Option<RecoveryView>> {
        let shown = self.session().finish_create(&password, Now::current())?;
        Ok(shown.map(Into::into))
    }

    pub fn unlock_with_password(
        &self,
        password: Zeroizing<String>,
    ) -> Result<Option<RecoveryView>> {
        let shown = self
            .session()
            .unlock_with_password(&password, Now::current())?;
        Ok(shown.map(Into::into))
    }

    pub fn unlock_with_recovery_key(&self, key: Zeroizing<String>) -> Result<()> {
        Ok(self
            .session()
            .unlock_with_recovery_key(&key, Now::current())?)
    }

    pub fn set_password_after_recovery(
        &self,
        password: Zeroizing<String>,
        compromised: bool,
    ) -> Result<RecoveryView> {
        let shown =
            self.session()
                .set_password_after_recovery(&password, compromised, Now::current())?;
        Ok(shown.into())
    }

    pub fn unlock_with_touch_id(&self) -> Result<()> {
        Ok(self.session().unlock_with_touch_id(Now::current())?)
    }

    pub fn recovery_to_confirm(&self) -> Option<RecoveryView> {
        self.session().recovery_to_confirm().map(Into::into)
    }

    pub fn confirm_recovery_key(&self, answers: &[String]) -> Result<()> {
        let answers: Vec<&str> = answers.iter().map(String::as_str).collect();
        Ok(self.session().confirm_recovery_key(&answers)?)
    }

    pub fn change_password(
        &self,
        current: Zeroizing<String>,
        password: Zeroizing<String>,
        compromised: bool,
    ) -> Result<Option<RecoveryView>> {
        let shown =
            self.session()
                .change_password(&current, &password, compromised, Now::current())?;
        Ok(shown.map(Into::into))
    }

    pub fn set_autolock_minutes(&self, minutes: u64) -> Result<()> {
        Ok(self
            .session()
            .set_autolock_minutes(minutes, Now::current())?)
    }

    pub fn lock(&self) {
        self.session().lock(Now::current());
    }

    pub fn activity(&self) {
        self.session().activity(Now::current());
    }

    /// The timer's step: true if the ledger just auto-locked.
    pub fn tick(&self) -> bool {
        self.session().tick(Now::current())
    }
}

pub fn strength(password: Zeroizing<String>) -> StrengthView {
    fm_core::unlock::strength(&password).into()
}

// ---- the app's concrete session -----------------------------------------------------

#[cfg(target_os = "macos")]
pub type PlatformKeychain = fm_core::unlock::Keychain;
#[cfg(not(target_os = "macos"))]
pub type PlatformKeychain = fm_core::unlock::NoTouchId;

pub type AppUnlock = UnlockState<VaultStore, PlatformKeychain>;

/// The session for the ledger stored in `ledger_dir`.
pub fn app_unlock(ledger_dir: &Path) -> AppUnlock {
    #[cfg(target_os = "macos")]
    let keychain = fm_core::unlock::Keychain::for_ledger(ledger_dir);
    #[cfg(not(target_os = "macos"))]
    let keychain = fm_core::unlock::NoTouchId;
    UnlockState::new(VaultStore::new(Vault::at(ledger_dir)), keychain)
}

/// Keeps the crash reporter's scrub list in step with the lock state: while the ledger
/// is open, its account names are scrubbed from crash reports; once locked, the list is
/// cleared so no names stay in memory.
pub fn sync_crash_terms(unlock: &AppUnlock, reporter: &CrashReporter) {
    match unlock.with_ledger(|open| sensitive_terms(open.ledger())) {
        Some(terms) => reporter.set_sensitive_terms(terms),
        None => reporter.clear_sensitive_terms(),
    }
}

fn sensitive_terms(ledger: &Unlocked) -> Vec<String> {
    // TODO(T2.2): add merchant names once the merchant table exists.
    let mut terms = Vec::new();
    for account in ledger.accounts().unwrap_or_default() {
        terms.push(account.name);
        terms.extend(account.institution);
    }
    terms
}

/// Locks the ledger after the idle timeout, checking every [`TICK`].
pub fn start_autolock_timer(app: AppHandle) {
    thread::spawn(move || {
        loop {
            thread::sleep(TICK);
            let unlock = app.state::<AppUnlock>();
            if unlock.tick() {
                sync_crash_terms(&unlock, &app.state::<CrashReporter>());
                let _ = app.emit(LOCKED_EVENT, ());
            }
        }
    });
}

/// The Tauri commands behind `app/src/routes/unlock/api.ts`. The ones that derive keys,
/// re-encrypt or wait for Touch ID run off the main thread.
pub mod commands {
    use fm_core::crash::CrashReporter;
    use tauri::State;
    use zeroize::Zeroizing;

    use super::{AppUnlock, ErrorView, RecoveryView, StateView, StrengthView, sync_crash_terms};

    type Result<T> = std::result::Result<T, ErrorView>;

    /// Runs a command, then brings the crash reporter's scrub list up to date.
    fn synced<T>(
        unlock: &AppUnlock,
        reporter: &CrashReporter,
        f: impl FnOnce(&AppUnlock) -> T,
    ) -> T {
        let out = f(unlock);
        sync_crash_terms(unlock, reporter);
        out
    }

    #[tauri::command]
    pub fn unlock_state(unlock: State<'_, AppUnlock>) -> StateView {
        unlock.view()
    }

    #[tauri::command(async)]
    pub fn password_strength(password: String) -> StrengthView {
        super::strength(Zeroizing::new(password))
    }

    #[tauri::command(async)]
    pub fn create_ledger(
        unlock: State<'_, AppUnlock>,
        reporter: State<'_, CrashReporter>,
        password: String,
    ) -> Result<RecoveryView> {
        synced(&unlock, &reporter, |u| u.create(Zeroizing::new(password)))
    }

    #[tauri::command(async)]
    pub fn finish_create(
        unlock: State<'_, AppUnlock>,
        reporter: State<'_, CrashReporter>,
        password: String,
    ) -> Result<Option<RecoveryView>> {
        synced(&unlock, &reporter, |u| {
            u.finish_create(Zeroizing::new(password))
        })
    }

    #[tauri::command(async)]
    pub fn unlock_with_password(
        unlock: State<'_, AppUnlock>,
        reporter: State<'_, CrashReporter>,
        password: String,
    ) -> Result<Option<RecoveryView>> {
        synced(&unlock, &reporter, |u| {
            u.unlock_with_password(Zeroizing::new(password))
        })
    }

    #[tauri::command(async)]
    pub fn unlock_with_recovery_key(
        unlock: State<'_, AppUnlock>,
        reporter: State<'_, CrashReporter>,
        recovery_key: String,
    ) -> Result<()> {
        synced(&unlock, &reporter, |u| {
            u.unlock_with_recovery_key(Zeroizing::new(recovery_key))
        })
    }

    #[tauri::command(async)]
    pub fn set_password_after_recovery(
        unlock: State<'_, AppUnlock>,
        reporter: State<'_, CrashReporter>,
        password: String,
        compromised: bool,
    ) -> Result<RecoveryView> {
        synced(&unlock, &reporter, |u| {
            u.set_password_after_recovery(Zeroizing::new(password), compromised)
        })
    }

    #[tauri::command(async)]
    pub fn unlock_with_touch_id(
        unlock: State<'_, AppUnlock>,
        reporter: State<'_, CrashReporter>,
    ) -> Result<()> {
        synced(&unlock, &reporter, |u| u.unlock_with_touch_id())
    }

    #[tauri::command]
    pub fn recovery_to_confirm(unlock: State<'_, AppUnlock>) -> Option<RecoveryView> {
        unlock.recovery_to_confirm()
    }

    #[tauri::command]
    pub fn confirm_recovery_key(
        unlock: State<'_, AppUnlock>,
        reporter: State<'_, CrashReporter>,
        answers: Vec<String>,
    ) -> Result<()> {
        synced(&unlock, &reporter, |u| u.confirm_recovery_key(&answers))
    }

    #[tauri::command(async)]
    pub fn change_password(
        unlock: State<'_, AppUnlock>,
        reporter: State<'_, CrashReporter>,
        current: String,
        password: String,
        compromised: bool,
    ) -> Result<Option<RecoveryView>> {
        synced(&unlock, &reporter, |u| {
            u.change_password(
                Zeroizing::new(current),
                Zeroizing::new(password),
                compromised,
            )
        })
    }

    #[tauri::command]
    pub fn set_autolock_minutes(unlock: State<'_, AppUnlock>, minutes: u64) -> Result<()> {
        unlock.set_autolock_minutes(minutes)
    }

    #[tauri::command]
    pub fn lock(unlock: State<'_, AppUnlock>, reporter: State<'_, CrashReporter>) {
        synced(&unlock, &reporter, |u| u.lock());
    }

    #[tauri::command]
    pub fn activity(unlock: State<'_, AppUnlock>) {
        unlock.activity();
    }
}

fn status_name(status: Status) -> &'static str {
    match status {
        Status::NoLedger => "NoLedger",
        Status::Incomplete => "Incomplete",
        Status::Unrecoverable => "Unrecoverable",
        Status::Locked => "Locked",
        Status::NeedsNewPassword => "NeedsNewPassword",
        Status::ConfirmRecoveryKey => "ConfirmRecoveryKey",
        Status::Unlocked => "Unlocked",
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fm_core::storage::{StorageError, VaultState};
    use fm_core::unlock::{NoTouchId, OpenLedger};

    use super::*;

    const STRONG: &str = "velvet anchor mosaic tundra";

    /// Just enough of a ledger to drive the session from this crate.
    #[derive(Default)]
    struct Disk {
        password: Option<Vec<u8>>,
        recovery: Option<Vec<u8>>,
        settings: HashMap<String, String>,
    }

    #[derive(Clone, Default)]
    struct Store(Rc<RefCell<Disk>>);
    struct Open(Rc<RefCell<Disk>>);

    impl LedgerStore for Store {
        type Open = Open;
        fn state(&self) -> VaultState {
            if self.0.borrow().password.is_some() {
                VaultState::Ready
            } else {
                VaultState::Missing
            }
        }
        fn create(&self, password: &[u8]) -> std::result::Result<Open, StorageError> {
            self.0.borrow_mut().password = Some(password.to_vec());
            Ok(Open(self.0.clone()))
        }
        fn finish_create(&self, password: &[u8]) -> std::result::Result<Open, StorageError> {
            self.unlock(password)
        }
        fn unlock(&self, password: &[u8]) -> std::result::Result<Open, StorageError> {
            match self.0.borrow().password.as_deref() == Some(password) {
                true => Ok(Open(self.0.clone())),
                false => Err(StorageError::WrongSecret),
            }
        }
        fn unlock_with_recovery_key(&self, k: &[u8]) -> std::result::Result<Open, StorageError> {
            match self.0.borrow().recovery.as_deref() == Some(k) {
                true => Ok(Open(self.0.clone())),
                false => Err(StorageError::WrongSecret),
            }
        }
        fn unlock_with_data_key(&self, _: &[u8; 32]) -> std::result::Result<Open, StorageError> {
            Err(StorageError::DatabaseKeyRejected)
        }
    }

    impl OpenLedger for Open {
        fn has_recovery_key(&self) -> bool {
            self.0.borrow().recovery.is_some()
        }
        fn set_password(&mut self, _: &[u8], new: &[u8]) -> std::result::Result<(), StorageError> {
            self.0.borrow_mut().password = Some(new.to_vec());
            Ok(())
        }
        fn reset_password(
            &mut self,
            _: &[u8],
            new: &[u8],
        ) -> std::result::Result<(), StorageError> {
            self.set_password(&[], new)
        }
        fn set_recovery_key(
            &mut self,
            _: &[u8],
            k: &[u8],
        ) -> std::result::Result<(), StorageError> {
            self.0.borrow_mut().recovery = Some(k.to_vec());
            Ok(())
        }
        fn rotate_data_key(
            &mut self,
            _: &[u8],
            k: Option<&[u8]>,
        ) -> std::result::Result<(), StorageError> {
            self.0.borrow_mut().recovery = k.map(<[u8]>::to_vec);
            Ok(())
        }
        fn data_key_copy(&self) -> Zeroizing<[u8; 32]> {
            Zeroizing::new([0; 32])
        }
        fn setting(&self, key: &str) -> std::result::Result<Option<String>, StorageError> {
            Ok(self.0.borrow().settings.get(key).cloned())
        }
        fn set_setting(&self, key: &str, v: Option<&str>) -> std::result::Result<(), StorageError> {
            let mut d = self.0.borrow_mut();
            match v {
                Some(v) => d.settings.insert(key.into(), v.into()),
                None => d.settings.remove(key),
            };
            Ok(())
        }
    }

    fn secret(s: &str) -> Zeroizing<String> {
        Zeroizing::new(s.to_string())
    }

    #[test]
    fn create_confirm_lock_and_unlock_through_the_app_layer() {
        let app = UnlockState::new(Store::default(), NoTouchId);
        assert_eq!(app.view().status, "NoLedger");
        let shown = app.create(secret(STRONG)).unwrap();
        assert_eq!(app.view().status, "ConfirmRecoveryKey");
        assert!(app.with_ledger(|_| ()).is_none());
        let answers: Vec<String> = shown
            .challenge
            .iter()
            .map(|&i| shown.groups[i].clone())
            .collect();
        app.confirm_recovery_key(&answers).unwrap();
        let view = app.view();
        assert_eq!(view.status, "Unlocked");
        assert_eq!(view.has_recovery_key, Some(true));
        assert_eq!(view.autolock_minutes, 10);
        assert!(app.with_ledger(|_| ()).is_some());

        app.lock();
        let view = app.view();
        assert_eq!(view.status, "Locked");
        assert_eq!(view.has_recovery_key, None);
        assert_eq!(
            view.touch_id_blocked.as_deref(),
            Some("Touch ID isn't available")
        );
        assert!(app.unlock_with_password(secret(STRONG)).unwrap().is_none());
        assert_eq!(app.view().status, "Unlocked");
    }

    #[test]
    fn errors_reach_the_ui_with_a_code_and_message() {
        let app = UnlockState::new(Store::default(), NoTouchId);
        let weak = app.create(secret("password1234")).unwrap_err();
        assert_eq!(weak.code, "WeakPassword");
        let strength = weak.strength.expect("strength for the meter");
        assert!(!strength.acceptable);
        assert_eq!(strength.min_chars, 12);

        app.create(secret(STRONG)).unwrap();
        app.lock();
        let wrong = app.unlock_with_password(secret("wrong")).unwrap_err();
        assert_eq!(wrong.code, "WrongSecret");
        assert_eq!(wrong.message, "wrong password or recovery key");
        let json = serde_json::to_string(&wrong).unwrap();
        assert_eq!(
            json,
            r#"{"code":"WrongSecret","message":"wrong password or recovery key"}"#
        );
    }

    #[test]
    fn views_never_print_recovery_keys() {
        let app = UnlockState::new(Store::default(), NoTouchId);
        let shown = app.create(secret(STRONG)).unwrap();
        let debug = format!("{shown:?}");
        assert!(!debug.contains(&shown.groups[0]));
    }

    #[test]
    fn crash_reports_hide_account_names_only_while_unlocked() {
        use fm_core::crash::{CrashStore, PanicFacts};
        use fm_core::model::{Account, AccountId, AccountKind, Currency, Timestamp};

        let dir = tempfile::tempdir().unwrap();
        let store = CrashStore::new(dir.path().join("crash"));
        store.set_enabled(true).unwrap();
        let reporter = CrashReporter::new(store, "test");
        let unlock = app_unlock(&dir.path().join("ledger"));
        let shown = unlock.create(secret(STRONG)).unwrap();
        let answers: Vec<String> = shown
            .challenge
            .iter()
            .map(|&i| shown.groups[i].clone())
            .collect();
        unlock.confirm_recovery_key(&answers).unwrap();
        unlock
            .with_ledger(|open| {
                open.ledger().insert_account(&Account {
                    id: AccountId(1),
                    name: "Joint Savings Zephyr".into(),
                    kind: AccountKind::Savings,
                    currency: Currency::new("USD").unwrap(),
                    institution: Some("Quokka Credit Union".into()),
                    masked_number: None,
                    external_id: None,
                    own_account: true,
                    created_at: Timestamp::now(),
                })
            })
            .unwrap()
            .unwrap();

        let crash = |reporter: &CrashReporter| {
            let saved = reporter
                .record(PanicFacts {
                    message: "failed on Joint Savings Zephyr at Quokka Credit Union",
                    file: None,
                    line: 0,
                    column: 0,
                    thread: None,
                    backtrace: "",
                })
                .unwrap()
                .unwrap();
            reporter.store().read(&saved.id).unwrap()
        };

        sync_crash_terms(&unlock, &reporter);
        let report = crash(&reporter);
        assert!(!report.contains("Zephyr"), "{report}");
        assert!(!report.contains("Quokka"), "{report}");

        unlock.lock();
        sync_crash_terms(&unlock, &reporter);
        let report = crash(&reporter);
        assert!(report.contains("Zephyr"), "terms cleared on lock: {report}");
    }

    #[test]
    fn state_serialises_as_the_ui_expects() {
        let app = UnlockState::new(Store::default(), NoTouchId);
        let json = serde_json::to_value(app.view()).unwrap();
        assert_eq!(json["status"], "NoLedger");
        assert_eq!(
            json["autolockChoices"],
            serde_json::json!([1, 5, 10, 30, 60])
        );
        assert!(json.get("touchIdBlocked").is_some());
        let s = serde_json::to_value(strength(secret("purple monkey dishwasher"))).unwrap();
        assert_eq!(s["acceptable"], true);
        assert_eq!(s["minChars"], 12);
    }
}
