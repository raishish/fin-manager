//! The unlock screens' side of the app (T5.1). The rules live in
//! `fm_core::unlock::Session`; this module turns its results into what the UI shows
//! (`app/src/routes/unlock/api.ts` mirrors these types), gates every other command on
//! an open ledger, and runs the auto-lock timer.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread;
use std::time::Duration;

use fm_core::crash::CrashReporter;
use fm_core::storage::{StorageError, Unlocked, Vault};
use fm_core::unlock::{
    CHOICES_MINUTES, LedgerStore, MIN_CHARS, Now, RecoveryToConfirm, Session, Status, Strength,
    TouchIdBlocked, TouchIdKeychain, UnlockError, VaultStore,
};
use serde::Serialize;
use serde::ser::{SerializeStruct, Serializer};
use tauri::ipc::Invoke;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use zeroize::Zeroizing;

/// Emitted when auto-lock closes the ledger (`LOCKED_EVENT` in api.ts).
pub const LOCKED_EVENT: &str = "unlock://locked";
/// How often the auto-lock timer is checked.
pub const TICK: Duration = Duration::from_secs(5);

/// The only commands that run while the ledger isn't open: they are how it gets
/// opened. Every other command, including ones added later, is refused with
/// [`LOCKED`] until the ledger is unlocked (see [`gated`]).
pub const ALLOWED_WHILE_LOCKED: &[&str] = &[
    "core_info",
    "unlock_state",
    "password_strength",
    "create_ledger",
    "finish_create",
    "unlock_with_password",
    "unlock_with_recovery_key",
    "set_password_after_recovery",
    "unlock_with_touch_id",
    "recovery_to_confirm",
    "confirm_recovery_key",
    "lock",
    "activity",
    // Restoring a backup on a fresh install; refused unless there is no ledger yet.
    "restore_backup",
    // After "delete everything" there is no ledger; this acts only on the offer the
    // app itself stored when it deleted (deletion.rs).
    "delete_old_backups",
    "pending_backup_offer",
];

/// The error every other command returns while locked.
pub const LOCKED: &str = "locked";

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

/// A recovery key on its way to the screen that shows it once. The groups are wiped
/// when it is dropped, after serialising.
pub struct RecoveryView {
    groups: Vec<Zeroizing<String>>,
    challenge: Vec<usize>,
}

impl From<RecoveryToConfirm> for RecoveryView {
    fn from(r: RecoveryToConfirm) -> Self {
        Self {
            groups: r.groups,
            challenge: r.challenge.to_vec(),
        }
    }
}

impl Serialize for RecoveryView {
    fn serialize<Ser: Serializer>(
        &self,
        serializer: Ser,
    ) -> std::result::Result<Ser::Ok, Ser::Error> {
        let groups: Vec<&str> = self.groups.iter().map(|g| g.as_str()).collect();
        let mut s = serializer.serialize_struct("RecoveryView", 2)?;
        s.serialize_field("groups", &groups)?;
        s.serialize_field("challenge", &self.challenge)?;
        s.end()
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

/// Called with the open ledger (or `None` once it isn't open) after every change,
/// while the session's lock is still held.
/// Called after every change of state with the open ledger (if any) and whether the
/// change wrote to it.
type Observer<O> = Box<dyn Fn(Option<&O>, bool) + Send + Sync>;

/// The app's one unlock session, shared by the commands and the timer thread.
pub struct UnlockState<S: LedgerStore, K: TouchIdKeychain> {
    session: Mutex<Session<S, K>>,
    observer: Observer<S::Open>,
    /// Set while a lock is waiting for the session: long work holding it (a backup)
    /// checks this and stops, so locking never waits for it.
    pub(crate) interrupt: AtomicBool,
    /// A backup was restored in this run; the ledger takes a new backup id on first
    /// use (see `backup.rs`).
    pub(crate) restored: AtomicBool,
    /// After a deletion in this run: the offer to delete old backups still holding the
    /// data (decision 28). Kept here, not taken from the page, so the page can only
    /// ever delete what the app offered (see `deletion.rs`).
    pub(crate) backup_offer: Mutex<Option<fm_core::deletion::BackupOffer>>,
    /// Auto-lock's timing as of the last time the session was taken. Read by
    /// [`Self::tick`] without the session, so auto-lock can interrupt long work holding
    /// it instead of waiting for it (T3.6).
    autolock: Mutex<AutolockClock>,
    /// The latest activity not yet given to the session: [`Self::activity`] never waits
    /// for the session, and whoever takes it next hands this on. [`Self::tick`] counts it,
    /// so activity during long work keeps auto-lock away.
    pending_activity: Mutex<Option<Now>>,
    /// Changes whenever the ledger opens or closes, so a step of long work can tell the
    /// session is still the one it started on.
    generation: AtomicU64,
}

/// When auto-lock is due, and how long it waits after activity (`Now::mono`).
#[derive(Debug, Default, Clone, Copy)]
struct AutolockClock {
    /// `None` while locked.
    due: Option<Duration>,
    idle: Option<Duration>,
}

impl<S: LedgerStore, K: TouchIdKeychain> UnlockState<S, K> {
    pub fn new(store: S, keychain: K) -> Self {
        Self {
            session: Mutex::new(Session::new(store, keychain, Now::current())),
            observer: Box::new(|_, _| {}),
            interrupt: AtomicBool::new(false),
            restored: AtomicBool::new(false),
            backup_offer: Mutex::new(None),
            autolock: Mutex::new(AutolockClock::default()),
            pending_activity: Mutex::new(None),
            generation: AtomicU64::new(0),
        }
    }

    /// Calls `observer` after every change of state, under the session's lock, so it
    /// never sees a state that has already moved on. Its second argument says whether the
    /// change wrote to the ledger ([`Self::write_ledger`]).
    pub fn with_observer(
        mut self,
        observer: impl Fn(Option<&S::Open>, bool) + Send + Sync + 'static,
    ) -> Self {
        self.observer = Box::new(observer);
        self
    }

    /// The session. If a thread panicked while holding it, the ledger is locked
    /// before anyone uses it again: a panic may have stopped halfway through a step.
    fn session(&self) -> MutexGuard<'_, Session<S, K>> {
        let guard = match self.session.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                guard.lock(Now::current());
                (self.observer)(guard.ledger(), false);
                self.session.clear_poison();
                guard
            }
        };
        self.take_pending_activity(guard)
    }

    /// Gives the session the activity recorded while it was busy, and notes the new
    /// auto-lock deadline.
    fn take_pending_activity<'a>(
        &self,
        mut session: MutexGuard<'a, Session<S, K>>,
    ) -> MutexGuard<'a, Session<S, K>> {
        let pending = self
            .pending_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(at) = pending {
            session.activity(at);
            self.note_autolock(&session);
        }
        session
    }

    fn note_autolock(&self, session: &Session<S, K>) {
        let now = Now::current();
        *self.autolock.lock().unwrap_or_else(PoisonError::into_inner) = AutolockClock {
            due: session.idle_remaining(now).map(|left| now.mono + left),
            idle: Some(Duration::from_secs(session.autolock_minutes() * 60)),
        };
    }

    /// Changes whenever the ledger opens or closes.
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Runs a change on the session and tells the observer, all under one lock.
    pub(crate) fn change<T>(&self, f: impl FnOnce(&mut Session<S, K>) -> T) -> T {
        self.change_with(false, f)
    }

    fn change_with<T>(&self, wrote: bool, f: impl FnOnce(&mut Session<S, K>) -> T) -> T {
        self.change_on(self.session(), wrote, f)
    }

    /// [`Self::change`] if the session is free right now; `None` (nothing run) if long
    /// work holds it.
    fn try_change<T>(&self, f: impl FnOnce(&mut Session<S, K>) -> T) -> Option<T> {
        match self.session.try_lock() {
            Ok(session) => Some(self.change_on(self.take_pending_activity(session), false, f)),
            Err(TryLockError::WouldBlock) => None,
            // Poisoned: let `session()` lock the ledger and clear it.
            Err(TryLockError::Poisoned(_)) => Some(self.change(f)),
        }
    }

    fn change_on<T>(
        &self,
        mut session: MutexGuard<'_, Session<S, K>>,
        wrote: bool,
        f: impl FnOnce(&mut Session<S, K>) -> T,
    ) -> T {
        let was_open = session.ledger().is_some();
        let out = f(&mut session);
        if session.ledger().is_some() != was_open {
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        self.note_autolock(&session);
        (self.observer)(session.ledger(), wrote);
        out
    }

    /// Runs `f` on the open ledger, if it is fully unlocked.
    pub fn with_ledger<T>(&self, f: impl FnOnce(&S::Open) -> T) -> Option<T> {
        self.session().ledger().map(f)
    }

    /// Runs a write on the open ledger, then tells the observer (e.g. so new account
    /// names are scrubbed from crash reports). Later features use this for writes.
    pub fn write_ledger<T>(&self, f: impl FnOnce(&mut S::Open) -> T) -> Option<T> {
        self.change_with(true, |s| s.ledger_mut().map(f))
    }

    pub fn is_unlocked(&self) -> bool {
        self.session().status() == Status::Unlocked
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
        Ok(self.change(|s| s.create(&password, Now::current()))?.into())
    }

    pub fn finish_create(&self, password: Zeroizing<String>) -> Result<Option<RecoveryView>> {
        let shown = self.change(|s| s.finish_create(&password, Now::current()))?;
        Ok(shown.map(Into::into))
    }

    pub fn unlock_with_password(
        &self,
        password: Zeroizing<String>,
    ) -> Result<Option<RecoveryView>> {
        let shown = self.change(|s| s.unlock_with_password(&password, Now::current()))?;
        Ok(shown.map(Into::into))
    }

    pub fn unlock_with_recovery_key(&self, key: Zeroizing<String>) -> Result<()> {
        Ok(self.change(|s| s.unlock_with_recovery_key(&key, Now::current()))?)
    }

    pub fn set_password_after_recovery(
        &self,
        password: Zeroizing<String>,
        compromised: bool,
    ) -> Result<RecoveryView> {
        let shown =
            self.change(|s| s.set_password_after_recovery(&password, compromised, Now::current()))?;
        Ok(shown.into())
    }

    pub fn unlock_with_touch_id(&self) -> Result<()> {
        Ok(self.change(|s| s.unlock_with_touch_id(Now::current()))?)
    }

    pub fn recovery_to_confirm(&self) -> Option<RecoveryView> {
        self.session().recovery_to_confirm().map(Into::into)
    }

    pub fn confirm_recovery_key(&self, answers: Zeroizing<Vec<String>>) -> Result<()> {
        let answers: Vec<&str> = answers.iter().map(String::as_str).collect();
        Ok(self.change(|s| s.confirm_recovery_key(&answers))?)
    }

    pub fn change_password(
        &self,
        current: Zeroizing<String>,
        password: Zeroizing<String>,
        compromised: bool,
    ) -> Result<Option<RecoveryView>> {
        let shown =
            self.change(|s| s.change_password(&current, &password, compromised, Now::current()))?;
        Ok(shown.map(Into::into))
    }

    pub fn set_autolock_minutes(&self, minutes: u64) -> Result<()> {
        Ok(self.change(|s| s.set_autolock_minutes(minutes, Now::current()))?)
    }

    pub fn lock(&self) {
        self.interrupting(|| self.change(|s| s.lock(Now::current())));
    }

    /// Runs `f` (which waits for the session) with [`Self::interrupt`] raised, so
    /// whatever holds the session lets go promptly.
    fn interrupting<T>(&self, f: impl FnOnce() -> T) -> T {
        self.interrupt.store(true, Ordering::SeqCst);
        let out = f();
        self.interrupt.store(false, Ordering::SeqCst);
        out
    }

    /// Locks and deletes the Touch ID Keychain item: the app is quitting, and the item
    /// must not outlive the process.
    pub fn shut_down(&self) {
        self.interrupting(|| {
            self.change(|s| {
                s.lock(Now::current());
                s.forget_touch_id();
            })
        });
    }

    /// The user did something. Never waits for the session (long work may hold it):
    /// recorded here, and given to the session now if it is free, or by whoever takes it
    /// next. [`Self::tick`] counts it either way.
    pub fn activity(&self) {
        *self
            .pending_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Now::current());
        if let Ok(session) = self.session.try_lock() {
            drop(self.take_pending_activity(session));
        }
    }

    /// The timer's step: true if the ledger just auto-locked.
    /// Auto-lock's check, every [`TICK`]. When auto-lock is due, it raises the interrupt
    /// flag as [`Self::lock`] does, so long work holding the session (an import, a
    /// backup) stops instead of delaying the lock.
    pub fn tick(&self) -> bool {
        let now = Now::current().mono;
        let clock = *self.autolock.lock().unwrap_or_else(PoisonError::into_inner);
        let recent = self
            .pending_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map(|at| at.mono);
        // Due by the last deadline, and not put off by activity the session hasn't
        // been given yet.
        let due = clock.due.is_some_and(|at| now >= at)
            && recent.is_none_or(|at| clock.idle.is_some_and(|idle| now >= at + idle));
        if due {
            self.interrupting(|| self.change(|s| s.tick(Now::current())))
        } else {
            // Not due: never wait for long work holding the session. If it's busy this
            // tick is skipped, and the next one (TICK later) looks again, and interrupts
            // once the deadline has passed.
            self.try_change(|s| s.tick(Now::current())).unwrap_or(false)
        }
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

/// The session for the ledger stored in `ledger_dir`. While the ledger is open, its
/// account names are scrubbed from crash reports; once locked, the list is cleared so
/// no names stay in memory. The SimpleFIN bridge is allowed on the network only while
/// the ledger is open and connected.
pub fn app_unlock(
    ledger_dir: &Path,
    reporter: CrashReporter,
    bridge: crate::connectors::BridgeAllowance,
) -> AppUnlock {
    #[cfg(target_os = "macos")]
    let keychain = fm_core::unlock::Keychain::for_ledger(ledger_dir);
    #[cfg(not(target_os = "macos"))]
    let keychain = fm_core::unlock::NoTouchId;
    let terms = Mutex::new(TermsCache::default());
    UnlockState::new(VaultStore::new(Vault::at(ledger_dir)), keychain).with_observer(
        move |open, wrote| {
            let mut terms = terms.lock().unwrap_or_else(PoisonError::into_inner);
            match open {
                Some(open) => {
                    if let Some(t) = terms.refresh(wrote, || sensitive_terms(open.ledger())) {
                        reporter.set_sensitive_terms(t);
                    }
                }
                None => {
                    terms.clear();
                    reporter.clear_sensitive_terms();
                }
            }
            bridge.update(open.map(|o| o.ledger()));
        },
    )
}

/// The crash scrubber's terms for the open ledger. They are read from the ledger only when
/// it has just been opened or was written to, never on a timer tick or a read, so the
/// session lock isn't held for a scan of every decision on each change of state.
#[derive(Debug, Default)]
struct TermsCache {
    terms: Option<Vec<String>>,
}

impl TermsCache {
    /// The terms to install, or `None` when nothing changed. A failed read keeps the
    /// previous terms in place (names stay scrubbed) and is retried after the next write.
    fn refresh<E>(
        &mut self,
        wrote: bool,
        read: impl FnOnce() -> std::result::Result<Vec<String>, E>,
    ) -> Option<&[String]> {
        if self.terms.is_some() && !wrote {
            return None;
        }
        match read() {
            Ok(terms) => {
                self.terms = Some(terms);
                self.terms.as_deref()
            }
            Err(_) => None,
        }
    }

    fn clear(&mut self) {
        self.terms = None;
    }
}

/// Words that could identify the user's finances:
/// - account names and institutions;
/// - transaction descriptions, payees and memos (T3.6);
/// - the merchant names decisions and rules assign, and rule patterns;
/// - category names the user chose: a renamed built-in category or one of their own
///   (T3.6). Built-in names ("Groceries") say nothing about the user.
///
/// Read after the ledger opens and after every write through
/// [`UnlockState::write_ledger`] (see [`TermsCache`]), never on a timer tick, so it
/// follows imports and corrections. Any read error fails the whole refresh, so a
/// partial list never replaces a complete one. Very short terms are ignored by the
/// scrubber itself.
fn sensitive_terms(ledger: &Unlocked) -> std::result::Result<Vec<String>, StorageError> {
    use fm_core::model::Assignment;
    let mut terms = std::collections::BTreeSet::new();
    for account in ledger.accounts()? {
        terms.insert(account.name);
        terms.extend(account.institution);
    }
    for t in ledger.transactions()? {
        terms.insert(t.description);
        terms.extend(t.payee);
        terms.extend(t.memo);
    }
    for decision in ledger.decisions()? {
        if let Assignment::Merchant(name) = decision.assignment {
            terms.insert(name);
        }
    }
    for rule in ledger.rules()? {
        terms.insert(rule.matcher.description.pattern().to_string());
        terms.extend(rule.action.merchant);
    }
    let built_in = fm_core::storage::default_taxonomy();
    let taxonomy = ledger.taxonomy()?;
    let is_built_in = |key: &str, name: &str| {
        built_in
            .categories
            .iter()
            .map(|c| (&c.key, &c.name))
            .chain(built_in.groups.iter().map(|g| (&g.key, &g.name)))
            .any(|(k, n)| k == key && n == name)
    };
    for c in &taxonomy.categories {
        if !is_built_in(&c.key, &c.name) {
            terms.insert(c.name.clone());
        }
    }
    for g in &taxonomy.groups {
        if !is_built_in(&g.key, &g.name) {
            terms.insert(g.name.clone());
        }
    }
    Ok(terms.into_iter().collect())
}

/// Wraps the app's command handler: unless the ledger is open, every command not in
/// [`ALLOWED_WHILE_LOCKED`] is refused with [`LOCKED`] before it runs. Deny by default,
/// so a command added later is protected without anyone remembering to.
pub fn gated<R: Runtime>(
    handler: impl Fn(Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static {
    move |invoke: Invoke<R>| {
        let allowed = ALLOWED_WHILE_LOCKED.contains(&invoke.message.command())
            || invoke
                .message
                .webview_ref()
                .state::<AppUnlock>()
                .is_unlocked();
        if allowed {
            handler(invoke)
        } else {
            invoke.resolver.reject(LOCKED);
            true
        }
    }
}

/// Locks the ledger after the idle timeout, checking every [`TICK`], and runs the daily
/// backup while it is open.
pub fn start_autolock_timer<R: Runtime>(app: AppHandle<R>) {
    thread::spawn(move || {
        loop {
            thread::sleep(TICK);
            let unlock = app.state::<AppUnlock>();
            if unlock.tick() {
                let _ = app.emit(LOCKED_EVENT, ());
            } else {
                // While open: the daily backup, if one is due (T5.2).
                unlock.daily_backup();
            }
        }
    });
}

/// The Tauri commands behind `app/src/routes/unlock/api.ts`. The ones that derive keys,
/// re-encrypt or wait for Touch ID run off the main thread.
pub mod commands {
    use tauri::State;
    use zeroize::Zeroizing;

    use super::{AppUnlock, ErrorView, RecoveryView, StateView, StrengthView};

    type Result<T> = std::result::Result<T, ErrorView>;

    #[tauri::command]
    pub fn unlock_state(unlock: State<'_, AppUnlock>) -> StateView {
        unlock.view()
    }

    #[tauri::command(async)]
    pub fn password_strength(password: String) -> StrengthView {
        super::strength(Zeroizing::new(password))
    }

    #[tauri::command(async)]
    pub fn create_ledger(unlock: State<'_, AppUnlock>, password: String) -> Result<RecoveryView> {
        unlock.create(Zeroizing::new(password))
    }

    #[tauri::command(async)]
    pub fn finish_create(
        unlock: State<'_, AppUnlock>,
        password: String,
    ) -> Result<Option<RecoveryView>> {
        unlock.finish_create(Zeroizing::new(password))
    }

    #[tauri::command(async)]
    pub fn unlock_with_password(
        unlock: State<'_, AppUnlock>,
        password: String,
    ) -> Result<Option<RecoveryView>> {
        unlock.unlock_with_password(Zeroizing::new(password))
    }

    #[tauri::command(async)]
    pub fn unlock_with_recovery_key(
        unlock: State<'_, AppUnlock>,
        recovery_key: String,
    ) -> Result<()> {
        unlock.unlock_with_recovery_key(Zeroizing::new(recovery_key))
    }

    #[tauri::command(async)]
    pub fn set_password_after_recovery(
        unlock: State<'_, AppUnlock>,
        password: String,
        compromised: bool,
    ) -> Result<RecoveryView> {
        unlock.set_password_after_recovery(Zeroizing::new(password), compromised)
    }

    #[tauri::command(async)]
    pub fn unlock_with_touch_id(unlock: State<'_, AppUnlock>) -> Result<()> {
        unlock.unlock_with_touch_id()
    }

    #[tauri::command]
    pub fn recovery_to_confirm(unlock: State<'_, AppUnlock>) -> Option<RecoveryView> {
        unlock.recovery_to_confirm()
    }

    #[tauri::command]
    pub fn confirm_recovery_key(unlock: State<'_, AppUnlock>, answers: Vec<String>) -> Result<()> {
        unlock.confirm_recovery_key(Zeroizing::new(answers))
    }

    #[tauri::command(async)]
    pub fn change_password(
        unlock: State<'_, AppUnlock>,
        current: String,
        password: String,
        compromised: bool,
    ) -> Result<Option<RecoveryView>> {
        unlock.change_password(
            Zeroizing::new(current),
            Zeroizing::new(password),
            compromised,
        )
    }

    #[tauri::command]
    pub fn set_autolock_minutes(unlock: State<'_, AppUnlock>, minutes: u64) -> Result<()> {
        unlock.set_autolock_minutes(minutes)
    }

    #[tauri::command]
    pub fn lock(unlock: State<'_, AppUnlock>) {
        unlock.lock();
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

    /// T3.6: the scrubber also knows descriptions, payees, memos and the category names
    /// the user chose, but not the built-in category names.
    #[test]
    fn scrub_terms_cover_descriptions_payees_and_custom_category_names() {
        use fm_core::categories::{Edit, edit};
        use fm_core::model::Ledger;

        let dir = tempfile::tempdir().unwrap();
        let mut u = Vault::at(&dir.path().join("ledger"))
            .create(STRONG.as_bytes())
            .unwrap();
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/synthetic/data/renter/ledger.json");
        let ledger: Ledger = serde_json::from_slice(&std::fs::read(data).unwrap()).unwrap();
        u.save_ledger(&ledger).unwrap();
        let category = ledger.taxonomy.categories[0].clone();
        let untouched = ledger.taxonomy.categories[1].clone();
        edit(
            &mut u,
            &Edit::RenameCategory {
                id: category.id,
                name: "Zephyr Fund".into(),
            },
        )
        .unwrap();

        let terms = sensitive_terms(&u).unwrap();
        let has = |t: &str| terms.iter().any(|x| x == t);
        let t = ledger
            .transactions
            .iter()
            .find(|t| t.payee.is_some())
            .unwrap_or(&ledger.transactions[0]);
        assert!(has(&t.description), "descriptions");
        if let Some(p) = &t.payee {
            assert!(has(p), "payees");
        }
        assert!(has("Zephyr Fund"), "a renamed category");
        assert!(
            !has(&untouched.name),
            "built-in names say nothing about the user"
        );
        assert!(!has(&category.name) || category.name == "Zephyr Fund");
        for a in &ledger.accounts {
            assert!(has(&a.name));
        }
    }

    /// T3.6: when auto-lock is due, its tick raises the interrupt flag, so long work
    /// holding the session stops and the tick doesn't wait for it. When it isn't due,
    /// the tick waits its turn and interrupts nothing.
    #[test]
    fn a_due_auto_lock_interrupts_long_work() {
        use std::time::Instant;
        use tauri::Manager;

        let dir = tempfile::tempdir().unwrap();
        let (app, w) = crate::tests::mock_app(&std::fs::canonicalize(dir.path()).unwrap());
        crate::tests::create_confirmed(&w);
        let unlock = app.state::<AppUnlock>();
        // Long work that stops only when interrupted (or after 20 s).
        let work = || {
            unlock.write_ledger(|_| {
                let started = Instant::now();
                while !unlock.interrupt.load(Ordering::SeqCst)
                    && started.elapsed() < Duration::from_secs(20)
                {
                    thread::sleep(Duration::from_millis(5));
                }
                started.elapsed()
            })
        };

        // Due: the tick interrupts the work and gets the session at once.
        let (worked, waited) = thread::scope(|s| {
            let running = s.spawn(work);
            thread::sleep(Duration::from_millis(200));
            unlock
                .autolock
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .due = Some(Duration::ZERO);
            let started = Instant::now();
            unlock.tick();
            (running.join().unwrap().unwrap(), started.elapsed())
        });
        assert!(
            waited < Duration::from_secs(2),
            "the tick waited {waited:?}"
        );
        assert!(
            worked < Duration::from_secs(5),
            "the work stopped: {worked:?}"
        );
        assert!(!unlock.interrupt.load(Ordering::SeqCst));

        // Not due (the deadline is far off): the tick doesn't interrupt anything.
        unlock
            .autolock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .due = Some(Duration::MAX);
        let quick = || {
            unlock.write_ledger(|_| {
                thread::sleep(Duration::from_millis(300));
                unlock.interrupt.load(Ordering::SeqCst)
            })
        };
        let interrupted = thread::scope(|s| {
            let running = s.spawn(quick);
            thread::sleep(Duration::from_millis(100));
            assert!(!unlock.tick(), "nothing to lock yet");
            running.join().unwrap().unwrap()
        });
        assert!(!interrupted, "a tick that isn't due interrupts nothing");
    }

    /// Review T3.6 #1: steady activity during long work keeps auto-lock away, even when
    /// the last noted deadline has passed, so the work is never interrupted and activity
    /// never waits for the session.
    #[test]
    fn activity_during_long_work_keeps_auto_lock_away() {
        use std::time::Instant;
        use tauri::Manager;

        let dir = tempfile::tempdir().unwrap();
        let (app, w) = crate::tests::mock_app(&std::fs::canonicalize(dir.path()).unwrap());
        crate::tests::create_confirmed(&w);
        let unlock = app.state::<AppUnlock>();
        let (interrupted, locked, slowest) = thread::scope(|s| {
            let work = s.spawn(|| {
                unlock.write_ledger(|_| {
                    let started = Instant::now();
                    let mut interrupted = false;
                    while started.elapsed() < Duration::from_millis(1500) {
                        interrupted |= unlock.interrupt.load(Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(5));
                    }
                    interrupted
                })
            });
            thread::sleep(Duration::from_millis(100));
            // The deadline noted before the work has passed…
            unlock
                .autolock
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .due = Some(Duration::ZERO);
            // …but the user keeps working while auto-lock checks.
            let mut slowest = Duration::ZERO;
            unlock.activity();
            let tick = s.spawn(|| unlock.tick());
            for _ in 0..5 {
                let started = Instant::now();
                unlock.activity();
                slowest = slowest.max(started.elapsed());
                thread::sleep(Duration::from_millis(100));
            }
            let locked = tick.join().unwrap();
            (work.join().unwrap().unwrap(), locked, slowest)
        });
        assert!(!interrupted, "the work wasn't interrupted");
        assert!(!locked, "auto-lock didn't lock");
        assert!(
            slowest < Duration::from_millis(200),
            "activity waited {slowest:?}"
        );
        assert!(unlock.is_unlocked());
    }

    /// Review T3.6 round 3: a tick that isn't due never waits for long work holding the
    /// session, so the tick after the deadline still runs and interrupts the work.
    #[test]
    fn a_tick_that_isnt_due_never_waits_for_long_work() {
        use std::time::Instant;
        use tauri::Manager;

        let dir = tempfile::tempdir().unwrap();
        let (app, w) = crate::tests::mock_app(&std::fs::canonicalize(dir.path()).unwrap());
        crate::tests::create_confirmed(&w);
        let unlock = app.state::<AppUnlock>();
        let set_due = |due| {
            unlock
                .autolock
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .due = Some(due);
        };
        let (worked, not_due_took, due_took) = thread::scope(|s| {
            let work = s.spawn(|| {
                unlock.write_ledger(|_| {
                    let started = Instant::now();
                    while !unlock.interrupt.load(Ordering::SeqCst)
                        && started.elapsed() < Duration::from_secs(20)
                    {
                        thread::sleep(Duration::from_millis(5));
                    }
                    started.elapsed()
                })
            });
            thread::sleep(Duration::from_millis(100));
            // Long work started before the deadline; a tick arrives before it's due.
            set_due(Duration::MAX);
            let started = Instant::now();
            assert!(!unlock.tick(), "skipped, not locked");
            let not_due_took = started.elapsed();
            // The deadline passes: the next tick interrupts the work.
            set_due(Duration::ZERO);
            let started = Instant::now();
            unlock.tick();
            let due_took = started.elapsed();
            (work.join().unwrap().unwrap(), not_due_took, due_took)
        });
        assert!(
            not_due_took < Duration::from_millis(200),
            "the not-due tick waited {not_due_took:?}"
        );
        assert!(
            due_took < Duration::from_secs(2),
            "the due tick waited {due_took:?}"
        );
        assert!(
            worked < Duration::from_secs(5),
            "the work was interrupted: {worked:?}"
        );
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
            .map(|&i| shown.groups[i].to_string())
            .collect();
        app.confirm_recovery_key(Zeroizing::new(answers)).unwrap();
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
        assert!(!debug.contains(shown.groups[0].as_str()));
    }

    /// Review T3.3 #1: the terms are read when the ledger opens and after writes, not on
    /// every change of state (the 5-second tick), and a failed read keeps the old terms.
    #[test]
    fn scrubber_terms_are_read_on_open_and_after_writes_only() {
        let mut cache = TermsCache::default();
        let reads = std::cell::Cell::new(0);
        let read = |terms: &[&str]| {
            reads.set(reads.get() + 1);
            Ok::<_, ()>(terms.iter().map(|t| t.to_string()).collect())
        };
        assert_eq!(
            cache.refresh(false, || read(&["a"])),
            Some(&["a".to_string()][..]),
            "open"
        );
        assert_eq!(
            cache.refresh(false, || read(&["b"])),
            None,
            "a tick reads nothing"
        );
        assert_eq!(reads.get(), 1);
        assert_eq!(
            cache
                .refresh(true, || read(&["a", "b"]))
                .map(<[String]>::len),
            Some(2)
        );
        assert_eq!(reads.get(), 2, "a write re-reads");
        assert_eq!(
            cache.refresh(true, || Err::<Vec<String>, ()>(())),
            None,
            "a failed read"
        );
        assert_eq!(
            cache.terms.as_deref().map(<[String]>::len),
            Some(2),
            "keeps the previous terms rather than dropping them"
        );
        cache.clear();
        assert!(cache.terms.is_none());
        assert_eq!(cache.refresh(false, || Err::<Vec<String>, ()>(())), None);
        assert_eq!(
            cache.refresh(false, || read(&["c"])).map(<[String]>::len),
            Some(1),
            "with nothing cached, the next change of state tries again"
        );
    }

    #[test]
    fn crash_reports_hide_account_names_only_while_unlocked() {
        use fm_core::crash::{CrashStore, PanicFacts};
        use fm_core::model::{Account, AccountId, AccountKind, Currency, Timestamp};

        let dir = tempfile::tempdir().unwrap();
        let store = CrashStore::new(dir.path().join("crash"));
        store.set_enabled(true).unwrap();
        let reporter = CrashReporter::new(store, "test");
        let net = std::sync::Arc::new(fm_netguard::NetGuard::new(fm_netguard::Allowlist::new()));
        let unlock = app_unlock(
            &dir.path().join("ledger"),
            reporter.clone(),
            crate::connectors::BridgeAllowance::new(net),
        );
        let shown = unlock.create(secret(STRONG)).unwrap();
        let answers: Vec<String> = shown
            .challenge
            .iter()
            .map(|&i| shown.groups[i].to_string())
            .collect();
        unlock
            .confirm_recovery_key(Zeroizing::new(answers))
            .unwrap();
        // A ledger write re-syncs the terms (review finding 7).
        unlock
            .write_ledger(|open| {
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

        let report = crash(&reporter);
        assert!(!report.contains("Zephyr"), "{report}");
        assert!(!report.contains("Quokka"), "{report}");

        unlock.lock();
        let report = crash(&reporter);
        assert!(report.contains("Zephyr"), "terms cleared on lock: {report}");
    }

    #[test]
    fn a_panic_while_the_session_is_held_locks_the_ledger() {
        // Review finding 6: whatever was half done, the next user of the session finds
        // the ledger locked.
        let app = UnlockState::new(Store::default(), NoTouchId);
        let shown = app.create(secret(STRONG)).unwrap();
        let answers: Vec<String> = shown
            .challenge
            .iter()
            .map(|&i| shown.groups[i].to_string())
            .collect();
        app.confirm_recovery_key(Zeroizing::new(answers)).unwrap();
        assert!(app.is_unlocked());
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.with_ledger(|_| panic!("simulated failure"));
        }));
        assert!(panicked.is_err());
        assert!(!app.is_unlocked());
        assert_eq!(app.view().status, "Locked");
        app.unlock_with_password(secret(STRONG)).unwrap();
        assert!(app.is_unlocked());
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
