//! Every fixture file parses with the real importers, and `expected.json` agrees with
//! the files and with itself.

use std::collections::{BTreeMap, BTreeSet};

use fm_import_csv::{ProfileSet, Statement, Status};
use serde_json::Value;

fn expected() -> Value {
    let bytes = std::fs::read(fm_import_edge_cases::root().join("expected.json")).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn cases() -> Vec<Value> {
    expected()["cases"].as_array().unwrap().clone()
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or_else(|| panic!("missing {k}"))
}

fn parse(file: &Value) -> Vec<Statement> {
    let path = s(file, "path");
    let bytes = std::fs::read(fm_import_edge_cases::root().join(path)).unwrap();
    match s(file, "format") {
        "csv" => {
            let st = fm_import_csv::import(&bytes, &ProfileSet::with_builtin(), "USD")
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            assert_eq!(
                st[0].info.institution.layout.as_deref(),
                file["layout"].as_str(),
                "{path}: detected layout"
            );
            st
        }
        _ => {
            fm_import_ofx::parse(&bytes)
                .unwrap_or_else(|e| panic!("{path}: {e}"))
                .statements
        }
    }
}

#[test]
fn every_file_parses_to_exactly_the_rows_listed() {
    let mut files_checked = BTreeSet::new();
    for case in cases() {
        for file in case["files"].as_array().unwrap() {
            let path = s(file, "path");
            let statements = parse(file);
            let rows: Vec<&Value> = case["rows"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| s(r, "file") == path)
                .collect();
            let listed: BTreeSet<u64> = rows
                .iter()
                .map(|r| r["statement"].as_u64().unwrap())
                .collect();
            assert_eq!(listed.len(), statements.len(), "{path}: statement count");
            for (si, st) in statements.iter().enumerate() {
                let mine: Vec<&&Value> = rows
                    .iter()
                    .filter(|r| r["statement"].as_u64() == Some(si as u64))
                    .collect();
                assert_eq!(
                    mine.len(),
                    st.records.len(),
                    "{path} statement {si}: row count"
                );
                for r in mine {
                    let i = r["row"].as_u64().unwrap() as usize;
                    let rec = &st.records[i];
                    let at = format!("{path} statement {si} row {i}");
                    // Field names only on failure: no amounts or text in test output.
                    let checks = [
                        (
                            "amount",
                            rec.amount_minor == r["amount_minor"].as_i64().unwrap(),
                        ),
                        (
                            "posted date",
                            rec.posted_date.to_string() == s(r, "posted_date"),
                        ),
                        (
                            "transaction date",
                            rec.transaction_date.map(|d| d.to_string()).as_deref()
                                == r["transaction_date"].as_str(),
                        ),
                        (
                            "status",
                            (rec.status == Status::Pending) == (s(r, "status") == "pending"),
                        ),
                        (
                            "bank id",
                            rec.external_id.as_deref() == r["external_id"].as_str(),
                        ),
                        (
                            "check number",
                            r.get("check_number").is_none()
                                || rec.check_number.as_deref() == r["check_number"].as_str(),
                        ),
                    ];
                    for (field, ok) in checks {
                        assert!(ok, "{at}: {field} differs");
                    }
                }
            }
            for st in &statements {
                if let Some(n) = &st.info.account_number {
                    assert!(n.starts_with("9999"), "{path}: account number must be fake");
                }
                if let Some(r) = &st.info.routing {
                    assert!(
                        r.starts_with("999999"),
                        "{path}: routing number must be fake"
                    );
                }
            }
            files_checked.insert(path.to_string());
        }
    }
    // Every file on disk is listed by some case (cases can share files to test both orders).
    let mut on_disk = BTreeSet::new();
    for dir in std::fs::read_dir(fm_import_edge_cases::root().join("cases")).unwrap() {
        let dir = dir.unwrap();
        for f in std::fs::read_dir(dir.path()).unwrap() {
            let name = f.unwrap().file_name().into_string().unwrap();
            on_disk.insert(format!(
                "cases/{}/{name}",
                dir.file_name().into_string().unwrap()
            ));
        }
    }
    assert_eq!(
        files_checked, on_disk,
        "every fixture file is in expected.json"
    );
}

/// Outcomes follow from the import order: a transaction's first row is inserted; a
/// later posted row is a duplicate, or settles the pending row seen before it.
#[test]
fn expected_outcomes_are_consistent() {
    for case in cases() {
        let id = s(&case, "id");
        let accounts: BTreeSet<&str> = case["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| s(a, "key"))
            .collect();
        let tx: BTreeMap<&str, &Value> = case["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (s(t, "key"), t))
            .collect();
        for t in tx.values() {
            assert!(accounts.contains(s(t, "account")), "{id}: unknown account");
        }
        let order: Vec<&str> = case["import_order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap())
            .collect();
        let files: BTreeSet<&str> = case["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| s(f, "path"))
            .collect();
        assert_eq!(
            order.iter().copied().collect::<BTreeSet<_>>(),
            files,
            "{id}: import_order lists every file once"
        );
        let file_account: BTreeMap<&str, &str> = case["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (s(f, "path"), s(f, "account")))
            .collect();
        let mut rows: Vec<&Value> = case["rows"].as_array().unwrap().iter().collect();
        rows.sort_by_key(|r| {
            (
                order.iter().position(|f| *f == s(r, "file")).unwrap(),
                r["statement"].as_u64().unwrap(),
                r["row"].as_u64().unwrap(),
            )
        });
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for r in rows {
            let key = s(r, "transaction");
            let t = tx
                .get(key)
                .unwrap_or_else(|| panic!("{id}: unknown transaction {key}"));
            assert_eq!(
                s(t, "account"),
                file_account[s(r, "file")],
                "{id}/{key}: row and transaction accounts"
            );
            let status = s(r, "status");
            if status == "posted" {
                assert!(
                    r["amount_minor"] == t["amount_minor"],
                    "{id}/{key}: posted rows carry the final amount"
                );
            }
            let want = match (seen.get(key).copied(), status) {
                (None, _) => "inserted",
                (Some("pending"), "posted") => "posted_pending",
                (Some(_), "posted") => "duplicate",
                (Some("posted"), "pending") => "pending_already_posted",
                (Some(_), _) => "duplicate",
            };
            assert_eq!(s(r, "outcome"), want, "{id}/{key}: outcome");
            if status == "posted" || !seen.contains_key(key) {
                seen.insert(key, status);
            }
        }
        for (key, t) in &tx {
            assert!(
                seen.contains_key(key),
                "{id}/{key}: no row is this transaction"
            );
            assert_eq!(seen[key], s(t, "final_status"), "{id}/{key}: final status");
        }
    }
}

#[test]
fn every_requested_case_is_present() {
    let ids: Vec<String> = cases().iter().map(|c| s(c, "id").to_string()).collect();
    for prefix in [
        "01_", "02_", "03a_", "03b_", "04_", "05_", "06_", "07a_", "07b_", "08a_", "08b_", "09a_",
        "09b_", "10_", "11_", "12_", "13_",
    ] {
        assert!(
            ids.iter().any(|i| i.starts_with(prefix)),
            "no case {prefix}"
        );
    }
    let auto: Vec<Value> = cases()
        .into_iter()
        .filter(|c| c["target"] == "auto")
        .collect();
    assert_eq!(
        auto.len(),
        1,
        "case 04 imports with automatic account matching"
    );
    let accounts = auto[0]["accounts"].as_array().unwrap();
    let last4: BTreeSet<&str> = accounts.iter().map(|a| s(a, "last4")).collect();
    assert_eq!(
        (accounts.len(), last4.len()),
        (2, 1),
        "two accounts, one last four"
    );
}

/// Cases imported in both orders share their files and their ground truth.
#[test]
fn both_orders_agree_on_the_truth() {
    let all = cases();
    for (a, b) in [("08a_", "08b_"), ("09a_", "09b_")] {
        let find = |p: &str| {
            all.iter()
                .find(|c| s(c, "id").starts_with(p))
                .unwrap()
                .clone()
        };
        let (x, y) = (find(a), find(b));
        assert_eq!(x["transactions"], y["transactions"], "{a}: same truth");
        assert_eq!(x["files"], y["files"], "{a}: same files");
        let mut order = x["import_order"].as_array().unwrap().clone();
        order.reverse();
        assert_eq!(
            &order,
            y["import_order"].as_array().unwrap(),
            "{a}: reversed order"
        );
        let keys = |c: &Value| -> BTreeSet<(String, u64, u64, String)> {
            c["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    (
                        s(r, "file").to_string(),
                        r["statement"].as_u64().unwrap(),
                        r["row"].as_u64().unwrap(),
                        s(r, "transaction").to_string(),
                    )
                })
                .collect()
        };
        assert_eq!(
            keys(&x),
            keys(&y),
            "{a}: each row is the same transaction in both orders"
        );
    }
}
