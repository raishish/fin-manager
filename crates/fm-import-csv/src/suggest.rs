//! A first guess at the mapping for a layout no profile knows, for the user to check
//! on the column-mapping screen and save (SPEC decision 34, step 3).

use crate::csv::records;
use crate::parse::{normalise, parse_amount, parse_date, text};
use crate::profile::{AmountFormat, Column, CsvSpec, DATE_FORMATS, Pad, Profile, Role};

fn role_for(name: &str, have: &[Role]) -> Role {
    let n = normalise(name);
    let has = |f: fn(&Role) -> bool| have.iter().any(f);
    let words = |ws: &[&str]| ws.iter().any(|w| n.contains(w));
    if n.contains("date") || n == "posted" || n == "cleared" {
        if words(&["trans", "purchase"]) && !has(|r| matches!(r, Role::TransactionDate)) {
            return Role::TransactionDate;
        }
        if !has(|r| matches!(r, Role::PostedDate)) {
            return Role::PostedDate;
        }
        return Role::TransactionDate;
    }
    if words(&["withdrawal", "debit"])
        && !words(&["indicator"])
        && !has(|r| matches!(r, Role::Debit))
    {
        return Role::Debit;
    }
    if words(&["deposit", "credit"])
        && !words(&["indicator", "card"])
        && !has(|r| matches!(r, Role::Credit { .. }))
    {
        return Role::Credit {
            negative_printed: false,
        };
    }
    if n.contains("balance") {
        return Role::Balance;
    }
    if n.contains("amount") && !has(|r| matches!(r, Role::Amount { .. })) {
        return Role::Amount {
            money_out: "negative".into(),
        };
    }
    if words(&["description", "details", "narrative"]) && !has(|r| matches!(r, Role::Description)) {
        return Role::Description;
    }
    if words(&["payee", "merchant", "name"]) && !has(|r| matches!(r, Role::Payee)) {
        return Role::Payee;
    }
    if n.contains("memo") {
        return Role::Memo;
    }
    if words(&["check", "cheque", "serial"]) {
        return Role::CheckNumber;
    }
    if n.contains("category") && !has(|r| matches!(r, Role::BankCategory)) {
        return Role::BankCategory;
    }
    if words(&["reference", "ref", "fitid", "transaction id"]) {
        return Role::Reference {
            wrapped_in: String::new(),
        };
    }
    if n == "status" {
        return Role::Status {
            pending: "Pending".into(),
            posted: "Posted".into(),
        };
    }
    if n.contains("type") {
        return Role::Type;
    }
    Role::Extra {
        what: name.trim().to_string(),
    }
}

/// Guesses a profile for a CSV with a header row. Returns `None` if no line in the
/// first 40 looks like a header (one naming a date and an amount, debit or credit).
///
/// Guesses to check: which date is the posting date; whether purchases are negative
/// (taken from the majority sign: most rows are purchases); a payee column taken as the
/// description when there is no other. The result's id is `suggested`; rename it
/// before saving.
pub fn suggest(bytes: &[u8]) -> Option<Profile> {
    let text = text(bytes).ok()?;
    let delimiter = [',', ';', '\t', '|'].into_iter().max_by_key(|d| {
        text.lines()
            .take(40)
            .map(|l| l.matches(*d).count())
            .sum::<usize>()
    })?;
    let (header_line, header) = text
        .split('\n')
        .take(40)
        .enumerate()
        .find_map(|(i, line)| {
            let fields = records(line, delimiter, 1).pop()?.fields;
            let names: Vec<String> = fields.iter().map(|f| normalise(f)).collect();
            let dated = names.iter().any(|n| n.contains("date"));
            let money = names.iter().any(|n| {
                ["amount", "debit", "credit", "withdrawal", "deposit"]
                    .iter()
                    .any(|w| n.contains(w))
            });
            (dated && money).then_some((i, fields))
        })?;
    let mut header = header;
    if header.last().is_some_and(|h| h.trim().is_empty()) {
        header.pop();
    }
    let mut roles: Vec<Role> = Vec::new();
    for name in &header {
        let r = role_for(name, &roles);
        roles.push(r);
    }
    if !roles.iter().any(|r| matches!(r, Role::Description))
        && let Some(r) = roles.iter_mut().find(|r| matches!(r, Role::Payee))
    {
        *r = Role::Description;
    }
    // Debit without credit (or the reverse) is a signed amount column.
    let debit = roles.iter().position(|r| matches!(r, Role::Debit));
    let credit = roles.iter().position(|r| matches!(r, Role::Credit { .. }));
    let lone = match (debit, credit) {
        (Some(i), None) | (None, Some(i)) => Some(i),
        _ => None,
    };
    if let Some(i) = lone {
        roles[i] = Role::Amount {
            money_out: "negative".into(),
        };
    }
    let body: String = text.split_inclusive('\n').skip(header_line + 1).collect();
    let rows: Vec<Vec<String>> = records(&body, delimiter, 1)
        .into_iter()
        .map(|r| r.fields)
        .take(200)
        .collect();
    let date_col = roles
        .iter()
        .position(|r| matches!(r, Role::PostedDate | Role::TransactionDate))?;
    let date_format = DATE_FORMATS
        .iter()
        .find(|f| {
            rows.iter()
                .filter_map(|r| r.get(date_col))
                .filter(|v| !v.trim().is_empty())
                .all(|v| parse_date(v, f).is_some())
        })?
        .to_string();
    if let Some(i) = roles.iter().position(|r| matches!(r, Role::Amount { .. })) {
        let (pos, neg) = rows
            .iter()
            .filter_map(|r| r.get(i).and_then(|v| parse_amount(v, 2).ok().flatten()))
            .fold((0, 0), |(p, n), a| {
                if a > 0 {
                    (p + 1, n)
                } else if a < 0 {
                    (p, n + 1)
                } else {
                    (p, n)
                }
            });
        if pos > neg {
            roles[i] = Role::Amount {
                money_out: "positive".into(),
            };
        }
    }
    let profile = Profile {
        id: "suggested".to_string(),
        institution: String::new(),
        account_kinds: Vec::new(),
        csv: CsvSpec {
            delimiter,
            quote_all: false,
            quote_header: false,
            quote_empty: false,
            line_ending: if text.contains("\r\n") { "crlf" } else { "lf" }.to_string(),
            utf8_bom: bytes.starts_with(b"\xEF\xBB\xBF"),
            skip_lines: header_line,
            has_header: true,
            columns: header
                .iter()
                .zip(roles)
                .map(|(h, role)| Column {
                    header: h.clone(),
                    quoted: false,
                    pad: Pad::None,
                    role,
                })
                .collect(),
            date_format,
            amount_format: AmountFormat::default(),
            skip_rows_without_amount: true,
            trailing_delimiter: "none".to_string(),
            trim: true,
        },
    };
    profile.validate().ok()?;
    Some(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guesses_a_simple_layout() {
        let csv = "Some Bank export\nTrans Date,Post Date,Details,Amount,Running Balance\n01/02/2025,01/03/2025,COFFEE,4.50,95.50\n01/04/2025,01/05/2025,BOOKS,12.00,83.50\n01/06/2025,01/06/2025,REFUND,-2.00,85.50\n";
        let p = suggest(csv.as_bytes()).unwrap();
        let roles: Vec<&Role> = p.csv.columns.iter().map(|c| &c.role).collect();
        assert_eq!(p.csv.skip_lines, 1);
        assert_eq!(roles[0], &Role::TransactionDate);
        assert_eq!(roles[1], &Role::PostedDate);
        assert_eq!(roles[2], &Role::Description);
        assert_eq!(
            roles[3],
            &Role::Amount {
                money_out: "positive".into()
            }
        );
        assert_eq!(roles[4], &Role::Balance);
        let parsed = crate::parse(csv.as_bytes(), &p, "USD").unwrap().remove(0);
        assert_eq!(parsed.records[0].amount_minor, -450);
        assert_eq!(parsed.records[2].amount_minor, 200);
    }

    #[test]
    fn no_header_no_suggestion() {
        assert!(suggest(b"just,some,text\n1,2,3\n").is_none());
    }
}
