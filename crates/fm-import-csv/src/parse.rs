//! Reading a file with a known profile.

use crate::csv::{RawRecord, records};
use crate::profile::{CsvSpec, Profile, Role};
use fm_import_types::{
    AccountKind, Date, Institution, Record, Statement, StatementInfo, Status, minor_exponent,
    parse_grouped_decimal_minor,
};

/// Why a file couldn't be read. Messages name the line and column but never repeat
/// cell contents: they can hold amounts and merchant names (SPEC §7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// Not UTF-8 text.
    NotText,
    /// The currency code given isn't an ISO 4217 code.
    UnknownCurrency,
    /// No header row matching the profile within the first lines.
    HeaderNotFound,
    /// No records after the header.
    Empty,
    ColumnCount {
        line: usize,
        expected: usize,
        found: usize,
    },
    BadDate {
        line: usize,
        column: String,
    },
    BadAmount {
        line: usize,
        column: String,
    },
    /// A row with no amount, in a layout where every row has one.
    MissingAmount {
        line: usize,
    },
    /// No profile fits the file.
    UnknownLayout,
    /// Several profiles fit equally well; their ids.
    Ambiguous(Vec<String>),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotText => f.write_str("the file is not UTF-8 text"),
            Self::UnknownCurrency => f.write_str("unknown currency code"),
            Self::HeaderNotFound => f.write_str("the expected header row was not found"),
            Self::Empty => f.write_str("the file has no transactions"),
            Self::ColumnCount {
                line,
                expected,
                found,
            } => {
                write!(f, "line {line}: expected {expected} columns, found {found}")
            }
            Self::BadDate { line, column } => {
                write!(f, "line {line}: unreadable date in column {column:?}")
            }
            Self::BadAmount { line, column } => {
                write!(f, "line {line}: unreadable amount in column {column:?}")
            }
            Self::MissingAmount { line } => write!(f, "line {line}: no amount"),
            Self::UnknownLayout => f.write_str("no known layout matches; map the columns by hand"),
            Self::Ambiguous(ids) => {
                write!(f, "several layouts match ({}); choose one", ids.join(", "))
            }
        }
    }
}

impl std::error::Error for ImportError {}

/// The file as text, without a byte-order mark.
pub(crate) fn text(bytes: &[u8]) -> Result<&str, ImportError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    std::str::from_utf8(bytes).map_err(|_| ImportError::NotText)
}

/// Header cells compare after trimming, ignoring case.
pub(crate) fn normalise(cell: &str) -> String {
    cell.trim().to_lowercase()
}

/// Drops the extra empty field a trailing delimiter leaves.
fn drop_trailing(fields: &mut Vec<String>, expected: usize) {
    if fields.len() == expected + 1 && fields.last().is_some_and(|f| f.trim().is_empty()) {
        fields.pop();
    }
}

/// Where the header is: the line index (0-based) it sits on, searching the first lines
/// so a summary block of a different length doesn't break the import.
pub(crate) fn find_header(text: &str, spec: &CsvSpec) -> Option<usize> {
    let want: Vec<String> = spec.columns.iter().map(|c| normalise(&c.header)).collect();
    for (i, line) in text.split('\n').take(40).enumerate() {
        let mut recs = records(line, spec.delimiter, 1);
        let Some(rec) = recs.pop() else { continue };
        let mut fields = rec.fields;
        drop_trailing(&mut fields, want.len());
        if fields.iter().map(|f| normalise(f)).eq(want.iter().cloned()) {
            return Some(i);
        }
    }
    None
}

/// Parses a date in the profile's format. Leading zeros are optional; two-digit years
/// are 20xx.
pub(crate) fn parse_date(s: &str, format: &str) -> Option<Date> {
    let s = s.trim();
    let num = |p: &str| p.parse::<u16>().ok();
    if format == "YYYY-MM-DD" {
        let mut it = s.split('-');
        let (y, m, d) = (num(it.next()?)?, num(it.next()?)?, num(it.next()?)?);
        if it.next().is_some() {
            return None;
        }
        return Date::new(y, u8::try_from(m).ok()?, u8::try_from(d).ok()?).ok();
    }
    let mut it = s.split('/');
    let (m, d, y) = (num(it.next()?)?, num(it.next()?)?, it.next()?);
    if it.next().is_some() {
        return None;
    }
    let year = match (format, y.len()) {
        ("MM/DD/YY", 2) => 2000 + num(y)?,
        (_, 4) => num(y)?,
        _ => return None,
    };
    Date::new(year, u8::try_from(m).ok()?, u8::try_from(d).ok()?).ok()
}

/// Parses an amount into minor units of a currency with `exponent` decimals.
/// `Ok(None)` for an empty cell. Accepts what bank files print around the number (a
/// sign before or after `$`, parentheses for negatives, comma grouping), then applies
/// the shared exact decimal rules (`fm_import_types::parse_grouped_decimal_minor`):
/// extra decimals only if they are zeros (`-43.9300`), no floats.
pub(crate) fn parse_amount(s: &str, exponent: u8) -> Result<Option<i64>, ()> {
    let mut s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let mut negative = false;
    if let Some(inner) = s.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        negative = true;
        s = inner.trim();
    }
    let mut take_sign = |s: &mut &str| {
        if let Some(rest) = s.strip_prefix('-') {
            negative = !negative;
            *s = rest.trim_start();
        } else if let Some(rest) = s.strip_prefix('+') {
            *s = rest.trim_start();
        }
    };
    take_sign(&mut s);
    if let Some(rest) = s.strip_prefix('$') {
        s = rest.trim_start();
        take_sign(&mut s);
    }
    let minor = parse_grouped_decimal_minor(s, exponent).map_err(|_| ())?;
    Ok(Some(if negative { -minor } else { minor }))
}

fn nonempty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// Reads `bytes` with `profile` into one statement. `currency` is the account's ISO
/// 4217 code (CSV files don't state it). Returns a list to match the OFX importer.
pub fn parse(
    bytes: &[u8],
    profile: &Profile,
    currency: &str,
) -> Result<Vec<Statement>, ImportError> {
    let spec = &profile.csv;
    let exponent = minor_exponent(currency).ok_or(ImportError::UnknownCurrency)?;
    let text = text(bytes)?;
    let first = if spec.has_header {
        find_header(text, spec).ok_or(ImportError::HeaderNotFound)? + 1
    } else {
        spec.skip_lines
    };
    let body: String = text.split_inclusive('\n').skip(first).collect();
    let raw = records(&body, spec.delimiter, first + 1);
    let cols = spec.columns.len();
    let pos = |f: fn(&Role) -> bool| spec.position(f);
    let posted_col = pos(|r| matches!(r, Role::PostedDate));
    let txn_col = pos(|r| matches!(r, Role::TransactionDate));
    let mut out = Vec::new();
    let card = profile.account_kinds.iter().all(|k| k == "credit_card")
        && !profile.account_kinds.is_empty();
    let mut last_digits: Option<String> = None;
    for RawRecord { line, mut fields } in raw {
        drop_trailing(&mut fields, cols);
        if fields.len() != cols {
            return Err(ImportError::ColumnCount {
                line,
                expected: cols,
                found: fields.len(),
            });
        }
        if spec.trim {
            for f in &mut fields {
                *f = f.trim().to_string();
            }
        }
        let Some(amount) = amount_of(spec, &fields, line, exponent)? else {
            if spec.skip_rows_without_amount {
                continue;
            }
            return Err(ImportError::MissingAmount { line });
        };
        let date = |i: usize| {
            parse_date(&fields[i], &spec.date_format).ok_or_else(|| ImportError::BadDate {
                line,
                column: spec.columns[i].header.clone(),
            })
        };
        let transaction_date = txn_col.map(date).transpose()?;
        let posted_date = match posted_col {
            Some(i) => date(i)?,
            None => transaction_date.expect("a validated profile has a date column"),
        };
        let mut record = Record {
            posted_date,
            transaction_date,
            amount_minor: amount,
            currency: currency.to_string(),
            description: String::new(),
            payee: None,
            memo: None,
            external_id: None,
            check_number: None,
            trn_type: None,
            balance_minor: None,
            original_amount_minor: None,
            original_currency: None,
            bank_category: None,
            status: Status::Posted,
            source_index: u32::try_from(out.len()).unwrap_or(u32::MAX),
        };
        for (c, v) in spec.columns.iter().zip(&fields) {
            match &c.role {
                Role::Description => record.description = v.clone(),
                Role::Payee => record.payee = nonempty(v),
                Role::Memo => record.memo = nonempty(v),
                Role::Reference { wrapped_in } => {
                    let inner = if wrapped_in.is_empty() {
                        v.as_str()
                    } else {
                        v.strip_prefix(wrapped_in.as_str())
                            .and_then(|x| x.strip_suffix(wrapped_in.as_str()))
                            .unwrap_or(v)
                    };
                    record.external_id = nonempty(inner);
                }
                Role::CheckNumber => record.check_number = nonempty(v.trim()),
                Role::Balance if !v.trim().is_empty() => {
                    record.balance_minor =
                        parse_amount(v, exponent).map_err(|()| ImportError::BadAmount {
                            line,
                            column: c.header.clone(),
                        })?;
                }
                Role::BankCategory => record.bank_category = nonempty(v),
                Role::Status { pending, .. } if v.trim().eq_ignore_ascii_case(pending) => {
                    record.status = Status::Pending
                }
                Role::Type => record.trn_type = nonempty(v.trim()),
                Role::AccountNumber if last_digits.is_none() => last_digits = nonempty(v.trim()),
                _ => {}
            }
        }
        out.push(record);
    }
    if out.is_empty() {
        return Err(ImportError::Empty);
    }
    let account_kind = match profile.account_kinds.as_slice() {
        [k] if k == "checking" => Some(AccountKind::Checking),
        [k] if k == "savings" => Some(AccountKind::Savings),
        [k] if k == "credit_card" => Some(AccountKind::CreditCard),
        _ => None,
    };
    let info = StatementInfo {
        account_kind,
        currency: Some(currency.to_string()),
        // Card layouts print the card's last digits; bank layouts the account's.
        card_last4: if card { last_digits.clone() } else { None },
        account_number: if card { None } else { last_digits },
        institution: Institution {
            layout: Some(profile.id.clone()),
            ..Institution::default()
        },
        ..StatementInfo::default()
    };
    Ok(vec![Statement { info, records: out }])
}

/// The signed amount (negative = money out), or `None` if the row has none.
fn amount_of(
    spec: &CsvSpec,
    fields: &[String],
    line: usize,
    exponent: u8,
) -> Result<Option<i64>, ImportError> {
    let read = |i: usize| -> Result<Option<i64>, ImportError> {
        parse_amount(&fields[i], exponent).map_err(|()| ImportError::BadAmount {
            line,
            column: spec.columns[i].header.clone(),
        })
    };
    if let Some(i) = spec.position(|r| matches!(r, Role::Amount { .. })) {
        let flip =
            matches!(&spec.columns[i].role, Role::Amount { money_out } if money_out == "positive");
        return Ok(read(i)?.map(|a| if flip { -a } else { a }));
    }
    if let (Some(d), Some(c)) = (
        spec.position(|r| matches!(r, Role::Debit)),
        spec.position(|r| matches!(r, Role::Credit { .. })),
    ) {
        return Ok(match (read(d)?, read(c)?) {
            (Some(debit), _) if debit != 0 => Some(-debit.abs()),
            (_, Some(credit)) => Some(credit.abs()),
            (Some(zero), None) => Some(zero),
            (None, None) => None,
        });
    }
    let i = spec
        .position(|r| matches!(r, Role::AbsAmount))
        .expect("validated profile");
    let k = spec
        .position(|r| matches!(r, Role::Indicator { .. }))
        .expect("validated profile");
    let Role::Indicator { debit, .. } = &spec.columns[k].role else {
        unreachable!()
    };
    let out = fields[k].trim().eq_ignore_ascii_case(debit);
    Ok(read(i)?.map(|a| if out { -a.abs() } else { a.abs() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        let usd = |s| parse_amount(s, 2);
        assert_eq!(usd("-1,234.56"), Ok(Some(-123_456)));
        assert_eq!(usd("$1,234.00"), Ok(Some(123_400)));
        assert_eq!(usd("(12.34)"), Ok(Some(-1234)));
        assert_eq!(usd("-$5.00"), Ok(Some(-500)));
        assert_eq!(usd("$-5.00"), Ok(Some(-500)));
        assert_eq!(usd("88.0"), Ok(Some(8800)));
        assert_eq!(usd("-43.9300"), Ok(Some(-4393)));
        assert_eq!(usd("-0.00"), Ok(Some(0)));
        assert_eq!(usd("  7 "), Ok(Some(700)));
        assert_eq!(usd(""), Ok(None));
        assert_eq!(usd("1.234"), Err(()));
        assert_eq!(usd("1,23.45"), Err(()));
        assert_eq!(usd("abc"), Err(()));
        assert_eq!(parse_amount("1500", 0), Ok(Some(1500)));
        assert_eq!(parse_amount("1.5", 0), Err(()));
    }

    #[test]
    fn dates() {
        let d = |y, m, dd| Date::new(y, m, dd).ok();
        assert_eq!(parse_date("06/01/2025", "MM/DD/YYYY"), d(2025, 6, 1));
        assert_eq!(parse_date("6/1/2025", "MM/DD/YYYY"), d(2025, 6, 1));
        assert_eq!(parse_date("12/31/25", "MM/DD/YY"), d(2025, 12, 31));
        assert_eq!(parse_date("2025-06-01", "YYYY-MM-DD"), d(2025, 6, 1));
        assert_eq!(parse_date("02/30/2025", "MM/DD/YYYY"), None);
        assert_eq!(parse_date("12/31/25", "MM/DD/YYYY"), None);
    }

    #[test]
    fn errors_name_the_place_not_the_value() {
        let e = ImportError::BadAmount {
            line: 7,
            column: "Amount".into(),
        };
        assert_eq!(
            e.to_string(),
            "line 7: unreadable amount in column \"Amount\""
        );
    }
}
