//! Importing a bank file from the UI (T3.3; T5.5's first run reuses it).
//!
//! One command does what the pipeline README orders after an import:
//! `pipeline::import_file` → `transfers::detect` → `categorize::run` over the rows the
//! import inserted or settled. Then `deletion::overlaps_deleted` decides whether the
//! summary carries [`OVERLAP_WARNING`] (SPEC decision 44).
//!
//! The report gives every record's outcome. A row matched to an existing transaction
//! creates no decision of its own, so a wrong match would otherwise be invisible: the UI
//! shows "N rows matched existing transactions" with a drill-down to each one.
//!
//! The file's bytes come as the raw IPC body; the rest as the `fm-import` header,
//! percent-encoded JSON ([`ImportArgs`]).

use std::sync::atomic::Ordering;

use fm_core::categories::names::{self, NameKind};
use fm_core::categorize::{self, Scope};
use fm_core::deletion::{OVERLAP_WARNING, overlaps_deleted};
use fm_core::model::{AccountId, AccountKind, Currency, Date, Timestamp, TransactionId};
use fm_core::pipeline::{self, ImportError, ImportRequest, RecordOutcome, Target};
use fm_core::transfers;
use serde::{Deserialize, Serialize};

use crate::ledger_view::{Result, UiError, counting_date, locked, ui_error};
use crate::unlock::AppUnlock;

/// The header carrying [`ImportArgs`].
pub const ARGS_HEADER: &str = "fm-import";

/// The largest file accepted, 50 MB. Bank exports are kilobytes to a few megabytes; the
/// cap keeps a wrong file from being read into memory whole. The UI checks it too,
/// before reading the file.
pub const MAX_IMPORT_BYTES: usize = 50 * 1024 * 1024;

/// What the UI says about the file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportArgs {
    pub file_name: Option<String>,
    pub target: TargetArg,
    /// The currency of a CSV file's account when it isn't known (CSV files don't say).
    pub csv_currency: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TargetArg {
    /// Find the account from the file, or create one.
    Auto,
    Account {
        id: i64,
    },
    NewAccount {
        name: String,
        account_kind: AccountKind,
        currency: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: i64,
    pub name: String,
    pub kind: AccountKind,
    pub currency: &'static str,
    pub institution: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementView {
    pub account_id: i64,
    pub account: String,
    pub account_created: bool,
    pub inserted: usize,
    pub duplicates: usize,
    pub posted_from_pending: usize,
    pub pending_already_posted: usize,
}

/// A file row that ended up as a transaction already in the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchedRow {
    /// The row's position in the file.
    pub source_index: u32,
    /// `duplicate`, `settledPending` (this posted row settled a pending one) or
    /// `alreadyPosted` (this pending row had already posted).
    pub outcome: &'static str,
    pub transaction_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportView {
    pub import_id: i64,
    pub statements: Vec<StatementView>,
    pub inserted: usize,
    /// "N rows matched existing transactions": duplicates, settled pending rows and
    /// pending rows already posted.
    pub matched: Vec<MatchedRow>,
    pub transfers_paired: usize,
    pub transfers_suggested: usize,
    pub rule_buckets: usize,
    pub rule_merchants: usize,
    pub cleaned_merchants: usize,
    pub suggestions: usize,
    pub rule_did_not_fit: usize,
    /// New or settled transactions still in Needs review after the rules ran.
    pub needs_review: usize,
    /// `None`: no backup folder is set (or the backup didn't run: see `incomplete`).
    pub backup: Option<BackupResult>,
    pub overlap_warning: Option<&'static str>,
    /// The import was kept, but this step didn't finish (it failed, or the app locked):
    /// "pairing transfers", "applying your rules" or "the backup". `finish_import` runs
    /// it, as does importing the same file again.
    pub incomplete: Option<&'static str>,
    /// This is `finish_import` completing an earlier import: no rows were read now.
    pub resumed: bool,
    /// Set when the saved CSV layouts couldn't be read, so only the built-in ones were
    /// tried (T3.5's notice).
    pub layouts_notice: Option<&'static str>,
}

/// An import whose later steps haven't run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnfinishedView {
    /// `None` when the app stopped while the file was being read.
    pub import_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupResult {
    pub ok: bool,
    pub message: Option<String>,
}

impl From<ImportError> for UiError {
    fn from(e: ImportError) -> Self {
        let code = match &e {
            ImportError::AlreadyImported { .. } => "AlreadyImported",
            ImportError::AccountNeeded => "AccountNeeded",
            ImportError::CurrencyMismatch { .. } => "CurrencyMismatch",
            ImportError::SeveralStatements => "SeveralStatements",
            ImportError::UnknownAccount(_) => "NotFound",
            // No CSV layout fits: the user can map the columns (T3.5).
            ImportError::Csv(
                fm_import_csv::ImportError::UnknownLayout
                | fm_import_csv::ImportError::Ambiguous(_)
                | fm_import_csv::ImportError::HeaderNotFound,
            ) => "NoLayout",
            ImportError::Ofx(_) | ImportError::Csv(_) => "Unreadable",
            ImportError::Interrupted => "Interrupted",
            ImportError::InvalidName(_) => "InvalidName",
            ImportError::InvalidRecord { .. } => "InvalidRecord",
            ImportError::TransactionOpen => "Busy",
            _ => "Storage",
        };
        ui_error(code, e.to_string())
    }
}

/// Decodes `%XX` escapes (as `encodeURIComponent` writes them).
pub fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn currency(code: &str) -> Result<Currency> {
    Currency::new(code).map_err(|e| ui_error("InvalidCurrency", e.to_string()))
}

impl AppUnlock {
    pub fn accounts(&self) -> Result<Vec<AccountView>> {
        self.with_ledger(|open| {
            Ok(open
                .ledger()
                .accounts()?
                .into_iter()
                .map(|a| AccountView {
                    id: a.id.0,
                    name: a.name,
                    kind: a.kind,
                    currency: a.currency.code(),
                    institution: a.institution,
                })
                .collect())
        })
        .ok_or_else(locked)?
    }

    /// Imports a file in steps, each holding the ledger only while it runs, so locking,
    /// auto-lock and quitting never wait for the whole of a long import:
    ///
    /// 1. the pipeline (stoppable: a lock rolls it back, and nothing is stored);
    /// 2. pairing transfers; 3. the rules; 4. the backup (stoppable).
    ///
    /// Once step 1 has committed, the import is kept whatever happens next. If a later
    /// step fails, or the app locks first, the result says what didn't finish
    /// (`incomplete`), and [`Self::finish_import`] (or importing the same file again)
    /// runs the rest. The import is marked unfinished in the ledger before step 1 and
    /// cleared only after step 4, so even a crash leaves it to finish later.
    pub fn import_file(&self, bytes: &[u8], args: ImportArgs) -> Result<ImportView> {
        if bytes.is_empty() {
            return Err(ui_error("Empty", "the file is empty"));
        }
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(ui_error(
                "TooLarge",
                "the file is over 50 MB; bank exports are much smaller",
            ));
        }
        let now = std::time::SystemTime::now();
        let started = self
            .write_ledger(|open| -> Result<Started> {
                let unlocked = open.ledger_mut();
                let previous = unfinished(unlocked)?;
                let (target, csv_currency) = import_target(unlocked, &args)?;
                // The built-in layouts and the user's saved ones (T3.5).
                let (profiles, layouts_notice) = crate::csv_mapping::load_user_profiles(unlocked);
                // Marked before the import, so a committed import is never left unmarked.
                set_unfinished(unlocked, Some(PENDING))?;
                let cancel = || self.interrupt.load(Ordering::SeqCst);
                let result = pipeline::import_file(
                    unlocked,
                    &ImportRequest {
                        bytes,
                        file_name: args.file_name.as_deref(),
                        target,
                        csv_currency,
                        csv_profiles: &profiles,
                        now,
                        // Taken after transfers and the rules (step 4).
                        backup: false,
                        cancel: &cancel,
                    },
                );
                match result {
                    Ok(report) => {
                        let mark = report.import_id.0.to_string();
                        set_unfinished(unlocked, Some(&mark))?;
                        Ok(Started::New {
                            report,
                            earlier_unfinished: previous.is_some(),
                            mark,
                            generation: self.generation(),
                            layouts_notice,
                        })
                    }
                    // The same file again, and it is the import left unfinished (or one
                    // whose id the app never learned): finish it.
                    Err(ImportError::AlreadyImported { import_id })
                        if previous.as_deref() == Some(&import_id.0.to_string())
                            || previous.as_deref() == Some(PENDING) =>
                    {
                        set_unfinished(unlocked, previous.as_deref())?;
                        Ok(Started::Resume(import_id))
                    }
                    Err(e) => {
                        set_unfinished(unlocked, previous.as_deref())?;
                        let mut e = UiError::from(e);
                        // Mapping columns only helps a file that is a table at all.
                        if e.code == "NoLayout" && !looks_tabular(bytes) {
                            e.code = "Unreadable";
                        }
                        Err(e)
                    }
                }
            })
            .ok_or_else(locked)??;
        match started {
            Started::New {
                report,
                earlier_unfinished,
                mark,
                generation,
                layouts_notice,
            } => {
                let mut changed = Vec::new();
                let mut matched = Vec::new();
                for s in &report.statements {
                    for (index, outcome) in &s.records {
                        match outcome {
                            RecordOutcome::Inserted(t) | RecordOutcome::PostedPending(t) => {
                                changed.push(*t);
                            }
                            _ => {}
                        }
                        let kind = match outcome {
                            RecordOutcome::Inserted(_) => None,
                            RecordOutcome::Duplicate(_) => Some("duplicate"),
                            RecordOutcome::PostedPending(_) => Some("settledPending"),
                            RecordOutcome::PendingAlreadyPosted(_) => Some("alreadyPosted"),
                        };
                        if let Some(kind) = kind {
                            matched.push(MatchedRow {
                                source_index: *index,
                                outcome: kind,
                                transaction_id: outcome.transaction().0,
                            });
                        }
                    }
                }
                // An earlier import left unfinished: the rules run over everything, so
                // its transactions are covered too.
                let scope = if earlier_unfinished {
                    None
                } else {
                    Some(changed.clone())
                };
                let after = self.after_import(scope, now, &mark, generation);
                let mut view = self.import_view(report, changed, matched, after)?;
                view.layouts_notice = layouts_notice;
                Ok(view)
            }
            Started::Resume(import_id) => self.finish_import(Some(import_id.0)),
        }
    }

    /// Runs what an import left unfinished (pairing transfers, the rules over every
    /// transaction, the backup). Safe to call again: when nothing is unfinished it does
    /// nothing and says so.
    /// `import_id`: the unfinished import, when known; its rows' review count is shown.
    pub fn finish_import(&self, import_id: Option<i64>) -> Result<ImportView> {
        let (pending, generation) = self
            .with_ledger(|open| unfinished(open.ledger()).map(|m| (m, self.generation())))
            .ok_or_else(locked)??;
        let after = match pending {
            Some(mark) => self.after_import(None, std::time::SystemTime::now(), &mark, generation),
            None => AfterImport::default(),
        };
        self.with_ledger(|open| -> Result<ImportView> {
            let ledger = open.ledger().load_ledger()?;
            let current = crate::ledger_view::current_by_transaction(&ledger);
            let needs_review = ledger
                .transactions
                .iter()
                .filter(|t| import_id.is_none_or(|id| t.source.import_id.0 == id))
                .filter(|t| current.get(&t.id).is_none_or(|c| c.needs_review()))
                .count();
            Ok(ImportView {
                import_id: import_id.unwrap_or(0),
                resumed: true,
                needs_review,
                ..after.into_view()
            })
        })
        .ok_or_else(locked)?
    }

    /// The unfinished import, if any: `Some(None)` when one was started but its id isn't
    /// known (the app stopped mid-import).
    pub fn unfinished_import(&self) -> Result<Option<UnfinishedView>> {
        let marker = self
            .with_ledger(|open| unfinished(open.ledger()))
            .ok_or_else(locked)??;
        Ok(marker.map(|m| UnfinishedView {
            import_id: m.parse().ok(),
        }))
    }

    fn interrupted(&self) -> bool {
        self.interrupt.load(Ordering::SeqCst)
    }

    /// Steps 2–4, each taking the ledger only while it runs. `scope`: the transactions
    /// the rules look at, or `None` for all of them. `mark`: this run's unfinished-import
    /// mark, the only one it clears. `generation`: the session the run started on; a step
    /// never runs on another (the app locked and unlocked in between). Stops at the first
    /// step that fails or finds the app locking, and says which; the import stays
    /// unfinished.
    pub(crate) fn after_import(
        &self,
        scope: Option<Vec<TransactionId>>,
        now: std::time::SystemTime,
        mark: &str,
        generation: u64,
    ) -> AfterImport {
        let mut after = AfterImport::default();
        let stamp = Timestamp::now();
        let step = |name: &'static str,
                    after: &mut AfterImport,
                    run: &mut dyn FnMut(&mut fm_core::storage::Unlocked) -> Result<()>|
         -> bool {
            if self.interrupted() {
                after.incomplete = Some(name);
                return false;
            }
            let ran = self.write_ledger(|open| {
                if self.generation() != generation {
                    return Err(locked());
                }
                run(open.ledger_mut())
            });
            match ran {
                Some(Ok(())) => true,
                Some(Err(_)) | None => {
                    after.incomplete = Some(name);
                    false
                }
            }
        };
        let mut transfers = None;
        if !step("pairing transfers", &mut after, &mut |u| {
            transfers = Some(
                transfers::detect(u, stamp).map_err(|e| ui_error("Transfers", e.to_string()))?,
            );
            Ok(())
        }) {
            return after;
        }
        if let Some(t) = transfers {
            after.transfers_paired = t.applied.len();
            after.transfers_suggested = t.suggested.len();
        }
        let mut run = None;
        let scope_ref = scope.as_deref();
        if !step("applying your rules", &mut after, &mut |u| {
            let s = match scope_ref {
                Some(ids) => Scope::Transactions(ids),
                None => Scope::All,
            };
            run = Some(categorize::run(u, s, stamp)?);
            Ok(())
        }) {
            return after;
        }
        if let Some(r) = run {
            after.rule_buckets = r.rule_buckets;
            after.rule_merchants = r.rule_merchants;
            after.cleaned_merchants = r.cleaned_merchants;
            after.suggestions = r.suggestions;
            after.rule_did_not_fit = r.rule_did_not_fit;
        }
        let mut backup = None;
        let cancel = || self.interrupted();
        if !step("the backup", &mut after, &mut |u| {
            // Cleared first, so the backup doesn't carry the mark (a restore would offer
            // to finish an import that finished), and only if it is still this run's.
            let cleared = pipeline::clear_unfinished_if(u, mark)
                .map_err(|e| ui_error("Storage", e.to_string()))?;
            backup = fm_core::backup::after_import_cancellable(u, now, &cancel);
            if matches!(backup, Some(Err(fm_core::backup::BackupError::Interrupted))) {
                if cleared {
                    set_unfinished(u, Some(mark))?;
                }
                return Err(ui_error("Interrupted", "the backup stopped"));
            }
            Ok(())
        }) {
            return after;
        }
        after.backup = backup.map(|b| match b {
            Ok(made) => BackupResult {
                ok: true,
                message: made.warning,
            },
            Err(e) => BackupResult {
                ok: false,
                message: Some(e.to_string()),
            },
        });
        // The model's turn (T4.6), for what the rules left in Needs review: both file
        // imports and SimpleFIN syncs end here. It runs in the background, in small
        // steps, so the import's summary and the backup don't wait for it.
        crate::categoriser::imported();
        after
    }

    /// The summary of a new import.
    fn import_view(
        &self,
        report: pipeline::ImportReport,
        changed: Vec<TransactionId>,
        matched: Vec<MatchedRow>,
        after: AfterImport,
    ) -> Result<ImportView> {
        let import_id = report.import_id.0;
        self.with_ledger(|open| -> Result<ImportView> {
            let unlocked = open.ledger();
            let ledger = unlocked.load_ledger()?;
            let current = crate::ledger_view::current_by_transaction(&ledger);
            let needs_review = changed
                .iter()
                .filter(|t| current.get(t).is_none_or(|c| c.needs_review()))
                .count();
            let mut overlap = false;
            let mut statements = Vec::new();
            for s in &report.statements {
                let dates: Vec<Date> = s
                    .records
                    .iter()
                    .filter_map(|(_, o)| {
                        ledger.transactions.iter().find(|t| t.id == o.transaction())
                    })
                    .map(counting_date)
                    .collect();
                if let (Some(from), Some(to)) = (dates.iter().min(), dates.iter().max()) {
                    overlap |= overlaps_deleted(unlocked, Some(s.account_id), *from, *to)
                        .map_err(|e| ui_error("Storage", e.to_string()))?;
                }
                statements.push(StatementView {
                    account_id: s.account_id.0,
                    account: ledger
                        .accounts
                        .iter()
                        .find(|a| a.id == s.account_id)
                        .map_or_else(|| format!("account {}", s.account_id.0), |a| a.name.clone()),
                    account_created: s.account_created,
                    inserted: s.inserted(),
                    duplicates: s.duplicates(),
                    posted_from_pending: s.posted_from_pending(),
                    pending_already_posted: s.pending_already_posted(),
                });
            }
            Ok(ImportView {
                import_id,
                inserted: statements.iter().map(|s| s.inserted).sum(),
                statements,
                matched,
                needs_review,
                overlap_warning: overlap.then_some(OVERLAP_WARNING),
                ..after.into_view()
            })
        })
        .ok_or_else(locked)?
    }
}

/// Step 1's outcome.
enum Started {
    New {
        report: pipeline::ImportReport,
        /// Another import was still unfinished when this one started.
        earlier_unfinished: bool,
        /// This import's unfinished mark (its id).
        mark: String,
        /// The session it ran on.
        generation: u64,
        /// Set when the saved CSV layouts couldn't be read.
        layouts_notice: Option<&'static str>,
    },
    /// The same file again while its import is unfinished.
    Resume(fm_core::model::ImportId),
}

/// What steps 2–4 did.
#[derive(Debug, Default)]
pub(crate) struct AfterImport {
    pub(crate) transfers_paired: usize,
    pub(crate) transfers_suggested: usize,
    pub(crate) rule_buckets: usize,
    pub(crate) rule_merchants: usize,
    pub(crate) cleaned_merchants: usize,
    pub(crate) suggestions: usize,
    pub(crate) rule_did_not_fit: usize,
    pub(crate) backup: Option<BackupResult>,
    /// The step that didn't finish.
    pub(crate) incomplete: Option<&'static str>,
}

impl AfterImport {
    fn into_view(self) -> ImportView {
        ImportView {
            import_id: 0,
            statements: vec![],
            inserted: 0,
            matched: vec![],
            transfers_paired: self.transfers_paired,
            transfers_suggested: self.transfers_suggested,
            rule_buckets: self.rule_buckets,
            rule_merchants: self.rule_merchants,
            cleaned_merchants: self.cleaned_merchants,
            suggestions: self.suggestions,
            rule_did_not_fit: self.rule_did_not_fit,
            needs_review: 0,
            backup: self.backup,
            overlap_warning: None,
            incomplete: self.incomplete,
            resumed: false,
            layouts_notice: None,
        }
    }
}

/// The target account and a CSV file's currency, from what the UI sent.
fn import_target(
    unlocked: &fm_core::storage::Unlocked,
    args: &ImportArgs,
) -> Result<(Target, Currency)> {
    let (target, account_currency) = match &args.target {
        TargetArg::Auto => (Target::Auto, None),
        TargetArg::Account { id } => {
            let account = unlocked
                .account(AccountId(*id))?
                .ok_or_else(|| ui_error("NotFound", format!("no account {id}")))?;
            (Target::Account(account.id), Some(account.currency))
        }
        TargetArg::NewAccount {
            name,
            account_kind,
            currency: code,
        } => {
            let name = names::clean(NameKind::Account, name)
                .map_err(|e| ui_error("InvalidName", e.to_string()))?;
            let c = currency(code)?;
            (
                Target::NewAccount {
                    name,
                    kind: *account_kind,
                    currency: c,
                },
                Some(c),
            )
        }
    };
    let csv_currency = match (&args.csv_currency, account_currency) {
        (Some(code), _) => currency(code)?,
        (None, Some(c)) => c,
        (None, None) => Currency::USD,
    };
    Ok((target, csv_currency))
}

/// A file whose first line has at least two columns (comma, semicolon or tab separated):
/// a CSV whose layout could be mapped by hand.
fn looks_tabular(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    let Ok(text) = std::str::from_utf8(head.split(|b| *b == b'\n').next().unwrap_or(head)) else {
        return false;
    };
    [',', ';', '\t'].iter().any(|d| {
        text.split(*d)
            .filter(|f| !f.trim().trim_matches('"').is_empty())
            .count()
            >= 2
    })
}

/// The value of the unfinished-import mark while step 1 runs (the id isn't known yet).
const PENDING: &str = "pending";

fn unfinished(unlocked: &fm_core::storage::Unlocked) -> Result<Option<String>> {
    pipeline::unfinished(unlocked).map_err(|e| ui_error("Storage", e.to_string()))
}

fn set_unfinished(unlocked: &fm_core::storage::Unlocked, value: Option<&str>) -> Result<()> {
    pipeline::set_unfinished(unlocked, value).map_err(|e| ui_error("Storage", e.to_string()))
}

pub mod commands {
    use tauri::State;
    use tauri::ipc::{InvokeBody, Request};

    use super::{ARGS_HEADER, AccountView, ImportArgs, ImportView, UnfinishedView, percent_decode};
    use crate::ledger_view::{Result, ui_error};
    use crate::unlock::AppUnlock;

    #[tauri::command(async)]
    pub fn accounts(unlock: State<'_, AppUnlock>) -> Result<Vec<AccountView>> {
        unlock.accounts()
    }

    /// The body is the file's bytes; the `fm-import` header is the percent-encoded JSON of
    /// [`ImportArgs`].
    #[tauri::command(async)]
    pub fn import_file(unlock: State<'_, AppUnlock>, request: Request<'_>) -> Result<ImportView> {
        let InvokeBody::Raw(bytes) = request.body() else {
            return Err(ui_error("BadRequest", "send the file's bytes as the body"));
        };
        let args: ImportArgs = request
            .headers()
            .get(ARGS_HEADER)
            .and_then(|v| v.to_str().ok())
            .and_then(percent_decode)
            .and_then(|json| serde_json::from_str(&json).ok())
            .ok_or_else(|| ui_error("BadRequest", "missing or invalid import details"))?;
        unlock.import_file(bytes, args)
    }

    /// Runs what an import left unfinished; harmless when nothing is.
    #[tauri::command(async)]
    pub fn finish_import(
        unlock: State<'_, AppUnlock>,
        import_id: Option<i64>,
    ) -> Result<ImportView> {
        unlock.finish_import(import_id)
    }

    /// The import left unfinished, if any, so the import screen can offer to finish it.
    #[tauri::command(async)]
    pub fn unfinished_import(unlock: State<'_, AppUnlock>) -> Result<Option<UnfinishedView>> {
        unlock.unfinished_import()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use tauri::Manager;

    use super::*;
    use crate::tests::{create_confirmed, mock_app};

    const PASSWORD: &str = "velvet anchor mosaic tundra";

    fn chase_checking() -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../testdata/bank-exports/data/chase/checking_2025.csv"),
        )
        .unwrap()
    }

    /// The Chase checking file with its rows repeated `times`: every copy is a new row.
    fn long_file(times: usize) -> Vec<u8> {
        let text = String::from_utf8(chase_checking()).unwrap();
        let (header, rows) = text.split_once('\n').unwrap();
        let mut out = format!("{header}\n");
        for _ in 0..times {
            out.push_str(rows);
            if !rows.ends_with('\n') {
                out.push('\n');
            }
        }
        out.into_bytes()
    }

    fn new_account() -> ImportArgs {
        ImportArgs {
            file_name: Some("checking.csv".into()),
            target: TargetArg::NewAccount {
                name: "Everyday checking".into(),
                account_kind: AccountKind::Checking,
                currency: "USD".into(),
            },
            csv_currency: None,
        }
    }

    fn unlocked_app(dir: &std::path::Path) -> tauri::App<tauri::test::MockRuntime> {
        let (app, w) = mock_app(&std::fs::canonicalize(dir).unwrap());
        create_confirmed(&w);
        app
    }

    fn reopen(unlock: &AppUnlock) {
        unlock
            .unlock_with_password(zeroize::Zeroizing::new(PASSWORD.to_string()))
            .unwrap();
    }

    /// T3.6 done-when: locking during a long import returns promptly; the import stops,
    /// keeps nothing, and the file imports normally later.
    #[test]
    fn a_lock_during_a_long_import_returns_promptly_and_keeps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let app = unlocked_app(dir.path());
        let unlock = app.state::<AppUnlock>();
        let file = long_file(150);
        let (result, waited) = std::thread::scope(|s| {
            let import = s.spawn(|| unlock.import_file(&file, new_account()));
            std::thread::sleep(Duration::from_millis(300));
            let started = Instant::now();
            unlock.lock();
            let waited = started.elapsed();
            (import.join().unwrap(), waited)
        });
        assert!(
            waited < Duration::from_secs(3),
            "the lock waited {waited:?}"
        );
        let err = result.expect_err("the file is long enough to still be importing");
        assert_eq!(err.code, "Interrupted", "{}", err.message);
        assert!(
            !unlock.interrupt.load(Ordering::SeqCst),
            "the flag is lowered again"
        );

        reopen(&unlock);
        let ledger = unlock
            .with_ledger(|o| o.ledger().load_ledger().unwrap())
            .unwrap();
        assert!(ledger.transactions.is_empty(), "nothing was kept");
        assert!(ledger.accounts.is_empty(), "not even the new account");
        assert_eq!(unlock.unfinished_import().unwrap(), None);
        let report = unlock
            .import_file(&chase_checking(), new_account())
            .unwrap();
        assert_eq!(report.incomplete, None);
        assert!(report.inserted > 100);
    }

    /// T3.6: steps left undone after the import committed are reported, survive a lock,
    /// and are finished by finish_import or by importing the same file again, once.
    #[test]
    fn unfinished_steps_are_reported_and_finished_once() {
        let dir = tempfile::tempdir().unwrap();
        let app = unlocked_app(dir.path());
        let unlock = app.state::<AppUnlock>();
        let file = chase_checking();
        let first = unlock.import_file(&file, new_account()).unwrap();
        assert_eq!(first.incomplete, None);
        assert_eq!(unlock.unfinished_import().unwrap(), None);

        // The app starts locking right after the import commits: the later steps stop.
        unlock
            .write_ledger(|o| {
                pipeline::set_unfinished(o.ledger(), Some(&first.import_id.to_string()))
            })
            .unwrap()
            .unwrap();
        unlock.interrupt.store(true, Ordering::SeqCst);
        let mark = first.import_id.to_string();
        let after = unlock.after_import(
            None,
            std::time::SystemTime::now(),
            &mark,
            unlock.generation(),
        );
        unlock.interrupt.store(false, Ordering::SeqCst);
        assert_eq!(after.incomplete, Some("pairing transfers"));
        unlock.lock();
        reopen(&unlock);
        assert_eq!(
            unlock.unfinished_import().unwrap(),
            Some(UnfinishedView {
                import_id: Some(first.import_id)
            }),
            "kept across a lock"
        );

        // Importing the same file again finishes it instead of refusing it.
        let resumed = unlock.import_file(&file, new_account()).unwrap();
        assert!(resumed.resumed);
        assert_eq!(resumed.import_id, first.import_id);
        assert_eq!(resumed.incomplete, None);
        assert_eq!(unlock.unfinished_import().unwrap(), None);
        // Once finished, the same file is refused as usual.
        let err = unlock.import_file(&file, new_account()).unwrap_err();
        assert_eq!(err.code, "AlreadyImported");
        // finish_import is harmless when nothing is unfinished, however often.
        for _ in 0..2 {
            let again = unlock.finish_import(Some(first.import_id)).unwrap();
            assert!(again.resumed);
            assert_eq!(again.incomplete, None);
            assert_eq!(
                again.transfers_paired + again.rule_buckets,
                0,
                "nothing re-run"
            );
        }
    }

    /// T3.6: the backup after an import is taken after transfers and the rules, so it
    /// holds what they decided.
    #[test]
    fn the_post_import_backup_holds_transfers_and_rules() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let app = unlocked_app(&root);
        let unlock = app.state::<AppUnlock>();
        let folder = root.join("backups");
        std::fs::create_dir(&folder).unwrap();
        unlock.set_backup_folder(&folder).unwrap();

        let report = unlock
            .import_file(&chase_checking(), new_account())
            .unwrap();
        assert!(
            report.backup.as_ref().is_some_and(|b| b.ok),
            "{:?}",
            report.backup
        );
        assert!(report.cleaned_merchants > 0, "the rules ran");
        let live = unlock
            .with_ledger(|o| o.ledger().load_ledger().unwrap())
            .unwrap();

        let backup = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "fmbackup"))
            .unwrap();
        let restored = fm_core::storage::Vault::at(&root.join("restored"));
        fm_core::backup::restore(&backup, &restored).unwrap();
        let reopened = restored.unlock(PASSWORD.as_bytes()).unwrap();
        assert_eq!(
            pipeline::unfinished(&reopened).unwrap(),
            None,
            "a restored backup offers no unfinished import (review T3.6 #2)"
        );
        let restored = reopened.load_ledger().unwrap();
        assert_eq!(
            restored.decisions, live.decisions,
            "the backup has the rules' work"
        );
        assert_eq!(restored.transactions, live.transactions);
    }

    /// Review T3.6 #3, #6, #5: a run clears only its own mark; re-importing a finished
    /// file never resumes another import; a step never runs on a session other than the
    /// one it started on.
    #[test]
    fn marks_and_sessions_are_never_mixed_up() {
        let dir = tempfile::tempdir().unwrap();
        let app = unlocked_app(dir.path());
        let unlock = app.state::<AppUnlock>();
        let first = unlock
            .import_file(&chase_checking(), new_account())
            .unwrap();
        let other = long_file(2);
        let mut args = new_account();
        args.target = TargetArg::NewAccount {
            name: "Second".into(),
            account_kind: AccountKind::Checking,
            currency: "USD".into(),
        };
        let second = unlock.import_file(&other, args).unwrap();
        let set_mark = |m: &str| {
            unlock
                .write_ledger(|o| pipeline::set_unfinished(o.ledger(), Some(m)))
                .unwrap()
                .unwrap();
        };
        let mark_now = || unlock.unfinished_import().unwrap();

        // #3: a run finishing with its own mark leaves another import's mark alone.
        set_mark(&second.import_id.to_string());
        let after = unlock.after_import(
            None,
            std::time::SystemTime::now(),
            &first.import_id.to_string(),
            unlock.generation(),
        );
        assert_eq!(after.incomplete, None);
        assert_eq!(
            mark_now(),
            Some(UnfinishedView {
                import_id: Some(second.import_id)
            })
        );

        // #6: the first file again doesn't resume the second import.
        let err = unlock
            .import_file(&chase_checking(), new_account())
            .unwrap_err();
        assert_eq!(err.code, "AlreadyImported");
        assert!(mark_now().is_some(), "the other import is still unfinished");

        // #5: a step started before a lock never runs after the unlock.
        let started_on = unlock.generation();
        unlock.lock();
        reopen(&unlock);
        assert_ne!(unlock.generation(), started_on);
        let stale = unlock.after_import(
            None,
            std::time::SystemTime::now(),
            &second.import_id.to_string(),
            started_on,
        );
        assert_eq!(stale.incomplete, Some("pairing transfers"));
        assert!(mark_now().is_some(), "nothing ran, nothing cleared");
    }

    /// T3.6: account names are limited like every other name.
    #[test]
    fn a_new_account_name_must_be_printable_and_short() {
        let dir = tempfile::tempdir().unwrap();
        let app = unlocked_app(dir.path());
        let unlock = app.state::<AppUnlock>();
        for name in ["", "Checking\u{202E}gnikcehc", "Tab\there", &"x".repeat(61)] {
            let mut args = new_account();
            args.target = TargetArg::NewAccount {
                name: name.into(),
                account_kind: AccountKind::Checking,
                currency: "USD".into(),
            };
            let err = unlock.import_file(&chase_checking(), args).unwrap_err();
            assert_eq!(err.code, "InvalidName", "{name:?}");
        }
        assert_eq!(unlock.unfinished_import().unwrap(), None);
        assert!(unlock.accounts().unwrap().is_empty());
    }

    /// A CSV no layout fits points the user at mapping its columns (T3.5).
    #[test]
    fn a_csv_with_no_known_layout_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let app = unlocked_app(dir.path());
        let unlock = app.state::<AppUnlock>();
        let err = unlock
            .import_file(
                b"When,What,How much\n2025-01-02,Coffee,-3.50\n",
                new_account(),
            )
            .unwrap_err();
        assert_eq!(err.code, "NoLayout", "{}", err.message);
    }

    #[test]
    fn percent_decoding_matches_encode_uri_component() {
        assert_eq!(
            percent_decode("%7B%22a%22%3A%22caf%C3%A9.csv%22%7D").as_deref(),
            Some("{\"a\":\"café.csv\"}")
        );
        assert_eq!(percent_decode("plain").as_deref(), Some("plain"));
        assert_eq!(percent_decode("%ZZ"), None);
        assert_eq!(percent_decode("%4"), None);
        assert_eq!(percent_decode("%FF"), None, "not UTF-8");
    }
}
