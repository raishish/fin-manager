use serde::{Deserialize, Serialize};

use crate::Date;

/// Whether the bank reports a transaction as settled or still pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Posted,
    Pending,
}

/// One transaction as it appears in a bank file, normalised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    /// The date the bank posted it (for a pending item, the date the bank reports).
    pub posted_date: Date,
    /// The purchase date, when the file gives one separately.
    pub transaction_date: Option<Date>,
    /// Signed minor units, negative = money out of the account (cards too).
    pub amount_minor: i64,
    /// ISO 4217 code of `amount_minor`. CSV files rarely carry one, so it comes from the
    /// layout profile or the caller.
    pub currency: String,
    /// Exactly as written; only layouts that pad fields are trimmed.
    pub description: String,
    pub payee: Option<String>,
    pub memo: Option<String>,
    /// The bank's own id for the transaction (OFX FITID, a CSV reference column). Used to
    /// remove duplicates across imports.
    pub external_id: Option<String>,
    pub check_number: Option<String>,
    /// The raw transaction type when the file has one (OFX TRNTYPE such as DEBIT, XFER,
    /// PAYMENT; a CSV type column). Informational; a hint for transfer detection.
    pub trn_type: Option<String>,
    /// The running balance after this row, if the file has one.
    pub balance_minor: Option<i64>,
    /// Foreign-currency amount for purchases made abroad, with the same sign as
    /// `amount_minor`. Both `original_*` fields are set together or not at all.
    pub original_amount_minor: Option<i64>,
    pub original_currency: Option<String>,
    /// The bank's own category label, if any (never used as our category).
    pub bank_category: Option<String>,
    pub status: Status,
    /// Zero-based position of the record in the file (becomes `SourceRef.index`).
    pub source_index: u32,
}

impl Record {
    /// Checks the invariants that don't need the rest of the file: a known currency, and
    /// the `original_*` pair set together with a known currency and the same sign.
    pub fn validate(&self) -> Result<(), &'static str> {
        if crate::minor_exponent(&self.currency).is_none() {
            return Err("unknown currency");
        }
        match (self.original_amount_minor, &self.original_currency) {
            (None, None) => Ok(()),
            (Some(original), Some(code)) => {
                if crate::minor_exponent(code).is_none() {
                    Err("unknown original currency")
                } else if original.signum() != self.amount_minor.signum() {
                    Err("original amount has a different sign")
                } else {
                    Ok(())
                }
            }
            _ => Err("original amount and currency must be set together"),
        }
    }
}

/// The kind of account a statement is for, when the file says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Checking,
    Savings,
    MoneyMarket,
    CreditCard,
    /// A revolving line of credit that isn't a card (e.g. a HELOC). OFX files that send a
    /// card as a CREDITLINE bank statement are reported as `CreditCard` instead.
    CreditLine,
    Other,
}

/// Who produced the file. OFX fills `org`, `fid` and `intu_bid`; CSV fills `layout` with
/// its profile id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Institution {
    pub org: Option<String>,
    pub fid: Option<String>,
    pub intu_bid: Option<String>,
    pub layout: Option<String>,
}

/// Account-level facts that come with a statement rather than with each row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatementInfo {
    pub account_kind: Option<AccountKind>,
    /// ISO 4217 code (OFX CURDEF; for CSV, from the profile).
    pub currency: Option<String>,
    /// OFX ACCTID. May be a full account number: keep it inside the encrypted ledger only.
    pub account_number: Option<String>,
    /// OFX BANKID (routing number).
    pub routing: Option<String>,
    /// Last four digits of a card, when a CSV gives them.
    pub card_last4: Option<String>,
    pub ledger_balance_minor: Option<i64>,
    pub ledger_balance_date: Option<Date>,
    pub available_balance_minor: Option<i64>,
    /// DTSTART / DTEND. Placeholder dates (e.g. 1970-01-01) become `None`.
    pub period_start: Option<Date>,
    pub period_end: Option<Date>,
    pub institution: Institution,
}

/// One account's worth of a file. An OFX file can hold several; a CSV file holds one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Statement {
    pub info: StatementInfo,
    pub records: Vec<Record>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Record {
        Record {
            posted_date: Date::new(2026, 3, 2).unwrap(),
            transaction_date: Some(Date::new(2026, 2, 28).unwrap()),
            amount_minor: -6779,
            currency: "USD".into(),
            description: "COFFEE SHOP 123".into(),
            payee: None,
            memo: None,
            external_id: Some("FITID-1".into()),
            check_number: None,
            trn_type: Some("DEBIT".into()),
            balance_minor: Some(120_000),
            original_amount_minor: None,
            original_currency: None,
            bank_category: None,
            status: Status::Posted,
            source_index: 0,
        }
    }

    #[test]
    fn statement_round_trips_through_json() {
        let s = Statement {
            info: StatementInfo {
                account_kind: Some(AccountKind::CreditCard),
                currency: Some("USD".into()),
                period_start: Some(Date::new(2026, 2, 1).unwrap()),
                institution: Institution {
                    org: Some("BANK".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            records: vec![record()],
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"posted_date\":\"2026-03-02\""));
        assert!(json.contains("\"account_kind\":\"credit_card\""));
        assert!(json.contains("\"status\":\"posted\""));
        assert_eq!(serde_json::from_str::<Statement>(&json).unwrap(), s);
    }

    #[test]
    fn unknown_fields_are_refused() {
        let mut v = serde_json::to_value(record()).unwrap();
        v["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Record>(v).is_err());
    }

    #[test]
    fn validation() {
        assert_eq!(record().validate(), Ok(()));
        let mut r = record();
        r.currency = "XYZ".into();
        assert!(r.validate().is_err());
        let mut r = record();
        r.original_amount_minor = Some(-8000);
        assert!(r.validate().is_err(), "half the pair");
        r.original_currency = Some("EUR".into());
        assert_eq!(r.validate(), Ok(()));
        r.original_amount_minor = Some(8000);
        assert!(r.validate().is_err(), "sign mismatch");
    }
}
