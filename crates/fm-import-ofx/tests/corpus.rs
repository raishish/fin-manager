//! Every OFX/QFX file in TD.2's bank-export corpus (`testdata/bank-exports`) parses and
//! matches its `<file>.expected.json`, field for field.
//!
//! Failure messages name the file, the row and the fields that differ, never their
//! values (no financial data in test output).

use std::fs;
use std::path::PathBuf;

use fm_import_ofx::{AccountKind, Record, Status, parse};
use serde_json::Value;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/bank-exports/data")
}

fn json(path: PathBuf) -> Value {
    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap()
}

fn kind_name(kind: Option<AccountKind>) -> Option<&'static str> {
    kind.map(|k| match k {
        AccountKind::Checking => "checking",
        AccountKind::Savings => "savings",
        AccountKind::MoneyMarket => "money_market",
        AccountKind::CreditCard => "credit_card",
        AccountKind::CreditLine => "credit_line",
        AccountKind::Other => "other",
    })
}

fn money(v: &Value) -> Option<(i64, String)> {
    (!v.is_null()).then(|| {
        (
            v["minor"].as_i64().unwrap(),
            v["currency"].as_str().unwrap().to_owned(),
        )
    })
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

/// The names of the fields where `record` differs from the expected row.
fn differing(record: &Record, row: &Value) -> Vec<&'static str> {
    let status = match record.status {
        Status::Posted => "posted",
        Status::Pending => "pending",
    };
    let checks = [
        ("status", row["status"] == status),
        (
            "posted_date",
            text(&row["posted_date"]) == Some(record.posted_date.to_string()),
        ),
        (
            "transaction_date",
            text(&row["transaction_date"]) == record.transaction_date.map(|d| d.to_string()),
        ),
        (
            "amount",
            money(&row["amount"]) == Some((record.amount_minor, record.currency.clone())),
        ),
        (
            "description",
            text(&row["description"]) == Some(record.description.clone()),
        ),
        ("payee", text(&row["payee"]) == record.payee),
        ("memo", text(&row["memo"]) == record.memo),
        (
            "external_id",
            text(&row["external_id"]) == record.external_id,
        ),
        (
            "check_number",
            text(&row["check_number"]) == record.check_number,
        ),
        (
            "balance",
            money(&row["balance"]).map(|m| m.0) == record.balance_minor,
        ),
        (
            "bank_category",
            text(&row["bank_category"]) == record.bank_category,
        ),
    ];
    checks
        .iter()
        .filter(|(_, same)| !same)
        .map(|(n, _)| *n)
        .collect()
}

#[test]
fn every_ofx_and_qfx_file_matches_its_expected_output() {
    let manifest = json(corpus().join("manifest.json"));
    let mut files = 0;
    let mut rows = 0;
    for entry in manifest["files"].as_array().unwrap() {
        let format = entry["format"].as_str().unwrap();
        if format == "csv" {
            continue;
        }
        let path = entry["path"].as_str().unwrap();
        let expected = json(corpus().join(entry["expected"].as_str().unwrap()));
        let bytes = fs::read(corpus().join(path)).unwrap();
        let file = parse(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(file.statements.len(), 1, "{path}: statement count");
        let statement = &file.statements[0];
        let info = &statement.info;
        let account = &expected["account"];

        assert_eq!(
            kind_name(info.account_kind),
            account["kind"].as_str(),
            "{path}: account kind"
        );
        assert_eq!(
            info.currency.as_deref(),
            account["currency"].as_str(),
            "{path}: currency"
        );
        assert!(
            info.account_number == text(&account["number"]),
            "{path}: account number"
        );
        assert!(info.routing == text(&account["routing"]), "{path}: routing");
        assert!(
            info.ledger_balance_minor == money(&account["ledger_balance"]).map(|m| m.0),
            "{path}: ledger balance"
        );
        // QFX files identify the bank to Quicken; plain OFX doesn't.
        assert_eq!(
            info.institution.intu_bid.is_some(),
            format == "qfx",
            "{path}: INTU.BID"
        );
        assert!(
            info.institution.org.is_some() && info.institution.fid.is_some(),
            "{path}: FI"
        );

        let expected_rows = expected["transactions"].as_array().unwrap();
        assert_eq!(
            statement.records.len(),
            expected_rows.len(),
            "{path}: row count"
        );
        for (i, (record, row)) in statement.records.iter().zip(expected_rows).enumerate() {
            let fields = differing(record, row);
            assert!(
                fields.is_empty(),
                "{path} row {i}: {} differ",
                fields.join(", ")
            );
            assert_eq!(
                record.source_index as usize, i,
                "{path} row {i}: source index"
            );
        }
        files += 1;
        rows += expected_rows.len();
    }
    // 19 QFX + 1 OFX in corpus version 1.
    assert_eq!(files, 20, "OFX/QFX files in the corpus");
    assert!(rows > 3_000, "{rows} rows");
}

#[test]
fn quirks_listed_in_layouts_json_are_present_and_handled() {
    let read = |path: &str| parse(&fs::read(corpus().join(path)).unwrap()).unwrap();
    // PNC's epoch DTSTART isn't taken as the period start.
    let pnc = read("pnc/card_2026.qfx");
    assert_eq!(pnc.statements[0].info.period_start, None);
    assert!(pnc.statements[0].info.period_end.is_some());
    // Citi's card arrives as a bank CREDITLINE statement.
    let citi = read("citi/card_2026.qfx");
    assert_eq!(
        citi.statements[0].info.account_kind,
        Some(AccountKind::CreditCard)
    );
    // XML versions.
    assert_eq!(
        read("capital_one/savings_2026.qfx").version.as_deref(),
        Some("202")
    );
    assert_eq!(
        read("navy_federal_credit_union/checking_2026.ofx")
            .version
            .as_deref(),
        Some("220")
    );
    assert_eq!(
        read("chase/freedom_2025.qfx").version.as_deref(),
        Some("102")
    );
    // MEMO-only text (Capital One 360) becomes the description.
    let c1 = read("capital_one/savings_2026.qfx");
    assert!(
        c1.statements[0]
            .records
            .iter()
            .all(|r| Some(&r.description) == r.memo.as_ref())
    );
    // `&amp;` is unescaped (single-line and one-tag-per-line SGML alike).
    let mut unescaped = 0;
    for path in [
        "citi/card_2026.qfx",
        "american_express/blue_cash_2026.qfx",
        "barclays/card_2026.qfx",
        "chase/freedom_2025.qfx",
    ] {
        let raw = String::from_utf8(fs::read(corpus().join(path)).unwrap()).unwrap();
        assert!(raw.contains("&amp;"), "{path} no longer has &amp;");
        let f = read(path);
        let records = &f.statements[0].records;
        assert!(
            records.iter().any(|r| r.description.contains('&')),
            "{path}"
        );
        assert!(
            records.iter().all(|r| !r.description.contains("&amp;")),
            "{path}"
        );
        unescaped += 1;
    }
    assert_eq!(unescaped, 4);
}
