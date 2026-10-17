//! Checks on the corpus. The core test is a small reference reader that parses every
//! file using only its entry in `layouts.json` and must reproduce the expected output
//! exactly: proof that the files, the expected output and the layout spec agree, and
//! that the spec is enough to write an importer from.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use fm_bank_exports::expected::{ExpectedFile, ExpectedTx};
use fm_bank_exports::{Manifest, render_from_committed, source};
use fm_core::model::{Date, Money, TransactionStatus};
use serde_json::Value;

type Files = BTreeMap<String, Vec<u8>>;

fn files() -> Files {
    render_from_committed()
}

fn manifest(files: &Files) -> Manifest {
    serde_json::from_slice(&files["manifest.json"]).unwrap()
}

fn layouts(files: &Files) -> BTreeMap<String, Value> {
    let v: Vec<Value> = serde_json::from_slice(&files["layouts.json"]).unwrap();
    v.into_iter()
        .map(|l| (l["id"].as_str().unwrap().to_string(), l))
        .collect()
}

fn expected(files: &Files, path: &str) -> ExpectedFile {
    serde_json::from_slice(&files[path]).unwrap()
}

// ---- Reference reader ----

fn parse_date(s: &str, format: &str) -> Date {
    let s = s.trim();
    let (y, m, d): (i32, u8, u8) = match format {
        "YYYY-MM-DD" | "YYYYMMDD" => {
            let digits: String = s.chars().filter(char::is_ascii_digit).take(8).collect();
            (
                digits[0..4].parse().unwrap(),
                digits[4..6].parse().unwrap(),
                digits[6..8].parse().unwrap(),
            )
        }
        _ => {
            let parts: Vec<&str> = s.split('/').collect();
            let y: i32 = parts[2].parse().unwrap();
            (
                if y < 100 { 2000 + y } else { y },
                parts[0].parse().unwrap(),
                parts[1].parse().unwrap(),
            )
        }
    };
    Date::from_ymd(y, m, d).unwrap()
}

fn parse_amount(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let negative = s.starts_with('-') || s.starts_with('(');
    let digits: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let (whole, frac) = digits.split_once('.').unwrap_or((&digits, "00"));
    let frac = format!("{frac:0<2}");
    let minor = whole.parse::<i64>().unwrap_or(0) * 100 + frac[..2].parse::<i64>().unwrap();
    Some(if negative { -minor } else { minor })
}

fn usd(minor: i64) -> Money {
    source::money(minor)
}

/// RFC 4180 records.
fn csv_records(text: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' {
            quoted = true;
        } else if c == delimiter {
            record.push(std::mem::take(&mut field));
        } else if c == '\r' {
        } else if c == '\n' {
            record.push(std::mem::take(&mut field));
            records.push(std::mem::take(&mut record));
        } else {
            field.push(c);
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

fn read_csv(bytes: &[u8], spec: &Value) -> Vec<ExpectedTx> {
    let mut text = std::str::from_utf8(bytes).unwrap();
    if spec["utf8_bom"].as_bool().unwrap() {
        text = text.strip_prefix('\u{feff}').expect("BOM present");
    }
    let eol = if spec["line_ending"] == "crlf" {
        "\r\n"
    } else {
        "\n"
    };
    assert!(text.ends_with(eol), "line ending");
    if spec["line_ending"] == "lf" {
        assert!(!text.contains('\r'));
    }
    let skip = spec["skip_lines"].as_u64().unwrap() as usize;
    let body: String = text.split_inclusive('\n').skip(skip).collect();
    let delimiter = spec["delimiter"].as_str().unwrap().chars().next().unwrap();
    let mut records = csv_records(&body, delimiter);
    let columns = spec["columns"].as_array().unwrap();
    let trailing = spec["trailing_delimiter"].as_str().unwrap();
    let trim = spec["trim"].as_bool().unwrap();
    if spec["has_header"].as_bool().unwrap() {
        let mut header = records.remove(0);
        let names: Vec<&str> = columns
            .iter()
            .map(|c| c["header"].as_str().unwrap())
            .collect();
        if trailing == "all" {
            assert_eq!(header.pop().as_deref(), Some(""));
        }
        assert_eq!(header, names, "header row");
    }
    let format = spec["date_format"].as_str().unwrap();
    let role = |name: &str| columns.iter().position(|c| c["role"] == name);
    let mut out = Vec::new();
    for mut r in records {
        if trailing != "none" {
            assert_eq!(r.pop().as_deref(), Some(""));
        }
        assert_eq!(r.len(), columns.len(), "column count");
        if trim {
            r = r.into_iter().map(|f| f.trim().to_string()).collect();
        }
        let get = |name: &str| role(name).map(|i| r[i].clone());
        let amount = if let Some(i) = role("amount") {
            let a = parse_amount(&r[i]);
            if columns[i]["money_out"] == "positive" {
                a.map(|a| -a)
            } else {
                a
            }
        } else if let (Some(d), Some(c)) = (role("debit"), role("credit")) {
            match (parse_amount(&r[d]), parse_amount(&r[c])) {
                (Some(d), _) => Some(-d.abs()),
                (None, Some(c)) => Some(c.abs()),
                (None, None) => None,
            }
        } else {
            let i = role("abs_amount").expect("an amount column");
            let k = role("indicator").expect("an indicator column");
            parse_amount(&r[i]).map(|a| {
                if r[k] == columns[k]["debit"].as_str().unwrap() {
                    -a.abs()
                } else {
                    a.abs()
                }
            })
        };
        let Some(amount) = amount else {
            assert!(
                spec["skip_rows_without_amount"].as_bool().unwrap(),
                "row without an amount"
            );
            continue;
        };
        let posted = get("posted_date").map(|s| parse_date(&s, format));
        let txn = get("transaction_date").map(|s| parse_date(&s, format));
        let nonempty = |v: Option<String>| v.filter(|s| !s.is_empty());
        let status = match role("status") {
            Some(i) if r[i] == columns[i]["pending"].as_str().unwrap() => {
                TransactionStatus::Pending
            }
            _ => TransactionStatus::Posted,
        };
        out.push(ExpectedTx {
            truth_id: fm_core::model::TransactionId(0),
            status,
            posted_date: posted.or(txn).unwrap(),
            transaction_date: txn,
            amount: usd(amount),
            description: get("description").unwrap(),
            payee: nonempty(get("payee")),
            memo: nonempty(get("memo")),
            external_id: role("reference").and_then(|i| {
                let wrap = columns[i]["wrapped_in"].as_str().unwrap();
                let v = r[i].as_str();
                let v = if wrap.is_empty() {
                    v
                } else {
                    v.strip_prefix(wrap)
                        .and_then(|v| v.strip_suffix(wrap))
                        .unwrap_or(v)
                };
                nonempty(Some(v.to_string()))
            }),
            check_number: nonempty(get("check_number")),
            balance: nonempty(get("balance"))
                .and_then(|b| parse_amount(&b))
                .map(usd),
            bank_category: nonempty(get("bank_category")),
        });
    }
    out
}

/// OFX 1.x SGML or 2.x XML: a flat scan of tags, enough for statement downloads.
fn read_ofx(bytes: &[u8]) -> (Vec<ExpectedTx>, BTreeMap<String, String>) {
    let text = std::str::from_utf8(bytes).unwrap();
    let start = text.find("<OFX>").expect("an <OFX> element");
    let header = &text[..start];
    assert!(header.contains("OFXHEADER"), "OFX header");
    let unescape = |s: &str| {
        s.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    };
    let mut account = BTreeMap::new();
    let mut txs = Vec::new();
    let mut cur: Option<BTreeMap<String, String>> = None;
    let mut rest = &text[start..];
    while let Some(open) = rest.find('<') {
        let close = rest[open..].find('>').unwrap() + open;
        let tag = &rest[open + 1..close];
        rest = &rest[close + 1..];
        let value_end = rest.find('<').unwrap_or(rest.len());
        let value = rest[..value_end].trim();
        match tag {
            "STMTTRN" => cur = Some(BTreeMap::new()),
            "/STMTTRN" => {
                let t = cur.take().unwrap();
                let date = |k: &str| t.get(k).map(|v| parse_date(v, "YYYYMMDD"));
                txs.push(ExpectedTx {
                    truth_id: fm_core::model::TransactionId(0),
                    status: TransactionStatus::Posted,
                    posted_date: date("DTPOSTED").unwrap(),
                    transaction_date: date("DTUSER"),
                    amount: usd(parse_amount(&t["TRNAMT"]).unwrap()),
                    description: t
                        .get("NAME")
                        .or(t.get("MEMO"))
                        .cloned()
                        .expect("NAME or MEMO"),
                    payee: None,
                    memo: t.get("MEMO").cloned(),
                    external_id: t.get("FITID").cloned(),
                    check_number: t.get("CHECKNUM").cloned(),
                    balance: None,
                    bank_category: None,
                });
            }
            t if t.starts_with('/') => {}
            t if !value.is_empty() => {
                let v = unescape(value);
                match &mut cur {
                    Some(m) => {
                        m.insert(t.to_string(), v);
                    }
                    None => {
                        account.insert(t.to_string(), v);
                    }
                }
            }
            _ => {}
        }
    }
    (txs, account)
}

/// Names of the fields that differ (not their values: no financial data in test output).
fn differing_fields(a: &ExpectedTx, b: &ExpectedTx) -> Vec<&'static str> {
    let checks = [
        ("status", a.status == b.status),
        ("posted_date", a.posted_date == b.posted_date),
        ("transaction_date", a.transaction_date == b.transaction_date),
        ("amount", a.amount == b.amount),
        ("description", a.description == b.description),
        ("payee", a.payee == b.payee),
        ("memo", a.memo == b.memo),
        ("external_id", a.external_id == b.external_id),
        ("check_number", a.check_number == b.check_number),
        ("balance", a.balance == b.balance),
        ("bank_category", a.bank_category == b.bank_category),
    ];
    checks
        .iter()
        .filter(|(_, same)| !same)
        .map(|(name, _)| *name)
        .collect()
}

fn without_truth_ids(mut txs: Vec<ExpectedTx>) -> Vec<ExpectedTx> {
    for t in &mut txs {
        t.truth_id = fm_core::model::TransactionId(0);
    }
    txs
}

#[test]
fn reference_reader_reproduces_every_expected_output() {
    let files = files();
    let layouts = layouts(&files);
    for entry in manifest(&files).files {
        let exp = expected(&files, &entry.expected);
        let layout = &layouts[&entry.layout];
        let bytes = &files[&entry.path];
        let parsed = if let Some(csv) = layout["format"].get("csv") {
            read_csv(bytes, csv)
        } else {
            let (txs, acct) = read_ofx(bytes);
            assert_eq!(
                acct.get("ACCTID"),
                exp.account.number.as_ref(),
                "{}",
                entry.path
            );
            assert_eq!(
                acct.get("BANKID"),
                exp.account.routing.as_ref(),
                "{}",
                entry.path
            );
            let bal = parse_amount(&acct["BALAMT"]).map(usd);
            assert!(
                bal == exp.account.ledger_balance,
                "{}: ledger balance differs",
                entry.path
            );
            let qfx = layout["format"]["ofx"]["qfx"].as_bool().unwrap();
            assert_eq!(acct.contains_key("INTU.BID"), qfx, "{}", entry.path);
            txs
        };
        assert_eq!(
            parsed.len(),
            exp.transactions.len(),
            "{}: row count",
            entry.path
        );
        for (i, (p, e)) in parsed
            .iter()
            .zip(without_truth_ids(exp.transactions.clone()))
            .enumerate()
        {
            let fields = differing_fields(p, &e);
            assert!(
                fields.is_empty(),
                "{} row {i}: {} differ",
                entry.path,
                fields.join(", ")
            );
        }
    }
}

#[test]
fn expected_rows_match_the_synthetic_ledgers() {
    let files = files();
    let sources = source::load(&source::synthetic_data_dir());
    for entry in manifest(&files).files {
        let exp = expected(&files, &entry.expected);
        let src = sources
            .iter()
            .find(|s| s.persona == exp.source.persona)
            .unwrap();
        let ledger: BTreeMap<_, _> = src.ledger.transactions.iter().map(|t| (t.id, t)).collect();
        let mut seen = BTreeSet::new();
        for e in &exp.transactions {
            assert!(
                seen.insert(e.truth_id),
                "{}: a transaction twice",
                entry.path
            );
            let t = ledger[&e.truth_id];
            assert_eq!(t.account_id, exp.source.account_id, "{}", entry.path);
            assert!(
                e.description.len() <= t.description.len()
                    && t.description.starts_with(e.description.as_str())
            );
            match e.status {
                TransactionStatus::Posted => {
                    assert!(
                        e.amount == t.amount,
                        "{}: amount differs from TD.1",
                        entry.path
                    );
                    assert!(
                        t.posted_date <= exp.as_of
                            && (exp.start..=exp.end).contains(&t.posted_date)
                    );
                }
                TransactionStatus::Pending => {
                    assert!(t.status == TransactionStatus::Pending || t.posted_date > exp.as_of);
                    assert_eq!(e.amount.is_negative(), t.amount.is_negative());
                }
            }
        }
        // Nothing posted in the window is missing.
        let posted_in_window = src
            .ledger
            .transactions
            .iter()
            .filter(|t| {
                t.account_id == exp.source.account_id
                    && t.status == TransactionStatus::Posted
                    && t.posted_date <= exp.as_of
                    && (exp.start..=exp.end).contains(&t.posted_date)
            })
            .count();
        let posted_rows = exp
            .transactions
            .iter()
            .filter(|e| e.status == TransactionStatus::Posted)
            .count();
        assert_eq!(posted_rows, posted_in_window, "{}", entry.path);
    }
}

#[test]
fn committed_corpus_matches_the_generator() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    let files = files();
    for (path, bytes) in &files {
        let on_disk = std::fs::read(dir.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(
            &on_disk == bytes,
            "data/{path} is out of date: run `cargo run --release`"
        );
    }
    assert!(
        files == render_from_committed(),
        "rendering is deterministic"
    );
}

#[test]
fn coverage_and_fake_numbers() {
    let files = files();
    let m = manifest(&files);
    let layouts = layouts(&files);
    assert!(
        m.institutions.len() >= 20,
        "{} institutions",
        m.institutions.len()
    );
    let used: BTreeSet<&str> = m.files.iter().map(|f| f.layout.as_str()).collect();
    for (id, l) in &layouts {
        assert!(used.contains(id.as_str()), "layout {id} has no file");
        assert!(!l["provenance"]["confidence"].as_str().unwrap().is_empty());
    }
    for f in &m.files {
        assert!(f.transactions > 0, "{} is empty", f.path);
        let exp = expected(&files, &f.expected);
        if let Some(n) = &exp.account.number {
            assert!(
                n.starts_with("9999"),
                "{}: account number must be fake",
                f.path
            );
        }
        if let Some(r) = &exp.account.routing {
            assert!(
                r.starts_with("999999"),
                "{}: routing number must be fake",
                f.path
            );
        }
    }
    let formats: BTreeSet<&str> = m.files.iter().map(|f| f.format.as_str()).collect();
    assert_eq!(formats, BTreeSet::from(["csv", "ofx", "qfx"]));
}

#[test]
fn overlapping_exports_and_pending_then_posted() {
    let files = files();
    let m = manifest(&files);
    let mut by_account: BTreeMap<(String, i64), Vec<ExpectedFile>> = BTreeMap::new();
    for f in &m.files {
        let e = expected(&files, &f.expected);
        by_account
            .entry((e.source.persona.clone(), e.source.account_id.0))
            .or_default()
            .push(e);
    }
    let mut overlaps = 0;
    let mut pending_then_posted = BTreeSet::new();
    for exports in by_account.values() {
        for (i, a) in exports.iter().enumerate() {
            for b in &exports[i + 1..] {
                let ids = |e: &ExpectedFile, s: TransactionStatus| -> BTreeSet<_> {
                    e.transactions
                        .iter()
                        .filter(|t| t.status == s)
                        .map(|t| t.truth_id)
                        .collect()
                };
                if !ids(a, TransactionStatus::Posted)
                    .is_disjoint(&ids(b, TransactionStatus::Posted))
                {
                    overlaps += 1;
                }
                let (early, late) = if a.as_of <= b.as_of { (a, b) } else { (b, a) };
                pending_then_posted.extend(
                    ids(early, TransactionStatus::Pending)
                        .intersection(&ids(late, TransactionStatus::Posted))
                        .map(|id| (early.source.persona.clone(), *id)),
                );
            }
        }
    }
    assert!(overlaps >= 3, "overlapping exports: {overlaps}");
    assert!(
        pending_then_posted.len() >= 5,
        "distinct purchases pending in one download and posted in a later one: {}",
        pending_then_posted.len()
    );
}
