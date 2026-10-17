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
    /// `None`: no backup folder is set.
    pub backup: Option<BackupResult>,
    pub overlap_warning: Option<&'static str>,
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
            ImportError::Ofx(_) | ImportError::Csv(_) => "Unreadable",
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
        self.write_ledger(|open| {
            let unlocked = open.ledger_mut();
            let (target, account_currency) = match args.target {
                TargetArg::Auto => (Target::Auto, None),
                TargetArg::Account { id } => {
                    let account = unlocked
                        .account(AccountId(id))?
                        .ok_or_else(|| ui_error("NotFound", format!("no account {id}")))?;
                    (Target::Account(account.id), Some(account.currency))
                }
                TargetArg::NewAccount {
                    name,
                    account_kind,
                    currency: code,
                } => {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        return Err(ui_error("InvalidName", "the account needs a name"));
                    }
                    let c = currency(&code)?;
                    (
                        Target::NewAccount {
                            name,
                            kind: account_kind,
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
            let profiles = fm_import_csv::ProfileSet::with_builtin();
            let now = std::time::SystemTime::now();
            let report = pipeline::import_file(
                unlocked,
                &ImportRequest {
                    bytes,
                    file_name: args.file_name.as_deref(),
                    target,
                    csv_currency,
                    csv_profiles: &profiles,
                    now,
                },
            )?;

            let stamp = Timestamp::now();
            let transfers = transfers::detect(unlocked, stamp)
                .map_err(|e| ui_error("Transfers", e.to_string()))?;
            let mut changed: Vec<TransactionId> = Vec::new();
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
            let run = categorize::run(unlocked, Scope::Transactions(&changed), stamp)?;

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
                import_id: report.import_id.0,
                inserted: statements.iter().map(|s| s.inserted).sum(),
                statements,
                matched,
                transfers_paired: transfers.applied.len(),
                transfers_suggested: transfers.suggested.len(),
                rule_buckets: run.rule_buckets,
                rule_merchants: run.rule_merchants,
                cleaned_merchants: run.cleaned_merchants,
                suggestions: run.suggestions,
                rule_did_not_fit: run.rule_did_not_fit,
                needs_review,
                backup: report.backup.map(|b| match b {
                    Ok(made) => BackupResult {
                        ok: true,
                        message: made.warning,
                    },
                    Err(e) => BackupResult {
                        ok: false,
                        message: Some(e.to_string()),
                    },
                }),
                overlap_warning: overlap.then_some(OVERLAP_WARNING),
            })
        })
        .ok_or_else(locked)?
    }
}

pub mod commands {
    use tauri::State;
    use tauri::ipc::{InvokeBody, Request};

    use super::{ARGS_HEADER, AccountView, ImportArgs, ImportView, percent_decode};
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
}

#[cfg(test)]
mod tests {
    use super::percent_decode;

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
