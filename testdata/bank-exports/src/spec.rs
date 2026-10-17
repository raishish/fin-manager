//! Layout specs: how each institution's export is laid out, in a form a generic reader
//! can follow. They are written to `layouts.json`; the importers (T1.1, T1.2) treat them
//! as the spec, and the corpus tests parse every file with nothing but its spec.

use serde::Serialize;

use crate::source::{Kind, Row};

/// Where a layout's details come from.
#[derive(Debug, Clone, Serialize)]
pub struct Provenance {
    /// Public pages the layout was checked against. Empty when reconstructed.
    pub sources: Vec<&'static str>,
    /// `high`: matches a public sample or the bank's own documentation.
    /// `medium`: matches importer code or community docs for that bank.
    /// `low`: partly confirmed. `reconstructed`: no public source found.
    pub confidence: &'static str,
    pub notes: &'static str,
}

/// What a CSV column holds. `Amount`-like roles carry the sign rule.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Role {
    PostedDate,
    TransactionDate,
    /// The main description text (whatever the bank calls the column).
    Description,
    Payee,
    Memo,
    /// One signed amount column. `money_out` says which sign a purchase has.
    Amount {
        money_out: &'static str,
    },
    /// Money out, as a positive number (sign, if any, is ignored).
    Debit,
    /// Money in. Some banks print it as a negative number (`negative_printed`); readers
    /// should take the absolute value.
    Credit {
        negative_printed: bool,
    },
    /// The amount without a sign; `Indicator` says which way it went.
    AbsAmount,
    Indicator {
        debit: &'static str,
        credit: &'static str,
    },
    Balance,
    CheckNumber,
    /// The bank's id for the transaction. `wrapped_in` (e.g. `'`) surrounds the value and
    /// isn't part of it.
    Reference {
        wrapped_in: &'static str,
    },
    Status {
        pending: &'static str,
        posted: &'static str,
    },
    BankCategory,
    /// A second category level (informational).
    BankSubcategory,
    /// The bank's transaction type (informational).
    Type,
    /// Last digits of the card or account (informational).
    AccountNumber,
    /// A value that is the same on every row, e.g. Wells Fargo's `*` (informational).
    Fixed {
        value: &'static str,
    },
    /// Other per-row detail, e.g. a time of day or a cardholder name (informational).
    Extra {
        what: &'static str,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Column {
    /// Header text; empty when the file has no header row.
    pub header: &'static str,
    /// This column is always quoted (non-empty values; see `quote_empty`).
    pub quoted: bool,
    /// Spaces added after the value: a fixed count, or up to a width.
    pub pad: Pad,
    #[serde(flatten)]
    pub role: Role,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pad {
    None,
    Trailing(usize),
    Width(usize),
}

#[derive(Debug, Clone, Serialize)]
pub struct AmountFormat {
    /// `1,234.56` rather than `1234.56`.
    pub thousands_separator: bool,
    /// `$1,234.56`.
    pub currency_symbol: bool,
    /// `minus` (`-12.34`) or `parentheses` (`(12.34)`).
    pub negative: &'static str,
}

pub const PLAIN: AmountFormat = AmountFormat {
    thousands_separator: false,
    currency_symbol: false,
    negative: "minus",
};

#[derive(Debug, Clone, Serialize)]
pub struct CsvSpec {
    pub delimiter: char,
    /// Every data field quoted, rather than only fields that need it (and the columns
    /// marked `quoted`).
    pub quote_all: bool,
    /// Header fields quoted.
    pub quote_header: bool,
    /// Empty fields quoted too (`""` rather than nothing) where quoting applies.
    pub quote_empty: bool,
    /// `crlf` or `lf`.
    pub line_ending: &'static str,
    pub utf8_bom: bool,
    /// Lines before the header (or before the first row, with no header).
    pub skip_lines: usize,
    pub has_header: bool,
    pub columns: Vec<Column>,
    /// `MM/DD/YYYY`, `M/D/YYYY`, `MM/DD/YY` or `YYYY-MM-DD`.
    pub date_format: &'static str,
    pub amount_format: AmountFormat,
    /// Rows with no amount are summaries (an opening-balance line), not transactions.
    pub skip_rows_without_amount: bool,
    /// `none`, `rows` (data rows only: one more field than the header) or `all`.
    pub trailing_delimiter: &'static str,
    /// Readers should trim spaces around fields; expected values are trimmed.
    pub trim: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OfxSpec {
    /// `102` (1.0.2 SGML) … `220` (2.2 XML).
    pub version: &'static str,
    /// Quicken Web Connect (`.qfx`): adds `INTU.BID`.
    pub qfx: bool,
    /// SGML only: leaf elements get closing tags (`<NAME>x</NAME>`), as some banks do.
    pub close_leaf_tags: bool,
    /// One element per line, rather than the whole document on one line.
    pub line_per_element: bool,
    pub org: &'static str,
    pub fid: &'static str,
    pub intu_bid: Option<&'static str>,
    /// Suffix on date-times, e.g. `[-5:EST]`; empty for none.
    pub tz: &'static str,
    /// `NAME` is cut to this many characters (32 in OFX 1.x).
    pub name_max: usize,
    /// `overflow`: `MEMO` carries the full description when `NAME` was cut.
    /// `none`: no `MEMO`.
    pub memo: &'static str,
    /// Whether `DTUSER` (the transaction date) is included.
    pub dtuser: bool,
    /// `name`: description in `NAME`. `memo_only`: no `NAME`, the description is in
    /// `MEMO`. `name_and_memo`: both carry the description.
    pub text: &'static str,
    /// `DTPOSTED`/`DTUSER` carry only the date even when other date-times have a time.
    pub date_only_postings: bool,
    /// A blank line before the header (seen in Chase downloads).
    pub leading_blank_line: bool,
    /// The SGML header has `OLDFILEUID`/`NEWFILEUID` lines.
    pub header_fileuids: bool,
    /// A card statement sent as a bank statement with `ACCTTYPE` `CREDITLINE` (Citi).
    pub card_as_creditline: bool,
    /// `LANGUAGE` value (`ENG`, or `eng` as Barclays writes it).
    pub language: &'static str,
    /// `SEVERITY` value (`INFO`, or `Info` as Ally writes it).
    pub severity: &'static str,
    /// Literal `DTSTART`, when the bank doesn't send the requested start (PNC sends the
    /// Unix epoch).
    pub dtstart: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Csv(CsvSpec),
    Ofx(OfxSpec),
}

/// One institution's export layout for one kind of account.
#[derive(Debug, Clone, Serialize)]
pub struct Layout {
    pub id: &'static str,
    pub institution: &'static str,
    /// `checking`, `savings` or `credit_card`.
    pub account_kinds: Vec<&'static str>,
    /// File extension: `csv`, `ofx` or `qfx`.
    pub extension: &'static str,
    pub provenance: Provenance,
    pub newest_first: bool,
    /// Whether the export lists pending transactions.
    pub includes_pending: bool,
    pub format: Format,
    /// The bank's "Type" column value for a row.
    #[serde(skip)]
    pub type_of: fn(Kind, &Row) -> &'static str,
    /// The bank's own category for a row (from its truth category key, if any).
    #[serde(skip)]
    pub category_of: fn(Option<&str>, Kind) -> &'static str,
    /// Values for `Extra` columns.
    #[serde(skip)]
    pub extra_of: fn(&str, &Row, &crate::render::FileCtx) -> String,
    /// Lines before the header (summary blocks), given the rows and as-of date.
    #[serde(skip)]
    pub preamble: fn(&[Row], &crate::render::FileCtx) -> Vec<String>,
    /// Non-transaction lines after the header (an opening-balance row), written as is.
    #[serde(skip)]
    pub lead_rows: fn(&[Row], &crate::render::FileCtx) -> Vec<String>,
}

pub fn no_type(_: Kind, _: &Row) -> &'static str {
    ""
}

pub fn no_category(_: Option<&str>, _: Kind) -> &'static str {
    ""
}

pub fn no_extra(what: &str, _: &Row, _: &crate::render::FileCtx) -> String {
    panic!("no value for extra column {what}")
}

pub fn no_preamble(_: &[Row], _: &crate::render::FileCtx) -> Vec<String> {
    Vec::new()
}

pub fn no_lead_rows(_: &[Row], _: &crate::render::FileCtx) -> Vec<String> {
    Vec::new()
}

/// A column quoted only when its value needs it.
pub const fn col(header: &'static str, role: Role) -> Column {
    Column {
        header,
        quoted: false,
        pad: Pad::None,
        role,
    }
}

/// A column that is always quoted.
pub const fn qcol(header: &'static str, role: Role) -> Column {
    Column {
        header,
        quoted: true,
        pad: Pad::None,
        role,
    }
}

impl Layout {
    /// Folder name for the institution: lowercase, words joined by `_`.
    pub fn institution_key(&self) -> String {
        let mut key = String::new();
        for c in self.institution.chars() {
            if c == '.' {
                continue;
            } else if c.is_ascii_alphanumeric() {
                key.push(c.to_ascii_lowercase());
            } else if !key.ends_with('_') && !key.is_empty() {
                key.push('_');
            }
        }
        key.trim_end_matches('_').to_string()
    }
}
