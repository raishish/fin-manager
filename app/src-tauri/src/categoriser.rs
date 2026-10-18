//! The model categoriser in the app (T4.6): T2.5's fallback (`fm_core::categorize::
//! run_fallback`) on the app's loaded model ([`ModelHost`]), for the transactions the
//! rules left in Needs review.
//!
//! - **When it runs.** A worker thread looks every [`POLL`] and runs over every
//!   transaction still waiting for the model (see [`waiting`]):
//!   - once per unlocked session of the ledger while the model is ready (the same edge as
//!     `model://ready`), which covers everything imported before the model was ready;
//!   - after each import, file or SimpleFIN sync ([`imported`], called at the end of
//!     `after_import`).
//!
//!   Nothing runs before the model is loaded, and that load is its verified file.
//! - **The model never answers under the ledger's lock.** A run reads the ledger once
//!   (a snapshot), builds the session over the instructions (the slow part), and asks the
//!   model about a batch of transactions, all without the lock. Then the batch is applied
//!   in one `run_fallback` under the write lock, replaying the recorded answers: the
//!   planner decides against the ledger as it is then (the user's decisions, seen
//!   decisions and dismissals win), in one savepoint. The lock is held for the load, the
//!   plan and the write, not for the model.
//! - **One bad transaction doesn't stop it.** An answer the model can't give (a prompt
//!   too long, no allowed token) leaves that transaction in review and the run goes on;
//!   it isn't asked again in this ledger session. Descriptions are cut to
//!   [`MAX_DESCRIPTION_BYTES`] for the model, so a long memo can't overflow the prompt.
//! - **Locking and quitting stop it within one answer.** Before every token the model
//!   checks whether the ledger's session ended, a lock is waiting (the session's
//!   interrupt), or the app is quitting; so does the wait for a question (below). The
//!   batch in hand is dropped unwritten; the session is dropped as soon as the ledger's
//!   session ends: no session outlives a lock.
//! - **Ask goes first.** Between answers the worker waits while a question is running
//!   ([`ModelHost::interactive_pending`]).
//! - **On CPU-only machines** it uses half the cores, so the app stays responsive.
//! - **No financial data leaves it:** the snapshot and the recorded answers are the
//!   run's locals; nothing is logged; errors are counted, not kept.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fm_core::categorize::llm_fallback::{
    PROMPT_VERSION, choices, describe, instructions, user_examples,
};
use fm_core::categorize::plan::LedgerView;
use fm_core::categorize::{
    Chooser, ChooserError, FALLBACK_THRESHOLD_BP, FallbackOptions, Scope, run_fallback,
};
use fm_core::model::{
    Assignment, BuiltinLogic, DecisionSource, DecisionState, Ledger, Timestamp, TransactionId,
};
use fm_llm::{Cancel, Choices, GenerateOptions, LlmError, Model, Session, SessionOptions};
use tauri::{AppHandle, Manager, Runtime};

use crate::model::{ANSWER_LEAD, ModelHost, ReadyEdge};
use crate::unlock::AppUnlock;

/// How often the worker looks for work.
pub const POLL: Duration = Duration::from_millis(250);
/// The most answers applied together.
pub const MAX_BATCH: usize = 25;
/// About how long a batch's answers take before they are applied, so a long run shows
/// its progress.
pub const BATCH_TIME: Duration = Duration::from_secs(5);
/// A transaction's description as the model sees it, at most: a token is at least a
/// byte, so the transaction's turn stays well inside [`MAX_SUFFIX_TOKENS`].
pub const MAX_DESCRIPTION_BYTES: usize = 160;
/// The longest transaction turn, in tokens (about 70 in TD.1).
const MAX_SUFFIX_TOKENS: u32 = 256;
/// The longest answer: a key and the closing of `{"category":"…"}`.
const MAX_ANSWER_TOKENS: u32 = 48;
/// Marks where the user's turn goes in the chat template, to split the prompt there.
const MARK: &str = "\u{1}";
/// How often the wait for a question looks again.
const ASK_POLL: Duration = Duration::from_millis(20);

/// Imports finished in this process: [`imported`] moves it on, and every categoriser
/// looks at its own ledger when it does. It carries no data.
static IMPORTS: AtomicU64 = AtomicU64::new(0);

/// An import finished (file or SimpleFIN sync): what the rules left is the model's.
pub fn imported() {
    IMPORTS.fetch_add(1, Ordering::SeqCst);
}

/// What the categoriser has done in this process (for the tests and the review's
/// numbers).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    /// Runs started (a trigger with work to do).
    pub runs: usize,
    /// Batches applied.
    pub batches: usize,
    /// Answers applied: categorised or kept in review by the planner's rules.
    pub asked: usize,
    /// Of those, applied as a category, refund or income (at or above the threshold).
    pub categorised: usize,
    /// Of those, kept in Needs review.
    pub needs_review: usize,
    /// Transactions the model couldn't answer, left in review.
    pub skipped: usize,
    /// Runs stopped by a lock, a waiting lock, or quitting.
    pub interrupted: usize,
    /// Batches not applied because the ledger changed under them (asked again later).
    pub stale: usize,
    /// Runs stopped because the ledger couldn't be read or written, or the session
    /// couldn't be built.
    pub failed: usize,
    /// Sessions built, and alive now.
    pub sessions_built: usize,
    pub sessions_alive: usize,
    /// The longest the ledger was held to apply a batch, from taking the lock.
    pub max_lock_hold: Duration,
}

/// Test hooks: called before every token, and an answer that fails.
#[derive(Clone, Default)]
pub struct Probe {
    /// Called before every token (inside the cancel check).
    pub on_token: Option<Arc<dyn Fn() + Send + Sync>>,
    /// An answer for a transaction whose text contains this fails, as a too-long prompt
    /// would.
    pub fail_if: Option<String>,
}

/// The categoriser (see the module docs).
pub struct Categoriser {
    quitting: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    #[cfg_attr(not(test), allow(dead_code, reason = "read by the tests"))]
    stats: Arc<Mutex<Stats>>,
    #[cfg_attr(not(test), allow(dead_code, reason = "read by the tests"))]
    alive: Arc<AtomicUsize>,
}

impl Categoriser {
    /// Starts the worker for the app's ledger and model.
    pub fn start<R: Runtime>(app: AppHandle<R>, host: Arc<ModelHost>) -> Arc<Self> {
        Self::start_with(app, host, Probe::default())
    }

    pub fn start_with<R: Runtime>(
        app: AppHandle<R>,
        host: Arc<ModelHost>,
        probe: Probe,
    ) -> Arc<Self> {
        let quitting = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(Stats::default()));
        let alive = Arc::new(AtomicUsize::new(0));
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let worker = Worker {
            app,
            host,
            quitting: Arc::clone(&quitting),
            stats: Arc::clone(&stats),
            alive: Arc::clone(&alive),
            // The GPU does the work where there is one; otherwise half the cores.
            threads: (!fm_llm::gpu_offload_supported().unwrap_or(false))
                .then(|| u32::try_from((cores / 2).max(1)).unwrap_or(1)),
            probe,
        };
        let worker = std::thread::Builder::new()
            .name("categoriser".into())
            .spawn(move || worker.serve())
            .ok();
        Arc::new(Self {
            quitting,
            worker: Mutex::new(worker),
            stats,
            alive,
        })
    }

    #[cfg_attr(not(test), allow(dead_code, reason = "read by the tests"))]
    pub fn stats(&self) -> Stats {
        let mut stats = self
            .stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        stats.sessions_alive = self.alive.load(Ordering::SeqCst);
        stats
    }

    /// Quitting: an answer in progress stops at its next token, the worker drops its
    /// session and its model, and ends. Call before [`ModelHost::shut_down`].
    pub fn shut_down(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

/// Counts a live session while it exists.
struct Alive(Arc<AtomicUsize>);

impl Alive {
    fn new(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(count))
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A session over the fallback's instructions, for one ledger session (generation).
struct Cached<'m> {
    instructions: String,
    generation: u64,
    tail: String,
    session: Session<'m>,
    /// The closed list for the last keys, kept while they don't change.
    choices: Option<(Vec<String>, Choices)>,
    _alive: Alive,
}

/// Why a run stopped early.
enum Stop {
    /// Quitting, a lock, or a lock waiting: nothing more for this ledger session.
    Interrupted,
    /// The ledger can't be read or written, or the session can't be built.
    Failed,
}

/// llama.cpp takes C strings, so a NUL can't pass; and [`MARK`] must appear only
/// where this module puts it.
fn clean(text: &str) -> String {
    text.replace(['\0', '\u{1}'], " ")
}

/// The transaction's turn for the model: [`clean`], and every line cut to
/// [`MAX_DESCRIPTION_BYTES`] (on a character boundary) with a marker.
fn for_model(transaction: &str) -> String {
    clean(transaction)
        .split_inclusive('\n')
        .map(|line| {
            let body = line.trim_end_matches('\n');
            if body.len() <= MAX_DESCRIPTION_BYTES {
                return line.to_owned();
            }
            let mut end = MAX_DESCRIPTION_BYTES;
            while !body.is_char_boundary(end) {
                end -= 1;
            }
            format!("{} […]\n", &body[..end])
        })
        .collect()
}

/// The transactions to ask the model about (`model_id` and [`PROMPT_VERSION`] being
/// what would answer): no bucket, or only the import's Needs-review placeholder; and not
/// answered by that model and prompt before, whatever became of the answer (applied,
/// kept in review, made a suggestion, dismissed).
fn waiting(ledger: &Ledger, model_id: &str) -> Vec<TransactionId> {
    let mut done = HashSet::new();
    for d in &ledger.decisions {
        let asked = match &d.source {
            DecisionSource::Model(p) => {
                p.model_id == model_id && p.prompt_version == PROMPT_VERSION
            }
            _ => false,
        };
        let placeholder = matches!(
            d.source,
            DecisionSource::Builtin {
                logic: BuiltinLogic::ImportPipeline,
                ..
            }
        );
        let decided = d.state == DecisionState::Applied
            && matches!(d.assignment, Assignment::Bucket(_))
            && !placeholder;
        if asked || decided {
            done.insert(d.transaction_id);
        }
    }
    ledger
        .transactions
        .iter()
        .map(|t| t.id)
        .filter(|id| !done.contains(id))
        .collect()
}

struct Worker<R: Runtime> {
    app: AppHandle<R>,
    host: Arc<ModelHost>,
    quitting: Arc<AtomicBool>,
    stats: Arc<Mutex<Stats>>,
    alive: Arc<AtomicUsize>,
    threads: Option<u32>,
    probe: Probe,
}

impl<R: Runtime> Worker<R> {
    fn stats(&self) -> std::sync::MutexGuard<'_, Stats> {
        self.stats.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn unlock(&self) -> tauri::State<'_, AppUnlock> {
        self.app.state::<AppUnlock>()
    }

    /// Waits for the model, then serves it until it goes away or the app quits.
    fn serve(self) {
        let mut edge = ReadyEdge::default();
        let mut imports = IMPORTS.load(Ordering::SeqCst);
        while !self.quitting.load(Ordering::SeqCst) {
            if let Some(model) = self.host.model() {
                self.serve_model(&model, &mut edge, &mut imports);
            } else {
                std::thread::sleep(POLL);
            }
        }
    }

    fn serve_model(&self, model: &Arc<Model>, edge: &mut ReadyEdge, imports: &mut u64) {
        let mut cached: Option<Cached<'_>> = None;
        // Transactions the model couldn't answer in this ledger session.
        let mut skipped: (u64, HashSet<TransactionId>) = (0, HashSet::new());
        while !self.quitting.load(Ordering::SeqCst)
            && self.host.model().is_some_and(|m| Arc::ptr_eq(&m, model))
        {
            let generation = self.unlock().generation();
            if cached.as_ref().is_some_and(|c| c.generation != generation) {
                cached = None;
            }
            if skipped.0 != generation {
                skipped = (generation, HashSet::new());
            }
            let now_imports = IMPORTS.load(Ordering::SeqCst);
            let unlocked = self.unlock().is_unlocked();
            let ready = edge.step(true, unlocked.then_some(generation));
            if unlocked && (ready || now_imports != *imports) {
                *imports = now_imports;
                self.run(model, &mut cached, generation, &mut skipped.1);
            }
            std::thread::sleep(POLL);
        }
    }

    /// Stops the model when the ledger leaves the session `generation`, a lock is
    /// waiting for the ledger, or the app quits: checked before every token.
    fn cancel(&self, generation: u64) -> Cancel {
        let app = self.app.clone();
        let quitting = Arc::clone(&self.quitting);
        let on_token = self.probe.on_token.clone();
        Cancel::new(move || {
            if let Some(f) = &on_token {
                f();
            }
            let unlock = app.state::<AppUnlock>();
            quitting.load(Ordering::SeqCst)
                || unlock.interrupt.load(Ordering::SeqCst)
                || unlock.generation() != generation
        })
    }

    /// One run over everything waiting for the model in the ledger session `generation`.
    fn run<'m>(
        &self,
        model: &'m Model,
        cached: &mut Option<Cached<'m>>,
        generation: u64,
        skipped: &mut HashSet<TransactionId>,
    ) {
        let Some(Ok(ledger)) = self
            .unlock()
            .with_ledger(|open| open.ledger().load_ledger())
        else {
            return;
        };
        if self.unlock().generation() != generation {
            return;
        }
        let ids: Vec<TransactionId> = waiting(&ledger, model.id())
            .into_iter()
            .filter(|id| !skipped.contains(id))
            .collect();
        if ids.is_empty() {
            return;
        }
        self.stats().runs += 1;
        match self.answer_all(model, cached, generation, &ledger, &ids, skipped) {
            Ok(()) => {}
            Err(Stop::Interrupted) => self.stats().interrupted += 1,
            Err(Stop::Failed) => self.stats().failed += 1,
        }
        if self.unlock().generation() != generation {
            *cached = None;
        }
    }

    /// Answers `ids` from the snapshot `ledger` in batches, applying each batch.
    fn answer_all<'m>(
        &self,
        model: &'m Model,
        cached: &mut Option<Cached<'m>>,
        generation: u64,
        ledger: &Ledger,
        ids: &[TransactionId],
        skipped: &mut HashSet<TransactionId>,
    ) -> Result<(), Stop> {
        let cancel = self.cancel(generation);
        let view = LedgerView::from(ledger);
        let text = instructions(view, &ledger.accounts, &user_examples(view));
        if !cached
            .as_ref()
            .is_some_and(|c| c.instructions == text && c.generation == generation)
        {
            *cached = None;
            *cached = Some(self.build(model, &text, generation, &cancel)?);
        }
        let c = cached.as_mut().ok_or(Stop::Failed)?;
        let keys = choices(&ledger.taxonomy);
        let accounts: HashMap<_, _> = ledger.accounts.iter().map(|a| (a.id, a)).collect();
        let transactions: HashMap<_, _> = ledger.transactions.iter().map(|t| (t.id, t)).collect();
        let mut next = ids.iter();
        let mut exhausted = false;
        while !exhausted {
            let mut answers: HashMap<String, (String, u16)> = HashMap::new();
            let mut batch = Vec::new();
            let started = Instant::now();
            while batch.len() < MAX_BATCH && started.elapsed() < BATCH_TIME {
                let Some(id) = next.next() else {
                    exhausted = true;
                    break;
                };
                self.wait_for_ask(&cancel)?;
                let Some(t) = transactions.get(id) else {
                    continue;
                };
                let text = describe(t, accounts.get(&t.account_id).copied());
                match self.answer(c, &text, &keys) {
                    Ok(answer) => {
                        answers.insert(text, answer);
                        batch.push(*id);
                    }
                    Err(LlmError::Cancelled) => return Err(Stop::Interrupted),
                    // This one stays in review; the others go on.
                    Err(_) => {
                        skipped.insert(*id);
                        self.stats().skipped += 1;
                    }
                }
            }
            if !batch.is_empty() {
                self.apply(model, generation, &batch, answers)?;
            }
        }
        Ok(())
    }

    /// Waits while a question uses the model, stopping like an answer does.
    fn wait_for_ask(&self, cancel: &Cancel) -> Result<(), Stop> {
        loop {
            if cancel.is_cancelled() {
                return Err(Stop::Interrupted);
            }
            if !self.host.interactive_pending() {
                return Ok(());
            }
            std::thread::sleep(ASK_POLL);
        }
    }

    /// One answer, outside the ledger's lock.
    fn answer(
        &self,
        c: &mut Cached<'_>,
        transaction: &str,
        keys: &[String],
    ) -> Result<(String, u16), LlmError> {
        if self
            .probe
            .fail_if
            .as_ref()
            .is_some_and(|f| transaction.contains(f.as_str()))
        {
            return Err(LlmError::NoAllowedToken);
        }
        let choices = match &c.choices {
            Some((k, choices)) if k == keys => choices,
            _ => {
                let choices = Choices::new(keys)
                    .map_err(|e| LlmError::Runtime(format!("the category list: {e}")))?;
                &c.choices.insert((keys.to_vec(), choices)).1
            }
        };
        let suffix = format!(
            "Categorise this transaction.\n{}{}{ANSWER_LEAD}",
            for_model(transaction),
            c.tail
        );
        let options = GenerateOptions {
            max_tokens: MAX_ANSWER_TOKENS,
            threads: self.threads,
            ..GenerateOptions::default()
        };
        // A hybrid model that can't cut its cache where the prompt parts from the prefix
        // (SessionBoundary) can't happen here: the prefix ends right after the chat
        // template's special tokens and a newline, where no token spans the split. If it
        // ever did, it is this transaction's error, not a slow uncancellable path.
        let choice = c.session.choose(&suffix, choices, &options)?;
        Ok((choice.key, choice.confidence_bp))
    }

    /// Applies a batch's answers under the ledger's write lock: the planner decides
    /// against the ledger as it is now, in one savepoint. Nothing is written for a
    /// ledger session that has ended.
    fn apply(
        &self,
        model: &Model,
        generation: u64,
        batch: &[TransactionId],
        answers: HashMap<String, (String, u16)>,
    ) -> Result<(), Stop> {
        let mut replay = Replay {
            model,
            answers,
            missing: false,
        };
        let applied = self.unlock().write_ledger(|open| {
            let held = Instant::now();
            if self.unlock().generation() != generation {
                return None;
            }
            let out = run_fallback(
                open.ledger_mut(),
                &mut replay,
                &FallbackOptions {
                    threshold_bp: FALLBACK_THRESHOLD_BP,
                    scope: Scope::Transactions(batch),
                },
                Timestamp::now(),
            );
            Some((out, held.elapsed()))
        });
        let Some(Some((out, held))) = applied else {
            return Err(Stop::Interrupted);
        };
        let mut stats = self.stats();
        stats.max_lock_hold = stats.max_lock_hold.max(held);
        match out {
            Ok(report) => {
                stats.batches += 1;
                stats.asked += report.asked;
                stats.categorised += report.categorised;
                stats.needs_review += report.needs_review;
                Ok(())
            }
            // An answer recorded for a list that has changed since (a category hidden or
            // renamed): nothing written; those transactions are asked again next run.
            Err(_) if replay.missing => {
                stats.stale += 1;
                Ok(())
            }
            Err(_) => Err(Stop::Failed),
        }
    }

    /// A session over `instructions`, evaluated now (stops like an answer does).
    fn build<'m>(
        &self,
        model: &'m Model,
        instructions: &str,
        generation: u64,
        cancel: &Cancel,
    ) -> Result<Cached<'m>, Stop> {
        let full = model
            .chat_prompt(&clean(instructions), MARK)
            .map_err(|_| Stop::Failed)?;
        let (prefix, tail) = full.rsplit_once(MARK).ok_or(Stop::Failed)?;
        let session = model
            .session_cancellable(
                prefix,
                &SessionOptions {
                    max_suffix_tokens: MAX_SUFFIX_TOKENS,
                    max_answer_tokens: MAX_ANSWER_TOKENS,
                    threads: self.threads,
                },
                cancel.clone(),
            )
            .map_err(|e| match e {
                LlmError::Cancelled => Stop::Interrupted,
                _ => Stop::Failed,
            })?;
        self.stats().sessions_built += 1;
        Ok(Cached {
            instructions: instructions.to_owned(),
            generation,
            tail: tail.to_owned(),
            session,
            choices: None,
            _alive: Alive::new(&self.alive),
        })
    }
}

/// fm-core's [`Chooser`] replaying answers the model gave outside the lock, by the
/// transaction's text. Instant: nothing waits for the model under the lock.
struct Replay<'m> {
    model: &'m Model,
    answers: HashMap<String, (String, u16)>,
    /// An answer was missing or no longer on the list.
    missing: bool,
}

impl Chooser for Replay<'_> {
    fn model_id(&self) -> &str {
        self.model.id()
    }

    fn gguf_sha256(&self) -> [u8; 32] {
        self.model.gguf_sha256()
    }

    /// The answers were given with the instructions of the snapshot; a correction made
    /// since shows in the next run's.
    fn begin(&mut self, _instructions: &str) -> Result<(), ChooserError> {
        Ok(())
    }

    fn choose(
        &mut self,
        transaction: &str,
        keys: &[String],
    ) -> Result<(String, u16), ChooserError> {
        match self.answers.get(transaction) {
            Some((key, bp)) if keys.contains(key) || key == fm_llm::UNCATEGORISED => {
                Ok((key.clone(), *bp))
            }
            _ => {
                self.missing = true;
                Err(ChooserError("no recorded answer".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc;

    use fm_core::contract;
    use fm_llm::LoadOptions;
    use serde_json::{Value, json};

    use super::*;
    use crate::tests::{Webview, create_confirmed, invoke, invoke_with, mock_app};

    type App = tauri::App<tauri::test::MockRuntime>;

    const PASSWORD: &str = "velvet anchor mosaic tundra";
    const FILE: &str = "chase/freedom_2025_h1.csv";
    /// Rows imported in the tests CI runs (CI's model job has minutes to spare, not tens).
    const ROWS: usize = 30;

    /// The model at `FM_LLM_TEST_MODEL` (CPU only with `FM_LLM_TEST_CPU`), as fm-llm's
    /// tests take it.
    fn ci_model() -> Option<PathBuf> {
        let path = std::env::var_os("FM_LLM_TEST_MODEL").map(PathBuf::from);
        if path.is_none() {
            eprintln!("skipping: set FM_LLM_TEST_MODEL to the CI test model's path");
        }
        path
    }

    fn host() -> Arc<ModelHost> {
        ModelHost::new(LoadOptions {
            gpu: std::env::var_os("FM_LLM_TEST_CPU").is_none(),
        })
    }

    fn loaded_host() -> Option<Arc<ModelHost>> {
        let path = ci_model()?;
        let host = host();
        host.load(&path, &fm_llm::CI_TEST_MODEL)
            .expect("load the test model");
        Some(host)
    }

    fn app() -> (tempfile::TempDir, App, Webview) {
        let dir = tempfile::tempdir().unwrap();
        let (app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
        create_confirmed(&w);
        (dir, app, w)
    }

    /// The renter's TD.2 file through `import_file`, into a new account: its first
    /// `rows` rows, or all 220; `edit` may change the rows (not the header).
    fn import_rows(w: &Webview, rows: Option<usize>, edit: impl Fn(&mut Vec<String>)) -> Value {
        let exports = crate::ui_tests::exports();
        let expected: Value = serde_json::from_slice(
            &std::fs::read(exports.join(format!("{FILE}.expected.json"))).unwrap(),
        )
        .unwrap();
        let target = json!({
            "kind": "newAccount",
            "name": expected["institution"],
            "accountKind": expected["account"]["kind"],
            "currency": expected["account"]["currency"],
        });
        let file = String::from_utf8(std::fs::read(exports.join(FILE)).unwrap()).unwrap();
        let mut lines: Vec<String> = file.split_inclusive('\n').map(String::from).collect();
        let keep = rows.map_or(lines.len(), |n| (n + 1).min(lines.len()));
        lines.truncate(keep);
        let mut body = lines.split_off(1);
        edit(&mut body);
        lines.extend(body);
        crate::ui_tests::import(
            w,
            lines.concat().into_bytes(),
            &json!({ "fileName": FILE, "target": target, "csvCurrency": null }),
        )
        .unwrap()
    }

    fn import(w: &Webview, rows: Option<usize>) -> Value {
        import_rows(w, rows, |_| {})
    }

    fn ledger(app: &App) -> Ledger {
        app.state::<AppUnlock>()
            .with_ledger(|open| open.ledger().load_ledger().unwrap())
            .unwrap()
    }

    fn left(app: &App) -> Vec<TransactionId> {
        waiting(&ledger(app), &fm_llm::CI_TEST_MODEL.id)
    }

    fn model_decisions(ledger: &Ledger) -> Vec<&fm_core::model::Decision> {
        ledger
            .decisions
            .iter()
            .filter(|d| matches!(d.source, DecisionSource::Model(_)))
            .collect()
    }

    /// Waits up to `secs` for `done`.
    fn wait(secs: u64, mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !done() {
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    /// The ledger after a run: the contract holds (constraints 2, 3, 5, 7) and every
    /// transaction has at most one applied bucket (nothing half-written).
    fn consistent(ledger: &Ledger) {
        let check = contract::check(ledger);
        assert!(check.is_ok(), "{check}");
        let mut seen = HashSet::new();
        for d in &ledger.decisions {
            if d.state == DecisionState::Applied && matches!(d.assignment, Assignment::Bucket(_)) {
                assert!(seen.insert(d.transaction_id), "two applied buckets");
            }
        }
    }

    #[test]
    fn long_lines_are_cut_for_the_model_on_a_character_boundary() {
        let long = format!("Description: {}\nAmount: -1.00 USD\n", "é".repeat(500));
        let cut = for_model(&long);
        let first = cut.lines().next().unwrap();
        assert!(first.len() <= MAX_DESCRIPTION_BYTES + " […]".len());
        assert!(first.ends_with(" […]"));
        assert!(cut.ends_with("Amount: -1.00 USD\n"));
        assert_eq!(for_model("a\0b\u{1}c\n"), "a b c\n");
    }

    /// Done-when: an import through IPC ends with the model's decisions on the rows the
    /// rules left in Needs review (constraints 3, 5 and 7 checked by the contract), with
    /// one session for every batch.
    #[test]
    fn an_import_ends_with_model_decisions_on_needs_review_rows() {
        import_ends_with_model_decisions(Some(ROWS));
    }

    /// The same on the whole file (220 rows). Local only: CI runs the 50-row version.
    #[test]
    #[ignore = "the whole file: minutes on CI's model job; local only"]
    fn a_whole_file_import_ends_with_model_decisions() {
        import_ends_with_model_decisions(None);
    }

    fn import_ends_with_model_decisions(rows: Option<usize>) {
        let Some(host) = loaded_host() else { return };
        let (_dir, app, w) = app();
        let categoriser = Categoriser::start(app.handle().clone(), Arc::clone(&host));
        let report = import(&w, rows);
        assert!(report["needsReview"].as_u64().unwrap() > 0);
        assert!(wait(600, || left(&app).is_empty()), "every row answered");
        let ledger = ledger(&app);
        let stats = categoriser.stats();
        let decisions = model_decisions(&ledger);
        assert_eq!(decisions.len(), stats.asked, "one decision per answer");
        assert_eq!(stats.asked, stats.categorised + stats.needs_review);
        assert_eq!((stats.failed, stats.interrupted, stats.skipped), (0, 0, 0));
        for d in &decisions {
            let DecisionSource::Model(p) = &d.source else {
                unreachable!()
            };
            assert_eq!(p.model_id, fm_llm::CI_TEST_MODEL.id);
            assert_eq!(p.prompt_version, PROMPT_VERSION);
            // Constraint 5: below the threshold is Needs review.
            if p.confidence.basis_points() < FALLBACK_THRESHOLD_BP {
                assert_eq!(
                    d.assignment,
                    Assignment::Bucket(fm_core::model::Bucket::NeedsReview)
                );
            }
        }
        consistent(&ledger);
        assert_eq!(stats.sessions_built, 1, "one session for every batch");
        eprintln!(
            "asked {} in {} batches; longest lock hold {:?}",
            stats.asked, stats.batches, stats.max_lock_hold
        );
        // Another trigger (an import elsewhere): nothing is asked again.
        imported();
        std::thread::sleep(POLL * 4);
        assert_eq!(categoriser.stats().asked, stats.asked);
        assert_eq!(categoriser.stats().runs, stats.runs);
        categoriser.shut_down();
        assert_eq!(categoriser.stats().sessions_alive, 0);
        host.shut_down();
    }

    /// Done-when: nothing runs before the model is ready; once it is, everything the
    /// earlier import left is answered.
    #[test]
    fn nothing_runs_before_the_model_is_ready() {
        let host = host();
        let (_dir, app, w) = app();
        let categoriser = Categoriser::start(app.handle().clone(), Arc::clone(&host));
        // A few rows are enough: what this proves is when the model runs.
        import(&w, Some(10));
        std::thread::sleep(POLL * 6);
        assert_eq!(categoriser.stats(), Stats::default());
        let before = ledger(&app);
        assert!(model_decisions(&before).is_empty());
        assert!(!waiting(&before, &fm_llm::CI_TEST_MODEL.id).is_empty());

        if let Some(path) = ci_model() {
            host.load(&path, &fm_llm::CI_TEST_MODEL).unwrap();
            assert!(wait(600, || left(&app).is_empty()));
            assert!(categoriser.stats().asked > 0);
            consistent(&ledger(&app));
        }
        categoriser.shut_down();
        host.shut_down();
    }

    /// Review finding 1, on one import: a description far longer than a bank's (2,000
    /// characters) is cut for the model and answered like the rest; transactions the
    /// model can't answer stay in review while the others are answered, and later runs
    /// neither stall on them nor ask them again.
    #[test]
    fn a_long_description_is_answered_and_what_cant_be_answered_is_left() {
        let Some(host) = loaded_host() else { return };
        let (_dir, app, w) = app();
        import_rows(&w, Some(ROWS), |rows| {
            rows[3] = format!(
                "06/20/2025,06/21/2025,{},Shopping,Sale,-12.34,\n",
                "LONG MEMO TEXT ".repeat(134)
            );
        });
        let stored = ledger(&app);
        let long = stored
            .transactions
            .iter()
            .find(|t| t.description.len() >= 2000)
            .unwrap()
            .id;
        let failing = stored
            .transactions
            .iter()
            .filter(|t| t.description.contains("TARGET"))
            .count();
        assert!(failing > 0);
        let categoriser = Categoriser::start_with(
            app.handle().clone(),
            Arc::clone(&host),
            Probe {
                fail_if: Some("TARGET".into()),
                ..Probe::default()
            },
        );
        assert!(wait(600, || categoriser.stats().runs == 1
            && left(&app).len() == failing
            && categoriser.stats().skipped == failing));
        let stats = categoriser.stats();
        assert_eq!(stats.asked, ROWS - failing);
        assert_eq!((stats.failed, stats.interrupted), (0, 0));
        assert!(
            model_decisions(&ledger(&app))
                .iter()
                .any(|d| d.transaction_id == long),
            "the long one was answered"
        );
        // A later trigger: nothing left to ask in this ledger session.
        imported();
        std::thread::sleep(POLL * 4);
        let later = categoriser.stats();
        assert_eq!((later.runs, later.skipped), (1, failing));
        consistent(&ledger(&app));
        categoriser.shut_down();
        host.shut_down();
    }

    /// Done-when (and review findings 6 and 7), on one import, two locks:
    /// 1. While a question has the model (the probe hands the test the model's turn
    ///    mid-run): the lock stops the run and drops its session at once, without
    ///    waiting for the question.
    /// 2. In the next session, a real lock landing between two tokens: the answer stops
    ///    at the next token, the batch in hand isn't written, and the session is dropped.
    ///
    /// After each, the ledger holds exactly the batches applied before the lock, and the
    /// next unlocked session finishes the run.
    #[test]
    fn a_lock_stops_the_run_while_a_question_runs_and_mid_token() {
        let Some(host) = loaded_host() else { return };
        let (_dir, app, w) = app();
        import(&w, Some(ROWS));
        let (reached_tx, reached_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let go_rx = Mutex::new(Some(go_rx));
        let tokens = Arc::new(AtomicUsize::new(0));
        // The token count at which the second lock lands; 0 until the test arms it.
        let lock_at = Arc::new(AtomicUsize::new(0));
        let handle = app.handle().clone();
        let probe = Probe {
            on_token: Some(Arc::new({
                let tokens = Arc::clone(&tokens);
                let lock_at = Arc::clone(&lock_at);
                move || {
                    let n = tokens.fetch_add(1, Ordering::SeqCst);
                    if n == 150 {
                        let _ = reached_tx.send(());
                        if let Some(rx) = go_rx.lock().unwrap().take() {
                            let _ = rx.recv();
                        }
                    }
                    if n > 0 && n == lock_at.load(Ordering::SeqCst) {
                        // The worker holds no ledger lock while it answers.
                        handle.state::<AppUnlock>().lock();
                    }
                }
            })),
            ..Probe::default()
        };
        let categoriser = Categoriser::start_with(app.handle().clone(), Arc::clone(&host), probe);
        let written_matches = |stopped: &Stats| {
            let after = ledger(&app);
            assert_eq!(model_decisions(&after).len(), stopped.asked);
            consistent(&after);
        };

        // 1. A lock while a question runs.
        reached_rx.recv_timeout(Duration::from_secs(300)).unwrap();
        let question = host.interactive();
        go_tx.send(()).unwrap();
        // The worker finishes that answer, then waits for the question.
        std::thread::sleep(POLL);
        assert_eq!(categoriser.stats().interrupted, 0);
        let start = Instant::now();
        invoke(&w, "lock").unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "locking didn't wait"
        );
        assert!(
            wait(5, || categoriser.stats().interrupted == 1
                && categoriser.stats().sessions_alive == 0),
            "stopped and dropped while the question still runs"
        );
        drop(question);
        let first = categoriser.stats();
        invoke_with(&w, "unlock_with_password", json!({ "password": PASSWORD })).unwrap();
        written_matches(&first);

        // 2. A lock between two tokens, about ten answers into the next session.
        lock_at.store(tokens.load(Ordering::SeqCst) + 120, Ordering::SeqCst);
        assert!(wait(300, || categoriser.stats().interrupted == 2));
        assert!(wait(5, || categoriser.stats().sessions_alive == 0));
        let second = categoriser.stats();
        assert_eq!(second.sessions_built, 2);
        invoke_with(&w, "unlock_with_password", json!({ "password": PASSWORD })).unwrap();
        written_matches(&second);
        assert!(second.asked < ROWS);

        assert!(
            wait(600, || left(&app).is_empty()),
            "the next session finishes it"
        );
        assert_eq!(
            categoriser.stats().sessions_built,
            3,
            "a new session each time"
        );
        consistent(&ledger(&app));
        categoriser.shut_down();
        host.shut_down();
    }

    /// A batch whose answers no longer fit the ledger (here: none recorded) writes
    /// nothing: `run_fallback` fails inside its savepoint.
    #[test]
    fn a_stale_batch_writes_nothing() {
        let Some(host) = loaded_host() else { return };
        let (_dir, app, w) = app();
        import(&w, Some(ROWS));
        let model = host.model().unwrap();
        let before = ledger(&app);
        let ids = waiting(&before, model.id());
        let mut replay = Replay {
            model: &model,
            answers: HashMap::new(),
            missing: false,
        };
        let out = app
            .state::<AppUnlock>()
            .write_ledger(|open| {
                run_fallback(
                    open.ledger_mut(),
                    &mut replay,
                    &FallbackOptions {
                        threshold_bp: FALLBACK_THRESHOLD_BP,
                        scope: Scope::Transactions(&ids),
                    },
                    Timestamp::now(),
                )
            })
            .unwrap();
        assert!(out.is_err() && replay.missing);
        assert_eq!(ledger(&app).decisions.len(), before.decisions.len());
        drop(model);
        host.shut_down();
    }

    /// The real model (SPEC decision 48) on the renter's file: how long applying a batch
    /// holds the ledger (from taking the lock), on Metal and on the CPU only. Local only:
    /// point `FM_APP_REAL_MODEL` at the Qwen3.5-4B file; `FM_LLM_TEST_CPU=1` for the CPU.
    #[test]
    #[ignore = "needs the real model (FM_APP_REAL_MODEL); local only"]
    fn real_model_lock_hold_per_batch() {
        let path = PathBuf::from(std::env::var_os("FM_APP_REAL_MODEL").expect("FM_APP_REAL_MODEL"));
        let host = host();
        host.load(&path, &crate::model::model_spec()).expect("load");
        let (_dir, app, w) = app();
        let categoriser = Categoriser::start(app.handle().clone(), Arc::clone(&host));
        let start = Instant::now();
        import(&w, None);
        let model_id = crate::model::model_spec().id.to_string();
        assert!(wait(3600, || waiting(&ledger(&app), &model_id).is_empty()));
        let stats = categoriser.stats();
        eprintln!(
            "{}: asked {} ({} applied, {} skipped) in {:.0?}, {} batches, {} session(s); \
             longest lock hold {:.3?}",
            if std::env::var_os("FM_LLM_TEST_CPU").is_some() {
                "CPU only"
            } else {
                "GPU"
            },
            stats.asked,
            stats.categorised,
            stats.skipped,
            start.elapsed(),
            stats.batches,
            stats.sessions_built,
            stats.max_lock_hold
        );
        consistent(&ledger(&app));
        categoriser.shut_down();
        host.shut_down();
    }
}
