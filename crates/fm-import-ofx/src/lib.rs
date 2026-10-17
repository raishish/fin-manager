//! OFX 1.x/2.x and QFX bank-export parsing.
//!
//! Reads the statement downloads US banks and card issuers offer (Quicken Web Connect
//! `.qfx`, plain `.ofx`) into normalised [`Statement`]s: one per account in the file,
//! each with its [`Record`]s in file order. Amounts are exact signed minor units,
//! negative meaning money out of the account (cards included), as OFX itself defines
//! `TRNAMT`.
//!
//! ```
//! let file = fm_import_ofx::parse(b"OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\n\n\
//!     <OFX><BANKMSGSRSV1><STMTTRNRS><STMTRS><CURDEF>USD\
//!     <BANKACCTFROM><BANKID>999999999<ACCTID>99990001<ACCTTYPE>CHECKING</BANKACCTFROM>\
//!     <BANKTRANLIST><STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20260105<TRNAMT>-4.50\
//!     <FITID>A1<NAME>COFFEE</STMTTRN></BANKTRANLIST></STMTRS></STMTTRNRS>\
//!     </BANKMSGSRSV1></OFX>").unwrap();
//! let record = &file.statements[0].records[0];
//! assert_eq!(record.amount_minor, -450);
//! assert_eq!(record.posted_date.to_string(), "2026-01-05");
//! ```
//!
//! What it handles, and the bank quirks behind each, is in `README.md`. Errors never
//! include values from the file (amounts, descriptions, account numbers): only what
//! kind of thing is wrong and where.

mod decode;
mod tree;

use std::fmt;

use tree::Element;

pub use fm_import_types::{
    AccountKind, Date, Institution, Record, Statement, StatementInfo, Status, minor_exponent,
};

/// Crate name, used by the workspace smoke test.
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Why a file couldn't be read. Never carries values from the file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// No OFX header (`OFXHEADER:` or `<?OFX ...?>`) or no `<OFX>` element.
    NotOfx,
    /// The markup is broken; the reason says how.
    Malformed(&'static str),
    /// The bank reported an error instead of a statement (OFX `STATUS/CODE`).
    StatementFailed { code: u32 },
    /// A statement has no currency (`CURDEF`) or one this build doesn't know.
    UnknownCurrency,
    /// A transaction lacks a required field.
    MissingField { field: &'static str, record: u32 },
    /// A transaction's field couldn't be read.
    InvalidField { field: &'static str, record: u32 },
    /// A transaction's amount is in a foreign currency (`STMTTRN/CURRENCY`) with no
    /// exact amount in the account's currency.
    ForeignAmount { record: u32 },
    /// The file has no bank or credit-card statement.
    NoStatements,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOfx => f.write_str("not an OFX or QFX file"),
            Self::Malformed(why) => write!(f, "malformed OFX: {why}"),
            Self::StatementFailed { code } => {
                write!(
                    f,
                    "the bank sent an error instead of a statement (code {code})"
                )
            }
            Self::UnknownCurrency => f.write_str("statement currency missing or unknown"),
            Self::MissingField { field, record } => {
                write!(f, "transaction {record} has no {field}")
            }
            Self::InvalidField { field, record } => {
                write!(f, "transaction {record} has an unreadable {field}")
            }
            Self::ForeignAmount { record } => write!(
                f,
                "transaction {record} is in a foreign currency with no amount in the account's currency"
            ),
            Self::NoStatements => f.write_str("no bank or credit-card statement in the file"),
        }
    }
}

impl std::error::Error for Error {}

/// A parsed OFX/QFX file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfxFile {
    /// The OFX version from the header: `"102"`, `"202"`, `"220"`, ...
    pub version: Option<String>,
    /// One per account statement in the file, in file order.
    pub statements: Vec<Statement>,
}

/// Parses an OFX 1.x (SGML) or 2.x (XML) file, QFX included.
pub fn parse(bytes: &[u8]) -> Result<OfxFile, Error> {
    let text = decode::decode(bytes);
    let start = find_ofx_element(&text).ok_or(Error::NotOfx)?;
    let version = header_version(&text[..start])?;
    let root = tree::parse(&text[start..])?;
    let statements = read_statements(&root)?;
    if statements.is_empty() {
        return Err(Error::NoStatements);
    }
    Ok(OfxFile {
        version,
        statements,
    })
}

/// Where `<OFX>` starts (any case, attributes or spaces allowed).
fn find_ofx_element(text: &str) -> Option<usize> {
    let upper = text.to_ascii_uppercase();
    let mut from = 0;
    while let Some(i) = upper[from..].find("<OFX") {
        let at = from + i;
        match upper.as_bytes().get(at + 4) {
            Some(b'>' | b' ' | b'\t' | b'\r' | b'\n') => return Some(at),
            _ => from = at + 4,
        }
    }
    None
}

/// The version from an SGML header (`VERSION:102`) or the XML `<?OFX ... VERSION="202"?>`
/// instruction. The header must say it is OFX.
fn header_version(header: &str) -> Result<Option<String>, Error> {
    let upper = header.to_ascii_uppercase();
    if let Some(pi) = upper.find("<?OFX") {
        let end = upper[pi..]
            .find("?>")
            .ok_or(Error::Malformed("unterminated OFX header"))?;
        let version = upper[pi..pi + end]
            .split_whitespace()
            .find_map(|a| a.strip_prefix("VERSION="))
            .map(|v| v.trim_matches(|c| c == '"' || c == '\'').to_owned());
        return Ok(version);
    }
    let mut saw_header = false;
    let mut version = None;
    for line in header.lines() {
        if let Some((key, value)) = line.trim().split_once(':') {
            match key.trim().to_ascii_uppercase().as_str() {
                "OFXHEADER" => saw_header = true,
                "VERSION" => version = Some(value.trim().to_owned()),
                _ => {}
            }
        }
    }
    if saw_header {
        Ok(version)
    } else {
        Err(Error::NotOfx)
    }
}

/// The first eight characters of an OFX date (`YYYYMMDD[HHMMSS[.XXX]][[tz]]`), as the
/// bank wrote it: the time and zone are ignored, never converted.
fn parse_date(value: &str) -> Option<Date> {
    let digits = value.get(..8)?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Date::new(
        digits[..4].parse().ok()?,
        digits[4..6].parse().ok()?,
        digits[6..8].parse().ok()?,
    )
    .ok()
}

/// Statement period dates. PNC's 1970-01-01 placeholder (and anything unreadable) is
/// treated as not given.
fn period_date(value: Option<&str>) -> Option<Date> {
    parse_date(value?).filter(|d| d.year() > 1970)
}

/// OFX amounts: a `.` or (as OFX allows) `,` decimal separator, no grouping. A missing
/// leading zero (`-.50`) is accepted.
fn parse_amount(value: &str, exponent: u8) -> Option<i64> {
    let mut text = value.replace(',', ".");
    let unsigned_at = usize::from(text.starts_with(['-', '+']));
    if text[unsigned_at..].starts_with('.') {
        text.insert(unsigned_at, '0');
    }
    fm_import_types::parse_decimal_minor(&text, exponent).ok()
}

fn account_kind(rs: &Element) -> Option<AccountKind> {
    if rs.name == "CCSTMTRS" {
        return Some(AccountKind::CreditCard);
    }
    Some(
        match rs
            .text(&["BANKACCTFROM", "ACCTTYPE"])?
            .to_ascii_uppercase()
            .as_str()
        {
            "CHECKING" => AccountKind::Checking,
            "SAVINGS" => AccountKind::Savings,
            "MONEYMRKT" => AccountKind::MoneyMarket,
            // Citi sends its cards as bank statements of type CREDITLINE.
            "CREDITLINE" => AccountKind::CreditCard,
            _ => AccountKind::Other,
        },
    )
}

fn read_statements(root: &Element) -> Result<Vec<Statement>, Error> {
    let sonrs = root.child("SIGNONMSGSRSV1").and_then(|s| s.child("SONRS"));
    let text = |path: &[&str]| sonrs.and_then(|s| s.text(path)).map(str::to_owned);
    let institution = Institution {
        org: text(&["FI", "ORG"]),
        fid: text(&["FI", "FID"]),
        intu_bid: text(&["INTU.BID"]),
        layout: None,
    };
    let mut statements = Vec::new();
    let mut next_index: u32 = 0;
    for msgset in &root.children {
        if !matches!(msgset.name.as_str(), "BANKMSGSRSV1" | "CREDITCARDMSGSRSV1") {
            continue;
        }
        for trnrs in &msgset.children {
            if !matches!(trnrs.name.as_str(), "STMTTRNRS" | "CCSTMTTRNRS") {
                continue;
            }
            let Some(rs) = trnrs.child("STMTRS").or(trnrs.child("CCSTMTRS")) else {
                let code = trnrs
                    .text(&["STATUS", "CODE"])
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0);
                if code != 0 {
                    return Err(Error::StatementFailed { code });
                }
                continue;
            };
            statements.push(read_statement(rs, &institution, &mut next_index)?);
        }
    }
    Ok(statements)
}

fn read_statement(
    rs: &Element,
    institution: &Institution,
    next_index: &mut u32,
) -> Result<Statement, Error> {
    let currency = rs
        .text(&["CURDEF"])
        .map(str::to_ascii_uppercase)
        .ok_or(Error::UnknownCurrency)?;
    let exponent = minor_exponent(&currency).ok_or(Error::UnknownCurrency)?;
    let from = rs.child("BANKACCTFROM").or(rs.child("CCACCTFROM"));
    let tranlist = rs.child("BANKTRANLIST");
    let balance = |agg: &str| -> Option<i64> {
        rs.text(&[agg, "BALAMT"])
            .and_then(|v| parse_amount(v, exponent))
    };
    let info = StatementInfo {
        account_kind: account_kind(rs),
        currency: Some(currency.clone()),
        account_number: from.and_then(|f| f.text(&["ACCTID"])).map(str::to_owned),
        routing: from.and_then(|f| f.text(&["BANKID"])).map(str::to_owned),
        card_last4: None,
        ledger_balance_minor: balance("LEDGERBAL"),
        ledger_balance_date: rs.text(&["LEDGERBAL", "DTASOF"]).and_then(parse_date),
        available_balance_minor: balance("AVAILBAL"),
        period_start: period_date(tranlist.and_then(|t| t.text(&["DTSTART"]))),
        period_end: period_date(tranlist.and_then(|t| t.text(&["DTEND"]))),
        institution: institution.clone(),
    };

    let mut records = Vec::new();
    for trn in tranlist
        .into_iter()
        .flat_map(|t| t.children_named("STMTTRN"))
    {
        let index = *next_index;
        *next_index = next_index
            .checked_add(1)
            .ok_or(Error::Malformed("too many transactions"))?;
        records.push(read_record(trn, index, &currency, exponent)?);
    }
    Ok(Statement { info, records })
}

fn read_record(trn: &Element, index: u32, currency: &str, exponent: u8) -> Result<Record, Error> {
    let missing = |field| Error::MissingField {
        field,
        record: index,
    };
    let invalid = |field| Error::InvalidField {
        field,
        record: index,
    };
    if let Some(cur) = trn.child("CURRENCY") {
        // TRNAMT is in a foreign currency; converting at CURRATE would mean rounding.
        if cur
            .text(&["CURSYM"])
            .is_some_and(|c| !c.eq_ignore_ascii_case(currency))
        {
            return Err(Error::ForeignAmount { record: index });
        }
    }
    let posted = trn.text(&["DTPOSTED"]).ok_or(missing("DTPOSTED"))?;
    let amount = trn.text(&["TRNAMT"]).ok_or(missing("TRNAMT"))?;
    let transaction_date = match trn.text(&["DTUSER"]) {
        Some(v) => Some(parse_date(v).ok_or(invalid("DTUSER"))?),
        None => None,
    };
    let owned = |path: &[&str]| trn.text(path).map(str::to_owned);
    let name = owned(&["NAME"]).or(owned(&["PAYEE", "NAME"]));
    let memo = owned(&["MEMO"]);
    Ok(Record {
        source_index: index,
        status: Status::Posted,
        posted_date: parse_date(posted).ok_or(invalid("DTPOSTED"))?,
        transaction_date,
        amount_minor: parse_amount(amount, exponent).ok_or(invalid("TRNAMT"))?,
        currency: currency.to_owned(),
        // ORIGCURRENCY gives a rate, not the original amount: left out rather than
        // computed with rounding.
        original_amount_minor: None,
        original_currency: None,
        description: name.or(memo.clone()).unwrap_or_default(),
        payee: None,
        memo,
        external_id: owned(&["FITID"]),
        check_number: owned(&["CHECKNUM"]),
        balance_minor: None,
        bank_category: None,
        trn_type: owned(&["TRNTYPE"]).map(|t| t.to_ascii_uppercase()),
    })
}

#[cfg(test)]
mod tests;
