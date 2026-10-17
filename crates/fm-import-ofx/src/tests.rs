//! Parser tests beyond the corpus: header forms, several statements in one file,
//! bank-specific oddities from `layouts.json` notes that the corpus doesn't render, and
//! errors (which must not contain file values).

use super::*;

const SGML_HEADER: &str = "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\n\
    ENCODING:USASCII\nCHARSET:1252\nCOMPRESSION:NONE\nOLDFILEUID:NONE\nNEWFILEUID:NONE\n\n";

fn bank(trns: &str) -> String {
    format!(
        "{SGML_HEADER}<OFX><SIGNONMSGSRSV1><SONRS><STATUS><CODE>0<SEVERITY>INFO</STATUS>\
         <DTSERVER>20260930<LANGUAGE>ENG<FI><ORG>TEST<FID>1</FI><INTU.BID>1</SONRS></SIGNONMSGSRSV1>\
         <BANKMSGSRSV1><STMTTRNRS><TRNUID>1<STATUS><CODE>0<SEVERITY>INFO</STATUS>\
         <STMTRS><CURDEF>USD<BANKACCTFROM><BANKID>999999001<ACCTID>99990001<ACCTTYPE>CHECKING</BANKACCTFROM>\
         <BANKTRANLIST><DTSTART>20260101<DTEND>20260930{trns}</BANKTRANLIST>\
         <LEDGERBAL><BALAMT>1234,50<DTASOF>20260930</LEDGERBAL><AVAILBAL><BALAMT>1000<DTASOF>20260930</AVAILBAL>\
         </STMTRS></STMTTRNRS></BANKMSGSRSV1></OFX>"
    )
}

fn one(trn: &str) -> Result<Record, Error> {
    let f = parse(bank(&format!("<STMTTRN>{trn}</STMTTRN>")).as_bytes())?;
    Ok(f.statements[0].records[0].clone())
}

#[test]
fn statement_fields() {
    let f = parse(bank("").as_bytes()).unwrap();
    let info = &f.statements[0].info;
    assert_eq!(info.account_kind, Some(AccountKind::Checking));
    assert_eq!(info.currency.as_deref(), Some("USD"));
    assert_eq!(info.account_number.as_deref(), Some("99990001"));
    assert_eq!(info.routing.as_deref(), Some("999999001"));
    // OFX allows a comma as the decimal separator (and no thousands separators).
    assert_eq!(info.ledger_balance_minor, Some(123_450));
    assert_eq!(info.available_balance_minor, Some(100_000));
    assert_eq!(info.ledger_balance_date, Date::new(2026, 9, 30).ok());
    assert_eq!(info.period_start, Date::new(2026, 1, 1).ok());
    assert_eq!(info.institution.org.as_deref(), Some("TEST"));
    assert!(f.statements[0].records.is_empty());
}

#[test]
fn record_fields_and_dates_as_written() {
    let r = one(
        "<TRNTYPE>check<DTPOSTED>20260105235959.000[-8:PST]<DTUSER>20260104<TRNAMT>-1,200.5\
         <FITID>F1<CHECKNUM>1042<NAME>  PADDED NAME   <MEMO>MCC=5411",
    );
    // "-1,200.5": a comma is the decimal separator, so this is not 1,200.50.
    assert!(r.is_err());
    let r = one(
        "<TRNTYPE>check<DTPOSTED>20260105235959.000[-8:PST]<DTUSER>20260104<TRNAMT>-1200.5\
         <FITID>F1<CHECKNUM>1042<NAME>  PADDED NAME   <MEMO>MCC=5411",
    )
    .unwrap();
    // 23:59:59 Pacific stays on the 5th: dates are never converted to UTC.
    assert_eq!(r.posted_date, Date::new(2026, 1, 5).unwrap());
    assert_eq!(r.transaction_date, Date::new(2026, 1, 4).ok());
    assert_eq!(r.amount_minor, -120_050);
    assert_eq!(r.description, "PADDED NAME");
    assert_eq!(r.memo.as_deref(), Some("MCC=5411"));
    assert_eq!(r.check_number.as_deref(), Some("1042"));
    assert_eq!(r.trn_type.as_deref(), Some("CHECK"));
    assert_eq!(r.status, Status::Posted);
}

#[test]
fn amount_forms() {
    for (text, minor) in [
        ("-67.79", -6779),
        ("+12", 1200),
        ("88.0", 8800), // PNC's one-decimal amounts
        ("-.50", -50),
        (".5", 50),
        ("12,34", 1234), // a comma decimal separator
        ("1500.000", 150_000),
    ] {
        let r = one(&format!("<DTPOSTED>20260105<TRNAMT>{text}")).unwrap();
        assert_eq!(r.amount_minor, minor, "{text}");
    }
    for bad in ["1.234,56", "1,234.56", "12.345", "", "-", "(5.00)", "5.00-"] {
        assert!(
            one(&format!("<DTPOSTED>20260105<TRNAMT>{bad}")).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn description_falls_back_from_name_to_payee_to_memo() {
    assert_eq!(
        one("<DTPOSTED>20260105<TRNAMT>1<NAME>N<MEMO>M")
            .unwrap()
            .description,
        "N"
    );
    // Synchrony sometimes sends an empty NAME.
    assert_eq!(
        one("<DTPOSTED>20260105<TRNAMT>1<NAME></NAME><MEMO>M")
            .unwrap()
            .description,
        "M"
    );
    assert_eq!(
        one("<DTPOSTED>20260105<TRNAMT>1<PAYEE><NAME>P<ADDR1>x</PAYEE><MEMO>M")
            .unwrap()
            .description,
        "P"
    );
    assert_eq!(
        one("<DTPOSTED>20260105<TRNAMT>1<MEMO>M")
            .unwrap()
            .description,
        "M"
    );
    assert_eq!(one("<DTPOSTED>20260105<TRNAMT>1").unwrap().description, "");
}

#[test]
fn unknown_and_proprietary_tags_are_ignored() {
    // Amex proprietary tags, Capital One's nonstandard CCACCTTO inside STMTTRN.
    let r = one(
        "<TRNTYPE>DEBIT<DTPOSTED>20260105<TRNAMT>-9.99<FITID>X<ORIGIN.ID>abc<REFNUM>X\
         <CCACCTTO><ACCTID>1</CCACCTTO><NAME>STREAMING",
    )
    .unwrap();
    assert_eq!(r.amount_minor, -999);
    assert_eq!(r.description, "STREAMING");
}

#[test]
fn several_statements_and_card_statements_in_one_file() {
    let text = format!(
        "{SGML_HEADER}<OFX><BANKMSGSRSV1>\
         <STMTTRNRS><STMTRS><CURDEF>USD<BANKACCTFROM><ACCTID>A<ACCTTYPE>SAVINGS</BANKACCTFROM>\
         <BANKTRANLIST><STMTTRN><DTPOSTED>20260101<TRNAMT>1<NAME>a</STMTTRN></BANKTRANLIST></STMTRS></STMTTRNRS>\
         <STMTTRNRS><STMTRS><CURDEF>usd<BANKACCTFROM><ACCTID>B<ACCTTYPE>MONEYMRKT</BANKACCTFROM>\
         <BANKTRANLIST><STMTTRN><DTPOSTED>20260102<TRNAMT>2<NAME>b</STMTTRN></BANKTRANLIST></STMTRS></STMTTRNRS>\
         </BANKMSGSRSV1><CREDITCARDMSGSRSV1><CCSTMTTRNRS><CCSTMTRS><CURDEF>JPY<CCACCTFROM><ACCTID>C</CCACCTFROM>\
         <BANKTRANLIST><STMTTRN><DTPOSTED>20260103<TRNAMT>-1500<NAME>c</STMTTRN></BANKTRANLIST></CCSTMTRS></CCSTMTTRNRS>\
         </CREDITCARDMSGSRSV1></OFX>"
    );
    let f = parse(text.as_bytes()).unwrap();
    let kinds: Vec<_> = f.statements.iter().map(|s| s.info.account_kind).collect();
    assert_eq!(
        kinds,
        [
            Some(AccountKind::Savings),
            Some(AccountKind::MoneyMarket),
            Some(AccountKind::CreditCard)
        ]
    );
    // Record positions count across the whole file.
    let indexes: Vec<_> = f
        .statements
        .iter()
        .flat_map(|s| &s.records)
        .map(|r| r.source_index)
        .collect();
    assert_eq!(indexes, [0, 1, 2]);
    // A zero-decimal currency.
    assert_eq!(f.statements[2].records[0].amount_minor, -1500);
    assert_eq!(f.statements[1].info.currency.as_deref(), Some("USD"));
}

#[test]
fn header_forms() {
    let body = "<OFX><BANKMSGSRSV1><STMTTRNRS><STMTRS><CURDEF>USD</STMTRS></STMTTRNRS></BANKMSGSRSV1></OFX>";
    // A blank line before the SGML header (Chase), no FILEUID lines (Ally), CRLF.
    for header in [
        "\r\nOFXHEADER:100\r\nDATA:OFXSGML\r\nVERSION:102\r\n\r\n",
        "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\n\n",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<?OFX OFXHEADER=\"200\" VERSION=\"211\" SECURITY=\"NONE\"?>\n",
        "\u{FEFF}<?xml version=\"1.0\"?><?OFX OFXHEADER=\"200\" VERSION='220'?>",
    ] {
        let f = parse(format!("{header}{body}").as_bytes())
            .unwrap_or_else(|e| panic!("{header:?}: {e}"));
        assert!(f.version.is_some(), "{header:?}");
    }
    assert_eq!(parse(body.as_bytes()), Err(Error::NotOfx), "no header");
    assert_eq!(parse(b"Date,Description,Amount\n"), Err(Error::NotOfx));
    assert_eq!(parse(b""), Err(Error::NotOfx));
    // <OFXHEADER inside text doesn't count as the <OFX> element.
    assert_eq!(parse(b"OFXHEADER:100\n<OFXX>"), Err(Error::NotOfx));
}

#[test]
fn windows_1252_text() {
    let mut bytes =
        bank("<STMTTRN><DTPOSTED>20260105<TRNAMT>-3<NAME>CAF\u{0}</STMTTRN>").into_bytes();
    let at = bytes.iter().position(|&b| b == 0).unwrap();
    bytes[at] = 0xC9; // É in Windows-1252
    let f = parse(&bytes).unwrap();
    assert_eq!(f.statements[0].records[0].description, "CAFÉ");
}

#[test]
fn errors_name_the_problem_but_never_file_values() {
    let cases: Vec<(String, Error)> = vec![
        (
            bank("<STMTTRN><TRNAMT>-12.34<NAME>SECRET SHOP</STMTTRN>"),
            Error::MissingField {
                field: "DTPOSTED",
                record: 0,
            },
        ),
        (
            bank("<STMTTRN><DTPOSTED>20260105<NAME>SECRET SHOP</STMTTRN>"),
            Error::MissingField {
                field: "TRNAMT",
                record: 0,
            },
        ),
        (
            bank("<STMTTRN><DTPOSTED>2026-01-05<TRNAMT>-12.34</STMTTRN>"),
            Error::InvalidField {
                field: "DTPOSTED",
                record: 0,
            },
        ),
        (
            bank("<STMTTRN><DTPOSTED>20260230<TRNAMT>-12.34</STMTTRN>"),
            Error::InvalidField {
                field: "DTPOSTED",
                record: 0,
            },
        ),
        (
            bank("<STMTTRN><DTPOSTED>20260105<TRNAMT>$12.34</STMTTRN>"),
            Error::InvalidField {
                field: "TRNAMT",
                record: 0,
            },
        ),
        (
            bank("<STMTTRN><DTPOSTED>20260105<TRNAMT>-12.345</STMTTRN>"),
            Error::InvalidField {
                field: "TRNAMT",
                record: 0,
            },
        ),
        (
            bank("<STMTTRN><DTPOSTED>20260105<DTUSER>soon<TRNAMT>1</STMTTRN>"),
            Error::InvalidField {
                field: "DTUSER",
                record: 0,
            },
        ),
        (
            bank(
                "<STMTTRN><DTPOSTED>20260105<TRNAMT>1</STMTTRN><STMTTRN><DTPOSTED>20260105<TRNAMT>-12.34<CURRENCY><CURRATE>1.1<CURSYM>EUR</CURRENCY></STMTTRN>",
            ),
            Error::ForeignAmount { record: 1 },
        ),
        (
            bank("").replace("<CURDEF>USD", "<CURDEF>ZZZ"),
            Error::UnknownCurrency,
        ),
        (bank("").replace("<CURDEF>USD", ""), Error::UnknownCurrency),
        (
            format!(
                "{SGML_HEADER}<OFX><BANKMSGSRSV1><STMTTRNRS><STATUS><CODE>2000<SEVERITY>ERROR</STATUS></STMTTRNRS></BANKMSGSRSV1></OFX>"
            ),
            Error::StatementFailed { code: 2000 },
        ),
        (
            format!("{SGML_HEADER}<OFX><SIGNONMSGSRSV1></SIGNONMSGSRSV1></OFX>"),
            Error::NoStatements,
        ),
        (
            format!("{SGML_HEADER}<OFX><BANKMSGSRSV1><STMTTRN"),
            Error::Malformed("unterminated tag"),
        ),
    ];
    for (text, expected) in cases {
        let err = parse(text.as_bytes()).unwrap_err();
        assert_eq!(err, expected);
        let message = err.to_string();
        for secret in [
            "SECRET",
            "12.34",
            "1234",
            "99990001",
            "999999001",
            "EUR",
            "ZZZ",
        ] {
            assert!(!message.contains(secret), "{secret} in {message:?}");
        }
    }
    // A CURRENCY aggregate in the account's own currency is harmless.
    assert!(one("<DTPOSTED>20260105<TRNAMT>-1<CURRENCY><CURRATE>1<CURSYM>USD</CURRENCY>").is_ok());
}

#[test]
fn crate_name_is_set() {
    assert_eq!(NAME, "fm-import-ofx");
}
