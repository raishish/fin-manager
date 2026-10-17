//! The import pipeline (T1.3): a downloaded bank file in, ledger transactions out.
//!
//! 1. **Raw layer.** The file's bytes and SHA-256 are stored as a [`RawImport`]. The same
//!    bytes a second time are refused ([`ImportError::AlreadyImported`]).
//! 2. **Parse.** OFX/QFX through `fm-import-ofx`, anything else as CSV through
//!    `fm-import-csv`; both give `fm_import_types::Statement`s.
//! 3. **Accounts.** Each statement goes to the account the caller names, or (with
//!    [`Target::Auto`]) to the existing account with the same account number, or the same
//!    last four digits at the same institution, or to a new account.
//! 4. **Transactions.** Each record is matched against the account's transactions
//!    (`matcher.rs`): duplicates of what is already there are skipped, a posted record
//!    settles the pending transaction it belongs to (through `Unlocked::post_pending`),
//!    a pending record for something already posted is dropped, and the rest are
//!    inserted, each with an applied Needs review decision (SPEC constraint 4).
//! 5. **Backup.** After the import commits, `backup::after_import` runs (T5.2).
//!
//! Steps 1–4 run in one savepoint: an import is stored completely or not at all.

mod institution;
mod matcher;

#[cfg(test)]
mod tests;

use std::fmt;
use std::time::SystemTime;

use fm_import_types as imported;

use crate::backup;
use crate::model::{
    Account, AccountId, AccountKind, Assignment, Bucket, BuiltinLogic, Currency, Date, Decision,
    DecisionId, DecisionSource, DecisionState, ImportId, ImportSource, Money, RawImport,
    Sha256Hash, SourceRef, Timestamp, Transaction, TransactionId, TransactionStatus,
};
use crate::storage::{StorageError, Table, Unlocked};

use matcher::{Candidate, DatesGiven, Matcher, Outcome};

/// The version of the pipeline's own decisions (Needs review for new transactions).
pub const PIPELINE_VERSION: u32 = 1;

/// Which account a file's statement belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Find it by account number or last digits, or create one. A CSV file usually
    /// can't be identified this way, so the caller asks the user instead.
    Auto,
    /// This existing account (all of the file's statements must be one account).
    Account(AccountId),
    /// A new account (all of the file's statements must be one account).
    NewAccount {
        name: String,
        kind: AccountKind,
        currency: Currency,
    },
}

/// One file to import.
pub struct ImportRequest<'a> {
    pub bytes: &'a [u8],
    pub file_name: Option<&'a str>,
    pub target: Target,
    /// The currency of a CSV file's account (CSV files don't state one).
    pub csv_currency: Currency,
    /// The CSV layouts to recognise the file with.
    pub csv_profiles: &'a fm_import_csv::ProfileSet,
    /// When the import happens (stored on the raw import and decisions).
    pub now: SystemTime,
}

/// What happened to one record of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    /// Inserted as a new transaction.
    Inserted(TransactionId),
    /// Already in the ledger as this transaction.
    Duplicate(TransactionId),
    /// The posted version of this pending transaction, which it settled.
    PostedPending(TransactionId),
    /// A pending version of this transaction, which had already posted.
    PendingAlreadyPosted(TransactionId),
}

impl RecordOutcome {
    /// The ledger transaction the record ended up as.
    pub fn transaction(self) -> TransactionId {
        match self {
            Self::Inserted(t)
            | Self::Duplicate(t)
            | Self::PostedPending(t)
            | Self::PendingAlreadyPosted(t) => t,
        }
    }
}

/// One statement's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementReport {
    pub account_id: AccountId,
    pub account_created: bool,
    /// `(source_index, outcome)` for every record, in file order.
    pub records: Vec<(u32, RecordOutcome)>,
}

impl StatementReport {
    fn count(&self, f: fn(&RecordOutcome) -> bool) -> usize {
        self.records.iter().filter(|(_, o)| f(o)).count()
    }

    pub fn inserted(&self) -> usize {
        self.count(|o| matches!(o, RecordOutcome::Inserted(_)))
    }

    pub fn duplicates(&self) -> usize {
        self.count(|o| matches!(o, RecordOutcome::Duplicate(_)))
    }

    pub fn posted_from_pending(&self) -> usize {
        self.count(|o| matches!(o, RecordOutcome::PostedPending(_)))
    }

    pub fn pending_already_posted(&self) -> usize {
        self.count(|o| matches!(o, RecordOutcome::PendingAlreadyPosted(_)))
    }
}

/// The result of importing one file.
#[derive(Debug)]
pub struct ImportReport {
    pub import_id: ImportId,
    pub statements: Vec<StatementReport>,
    /// The backup taken after the import: `None` if no backup folder is set.
    pub backup: Option<Result<backup::Made, backup::BackupError>>,
}

/// Why a file couldn't be imported. Never repeats file contents.
#[derive(Debug)]
#[non_exhaustive]
pub enum ImportError {
    /// These exact bytes were imported before, as this import.
    AlreadyImported {
        import_id: ImportId,
    },
    Ofx(fm_import_ofx::Error),
    Csv(fm_import_csv::ImportError),
    /// The file holds statements of several accounts, so a single target account is
    /// ambiguous.
    SeveralStatements,
    /// The target account doesn't exist.
    UnknownAccount(AccountId),
    /// The statement's currency isn't the account's.
    CurrencyMismatch {
        account: AccountId,
    },
    /// The file can't say which account it is for (a CSV): name one.
    AccountNeeded,
    /// The ledger connection is already inside a transaction.
    TransactionOpen,
    /// A record's currency or date doesn't fit the ledger's types.
    InvalidRecord {
        index: u32,
        reason: &'static str,
    },
    Storage(StorageError),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyImported { import_id } => {
                write!(f, "this file was already imported (import {})", import_id.0)
            }
            Self::Ofx(e) => write!(f, "{e}"),
            Self::Csv(e) => write!(f, "{e}"),
            Self::SeveralStatements => {
                f.write_str("the file holds several accounts; import it with automatic matching")
            }
            Self::UnknownAccount(id) => write!(f, "account {} doesn't exist", id.0),
            Self::CurrencyMismatch { account } => {
                write!(f, "the file's currency isn't account {}'s", account.0)
            }
            Self::AccountNeeded => f.write_str("choose which account this file is for"),
            Self::TransactionOpen => f.write_str("another change to the ledger is in progress"),
            Self::InvalidRecord { index, reason } => write!(f, "record {index}: {reason}"),
            Self::Storage(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<StorageError> for ImportError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

impl From<rusqlite::Error> for ImportError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(StorageError::Database(e))
    }
}

fn timestamp(now: SystemTime) -> Timestamp {
    let seconds = match now.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_secs()).map_or(i64::MIN, |s| -s),
    };
    Timestamp::from_unix_seconds(seconds)
}

fn is_ofx(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    let head = String::from_utf8_lossy(head).to_ascii_uppercase();
    head.contains("OFXHEADER")
}

/// Imports one file. See the module documentation for the steps.
pub fn import_file(
    ledger: &mut Unlocked,
    request: &ImportRequest<'_>,
) -> Result<ImportReport, ImportError> {
    let sha256 = Sha256Hash::of(request.bytes);
    if let Some(&import_id) = ledger.imports_with_sha256(sha256)?.first() {
        return Err(ImportError::AlreadyImported { import_id });
    }

    let ofx = is_ofx(request.bytes);
    let (statements, source, profile) = if ofx {
        let file = fm_import_ofx::parse(request.bytes).map_err(ImportError::Ofx)?;
        let qfx = file
            .statements
            .iter()
            .any(|s| s.info.institution.intu_bid.is_some());
        let source = if qfx {
            ImportSource::Qfx
        } else {
            ImportSource::Ofx
        };
        (file.statements, source, None)
    } else {
        let profile = fm_import_csv::detect(request.bytes, request.csv_profiles.profiles())
            .map_err(ImportError::Csv)?;
        let statements = fm_import_csv::parse(request.bytes, profile, request.csv_currency.code())
            .map_err(ImportError::Csv)?;
        (statements, ImportSource::Csv, Some(profile))
    };
    // A named target fits a file whose statements are all one account (OFX files can
    // hold several statement periods of it); several accounts need Target::Auto.
    let one_account = statements
        .windows(2)
        .all(|w| w[0].info.account_number == w[1].info.account_number);
    if !one_account && request.target != Target::Auto {
        return Err(ImportError::SeveralStatements);
    }

    let now = timestamp(request.now);
    let ledger: &Unlocked = ledger;
    let savepoint = Savepoint::begin(ledger.connection())?;
    let (import_id, statements) = (|| {
        let import_id = ImportId(ledger.next_id(Table::RawImports)?);
        let mut raw = RawImport::new(import_id, source, now, request.bytes.to_vec());
        raw.file_name = request.file_name.map(str::to_owned);
        raw.mapping_profile = profile.map(|p| p.id.clone());
        // The raw import goes in before its transactions (they refer to it). With one
        // statement it records that statement's account.
        let single = statements.len() == 1;
        if !single {
            ledger.insert_raw_import(&raw)?;
        }
        let mut reports = Vec::new();
        // A named target is one account for the whole file: resolve (or create) it once.
        let mut named: Option<AccountId> = None;
        for statement in &statements {
            let (account_id, created) = match (named, &request.target) {
                (Some(id), _) => {
                    let account = ledger.account(id)?.ok_or(ImportError::UnknownAccount(id))?;
                    check_statement_currency(&account, statement)?;
                    (id, false)
                }
                (None, target) => {
                    let resolved =
                        resolve_account(ledger, target, statement, profile, request, now)?;
                    if *target != Target::Auto {
                        named = Some(resolved.0);
                    }
                    resolved
                }
            };
            if single {
                raw.account_id = Some(account_id);
                ledger.insert_raw_import(&raw)?;
            }
            let records = import_statement(
                ledger,
                import_id,
                account_id,
                statement,
                profile,
                request.csv_profiles,
                now,
            )?;
            reports.push(StatementReport {
                account_id,
                account_created: created,
                records,
            });
        }
        if statements.is_empty() {
            ledger.insert_raw_import(&raw)?;
        }
        Ok::<_, ImportError>((import_id, reports))
    })()?;
    savepoint.release()?;
    let backup = backup::after_import(ledger, request.now);
    Ok(ImportReport {
        import_id,
        statements,
        backup,
    })
}

/// The import's savepoint. Rolled back unless released, including when the import
/// returns early or panics, and when the release itself fails, so the connection is
/// never left inside it.
struct Savepoint<'c> {
    conn: &'c rusqlite::Connection,
    open: bool,
}

impl<'c> Savepoint<'c> {
    const NAME: &'static str = "import_file";

    /// Starts the savepoint. Refuses to run inside another transaction: an import is
    /// its own unit of work, and nesting would let an outer rollback undo it silently.
    fn begin(conn: &'c rusqlite::Connection) -> Result<Savepoint<'c>, ImportError> {
        if !conn.is_autocommit() {
            return Err(ImportError::TransactionOpen);
        }
        conn.execute_batch(&format!("SAVEPOINT {}", Self::NAME))?;
        Ok(Savepoint { conn, open: true })
    }

    fn release(mut self) -> Result<(), ImportError> {
        self.conn
            .execute_batch(&format!("RELEASE {}", Self::NAME))?;
        self.open = false;
        Ok(())
    }
}

impl Drop for Savepoint<'_> {
    fn drop(&mut self) {
        if self.open {
            // Nothing to report from here; the caller already has the original error.
            let _ = self
                .conn
                .execute_batch(&format!("ROLLBACK TO {0}; RELEASE {0}", Self::NAME));
        }
    }
}

/// A statement that states its currency must be in its account's.
fn check_statement_currency(
    account: &Account,
    statement: &imported::Statement,
) -> Result<(), ImportError> {
    match statement.info.currency.as_deref() {
        Some(code) if code != account.currency.code() => Err(ImportError::CurrencyMismatch {
            account: account.id,
        }),
        _ => Ok(()),
    }
}

fn last4(number: Option<&str>) -> Option<String> {
    let digits: String = number?.chars().filter(char::is_ascii_digit).collect();
    (digits.len() >= 4).then(|| digits[digits.len() - 4..].to_owned())
}

fn account_kind(kind: Option<imported::AccountKind>) -> AccountKind {
    match kind {
        Some(imported::AccountKind::Checking) => AccountKind::Checking,
        Some(imported::AccountKind::Savings | imported::AccountKind::MoneyMarket) => {
            AccountKind::Savings
        }
        Some(imported::AccountKind::CreditCard) => AccountKind::CreditCard,
        Some(imported::AccountKind::CreditLine) => AccountKind::Loan,
        Some(imported::AccountKind::Other) | None => AccountKind::Other,
    }
}

/// The account for one statement, and whether it was just created.
fn resolve_account(
    ledger: &Unlocked,
    target: &Target,
    statement: &imported::Statement,
    profile: Option<&fm_import_csv::Profile>,
    request: &ImportRequest<'_>,
    now: Timestamp,
) -> Result<(AccountId, bool), ImportError> {
    let info = &statement.info;
    let statement_currency = match info.currency.as_deref() {
        Some(code) => Some(Currency::new(code).map_err(|_| ImportError::InvalidRecord {
            index: 0,
            reason: "unknown statement currency",
        })?),
        None => None,
    };
    let institution =
        institution::name(&info.institution).or_else(|| profile.map(|p| p.institution.clone()));
    let masked = last4(info.account_number.as_deref()).or_else(|| info.card_last4.clone());
    let check_currency = |account: &Account| -> Result<(), ImportError> {
        match statement_currency {
            Some(c) if c != account.currency => Err(ImportError::CurrencyMismatch {
                account: account.id,
            }),
            _ => Ok(()),
        }
    };

    match target {
        Target::Account(id) => {
            let account = ledger
                .account(*id)?
                .ok_or(ImportError::UnknownAccount(*id))?;
            check_currency(&account)?;
            Ok((account.id, false))
        }
        Target::NewAccount {
            name,
            kind,
            currency,
        } => {
            let account = Account {
                id: AccountId(ledger.next_id(Table::Accounts)?),
                name: name.clone(),
                kind: *kind,
                currency: *currency,
                institution,
                masked_number: masked,
                external_id: info.account_number.clone(),
                own_account: true,
                created_at: now,
            };
            check_currency(&account)?;
            ledger.insert_account(&account)?;
            Ok((account.id, true))
        }
        Target::Auto => {
            let accounts = ledger.accounts()?;
            // The kind the file says, when it says: a checking account and a card at one
            // bank can share a masked number or last four digits.
            let statement_kind = info.account_kind.map(|k| account_kind(Some(k)));
            let same_kind = |a: &Account| statement_kind.is_none_or(|k| k == a.kind);
            // A full account number identifies the account on its own; a short or
            // masked one ("XXXX1234") only together with the institution and kind.
            let by_number = info.account_number.as_ref().and_then(|n| {
                let full = n.len() >= 8
                    && n.chars().all(|c| c.is_ascii_alphanumeric())
                    && !n.contains(['X', 'x']);
                accounts.iter().find(|a| {
                    a.external_id.as_deref() == Some(n.as_str())
                        && (full
                            || (institution.is_some()
                                && a.institution == institution
                                && same_kind(a)))
                })
            });
            // The last four digits, likewise, only with the bank and the kind.
            let by_last4 = || {
                let m = masked.as_ref()?;
                accounts.iter().find(|a| {
                    a.masked_number.as_ref() == Some(m)
                        && institution.is_some()
                        && a.institution == institution
                        && same_kind(a)
                })
            };
            if let Some(account) = by_number.or_else(by_last4) {
                check_currency(account)?;
                return Ok((account.id, false));
            }
            if info.account_number.is_none() && masked.is_none() {
                return Err(ImportError::AccountNeeded);
            }
            let kind = account_kind(info.account_kind);
            let currency = statement_currency.unwrap_or(request.csv_currency);
            let name = match (&institution, &masked) {
                (Some(i), Some(m)) => format!("{i} …{m}"),
                (Some(i), None) => i.clone(),
                (None, Some(m)) => format!("Account …{m}"),
                (None, None) => "Account".to_owned(),
            };
            let account = Account {
                id: AccountId(ledger.next_id(Table::Accounts)?),
                name,
                kind,
                currency,
                institution,
                masked_number: masked,
                external_id: info.account_number.clone(),
                own_account: true,
                created_at: now,
            };
            ledger.insert_account(&account)?;
            Ok((account.id, true))
        }
    }
}

fn date(d: imported::Date, index: u32) -> Result<Date, ImportError> {
    Date::from_ymd(i32::from(d.year()), d.month(), d.day()).map_err(|_| {
        ImportError::InvalidRecord {
            index,
            reason: "date out of range",
        }
    })
}

/// Turns one record into the transaction it describes (with `id` as its id).
fn transaction(
    record: &imported::Record,
    id: TransactionId,
    account: AccountId,
    import_id: ImportId,
) -> Result<Transaction, ImportError> {
    let index = record.source_index;
    let invalid = |reason| ImportError::InvalidRecord { index, reason };
    let currency = Currency::new(&record.currency).map_err(|_| invalid("unknown currency"))?;
    let original_amount = match (record.original_amount_minor, &record.original_currency) {
        (Some(minor), Some(code)) => Some(Money::new(
            minor,
            Currency::new(code).map_err(|_| invalid("unknown original currency"))?,
        )),
        _ => None,
    };
    Ok(Transaction {
        id,
        account_id: account,
        source: SourceRef {
            import_id,
            index,
            external_id: record.external_id.clone(),
        },
        status: match record.status {
            imported::Status::Posted => TransactionStatus::Posted,
            imported::Status::Pending => TransactionStatus::Pending,
        },
        posted_date: date(record.posted_date, index)?,
        transaction_date: record
            .transaction_date
            .map(|d| date(d, index))
            .transpose()?,
        amount: Money::new(record.amount_minor, currency),
        original_amount,
        description: record.description.clone(),
        payee: record.payee.clone(),
        memo: record.memo.clone(),
        trn_type: record.trn_type.clone(),
        check_number: record.check_number.clone(),
    })
}

/// Which dates rows from a CSV layout carry, from its columns.
fn csv_dates(profile: &fm_import_csv::Profile) -> DatesGiven {
    use fm_import_csv::profile::Role;
    let has = |f: fn(&Role) -> bool| profile.csv.columns.iter().any(|c| f(&c.role));
    match (
        has(|r| matches!(r, Role::PostedDate)),
        has(|r| matches!(r, Role::TransactionDate)),
    ) {
        (true, true) => DatesGiven::Both,
        (true, false) => DatesGiven::Posted,
        (false, true) => DatesGiven::Purchase,
        (false, false) => DatesGiven::Unknown,
    }
}

/// Which dates an OFX row carries: `DTUSER` is optional per transaction.
fn ofx_dates(t: &Transaction) -> DatesGiven {
    if t.transaction_date.is_some() {
        DatesGiven::Both
    } else {
        DatesGiven::Posted
    }
}

#[allow(clippy::too_many_arguments)]
fn import_statement(
    ledger: &Unlocked,
    import_id: ImportId,
    account_id: AccountId,
    statement: &imported::Statement,
    profile: Option<&fm_import_csv::Profile>,
    profiles: &fm_import_csv::ProfileSet,
    now: Timestamp,
) -> Result<Vec<(u32, RecordOutcome)>, ImportError> {
    let account = ledger
        .account(account_id)?
        .ok_or(ImportError::UnknownAccount(account_id))?;
    let taxonomy_version = ledger.taxonomy()?.version;
    // Rows already written by this import (another statement of the same file, on the
    // same account) can match only by bank id or by settling; see matcher.rs. Each
    // existing row carries the dates its own source gives.
    let origins = ledger.import_origins()?;
    let existing: Vec<(Transaction, DatesGiven)> = ledger
        .transactions_for_account(account_id)?
        .into_iter()
        .map(|t| {
            let given = match origins.get(&t.source.import_id) {
                Some((ImportSource::Ofx | ImportSource::Qfx, _)) => ofx_dates(&t),
                Some((ImportSource::Csv, Some(id))) => profiles
                    .profiles()
                    .iter()
                    .find(|p| &p.id == id)
                    .map_or(DatesGiven::Unknown, csv_dates),
                _ => DatesGiven::Unknown,
            };
            (t, given)
        })
        .collect();
    let mut next_tx = ledger.next_id(Table::Transactions)?;
    let mut next_decision = ledger.next_id(Table::Decisions)?;

    let mut incoming = Vec::with_capacity(statement.records.len());
    for record in &statement.records {
        let t = transaction(record, TransactionId(0), account_id, import_id)?;
        if t.amount.currency() != account.currency {
            return Err(ImportError::CurrencyMismatch {
                account: account_id,
            });
        }
        incoming.push(t);
    }
    let candidates: Vec<Candidate> = incoming
        .iter()
        .map(|t| Candidate::of(t, profile.map_or_else(|| ofx_dates(t), csv_dates)))
        .collect();
    let classified = Matcher::new(existing, import_id).classify_all(&candidates);

    // Inserts and updates first, so a pending row can point at the transaction its
    // posted version (later in the same statement) became.
    let mut written: Vec<Option<RecordOutcome>> = Vec::with_capacity(incoming.len());
    let indexes: Vec<u32> = incoming.iter().map(|t| t.source.index).collect();
    // Posted rows that settle an earlier pending row of this statement, kept until that
    // row is written.
    let mut settling: Vec<Option<Transaction>> = incoming
        .iter()
        .zip(&classified)
        .map(|(t, o)| matches!(o, Outcome::SettlesRow(_)).then(|| t.clone()))
        .collect();
    for (record, outcome) in incoming.into_iter().zip(&classified) {
        let outcome = match *outcome {
            Outcome::Duplicate(id) => Some(RecordOutcome::Duplicate(id)),
            Outcome::AlreadyPosted(id) => Some(RecordOutcome::PendingAlreadyPosted(id)),
            Outcome::Posts(id) => {
                ledger.post_pending(&Transaction { id, ..record })?;
                Some(RecordOutcome::PostedPending(id))
            }
            Outcome::PendingOf(_) | Outcome::SettlesRow(_) => None,
            Outcome::New => {
                let id = TransactionId(next_tx);
                ledger.insert_transaction(&Transaction { id, ..record })?;
                ledger.insert_decision(&Decision {
                    id: DecisionId(next_decision),
                    transaction_id: id,
                    assignment: Assignment::Bucket(Bucket::NeedsReview),
                    source: DecisionSource::Builtin {
                        logic: BuiltinLogic::ImportPipeline,
                        version: PIPELINE_VERSION,
                    },
                    taxonomy_version: taxonomy_version.clone(),
                    decided_at: now,
                    state: DecisionState::Applied,
                    seen_at: None,
                    replaces: None,
                })?;
                next_tx += 1;
                next_decision += 1;
                Some(RecordOutcome::Inserted(id))
            }
        };
        written.push(outcome);
    }
    // Then the rows paired with an earlier row of this statement (written above).
    let mut outcomes = Vec::with_capacity(written.len());
    for i in 0..written.len() {
        let earlier = |j: usize| -> TransactionId {
            written[j]
                .expect("pairs with an earlier, written row")
                .transaction()
        };
        let outcome = match (written[i], classified[i]) {
            (Some(o), _) => o,
            (None, Outcome::PendingOf(j)) => RecordOutcome::PendingAlreadyPosted(earlier(j)),
            (None, Outcome::SettlesRow(j)) => {
                let id = earlier(j);
                let posted = settling[i].take().expect("a settling row is kept");
                ledger.post_pending(&Transaction { id, ..posted })?;
                RecordOutcome::PostedPending(id)
            }
            (None, _) => unreachable!("only rows paired within the statement are unwritten"),
        };
        outcomes.push((indexes[i], outcome));
    }
    Ok(outcomes)
}
