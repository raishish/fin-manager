//! A SimpleFIN `/accounts` response (protocol v2) as import statements.
//!
//! One statement per account. The account's SimpleFIN id becomes the statement's
//! account number, which the pipeline stores as the account's `external_id`, so the
//! same account is found again on every sync. Amounts are exact decimals in the
//! account's currency, negative for money out, as everywhere in the importers.
//!
//! Dates: SimpleFIN gives Unix timestamps. They are read as UTC calendar dates; see the
//! README for what the bridge's timestamps look like and why that is the bank's date.

use serde::Deserialize;

use fm_import_types::{
    Date as ImportDate, Institution, Record, Statement, StatementInfo, Status, minor_exponent,
    parse_decimal_minor,
};

use super::{MAX_MESSAGE_CHARS, MAX_MESSAGES, SimpleFinError};
use crate::model::Date;

/// A response's statements (one per account) and the bridge's messages.
#[derive(Debug)]
pub(super) struct Parsed {
    pub statements: Vec<Statement>,
    /// Parallel to `statements`.
    pub account_names: Vec<Option<String>>,
    /// Messages from the bridge, to show to the user as they are (the bridge asks for
    /// that), e.g. a connection needing attention or a quota warning.
    pub messages: Vec<String>,
}

#[derive(Deserialize)]
struct AccountSet {
    #[serde(default)]
    errlist: Vec<ErrItem>,
    /// Deprecated in v2 but still sent by bridges.
    #[serde(default)]
    errors: Vec<String>,
    #[serde(default)]
    connections: Vec<Connection>,
    #[serde(default)]
    accounts: Vec<Account>,
}

#[derive(Deserialize)]
struct ErrItem {
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Deserialize)]
struct Connection {
    conn_id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct Account {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    conn_id: Option<String>,
    currency: String,
    balance: String,
    #[serde(rename = "available-balance", default)]
    available_balance: Option<String>,
    #[serde(rename = "balance-date")]
    balance_date: i64,
    #[serde(default)]
    transactions: Vec<Transaction>,
}

#[derive(Deserialize)]
struct Transaction {
    id: String,
    posted: i64,
    amount: String,
    description: String,
    #[serde(default)]
    transacted_at: Option<i64>,
    #[serde(default)]
    pending: bool,
    /// v1 fields, still sent by some bridges.
    #[serde(default)]
    payee: Option<String>,
    #[serde(default)]
    memo: Option<String>,
}

/// The UTC calendar date of a Unix timestamp.
pub(super) fn utc_date(secs: i64) -> Option<ImportDate> {
    let days = i32::try_from(secs.div_euclid(86_400)).ok()?;
    let (y, m, d) = Date::from_days_since_epoch(days)?.ymd();
    ImportDate::new(u16::try_from(y).ok()?, m, d).ok()
}

/// Parses one response. `window` is the requested range (Unix seconds), recorded as
/// each statement's period. Never echoes response content in errors.
pub(super) fn parse(body: &[u8], window: (i64, i64)) -> Result<Parsed, SimpleFinError> {
    let set: AccountSet = serde_json::from_slice(body).map_err(|_| SimpleFinError::Malformed)?;
    let mut messages = Vec::new();
    for text in set.errlist.iter().filter_map(err_text) {
        push_message(&mut messages, text);
    }
    for text in set.errors.iter().filter(|e| !e.trim().is_empty()) {
        push_message(&mut messages, text.clone());
    }

    let mut statements = Vec::new();
    let mut account_names = Vec::new();
    let mut index: u32 = 0;
    for account in set.accounts {
        let label = account.name.clone().unwrap_or_else(|| "an account".into());
        let Some(exponent) = minor_exponent(&account.currency) else {
            // A custom currency (the protocol allows a URL), e.g. points.
            push_message(
                &mut messages,
                format!("{label} uses a currency this app can't track yet, so it was skipped"),
            );
            continue;
        };
        let amount = |text: &str| parse_decimal_minor(text, exponent).ok();
        let institution = account
            .conn_id
            .as_ref()
            .and_then(|c| set.connections.iter().find(|x| &x.conn_id == c))
            .and_then(|c| c.name.clone());
        let mut records = Vec::new();
        for t in account.transactions {
            let transacted = t.transacted_at.filter(|&s| s > 0).and_then(utc_date);
            // A pending transaction may have no posted time yet.
            let posted = if t.posted > 0 {
                utc_date(t.posted)
            } else {
                transacted
            };
            let (Some(posted_date), Some(amount_minor)) = (posted, amount(&t.amount)) else {
                push_message(
                    &mut messages,
                    format!("a transaction in {label} couldn't be read and was skipped"),
                );
                continue;
            };
            records.push(Record {
                posted_date,
                transaction_date: transacted,
                amount_minor,
                currency: account.currency.clone(),
                description: t.description,
                payee: t.payee.filter(|p| !p.trim().is_empty()),
                memo: t.memo.filter(|m| !m.trim().is_empty()),
                external_id: Some(t.id),
                check_number: None,
                trn_type: None,
                balance_minor: None,
                original_amount_minor: None,
                original_currency: None,
                bank_category: None,
                status: if t.pending {
                    Status::Pending
                } else {
                    Status::Posted
                },
                source_index: index,
            });
            index = index.checked_add(1).ok_or(SimpleFinError::Malformed)?;
        }
        statements.push(Statement {
            info: StatementInfo {
                account_kind: None,
                currency: Some(account.currency.clone()),
                account_number: Some(account.id),
                routing: None,
                card_last4: None,
                ledger_balance_minor: amount(&account.balance),
                ledger_balance_date: utc_date(account.balance_date),
                available_balance_minor: account.available_balance.as_deref().and_then(amount),
                period_start: utc_date(window.0),
                period_end: utc_date(window.1),
                institution: Institution {
                    org: institution,
                    ..Institution::default()
                },
            },
            records,
        });
        account_names.push(account.name);
    }
    Ok(Parsed {
        statements,
        account_names,
        messages,
    })
}

/// Keeps at most [`MAX_MESSAGES`] distinct messages of at most [`MAX_MESSAGE_CHARS`]
/// characters each: the bridge's text is shown as it is, but not without limit.
fn push_message(messages: &mut Vec<String>, text: String) {
    let text = if text.chars().count() > MAX_MESSAGE_CHARS {
        let mut cut: String = text.chars().take(MAX_MESSAGE_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        text
    };
    if messages.len() < MAX_MESSAGES && !messages.contains(&text) {
        messages.push(text);
    }
}

fn err_text(e: &ErrItem) -> Option<String> {
    match (&e.msg, &e.code) {
        (Some(m), _) if !m.trim().is_empty() => Some(m.clone()),
        (_, Some(c)) => Some(format!("The bridge reported a problem ({c})")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const SAMPLE: &str = r#"{
      "errlist": [{"code": "con.auth", "msg": "Reconnect My Bank in SimpleFIN", "conn_id": "CON-1"}],
      "errors": ["You've used most of today's requests"],
      "connections": [{"conn_id": "CON-1", "name": "My Bank", "org_id": "o", "org_url": "https://bank.example", "sfin_url": "https://bridge.example"}],
      "accounts": [
        {"id": "ACT-1", "name": "Checking ••1234", "conn_id": "CON-1", "currency": "USD",
         "balance": "1023.45", "available-balance": "1000.00", "balance-date": 1792152000,
         "transactions": [
           {"id": "TRN-1", "posted": 1792065600, "amount": "-12.34", "description": "COFFEE SHOP", "transacted_at": 1791979200},
           {"id": "TRN-2", "posted": 0, "amount": "-5.00", "description": "PARKING", "transacted_at": 1792152000, "pending": true},
           {"id": "TRN-3", "posted": 1792065600, "amount": "2500.00", "description": "PAYROLL", "extra": {"category": "income"}}
         ]},
        {"id": "ACT-2", "name": "Rewards", "conn_id": "CON-1", "currency": "https://points.example/currency",
         "balance": "100", "balance-date": 1792152000, "transactions": []}
      ]
    }"#;

    #[test]
    fn a_response_becomes_one_statement_per_account() {
        let p = parse(SAMPLE.as_bytes(), (1_784_376_000, 1_792_152_000)).unwrap();
        assert_eq!(p.statements.len(), 1, "the points account is skipped");
        assert_eq!(p.account_names, [Some("Checking ••1234".to_string())]);
        let s = &p.statements[0];
        assert_eq!(s.info.account_number.as_deref(), Some("ACT-1"));
        assert_eq!(s.info.institution.org.as_deref(), Some("My Bank"));
        assert_eq!(s.info.ledger_balance_minor, Some(102_345));
        assert_eq!(s.info.available_balance_minor, Some(100_000));
        assert_eq!(s.records.len(), 3);
        let coffee = &s.records[0];
        assert_eq!(coffee.amount_minor, -1234);
        assert_eq!(coffee.posted_date, ImportDate::new(2026, 10, 15).unwrap());
        assert_eq!(coffee.transaction_date, ImportDate::new(2026, 10, 14).ok());
        assert_eq!(coffee.external_id.as_deref(), Some("TRN-1"));
        assert_eq!(coffee.status, Status::Posted);
        let parking = &s.records[1];
        assert_eq!(parking.status, Status::Pending);
        assert_eq!(
            parking.posted_date,
            ImportDate::new(2026, 10, 16).unwrap(),
            "no posted time yet: the transaction time"
        );
        let indexes: Vec<u32> = s.records.iter().map(|r| r.source_index).collect();
        assert_eq!(indexes, [0, 1, 2]);
        for r in &s.records {
            assert!(r.validate().is_ok());
        }
        assert_eq!(p.messages.len(), 3, "{:?}", p.messages);
        assert!(p.messages.iter().any(|m| m.contains("Reconnect My Bank")));
        assert!(p.messages.iter().any(|m| m.contains("Rewards")));
    }

    #[test]
    fn malformed_responses_are_refused_without_echoing_them() {
        for body in [
            "",
            "<html>Bad Gateway</html>",
            "{\"accounts\": [{\"id\": 5}]}",
        ] {
            let err = parse(body.as_bytes(), (0, 1)).unwrap_err();
            assert!(matches!(err, SimpleFinError::Malformed));
            assert!(!err.to_string().contains("Gateway"));
        }
        // An empty set is fine.
        assert!(parse(b"{}", (0, 1)).unwrap().statements.is_empty());
    }

    /// Review finding L6: the bridge's messages are capped in number and length.
    #[test]
    fn bridge_messages_are_capped() {
        let long = "x".repeat(10_000);
        let errlist: Vec<String> = (0..500)
            .map(|i| format!(r#"{{"code": "c", "msg": "problem {i} {long}"}}"#))
            .collect();
        let body = format!(r#"{{"errlist": [{}], "accounts": []}}"#, errlist.join(","));
        let p = parse(body.as_bytes(), (0, 1)).unwrap();
        assert_eq!(p.messages.len(), MAX_MESSAGES);
        assert!(
            p.messages
                .iter()
                .all(|m| m.chars().count() == MAX_MESSAGE_CHARS)
        );
        assert!(p.messages[0].starts_with("problem 0 ") && p.messages[0].ends_with('…'));
    }

    #[test]
    fn unreadable_transactions_are_skipped_and_reported() {
        let body = r#"{"accounts": [{"id": "A", "name": "Card", "currency": "USD", "balance": "0",
            "balance-date": 1792152000, "transactions": [
              {"id": "T1", "posted": 1792065600, "amount": "twelve", "description": "X"},
              {"id": "T2", "posted": 1792065600, "amount": "-1.00", "description": "Y"}]}]}"#;
        let p = parse(body.as_bytes(), (0, 1)).unwrap();
        assert_eq!(p.statements[0].records.len(), 1);
        assert_eq!(p.statements[0].records[0].source_index, 0);
        assert!(p.messages[0].contains("couldn't be read"));
        assert!(!p.messages[0].contains("twelve"));
    }
}
