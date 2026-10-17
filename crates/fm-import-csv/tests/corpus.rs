//! The done-when check for T1.1: every CSV in the bank-export corpus (TD.2) is
//! recognised with no manual mapping and read exactly as its expected output says.

use std::path::PathBuf;

use fm_import_csv::profile::Role;
use fm_import_csv::{ImportError, ProfileSet, Record, Status, import, parse, profile, suggest};
use serde_json::Value;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/bank-exports/data")
}

fn read_json(path: &str) -> Value {
    let bytes = std::fs::read(corpus_dir().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_slice(&bytes).unwrap()
}

/// (file, layout id) for every CSV in the corpus.
fn csv_files() -> Vec<(String, String, String)> {
    let manifest = read_json("manifest.json");
    manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["format"] == "csv")
        .map(|f| {
            (
                f["path"].as_str().unwrap().to_string(),
                f["expected"].as_str().unwrap().to_string(),
                f["layout"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// The fields of an expected row that a record must reproduce, in the record's terms.
fn expected_record(e: &Value, index: usize) -> Record {
    let opt = |k: &str| e[k].as_str().map(str::to_string);
    let date = |k: &str| {
        e[k].as_str()
            .map(|s| fm_import_csv::Date::parse_iso(s).unwrap())
    };
    Record {
        posted_date: date("posted_date").unwrap(),
        transaction_date: date("transaction_date"),
        amount_minor: e["amount"]["minor"].as_i64().unwrap(),
        currency: e["amount"]["currency"].as_str().unwrap().to_string(),
        description: e["description"].as_str().unwrap().to_string(),
        payee: opt("payee"),
        memo: opt("memo"),
        external_id: opt("external_id"),
        check_number: opt("check_number"),
        trn_type: None,
        balance_minor: e["balance"]["minor"].as_i64(),
        original_amount_minor: None,
        original_currency: None,
        bank_category: opt("bank_category"),
        status: if e["status"] == "pending" {
            Status::Pending
        } else {
            Status::Posted
        },
        source_index: u32::try_from(index).unwrap(),
    }
}

/// Names of the differing fields; never their values (no financial data in output).
fn differing(a: &Record, b: &Record) -> Vec<&'static str> {
    [
        ("posted_date", a.posted_date == b.posted_date),
        ("transaction_date", a.transaction_date == b.transaction_date),
        ("amount", a.amount_minor == b.amount_minor),
        ("currency", a.currency == b.currency),
        ("description", a.description == b.description),
        ("payee", a.payee == b.payee),
        ("memo", a.memo == b.memo),
        ("external_id", a.external_id == b.external_id),
        ("check_number", a.check_number == b.check_number),
        (
            "original_amount",
            a.original_amount_minor == b.original_amount_minor,
        ),
        (
            "original_currency",
            a.original_currency == b.original_currency,
        ),
        ("balance", a.balance_minor == b.balance_minor),
        ("bank_category", a.bank_category == b.bank_category),
        ("status", a.status == b.status),
        ("source_index", a.source_index == b.source_index),
    ]
    .iter()
    .filter(|(_, same)| !same)
    .map(|(n, _)| *n)
    .collect()
}

/// Imports one file with no hints; `Ok` if it matches its expected output exactly.
fn check(path: &str, expected_path: &str, layout: &str, set: &ProfileSet) -> Result<usize, String> {
    let bytes = std::fs::read(corpus_dir().join(path)).unwrap();
    let mut statements = import(&bytes, set, "USD").map_err(|e| format!("{path}: {e}"))?;
    if statements.len() != 1 {
        return Err(format!("{path}: {} statements", statements.len()));
    }
    let parsed = statements.remove(0);
    let detected = parsed.info.institution.layout.as_deref().unwrap_or("");
    if detected != layout {
        return Err(format!("{path}: detected {detected} instead of {layout}"));
    }
    if parsed.info.currency.as_deref() != Some("USD") {
        return Err(format!("{path}: statement currency not set"));
    }
    // The bank's type column, when the layout has one, lands in `trn_type`.
    let profile = set.profiles().iter().find(|p| p.id == layout).unwrap();
    let has_type = profile.csv.columns.iter().any(|c| c.role == Role::Type);
    if parsed
        .records
        .iter()
        .any(|r| r.trn_type.is_some() != has_type)
    {
        return Err(format!(
            "{path}: trn_type set {} a type column",
            if has_type { "without" } else { "despite no" }
        ));
    }
    let exp = read_json(expected_path);
    let rows = exp["transactions"].as_array().unwrap();
    if rows.len() != parsed.records.len() {
        return Err(format!(
            "{path}: {} records, expected {}",
            parsed.records.len(),
            rows.len()
        ));
    }
    for (i, (r, e)) in parsed.records.iter().zip(rows).enumerate() {
        let mut want = expected_record(e, i);
        want.trn_type = r.trn_type.clone();
        let d = differing(r, &want);
        if !d.is_empty() {
            return Err(format!("{path} record {i}: {} differ", d.join(", ")));
        }
    }
    Ok(rows.len())
}

#[test]
fn every_corpus_csv_is_recognised_and_read_exactly() {
    let set = ProfileSet::with_builtin();
    let files = csv_files();
    assert!(files.len() >= 30, "corpus has {} CSV files", files.len());
    let mut failures = Vec::new();
    let mut records = 0;
    for (path, expected, layout) in &files {
        match check(path, expected, layout, &set) {
            Ok(n) => records += n,
            Err(e) => failures.push(e),
        }
    }
    let passed = files.len() - failures.len();
    println!(
        "{passed}/{} corpus CSV files read exactly ({records} records)",
        files.len()
    );
    // Done-when for T1.1 is >= 90%; every file passes, so hold the line at 100%.
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
}

#[test]
fn builtin_profiles_are_the_corpus_spec() {
    let layouts = read_json("layouts.json");
    let spec: Vec<(String, Value)> = layouts
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| {
            l["format"]
                .get("csv")
                .map(|c| (l["id"].as_str().unwrap().to_string(), c.clone()))
        })
        .collect();
    let builtin = profile::builtin();
    assert_eq!(builtin.len(), spec.len());
    for (p, (id, csv)) in builtin.iter().zip(&spec) {
        assert_eq!(&p.id, id);
        assert_eq!(
            serde_json::to_value(&p.csv).unwrap(),
            *csv,
            "{id} differs from layouts.json"
        );
    }
}

#[test]
fn files_with_no_known_layout_are_reported_not_guessed() {
    let set = ProfileSet::with_builtin();
    let csv = "When,What,How much\n2025-01-02,COFFEE,-4.50\n";
    assert_eq!(
        import(csv.as_bytes(), &set, "USD").unwrap_err(),
        ImportError::UnknownLayout
    );
}

#[test]
fn currency_sets_the_minor_unit() {
    let set = ProfileSet::with_builtin();
    let bytes = std::fs::read(corpus_dir().join("chase/freedom_2025_h1.csv")).unwrap();
    assert_eq!(
        import(&bytes, &set, "usd").unwrap_err(),
        ImportError::UnknownCurrency
    );
    // The same file read as yen: no decimals allowed, so cents are refused.
    assert!(matches!(
        import(&bytes, &set, "JPY").unwrap_err(),
        ImportError::BadAmount { .. }
    ));
}

#[test]
fn a_saved_suggestion_reads_the_next_file_without_help() {
    let csv = "Bank of Nowhere\nTrans Date,Post Date,Details,Amount\n01/02/2025,01/03/2025,COFFEE,4.50\n01/04/2025,01/05/2025,BOOKS,12.00\n";
    let mut set = ProfileSet::with_builtin();
    assert_eq!(
        import(csv.as_bytes(), &set, "USD").unwrap_err(),
        ImportError::UnknownLayout
    );
    let mut p = suggest(csv.as_bytes()).unwrap();
    p.id = "bank_of_nowhere".into();
    set.add(p).unwrap();
    let saved = set.user_profiles_json();
    let mut later = ProfileSet::with_builtin();
    later.load_user_profiles(&saved).unwrap();
    let next =
        "Bank of Nowhere\nTrans Date,Post Date,Details,Amount\n02/02/2025,02/03/2025,TEA,3.00\n";
    let parsed = import(next.as_bytes(), &later, "USD").unwrap().remove(0);
    assert_eq!(
        parsed.info.institution.layout.as_deref(),
        Some("bank_of_nowhere")
    );
    assert_eq!(parsed.records[0].amount_minor, -300);
}

/// How far `suggest` gets on its own: on each corpus file, does its guess read the
/// dates and amounts right? Reported, and held to a floor.
#[test]
fn suggestions_get_most_corpus_layouts_right() {
    let mut right = 0;
    let mut total = 0;
    for (path, expected, _) in csv_files() {
        let bytes = std::fs::read(corpus_dir().join(&path)).unwrap();
        let Some(p) = suggest(&bytes) else { continue };
        total += 1;
        let exp = read_json(&expected);
        let rows = exp["transactions"].as_array().unwrap();
        let Ok(mut parsed) = parse(&bytes, &p, "USD") else {
            continue;
        };
        let parsed = parsed.remove(0);
        let ok = parsed.records.len() == rows.len()
            && parsed.records.iter().zip(rows).all(|(r, e)| {
                r.amount_minor == e["amount"]["minor"].as_i64().unwrap()
                    && r.posted_date.to_string() == e["posted_date"].as_str().unwrap()
            });
        if ok {
            right += 1;
        }
    }
    println!("suggest: {right}/{total} corpus CSV layouts read with correct dates and amounts");
    assert!(total >= 25, "suggest found a header in only {total} files");
    assert!(right * 10 >= total * 6, "suggest right on {right}/{total}");
}
