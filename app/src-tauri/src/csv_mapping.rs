//! The column-mapping screen for CSV layouts no profile knows (T3.5; SPEC first run:
//! "unknown CSV layouts get a column-mapping screen saved per bank").
//!
//! Three stateless commands. Each gets the file's bytes as the raw IPC body and its
//! arguments as the `fm-mapping` header (percent-encoded JSON), like `import_file`;
//! nothing about the file is kept between calls, so locking has nothing to clear.
//!
//! 1. [`inspect`](commands::csv_mapping_inspect): whether a layout already fits, the
//!    columns and a few rows, and a first guess at the mapping (`fm_import_csv::suggest`).
//! 2. [`preview`](commands::csv_mapping_preview): the rows a mapping reads from the file.
//! 3. [`save`](commands::csv_mapping_save): the mapping as a layout of the user's, stored
//!    in the encrypted ledger (`Unlocked::save_csv_profiles`), so the next file from that
//!    bank is recognised without mapping.
//!
//! The UI sends a [`Mapping`]: what each column holds, the date format and the sign of
//! a purchase. The rest of the layout (quoting, padding, amount format) is filled in
//! here, and `Profile::validate` checks the result. Errors are fixed sentences that may
//! name a column's header or a line number, never a cell's value (SPEC §7).

use fm_core::model::{Currency, Money};
use fm_core::storage::Unlocked;
use fm_import_csv::profile::{AmountFormat, Column, CsvSpec, Pad, Role};
use fm_import_csv::{ImportError, Profile, ProfileSet};
use serde::{Deserialize, Serialize};

use crate::importing::{MAX_IMPORT_BYTES, percent_decode};
use crate::ledger_view::{MoneyView, Result, UiError, locked, ui_error};
use crate::unlock::AppUnlock;

/// The header carrying each command's arguments.
pub const ARGS_HEADER: &str = "fm-mapping";

/// The notice shown when the saved layouts no longer load.
pub const UNREADABLE_LAYOUTS: &str = "Saved CSV layouts couldn't be read; using the built-in ones.";

/// How many rows the screen shows: as sample cells, and as parsed rows in the preview.
const SAMPLE_ROWS: usize = 10;
const PREVIEW_ROWS: usize = 20;
/// How far down the file a header row is looked for.
const HEADER_SEARCH_LINES: usize = 40;
/// The longest bank name a layout is saved under.
const MAX_BANK_NAME: usize = 60;
/// The date formats the CSV reader knows (`fm_import_csv`'s `DATE_FORMATS`).
const DATE_FORMATS: [&str; 4] = ["MM/DD/YYYY", "M/D/YYYY", "MM/DD/YY", "YYYY-MM-DD"];

/// The layouts to read CSV files with: the built-in ones, then the user's saved ones.
/// T3.6's `import_file` reads with these, so a file mapped once is recognised after.
#[allow(dead_code, reason = "import_file reads with it from T3.6")]
pub(crate) fn user_profiles(ledger: &Unlocked) -> ProfileSet {
    load_user_profiles(ledger).0
}

/// [`user_profiles`], and the notice to show if the saved ones couldn't be loaded (they
/// are then left out, and the built-in layouts still work).
pub(crate) fn load_user_profiles(ledger: &Unlocked) -> (ProfileSet, Option<&'static str>) {
    profiles_from(ledger.csv_profiles().ok().ok_or(()))
}

/// The layouts from what the ledger holds: `Err` when it couldn't be read.
fn profiles_from(
    saved: std::result::Result<Option<String>, ()>,
) -> (ProfileSet, Option<&'static str>) {
    let mut set = ProfileSet::with_builtin();
    match saved {
        Ok(None) => (set, None),
        Ok(Some(json)) if set.load_user_profiles(&json).is_ok() => (set, None),
        _ => (ProfileSet::with_builtin(), Some(UNREADABLE_LAYOUTS)),
    }
}

/// What one column holds, as the screen offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColumnRole {
    PostedDate,
    TransactionDate,
    Description,
    Payee,
    Memo,
    /// One signed amount; [`Mapping::money_out`] says which sign a purchase has.
    Amount,
    /// Money out, in its own column.
    Debit,
    /// Money in, in its own column.
    Credit,
    /// An unsigned amount whose direction is in the indicator column.
    AbsAmount,
    Indicator,
    Balance,
    CheckNumber,
    Reference,
    BankCategory,
    Ignore,
}

/// The values an indicator column uses for money out and money in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndicatorValues {
    pub debit: String,
    pub credit: String,
}

/// The user's mapping of a file's columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mapping {
    /// One per column of the header row, in order.
    pub columns: Vec<ColumnRole>,
    /// One of [`DATE_FORMATS`].
    pub date_format: String,
    /// The sign of a purchase in an `amount` column: `negative` or `positive`.
    pub money_out: String,
    pub indicator: Option<IndicatorValues>,
}

/// Inspecting takes no details yet; the header is `{}`.
#[derive(Debug, Clone, Deserialize)]
pub struct InspectArgs {}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewArgs {
    pub mapping: Mapping,
    /// The currency to read amounts in (CSV files don't say); USD when not given.
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveArgs {
    pub mapping: Mapping,
    /// Who the layout is for, e.g. "Lakeline Credit Union".
    pub bank_name: String,
    /// `checking`, `savings` or `credit_card`: the accounts this layout is used for.
    #[serde(default)]
    pub account_kinds: Vec<String>,
}

/// What the screen starts from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectView {
    /// A layout already reads this file: import it instead of mapping it.
    pub known: Option<String>,
    pub headers: Vec<String>,
    /// The first rows, cell by cell, to show next to the headers.
    pub sample_rows: Vec<Vec<String>>,
    /// A first guess, for the user to check.
    pub suggestion: Mapping,
    pub date_formats: Vec<&'static str>,
    /// [`UNREADABLE_LAYOUTS`] when the saved layouts couldn't be loaded.
    pub notice: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRow {
    pub posted_date: String,
    pub transaction_date: Option<String>,
    pub description: String,
    pub payee: Option<String>,
    pub amount: MoneyView,
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewView {
    pub rows: Vec<PreviewRow>,
    pub row_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedView {
    /// The new layout's id.
    pub id: String,
    pub institution: String,
}

/// The file's lines as cells, read the way the CSV reader reads them: the delimiter
/// that occurs most in the first lines, fields in double quotes may hold it.
struct Table {
    delimiter: char,
    /// The line the header is on (0-based), and its cells.
    header_line: usize,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    /// The data rows carry one more, empty, field than the header (Chase).
    trailing_delimiter: bool,
}

fn split(line: &str, delimiter: char) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.trim_end_matches(['\r', '\n']).chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == delimiter && !quoted => fields.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    fields.push(field);
    fields
}

fn table(bytes: &[u8]) -> Result<Table> {
    let text = std::str::from_utf8(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes))
        .map_err(|_| ui_error("NotText", "the file isn't a text (CSV) file"))?;
    let lines: Vec<&str> = text.lines().collect();
    let delimiter = [',', ';', '\t', '|']
        .into_iter()
        .max_by_key(|d| {
            lines
                .iter()
                .take(HEADER_SEARCH_LINES)
                .map(|l| l.matches(*d).count())
                .sum::<usize>()
        })
        .unwrap_or(',');
    // The header is the first line with as many fields as most of the lines after it.
    let widths: Vec<usize> = lines.iter().map(|l| split(l, delimiter).len()).collect();
    let header_line = (0..lines.len().min(HEADER_SEARCH_LINES))
        .find(|&i| {
            !lines[i].trim().is_empty()
                && widths[i] > 1
                && widths[i + 1..]
                    .iter()
                    .take(5)
                    .all(|&w| w == widths[i] || w == widths[i] + 1)
        })
        .ok_or_else(|| ui_error("NoColumns", "no header row with several columns was found"))?;
    let mut header = split(lines[header_line], delimiter);
    let mut rows: Vec<Vec<String>> = lines[header_line + 1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| split(l, delimiter))
        .collect();
    if header.last().is_some_and(|h| h.trim().is_empty()) {
        header.pop();
    }
    let trailing_delimiter = !rows.is_empty()
        && rows
            .iter()
            .all(|r| r.len() == header.len() + 1 && r.last().is_some_and(|c| c.trim().is_empty()));
    if trailing_delimiter {
        for r in &mut rows {
            r.pop();
        }
    }
    Ok(Table {
        delimiter,
        header_line,
        header: header.into_iter().map(|h| h.trim().to_owned()).collect(),
        rows,
        trailing_delimiter,
    })
}

fn checked_size(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() {
        return Err(ui_error("Empty", "the file is empty"));
    }
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(ui_error(
            "TooLarge",
            "the file is over 50 MB; bank exports are much smaller",
        ));
    }
    Ok(())
}

fn column_role(role: &Role) -> ColumnRole {
    match role {
        Role::PostedDate => ColumnRole::PostedDate,
        Role::TransactionDate => ColumnRole::TransactionDate,
        Role::Description => ColumnRole::Description,
        Role::Payee => ColumnRole::Payee,
        Role::Memo => ColumnRole::Memo,
        Role::Amount { .. } => ColumnRole::Amount,
        Role::Debit => ColumnRole::Debit,
        Role::Credit { .. } => ColumnRole::Credit,
        Role::AbsAmount => ColumnRole::AbsAmount,
        Role::Indicator { .. } => ColumnRole::Indicator,
        Role::Balance => ColumnRole::Balance,
        Role::CheckNumber => ColumnRole::CheckNumber,
        Role::Reference { .. } => ColumnRole::Reference,
        Role::BankCategory => ColumnRole::BankCategory,
        _ => ColumnRole::Ignore,
    }
}

/// The guess for a file: `suggest`'s mapping if it read the header, otherwise every
/// column ignored, with the first date format that reads the file's dates.
fn suggestion(bytes: &[u8], table: &Table) -> Mapping {
    if let Some(p) = fm_import_csv::suggest(bytes)
        && p.csv.columns.len() == table.header.len()
    {
        let money_out = p
            .csv
            .columns
            .iter()
            .find_map(|c| match &c.role {
                Role::Amount { money_out } => Some(money_out.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "negative".into());
        return Mapping {
            columns: p.csv.columns.iter().map(|c| column_role(&c.role)).collect(),
            date_format: p.csv.date_format,
            money_out,
            indicator: None,
        };
    }
    Mapping {
        columns: vec![ColumnRole::Ignore; table.header.len()],
        date_format: DATE_FORMATS[0].to_owned(),
        money_out: "negative".into(),
        indicator: None,
    }
}

/// The layout a mapping describes for this file.
fn profile(bytes: &[u8], table: &Table, mapping: &Mapping) -> Result<Profile> {
    if mapping.columns.len() != table.header.len() {
        return Err(ui_error(
            "BadMapping",
            format!(
                "the file has {} columns but {} were mapped",
                table.header.len(),
                mapping.columns.len()
            ),
        ));
    }
    if !DATE_FORMATS.contains(&mapping.date_format.as_str()) {
        return Err(ui_error(
            "BadMapping",
            "choose one of the listed date formats",
        ));
    }
    if !["negative", "positive"].contains(&mapping.money_out.as_str()) {
        return Err(ui_error(
            "BadMapping",
            "say whether purchases are negative or positive",
        ));
    }
    let columns = table
        .header
        .iter()
        .zip(&mapping.columns)
        .map(|(header, role)| {
            let role = match role {
                ColumnRole::PostedDate => Role::PostedDate,
                ColumnRole::TransactionDate => Role::TransactionDate,
                ColumnRole::Description => Role::Description,
                ColumnRole::Payee => Role::Payee,
                ColumnRole::Memo => Role::Memo,
                ColumnRole::Amount => Role::Amount {
                    money_out: mapping.money_out.clone(),
                },
                ColumnRole::Debit => Role::Debit,
                ColumnRole::Credit => Role::Credit {
                    negative_printed: false,
                },
                ColumnRole::AbsAmount => Role::AbsAmount,
                ColumnRole::Indicator => {
                    let Some(values) = &mapping.indicator else {
                        return Err(ui_error(
                            "BadMapping",
                            "give the indicator's values for money out and money in",
                        ));
                    };
                    Role::Indicator {
                        debit: values.debit.clone(),
                        credit: values.credit.clone(),
                    }
                }
                ColumnRole::Balance => Role::Balance,
                ColumnRole::CheckNumber => Role::CheckNumber,
                ColumnRole::Reference => Role::Reference {
                    wrapped_in: String::new(),
                },
                ColumnRole::BankCategory => Role::BankCategory,
                ColumnRole::Ignore => Role::Extra {
                    what: header.clone(),
                },
            };
            Ok(Column {
                header: header.clone(),
                quoted: false,
                pad: Pad::None,
                role,
            })
        })
        .collect::<Result<Vec<Column>>>()?;
    let text = String::from_utf8_lossy(bytes);
    let profile = Profile {
        id: "mapping".into(),
        institution: String::new(),
        account_kinds: Vec::new(),
        csv: CsvSpec {
            delimiter: table.delimiter,
            quote_all: false,
            quote_header: false,
            quote_empty: false,
            line_ending: if text.contains("\r\n") { "crlf" } else { "lf" }.into(),
            utf8_bom: bytes.starts_with(b"\xEF\xBB\xBF"),
            skip_lines: table.header_line,
            has_header: true,
            columns,
            date_format: mapping.date_format.clone(),
            amount_format: AmountFormat {
                thousands_separator: true,
                currency_symbol: true,
                negative: "minus".into(),
            },
            skip_rows_without_amount: true,
            trailing_delimiter: if table.trailing_delimiter {
                "rows"
            } else {
                "none"
            }
            .into(),
            trim: true,
        },
    };
    profile.validate().map_err(|e| {
        use fm_import_csv::profile::ProfileError as E;
        let message = match e {
            E::NoDate => "map a date column",
            E::NoDescription => "map a description column",
            E::AmountColumns => {
                "map the amount: one amount column, a debit and a credit column, or an unsigned amount with an indicator"
            }
            _ => "the mapping doesn't make a usable layout",
        };
        ui_error("BadMapping", message)
    })?;
    Ok(profile)
}

/// A reading error as a fixed sentence: a line number and a column's header at most.
fn read_error(e: &ImportError) -> UiError {
    let message = match e {
        ImportError::NotText => "the file isn't a text (CSV) file".to_owned(),
        ImportError::HeaderNotFound => "the header row wasn't found".to_owned(),
        ImportError::Empty => "the file has no rows after the header".to_owned(),
        ImportError::ColumnCount { line, .. } => {
            format!("line {line} has a different number of columns")
        }
        ImportError::BadDate { line, column } => {
            format!("line {line}: \"{column}\" isn't a date in the chosen format")
        }
        ImportError::BadAmount { line, column } => {
            format!("line {line}: \"{column}\" isn't an amount")
        }
        ImportError::MissingAmount { line } => format!("line {line} has no amount"),
        _ => "the file can't be read with this mapping".to_owned(),
    };
    ui_error("CantRead", message)
}

fn currency(code: Option<&str>) -> Result<Currency> {
    Currency::new(code.unwrap_or("USD"))
        .map_err(|_| ui_error("BadCurrency", "unknown currency code"))
}

fn preview(bytes: &[u8], args: &PreviewArgs) -> Result<PreviewView> {
    checked_size(bytes)?;
    let table = table(bytes)?;
    let profile = profile(bytes, &table, &args.mapping)?;
    let currency = currency(args.currency.as_deref())?;
    let statements =
        fm_import_csv::parse(bytes, &profile, currency.code()).map_err(|e| read_error(&e))?;
    let records: Vec<_> = statements.iter().flat_map(|s| &s.records).collect();
    Ok(PreviewView {
        row_count: records.len(),
        rows: records
            .iter()
            .take(PREVIEW_ROWS)
            .map(|r| PreviewRow {
                posted_date: r.posted_date.to_string(),
                transaction_date: r.transaction_date.map(|d| d.to_string()),
                description: r.description.clone(),
                payee: r.payee.clone(),
                amount: MoneyView::from(Money::new(r.amount_minor, currency)),
                pending: r.status == fm_import_csv::Status::Pending,
            })
            .collect(),
    })
}

/// A layout id from the bank's name: `user_` and its letters and digits, made unique.
fn layout_id(bank: &str, taken: &ProfileSet) -> String {
    let slug: String = bank
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let base = format!("user_{}", if slug.is_empty() { "bank" } else { &slug });
    let used = |id: &str| taken.profiles().iter().any(|p| p.id == id);
    if !used(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}_{n}"))
        .find(|id| !used(id))
        .expect("an unused id")
}

const ACCOUNT_KINDS: [&str; 3] = ["checking", "savings", "credit_card"];

impl AppUnlock {
    pub fn csv_mapping_inspect(&self, bytes: &[u8], _args: InspectArgs) -> Result<InspectView> {
        checked_size(bytes)?;
        self.with_ledger(|open| {
            let (set, notice) = load_user_profiles(open.ledger());
            let known = fm_import_csv::detect(bytes, set.profiles())
                .ok()
                .map(|p| p.institution.clone());
            let table = table(bytes)?;
            Ok(InspectView {
                known,
                suggestion: suggestion(bytes, &table),
                sample_rows: table.rows.iter().take(SAMPLE_ROWS).cloned().collect(),
                headers: table.header,
                date_formats: DATE_FORMATS.to_vec(),
                notice,
            })
        })
        .ok_or_else(locked)?
    }

    pub fn csv_mapping_preview(&self, bytes: &[u8], args: PreviewArgs) -> Result<PreviewView> {
        self.with_ledger(|_| preview(bytes, &args))
            .ok_or_else(locked)?
    }

    pub fn csv_mapping_save(&self, bytes: &[u8], args: SaveArgs) -> Result<SavedView> {
        checked_size(bytes)?;
        let bank = args.bank_name.trim();
        if bank.is_empty() || bank.chars().count() > MAX_BANK_NAME {
            return Err(ui_error(
                "BadName",
                format!("give the bank a name of 1 to {MAX_BANK_NAME} characters"),
            ));
        }
        if bank.chars().any(char::is_control) {
            return Err(ui_error(
                "BadName",
                "the bank's name can't hold control characters",
            ));
        }
        if let Some(kind) = args
            .account_kinds
            .iter()
            .find(|k| !ACCOUNT_KINDS.contains(&k.as_str()))
        {
            return Err(ui_error(
                "BadMapping",
                format!("unknown account kind \"{kind}\""),
            ));
        }
        let table = table(bytes)?;
        let mut profile = profile(bytes, &table, &args.mapping)?;
        // The layout must read the file it was made from.
        fm_import_csv::parse(bytes, &profile, "USD").map_err(|e| read_error(&e))?;
        self.write_ledger(|open| {
            let ledger = open.ledger_mut();
            let (mut set, notice) = load_user_profiles(ledger);
            if notice.is_some() {
                // Saving now would overwrite layouts that may only be unreadable here.
                return Err(ui_error("SavedLayoutsUnreadable", UNREADABLE_LAYOUTS));
            }
            profile.id = layout_id(bank, &set);
            profile.institution = bank.to_owned();
            profile.account_kinds = args.account_kinds.clone();
            let saved = SavedView {
                id: profile.id.clone(),
                institution: profile.institution.clone(),
            };
            set.add(profile)
                .map_err(|_| ui_error("BadMapping", "the mapping doesn't make a usable layout"))?;
            ledger.save_csv_profiles(&set.user_profiles_json())?;
            Ok(saved)
        })
        .ok_or_else(locked)?
    }
}

/// The body and the `fm-mapping` header of a request.
fn request_parts<'a, T: serde::de::DeserializeOwned>(
    request: &'a tauri::ipc::Request<'_>,
) -> Result<(&'a [u8], T)> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(ui_error("BadRequest", "send the file's bytes as the body"));
    };
    let args = request
        .headers()
        .get(ARGS_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(percent_decode)
        .and_then(|json| serde_json::from_str(&json).ok())
        .ok_or_else(|| ui_error("BadRequest", "missing or invalid mapping details"))?;
    Ok((bytes, args))
}

pub mod commands {
    use tauri::State;
    use tauri::ipc::Request;

    use super::{InspectView, PreviewView, SavedView, request_parts};
    use crate::ledger_view::Result;
    use crate::unlock::AppUnlock;

    /// The body is the file's bytes; the `fm-mapping` header is `{}`.
    #[tauri::command(async)]
    pub fn csv_mapping_inspect(
        unlock: State<'_, AppUnlock>,
        request: Request<'_>,
    ) -> Result<InspectView> {
        let (bytes, args) = request_parts(&request)?;
        unlock.csv_mapping_inspect(bytes, args)
    }

    /// The body is the file's bytes; the `fm-mapping` header is `{ mapping, currency }`.
    #[tauri::command(async)]
    pub fn csv_mapping_preview(
        unlock: State<'_, AppUnlock>,
        request: Request<'_>,
    ) -> Result<PreviewView> {
        let (bytes, args) = request_parts(&request)?;
        unlock.csv_mapping_preview(bytes, args)
    }

    /// The body is the file's bytes; the `fm-mapping` header is
    /// `{ mapping, bankName, accountKinds }`.
    #[tauri::command(async)]
    pub fn csv_mapping_save(
        unlock: State<'_, AppUnlock>,
        request: Request<'_>,
    ) -> Result<SavedView> {
        let (bytes, args) = request_parts(&request)?;
        unlock.csv_mapping_save(bytes, args)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use fm_core::model::{AccountKind, Currency};
    use fm_core::pipeline::{self, ImportRequest, Target};
    use serde_json::{Value, json};
    use tauri::Manager;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, get_ipc_response};
    use tauri::webview::InvokeRequest;

    use super::*;
    use crate::tests::{Webview, create_confirmed, invoke, mock_app};

    fn exports() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/bank-exports/data")
    }

    /// `encodeURIComponent`.
    fn encode(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'_'
                | b'.'
                | b'!'
                | b'~'
                | b'*'
                | b'\''
                | b'('
                | b')' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    fn call(
        w: &Webview,
        cmd: &str,
        bytes: &[u8],
        args: Value,
    ) -> std::result::Result<Value, Value> {
        let mut headers = tauri::http::HeaderMap::new();
        headers.insert(ARGS_HEADER, encode(&args.to_string()).parse().unwrap());
        let request = InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Raw(bytes.to_vec()),
            headers,
            invoke_key: INVOKE_KEY.to_string(),
        };
        get_ipc_response(w, request).map(|body| body.deserialize().unwrap())
    }

    /// A Chase card export (TD.2) rewritten into a layout no built-in profile knows:
    /// the columns reordered and renamed, as another bank might write them.
    fn unknown_layout(file: &str) -> Vec<u8> {
        let text = std::fs::read_to_string(exports().join(file)).unwrap();
        // Chase: Transaction Date, Post Date, Description, Category, Type, Amount, Memo.
        let order = [1, 5, 2, 0, 4, 3, 6];
        let mut out = String::from("Posted Date,Amount,Details,Purchase Date,Kind,Group,Note\n");
        for line in text.lines().skip(1).filter(|l| !l.trim().is_empty()) {
            let cells = split(line, ',');
            let row: Vec<String> = order
                .iter()
                .map(|&i| {
                    let c = &cells[i];
                    if c.contains(',') || c.contains('"') {
                        format!("\"{}\"", c.replace('"', "\"\""))
                    } else {
                        c.clone()
                    }
                })
                .collect();
            out.push_str(&row.join(","));
            out.push('\n');
        }
        out.into_bytes()
    }

    fn expected(file: &str) -> Value {
        serde_json::from_slice(
            &std::fs::read(exports().join(format!("{file}.expected.json"))).unwrap(),
        )
        .unwrap()
    }

    /// Done-when: a TD.2-style file with an unknown layout imports after mapping once,
    /// and the next file of that layout imports with no mapping.
    #[test]
    fn an_unknown_layout_is_mapped_once_and_then_recognised() {
        let dir = tempfile::tempdir().unwrap();
        let (app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
        create_confirmed(&w);
        let first = unknown_layout("chase/freedom_2025_h1.csv");
        let second = unknown_layout("chase/freedom_2025_q2_q3.csv");
        let truth = expected("chase/freedom_2025_h1.csv");
        let rows = truth["transactions"].as_array().unwrap();

        // No layout knows it; the guess reads the columns' names.
        let shown = call(&w, "csv_mapping_inspect", &first, json!({})).unwrap();
        assert_eq!(shown["known"], Value::Null);
        assert_eq!(shown["notice"], Value::Null);
        assert_eq!(
            shown["headers"],
            json!([
                "Posted Date",
                "Amount",
                "Details",
                "Purchase Date",
                "Kind",
                "Group",
                "Note"
            ])
        );
        assert_eq!(shown["sampleRows"].as_array().unwrap().len(), SAMPLE_ROWS);
        let suggestion = &shown["suggestion"];
        assert_eq!(
            suggestion["columns"],
            json!([
                "postedDate",
                "amount",
                "description",
                "transactionDate",
                "ignore",
                "ignore",
                "ignore"
            ])
        );
        assert_eq!(suggestion["dateFormat"], "MM/DD/YYYY");
        assert_eq!(suggestion["moneyOut"], "negative");

        // The user keeps the guess, and maps the category too; the preview reads every
        // row as the bank meant it.
        let mut mapping = suggestion.clone();
        mapping["columns"][5] = json!("bankCategory");
        let preview = call(
            &w,
            "csv_mapping_preview",
            &first,
            json!({ "mapping": mapping, "currency": "USD" }),
        )
        .unwrap();
        assert_eq!(preview["rowCount"].as_u64().unwrap() as usize, rows.len());
        for (shown, row) in preview["rows"].as_array().unwrap().iter().zip(rows) {
            assert_eq!(shown["postedDate"], row["posted_date"]);
            assert_eq!(shown["transactionDate"], row["transaction_date"]);
            assert_eq!(shown["amount"]["minor"], row["amount"]["minor"]);
            assert_eq!(shown["description"], row["description"]);
        }

        // Saved, the layout is the user's, and this file is now recognised.
        let saved = call(
            &w,
            "csv_mapping_save",
            &first,
            json!({ "mapping": mapping, "bankName": "Example Card Co", "accountKinds": ["credit_card"] }),
        )
        .unwrap();
        assert_eq!(
            saved,
            json!({ "id": "user_example_card_co", "institution": "Example Card Co" })
        );
        let again = call(&w, "csv_mapping_inspect", &second, json!({})).unwrap();
        assert_eq!(again["known"], "Example Card Co");

        // Import both files with the layouts import_file reads with: the first mapped
        // once, the second with no mapping at all. Built-in layouts alone can't read it.
        let unlock = app.state::<AppUnlock>();
        unlock
            .write_ledger(|open| {
                let ledger = open.ledger_mut();
                let builtin = ProfileSet::with_builtin();
                let profiles = user_profiles(ledger);
                let target = Target::NewAccount {
                    name: "Example card".into(),
                    kind: AccountKind::CreditCard,
                    currency: Currency::USD,
                };
                let report =
                    pipeline::import_file(ledger, &import_request(&first, target, &profiles))
                        .unwrap();
                let account = report.statements[0].account_id;
                assert_eq!(report.statements[0].inserted(), rows.len());
                assert!(matches!(
                    pipeline::import_file(
                        ledger,
                        &import_request(&second, Target::Account(account), &builtin),
                    ),
                    Err(pipeline::ImportError::Csv(_))
                ));
                let report = pipeline::import_file(
                    ledger,
                    &import_request(&second, Target::Account(account), &profiles),
                )
                .unwrap();
                let s = &report.statements[0];
                let second_rows = expected("chase/freedom_2025_q2_q3.csv")["transactions"]
                    .as_array()
                    .unwrap()
                    .len();
                assert_eq!(s.records.len(), second_rows);
                assert!(s.duplicates() > 0, "the two exports overlap");
                let raw = ledger.raw_imports().unwrap();
                assert_eq!(
                    raw.last().unwrap().mapping_profile.as_deref(),
                    Some("user_example_card_co")
                );
            })
            .unwrap();
    }

    fn import_request<'a>(
        bytes: &'a [u8],
        target: Target,
        set: &'a ProfileSet,
    ) -> ImportRequest<'a> {
        ImportRequest {
            bytes,
            file_name: Some("mapped.csv"),
            target,
            csv_currency: Currency::USD,
            csv_profiles: set,
            now: std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000),
        }
    }

    #[test]
    fn mistakes_get_fixed_messages_without_the_files_values() {
        let dir = tempfile::tempdir().unwrap();
        let (_app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
        create_confirmed(&w);
        let file = unknown_layout("chase/freedom_2025_h1.csv");
        let suggestion =
            call(&w, "csv_mapping_inspect", &file, json!({})).unwrap()["suggestion"].clone();
        let with = |f: &dyn Fn(&mut Value)| {
            let mut m = suggestion.clone();
            f(&mut m);
            m
        };
        let cases: Vec<(Value, &str)> = vec![
            (
                with(&|m| {
                    m["columns"]
                        .as_array_mut()
                        .unwrap()
                        .pop()
                        .map(drop)
                        .unwrap()
                }),
                "BadMapping",
            ),
            // Only the purchase date: a usable layout (it is the date then).
            (with(&|m| m["columns"][0] = json!("ignore")), "Ok"),
            // The description column read as the amount: it isn't one.
            (
                with(&|m| {
                    m["columns"][1] = json!("description");
                    m["columns"][2] = json!("amount");
                }),
                "CantRead",
            ),
            (
                with(&|m| {
                    m["columns"][0] = json!("ignore");
                    m["columns"][3] = json!("ignore");
                }),
                "BadMapping",
            ),
            (with(&|m| m["columns"][1] = json!("ignore")), "BadMapping"),
            (with(&|m| m["columns"][2] = json!("ignore")), "BadMapping"),
            (
                with(&|m| m["dateFormat"] = json!("DD.MM.YYYY")),
                "BadMapping",
            ),
            (with(&|m| m["dateFormat"] = json!("YYYY-MM-DD")), "CantRead"),
            (with(&|m| m["moneyOut"] = json!("sideways")), "BadMapping"),
            (
                with(&|m| m["columns"][4] = json!("indicator")),
                "BadMapping",
            ),
        ];
        // Cell values that must never appear in a message.
        let cells: Vec<String> = file
            .split(|&b| b == b'\n')
            .skip(1)
            .take(5)
            .flat_map(|l| split(std::str::from_utf8(l).unwrap(), ','))
            .filter(|c| c.len() > 3)
            .collect();
        for (mapping, code) in cases {
            let result = call(
                &w,
                "csv_mapping_preview",
                &file,
                json!({ "mapping": mapping }),
            );
            if code == "Ok" {
                assert!(result.is_ok(), "{mapping}");
                continue;
            }
            let e = result.unwrap_err();
            assert_eq!(e["code"], code, "{mapping}");
            let message = e["message"].as_str().unwrap();
            assert!(
                cells.iter().all(|c| !message.contains(c.as_str())),
                "a cell's value in an error: {message}"
            );
        }
        // Saving needs a bank name and known account kinds; nothing is saved otherwise.
        for (args, code) in [
            (
                json!({ "mapping": suggestion, "bankName": "  " }),
                "BadName",
            ),
            (
                json!({ "mapping": suggestion, "bankName": "x".repeat(61) }),
                "BadName",
            ),
            (
                json!({ "mapping": suggestion, "bankName": "Bank\u{7}" }),
                "BadName",
            ),
            (
                json!({ "mapping": suggestion, "bankName": "Bank", "accountKinds": ["boat"] }),
                "BadMapping",
            ),
        ] {
            assert_eq!(
                call(&w, "csv_mapping_save", &file, args).unwrap_err()["code"],
                code
            );
        }
        assert_eq!(
            call(&w, "csv_mapping_inspect", &file, json!({})).unwrap()["known"],
            Value::Null
        );
        // Not a CSV, empty, or no header details.
        assert_eq!(
            call(&w, "csv_mapping_inspect", &[0xff, 0xfe, 0x00], json!({})).unwrap_err()["code"],
            "NotText"
        );
        assert_eq!(
            call(&w, "csv_mapping_inspect", b"", json!({})).unwrap_err()["code"],
            "Empty"
        );
        assert_eq!(
            call(&w, "csv_mapping_preview", &file, json!({})).unwrap_err()["code"],
            "BadRequest"
        );
        // Two layouts saved for one bank get their own ids.
        let a = call(
            &w,
            "csv_mapping_save",
            &file,
            json!({ "mapping": suggestion, "bankName": "Same Bank" }),
        )
        .unwrap();
        let mut other = suggestion.clone();
        other["columns"][4] = json!("memo");
        let b = call(
            &w,
            "csv_mapping_save",
            &file,
            json!({ "mapping": other, "bankName": "Same Bank" }),
        )
        .unwrap();
        assert_eq!(
            (a["id"].as_str(), b["id"].as_str()),
            (Some("user_same_bank"), Some("user_same_bank_2"))
        );
    }

    #[test]
    fn the_commands_need_an_unlocked_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let (_app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
        create_confirmed(&w);
        invoke(&w, "lock").unwrap();
        let file = unknown_layout("chase/freedom_2025_h1.csv");
        for cmd in [
            "csv_mapping_inspect",
            "csv_mapping_preview",
            "csv_mapping_save",
        ] {
            let e = call(&w, cmd, &file, json!({})).unwrap_err();
            assert_eq!(e, json!(crate::unlock::LOCKED), "{cmd}");
        }
    }

    #[test]
    fn unreadable_saved_layouts_fall_back_to_the_built_in_ones_with_a_notice() {
        let builtin = ProfileSet::with_builtin().profiles().len();
        for saved in [
            Err(()),
            Ok(Some("not json".to_owned())),
            Ok(Some("[{}]".to_owned())),
        ] {
            let (set, notice) = profiles_from(saved);
            assert_eq!(set.profiles().len(), builtin);
            assert_eq!(notice, Some(UNREADABLE_LAYOUTS));
        }
        let (set, notice) = profiles_from(Ok(None));
        assert_eq!((set.profiles().len(), notice), (builtin, None));
        let (set, notice) = profiles_from(Ok(Some("[]".to_owned())));
        assert_eq!((set.profiles().len(), notice), (builtin, None));
        assert!(!UNREADABLE_LAYOUTS.contains('{'));
    }
}
