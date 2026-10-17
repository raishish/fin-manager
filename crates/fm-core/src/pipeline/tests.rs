//! Import pipeline tests against TD.2's bank-export corpus (built from TD.1's ledgers).
//!
//! TD.2 reuses each TD.1 account across several institutions' layouts, so the 50 files
//! land on 16 accounts with 1 to 6 overlapping exports each, in CSV, QFX and OFX. Every
//! row carries the TD.1 `truth_id` it came from, which lets the tests check that each
//! real transaction ends up as exactly one ledger transaction.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tempfile::TempDir;

use super::*;
use crate::contract;
use crate::model::Ledger;
use crate::storage::{KdfParams, Vault};

const PASSWORD: &[u8] = b"correct horse battery staple";

fn data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

fn json(path: PathBuf) -> Value {
    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap()
}

fn when() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000)
}

/// One TD.2 file: its bytes, the TD.1 account it belongs to, and its expected rows.
struct CorpusFile {
    path: String,
    bytes: Vec<u8>,
    account: AccountId,
    /// Per row, in file order: `(truth_id, status, amount_minor, posted_date)`.
    rows: Vec<(i64, String, i64, String)>,
}

fn corpus(persona: &str) -> Vec<CorpusFile> {
    let dir = data().join("bank-exports/data");
    let manifest = json(dir.join("manifest.json"));
    manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["persona"] == persona)
        .map(|f| {
            let path = f["path"].as_str().unwrap().to_owned();
            let expected = json(dir.join(f["expected"].as_str().unwrap()));
            let rows = expected["transactions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    (
                        t["truth_id"].as_i64().unwrap(),
                        t["status"].as_str().unwrap().to_owned(),
                        t["amount"]["minor"].as_i64().unwrap(),
                        t["posted_date"].as_str().unwrap().to_owned(),
                    )
                })
                .collect();
            CorpusFile {
                bytes: fs::read(dir.join(&path)).unwrap(),
                path,
                account: AccountId(expected["source"]["account_id"].as_i64().unwrap()),
                rows,
            }
        })
        .collect()
}

fn td1(persona: &str) -> Ledger {
    serde_json::from_slice(
        &fs::read(data().join(format!("synthetic/data/{persona}/ledger.json"))).unwrap(),
    )
    .unwrap()
}

/// A ledger holding the persona's TD.1 accounts (same ids), and nothing else.
fn ledger_with_accounts(persona: &str) -> (TempDir, Unlocked) {
    let dir = tempfile::tempdir().unwrap();
    let u = Vault::at(&dir.path().join("ledger"))
        .create_with_params(PASSWORD, KdfParams::INSECURE_TEST)
        .unwrap();
    for account in td1(persona).accounts {
        u.insert_account(&account).unwrap();
    }
    (dir, u)
}

fn request<'a>(
    bytes: &'a [u8],
    path: &'a str,
    target: Target,
    profiles: &'a fm_import_csv::ProfileSet,
) -> ImportRequest<'a> {
    ImportRequest {
        bytes,
        file_name: Some(path),
        target,
        csv_currency: Currency::USD,
        csv_profiles: profiles,
        now: when(),
    }
}

const PERSONAS: [&str; 4] = ["renter", "family", "freelancer", "retiree"];

/// Imports every file of a persona in the given order and checks the result against
/// TD.1's truth. Returns the number of pending→posted merges seen.
fn import_persona(persona: &str, reverse: bool) -> usize {
    let mut files = corpus(persona);
    if reverse {
        files.reverse();
    }
    let profiles = fm_import_csv::ProfileSet::with_builtin();
    let (_dir, mut u) = ledger_with_accounts(persona);

    // truth_id → the ledger transactions its rows ended up as.
    let mut landed: BTreeMap<i64, BTreeSet<TransactionId>> = BTreeMap::new();
    let mut merges = 0;
    for f in &files {
        let report = import_file(
            &mut u,
            &request(&f.bytes, &f.path, Target::Account(f.account), &profiles),
        )
        .unwrap_or_else(|e| panic!("{persona} {}: {e}", f.path));
        assert_eq!(report.statements.len(), 1, "{}", f.path);
        let s = &report.statements[0];
        assert_eq!(s.account_id, f.account);
        assert!(!s.account_created);
        assert_eq!(s.records.len(), f.rows.len(), "{}: row count", f.path);
        for ((_, outcome), (truth_id, ..)) in s.records.iter().zip(&f.rows) {
            landed
                .entry(*truth_id)
                .or_default()
                .insert(outcome.transaction());
        }
        merges += s.posted_from_pending();
        assert!(report.backup.is_none(), "no backup folder is set");
    }

    // Zero duplicates and zero false merges: one transaction per real transaction.
    for (truth_id, txs) in &landed {
        assert_eq!(
            txs.len(),
            1,
            "{persona}: truth {truth_id} became {} transactions",
            txs.len()
        );
    }
    let mut owners: HashMap<TransactionId, i64> = HashMap::new();
    for (truth_id, txs) in &landed {
        let tx = *txs.iter().next().unwrap();
        if let Some(other) = owners.insert(tx, *truth_id) {
            panic!("{persona}: truths {other} and {truth_id} merged into one transaction");
        }
    }
    let ledger = u.load_ledger().unwrap();
    assert_eq!(
        ledger.transactions.len(),
        landed.len(),
        "{persona}: transaction count"
    );

    // Each transaction matches TD.1: posted ones at TD.1's amount, on a date one of the
    // files gave (layouts with only a transaction-date column report that date as the
    // posted date); ones that only ever appeared pending stay pending.
    let truth = td1(persona);
    let truth_tx: HashMap<i64, &Transaction> =
        truth.transactions.iter().map(|t| (t.id.0, t)).collect();
    let mut posted_dates: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for r in files
        .iter()
        .flat_map(|f| &f.rows)
        .filter(|r| r.1 == "posted")
    {
        posted_dates.entry(r.0).or_default().insert(r.3.clone());
    }
    for (truth_id, txs) in &landed {
        let tx = ledger
            .transactions
            .iter()
            .find(|t| txs.contains(&t.id))
            .unwrap();
        let t = truth_tx[truth_id];
        if let Some(dates) = posted_dates.get(truth_id) {
            assert_eq!(
                tx.status,
                TransactionStatus::Posted,
                "{persona}: truth {truth_id}"
            );
            assert_eq!(tx.amount, t.amount, "{persona}: truth {truth_id} amount");
            assert!(
                dates.contains(&tx.posted_date.to_string()),
                "{persona}: truth {truth_id} date"
            );
        } else {
            assert_eq!(
                tx.status,
                TransactionStatus::Pending,
                "{persona}: truth {truth_id}"
            );
        }
        assert_eq!(tx.account_id, t.account_id);
    }

    // Every transaction is in a bucket (Needs review), and the contract holds.
    let report = contract::check(&ledger);
    assert!(report.is_ok(), "{persona}: {report}");
    assert!(ledger.decisions.iter().all(|d| {
        d.assignment == Assignment::Bucket(Bucket::NeedsReview)
            && d.source
                == DecisionSource::Builtin {
                    logic: BuiltinLogic::ImportPipeline,
                    version: PIPELINE_VERSION,
                }
    }));
    assert_eq!(ledger.decisions.len(), ledger.transactions.len());

    // Importing everything again: every file is refused, nothing changes.
    for f in &files {
        let err = import_file(
            &mut u,
            &request(&f.bytes, &f.path, Target::Account(f.account), &profiles),
        )
        .unwrap_err();
        assert!(
            matches!(err, ImportError::AlreadyImported { .. }),
            "{}: {err}",
            f.path
        );
    }
    assert_eq!(u.load_ledger().unwrap(), ledger);
    merges
}

#[test]
fn the_whole_corpus_imports_without_duplicates_in_file_order() {
    for persona in PERSONAS {
        import_persona(persona, false);
    }
}

#[test]
fn the_whole_corpus_imports_without_duplicates_in_reverse_order() {
    for persona in PERSONAS {
        import_persona(persona, true);
    }
}

/// The 7 TD.2 pending→posted cases, each imported pending first and posted second, the
/// way a user downloading mid-cycle and again later would: all 7 merge into one
/// transaction at the posted amount and date.
#[test]
fn the_seven_pending_cases_merge_into_their_posted_transactions() {
    let pairs = [
        (
            "renter",
            "citi/card_2025_06_16.csv",
            "citi/card_2025_07_14.csv",
        ),
        (
            "renter",
            "citi/card_2025_12_15.csv",
            "citi/card_2026_01_12.csv",
        ),
        (
            "renter",
            "citi/card_2025_07_14.csv",
            "american_express/gold_2025.csv",
        ),
        (
            "renter",
            "usaa/checking_2026_03_13.csv",
            "usaa/checking_2026_04_13.csv",
        ),
        (
            "freelancer",
            "charles_schwab_bank/checking_2025_09_15.csv",
            "charles_schwab_bank/checking_2025_10_15.csv",
        ),
    ];
    let profiles = fm_import_csv::ProfileSet::with_builtin();
    let mut merged = 0;
    for (persona, pending_file, posted_file) in pairs {
        let files = corpus(persona);
        let get = |p: &str| files.iter().find(|f| f.path == p).unwrap();
        let (pending, posted) = (get(pending_file), get(posted_file));
        let (_dir, mut u) = ledger_with_accounts(persona);
        let first = import_file(
            &mut u,
            &request(
                &pending.bytes,
                &pending.path,
                Target::Account(pending.account),
                &profiles,
            ),
        )
        .unwrap();
        let pending_rows: Vec<(i64, TransactionId)> = first.statements[0]
            .records
            .iter()
            .zip(&pending.rows)
            .filter(|(_, row)| row.1 == "pending")
            .map(|((_, o), row)| (row.0, o.transaction()))
            .collect();
        let second = import_file(
            &mut u,
            &request(
                &posted.bytes,
                &posted.path,
                Target::Account(posted.account),
                &profiles,
            ),
        )
        .unwrap();
        for (truth_id, tx) in pending_rows {
            let Some(((_, outcome), row)) = second.statements[0]
                .records
                .iter()
                .zip(&posted.rows)
                .find(|(_, row)| row.0 == truth_id && row.1 == "posted")
            else {
                continue; // posts only in a later download than this pair
            };
            assert_eq!(
                *outcome,
                RecordOutcome::PostedPending(tx),
                "{posted_file}: truth {truth_id}"
            );
            let t = u.transaction(tx).unwrap().unwrap();
            assert_eq!(t.status, TransactionStatus::Posted);
            assert_eq!(t.amount.minor(), row.2, "the posted amount (tips included)");
            assert_eq!(t.posted_date.to_string(), row.3);
            // Settled in place: still one Needs review decision, on the same transaction.
            assert_eq!(u.decisions_for_transaction(tx).unwrap().len(), 1);
            merged += 1;
        }
        let report = contract::check(&u.load_ledger().unwrap());
        assert!(report.is_ok(), "{report}");
    }
    assert_eq!(merged, 7, "pending→posted merges");
}

#[test]
fn ofx_files_find_their_account_by_number() {
    let profiles = fm_import_csv::ProfileSet::with_builtin();
    let dir = tempfile::tempdir().unwrap();
    let mut u = Vault::at(&dir.path().join("ledger"))
        .create_with_params(PASSWORD, KdfParams::INSECURE_TEST)
        .unwrap();
    let path = "pnc/card_2026.qfx";
    let bytes = fs::read(data().join("bank-exports/data").join(path)).unwrap();
    let first = import_file(&mut u, &request(&bytes, path, Target::Auto, &profiles)).unwrap();
    let s = &first.statements[0];
    assert!(s.account_created);
    let account = u.account(s.account_id).unwrap().unwrap();
    assert_eq!(account.kind, AccountKind::CreditCard);
    assert_eq!(account.institution.as_deref(), Some("PNC"));
    assert!(account.external_id.is_some());
    assert_eq!(account.masked_number.as_ref().map(String::len), Some(4));
    assert_eq!(s.inserted(), s.records.len());

    // The same download again, re-saved (different bytes): same account, all duplicates.
    let mut resaved = bytes.clone();
    resaved.extend_from_slice(b"\r\n");
    let second = import_file(&mut u, &request(&resaved, path, Target::Auto, &profiles)).unwrap();
    let s2 = &second.statements[0];
    assert_eq!(s2.account_id, s.account_id);
    assert!(!s2.account_created);
    assert_eq!(s2.duplicates(), s2.records.len());
    assert_eq!(u.accounts().unwrap().len(), 1);
    assert_eq!(u.raw_imports().unwrap().len(), 2);
}

#[test]
fn a_csv_needs_an_account_and_failures_leave_nothing_behind() {
    let profiles = fm_import_csv::ProfileSet::with_builtin();
    let (_dir, mut u) = ledger_with_accounts("renter");
    let files = corpus("renter");
    let f = files
        .iter()
        .find(|f| f.path == "chase/checking_2025.csv")
        .unwrap();
    // A Chase checking CSV says nothing about which account it is.
    let err =
        import_file(&mut u, &request(&f.bytes, &f.path, Target::Auto, &profiles)).unwrap_err();
    assert!(matches!(err, ImportError::AccountNeeded), "{err}");
    // An account in another currency: refused part-way, all rolled back.
    let eur = Account {
        id: AccountId(50),
        currency: Currency::EUR,
        ..u.account(AccountId(1)).unwrap().unwrap()
    };
    u.insert_account(&eur).unwrap();
    let before = u.load_ledger().unwrap();
    let err = import_file(
        &mut u,
        &request(&f.bytes, &f.path, Target::Account(eur.id), &profiles),
    )
    .unwrap_err();
    assert!(matches!(err, ImportError::CurrencyMismatch { .. }), "{err}");
    assert_eq!(
        u.load_ledger().unwrap(),
        before,
        "a failed import left something behind"
    );
    // Unknown account.
    let err = import_file(
        &mut u,
        &request(&f.bytes, &f.path, Target::Account(AccountId(99)), &profiles),
    )
    .unwrap_err();
    assert!(matches!(err, ImportError::UnknownAccount(AccountId(99))));
    // A new account named by the caller.
    let report = import_file(
        &mut u,
        &request(
            &f.bytes,
            &f.path,
            Target::NewAccount {
                name: "Everyday checking".into(),
                kind: AccountKind::Checking,
                currency: Currency::USD,
            },
            &profiles,
        ),
    )
    .unwrap();
    assert!(report.statements[0].account_created);
    assert_eq!(report.statements[0].inserted(), f.rows.len());
}

#[test]
fn source_facts_are_kept() {
    let profiles = fm_import_csv::ProfileSet::with_builtin();
    let (_dir, mut u) = ledger_with_accounts("retiree");
    let files = corpus("retiree");
    let f = files
        .iter()
        .find(|f| f.path == "navy_federal_credit_union/checking_2026.ofx")
        .unwrap();
    import_file(
        &mut u,
        &request(&f.bytes, &f.path, Target::Account(f.account), &profiles),
    )
    .unwrap();
    let txs = u.transactions_for_account(f.account).unwrap();
    assert!(txs.iter().all(|t| t.trn_type.is_some()), "TRNTYPE kept");
    assert!(
        txs.iter().any(|t| t.check_number.is_some()),
        "CHECKNUM kept"
    );
    assert!(
        txs.iter().all(|t| t.source.external_id.is_some()),
        "FITID kept"
    );
    let raw = u.raw_imports().unwrap();
    assert_eq!(raw[0].source, ImportSource::Ofx);
    assert_eq!(raw[0].account_id, Some(f.account));
    assert_eq!(raw[0].bytes(), f.bytes.as_slice());
}

/// A small OFX file: one bank statement per `(ACCTID, rows)` entry, from institution
/// `org`. Each row is `(date, amount, name)`.
/// `(date, amount, name)`.
type Row<'a> = (&'a str, &'a str, &'a str);

fn ofx(org: &str, statements: &[(&str, &[Row<'_>])]) -> Vec<u8> {
    let mut s =
        String::from("OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\n\n<OFX><SIGNONMSGSRSV1><SONRS>");
    s.push_str(&format!(
        "<FI><ORG>{org}<FID>1</FI></SONRS></SIGNONMSGSRSV1><BANKMSGSRSV1>"
    ));
    for (acct, rows) in statements {
        s.push_str(&format!(
            "<STMTTRNRS><STMTRS><CURDEF>USD<BANKACCTFROM><BANKID>999999001<ACCTID>{acct}\
             <ACCTTYPE>CHECKING</BANKACCTFROM><BANKTRANLIST>"
        ));
        for (date, amount, name) in *rows {
            s.push_str(&format!(
                "<STMTTRN><TRNTYPE>DEBIT<DTPOSTED>{date}<TRNAMT>{amount}<NAME>{name}</STMTTRN>"
            ));
        }
        s.push_str("</BANKTRANLIST></STMTRS></STMTTRNRS>");
    }
    s.push_str("</BANKMSGSRSV1></OFX>");
    s.into_bytes()
}

fn empty_ledger() -> (TempDir, Unlocked) {
    let dir = tempfile::tempdir().unwrap();
    let u = Vault::at(&dir.path().join("ledger"))
        .create_with_params(PASSWORD, KdfParams::INSECURE_TEST)
        .unwrap();
    (dir, u)
}

/// Review 1, item 4: a masked or short account number matches only at the same bank;
/// a full number matches on its own.
#[test]
fn short_or_masked_numbers_need_the_same_institution() {
    let profiles = fm_import_csv::ProfileSet::with_builtin();
    let (_dir, mut u) = empty_ledger();
    let row: &[Row<'_>] = &[("20260105", "-4.50", "COFFEE")];
    let auto = |u: &mut Unlocked, bytes: &[u8]| {
        import_file(u, &request(bytes, "x.ofx", Target::Auto, &profiles))
            .unwrap()
            .statements[0]
            .account_id
    };
    let a = auto(&mut u, &ofx("BANK A", &[("XXXX1234", row)]));
    // The same masked number at another bank is another account.
    let b = auto(&mut u, &ofx("BANK B", &[("XXXX1234", row)]));
    assert_ne!(a, b);
    // At the same bank, it's the same account.
    assert_eq!(
        auto(
            &mut u,
            &ofx("BANK A", &[("XXXX1234", &[("20260106", "-1.00", "TEA")])])
        ),
        a
    );
    // A full number is enough on its own, whatever ORG says.
    let c = auto(&mut u, &ofx("BANK C", &[("999900001111", row)]));
    assert_eq!(
        auto(
            &mut u,
            &ofx(
                "Bank C Online",
                &[("999900001111", &[("20260107", "-2.00", "TEA")])]
            )
        ),
        c
    );
    assert_eq!(u.accounts().unwrap().len(), 3);
}

/// Review 1, item 7: two statements of one file on the same account never match each
/// other's rows, even identical ones.
#[test]
fn statements_of_one_file_do_not_match_each_other() {
    let profiles = fm_import_csv::ProfileSet::with_builtin();
    let (_dir, mut u) = empty_ledger();
    let row: &[Row<'_>] = &[("20260105", "-4.50", "COFFEE SHOP")];
    let bytes = ofx("BANK A", &[("999900002222", row), ("999900002222", row)]);
    let report = import_file(&mut u, &request(&bytes, "x.ofx", Target::Auto, &profiles)).unwrap();
    assert_eq!(report.statements.len(), 2);
    assert_eq!(
        report.statements[0].account_id,
        report.statements[1].account_id
    );
    assert_eq!(
        report.statements[0].inserted() + report.statements[1].inserted(),
        2
    );
    assert_eq!(u.transactions().unwrap().len(), 2);
}

/// Review 1, item 5: the import's savepoint is never left open, and an import refuses
/// to run inside someone else's transaction.
#[test]
fn the_savepoint_is_closed_on_every_path() {
    let profiles = fm_import_csv::ProfileSet::with_builtin();
    let (_dir, mut u) = empty_ledger();
    let good = ofx(
        "BANK A",
        &[("999900003333", &[("20260105", "-4.50", "COFFEE")])],
    );
    // Inside an open transaction: refused, nothing written, the caller's transaction
    // untouched.
    u.connection().execute_batch("BEGIN").unwrap();
    let err = import_file(&mut u, &request(&good, "x.ofx", Target::Auto, &profiles)).unwrap_err();
    assert!(matches!(err, ImportError::TransactionOpen), "{err}");
    assert!(!u.connection().is_autocommit());
    u.connection().execute_batch("ROLLBACK").unwrap();
    // A failure part-way (an unknown account) rolls back and closes the savepoint.
    let err = import_file(
        &mut u,
        &request(&good, "x.ofx", Target::Account(AccountId(9)), &profiles),
    )
    .unwrap_err();
    assert!(matches!(err, ImportError::UnknownAccount(_)));
    assert!(u.connection().is_autocommit(), "savepoint left open");
    assert!(u.raw_imports().unwrap().is_empty());
    // A panic inside the savepoint (simulated by dropping the guard unreleased).
    {
        let _sp = Savepoint::begin(u.connection()).unwrap();
        u.connection()
            .execute_batch("INSERT INTO meta (key, value) VALUES ('x', 'y')")
            .unwrap();
    }
    assert!(u.connection().is_autocommit());
    let rows: i64 = u
        .connection()
        .query_row("SELECT count(*) FROM meta WHERE key = 'x'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(rows, 0, "an unreleased savepoint was not rolled back");
    // And the good import still works afterwards.
    import_file(&mut u, &request(&good, "x.ofx", Target::Auto, &profiles)).unwrap();
}

// ---- TD.4: adversarial import fixtures (testdata/import-edge-cases).

struct EdgeCase {
    id: String,
    value: Value,
}

fn edge_cases() -> Vec<EdgeCase> {
    let expected = json(data().join("import-edge-cases/expected.json"));
    expected["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| EdgeCase {
            id: c["id"].as_str().unwrap().to_owned(),
            value: c.clone(),
        })
        .collect()
}

fn account_kind_of(kind: &str) -> AccountKind {
    serde_json::from_value(Value::String(kind.into())).unwrap()
}

/// Imports a TD.4 case's files in `order` into a fresh ledger. Returns the ledger, the
/// transaction each row became (keyed by `(file, statement, row)`) with its outcome,
/// and the account each account key resolved to.
#[allow(clippy::type_complexity)]
fn run_edge_case(
    case: &EdgeCase,
    order: &[String],
) -> (
    TempDir,
    Unlocked,
    BTreeMap<(String, usize, u32), RecordOutcome>,
    BTreeMap<String, AccountId>,
) {
    let c = &case.value;
    let profiles = fm_import_csv::ProfileSet::with_builtin();
    let (dir, mut u) = empty_ledger();
    let auto = c["target"] == "auto";
    let mut accounts: BTreeMap<String, AccountId> = BTreeMap::new();
    if !auto {
        for (i, a) in c["accounts"].as_array().unwrap().iter().enumerate() {
            let id = AccountId(i as i64 + 1);
            u.insert_account(&Account {
                id,
                name: a["name"].as_str().unwrap().into(),
                kind: account_kind_of(a["kind"].as_str().unwrap()),
                currency: Currency::new(a["currency"].as_str().unwrap()).unwrap(),
                institution: a["institution"].as_str().map(Into::into),
                masked_number: a["last4"].as_str().map(Into::into),
                external_id: None,
                own_account: true,
                created_at: Timestamp::from_unix_seconds(0),
            })
            .unwrap();
            accounts.insert(a["key"].as_str().unwrap().into(), id);
        }
    }
    let file_account = |path: &str| -> String {
        c["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == path)
            .unwrap()["account"]
            .as_str()
            .unwrap()
            .into()
    };
    let mut outcomes = BTreeMap::new();
    for path in order {
        let bytes = fs::read(data().join("import-edge-cases").join(path)).unwrap();
        let key = file_account(path);
        let target = if auto {
            Target::Auto
        } else {
            Target::Account(accounts[&key])
        };
        let report = import_file(&mut u, &request(&bytes, path, target, &profiles))
            .unwrap_or_else(|e| panic!("{} {path}: {e}", case.id));
        for (si, s) in report.statements.iter().enumerate() {
            if auto {
                let known = *accounts.entry(key.clone()).or_insert(s.account_id);
                assert_eq!(known, s.account_id, "{} {path}: account for {key}", case.id);
            }
            // TD.4's `row` is the position within its statement.
            for (row, (_, outcome)) in s.records.iter().enumerate() {
                outcomes.insert((path.clone(), si, row as u32), *outcome);
            }
        }
    }
    (dir, u, outcomes, accounts)
}

fn outcome_name(o: RecordOutcome) -> &'static str {
    match o {
        RecordOutcome::Inserted(_) => "inserted",
        RecordOutcome::Duplicate(_) => "duplicate",
        RecordOutcome::PostedPending(_) => "posted_pending",
        RecordOutcome::PendingAlreadyPosted(_) => "pending_already_posted",
    }
}

/// Checks the ledger holds exactly the case's transactions: same keys → one
/// transaction, different keys → different ones, each with its final status and amount,
/// on its account, dated as one of its rows.
fn check_final_transactions(
    case: &EdgeCase,
    u: &Unlocked,
    outcomes: &BTreeMap<(String, usize, u32), RecordOutcome>,
    accounts: &BTreeMap<String, AccountId>,
    order: &str,
) {
    let c = &case.value;
    let id = &case.id;
    let mut by_key: BTreeMap<String, BTreeSet<TransactionId>> = BTreeMap::new();
    let mut dates: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in c["rows"].as_array().unwrap() {
        let k = (
            r["file"].as_str().unwrap().to_owned(),
            r["statement"].as_u64().unwrap() as usize,
            r["row"].as_u64().unwrap() as u32,
        );
        let outcome = outcomes
            .get(&k)
            .unwrap_or_else(|| panic!("{id} ({order}): no outcome for {k:?}"));
        let key = r["transaction"].as_str().unwrap().to_owned();
        by_key
            .entry(key.clone())
            .or_default()
            .insert(outcome.transaction());
        dates
            .entry(key)
            .or_default()
            .insert(r["posted_date"].as_str().unwrap().to_owned());
    }
    let mut owner: HashMap<TransactionId, String> = HashMap::new();
    for (key, txs) in &by_key {
        assert_eq!(
            txs.len(),
            1,
            "{id} ({order}): {key} became {} transactions",
            txs.len()
        );
        if let Some(other) = owner.insert(*txs.iter().next().unwrap(), key.clone()) {
            panic!("{id} ({order}): {other} and {key} merged into one transaction");
        }
    }
    let ledger = u.load_ledger().unwrap();
    let expected = c["transactions"].as_array().unwrap();
    assert_eq!(
        ledger.transactions.len(),
        expected.len(),
        "{id} ({order}): transaction count"
    );
    for t in expected {
        let key = t["key"].as_str().unwrap();
        let tx_id = *by_key[key].iter().next().unwrap();
        let tx = ledger.transactions.iter().find(|x| x.id == tx_id).unwrap();
        let status = match tx.status {
            TransactionStatus::Posted => "posted",
            TransactionStatus::Pending => "pending",
        };
        assert_eq!(status, t["final_status"], "{id} ({order}): {key} status");
        assert_eq!(
            tx.amount.minor(),
            t["amount_minor"].as_i64().unwrap(),
            "{id} ({order}): {key} amount"
        );
        assert_eq!(
            tx.account_id,
            accounts[t["account"].as_str().unwrap()],
            "{id} ({order}): {key} account"
        );
        assert!(
            dates[key].contains(&tx.posted_date.to_string()),
            "{id} ({order}): {key} date"
        );
    }
    let report = contract::check(&ledger);
    assert!(report.is_ok(), "{id} ({order}): {report}");
}

#[test]
fn td4_edge_cases_in_import_order() {
    let cases = edge_cases();
    assert_eq!(cases.len(), 17);
    for case in &cases {
        let order: Vec<String> = case.value["import_order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_owned())
            .collect();
        let (_dir, u, outcomes, accounts) = run_edge_case(case, &order);
        for r in case.value["rows"].as_array().unwrap() {
            let k = (
                r["file"].as_str().unwrap().to_owned(),
                r["statement"].as_u64().unwrap() as usize,
                r["row"].as_u64().unwrap() as u32,
            );
            assert_eq!(
                outcome_name(outcomes[&k]),
                r["outcome"],
                "{}: {} statement {} row {} ({})",
                case.id,
                k.0,
                k.1,
                k.2,
                r["transaction"]
            );
        }
        // Under Target::Auto the files resolve to exactly the case's accounts.
        let ledger_accounts = u.accounts().unwrap();
        assert_eq!(
            ledger_accounts.len(),
            case.value["accounts"].as_array().unwrap().len(),
            "{}",
            case.id
        );
        for a in case.value["accounts"].as_array().unwrap() {
            let acct = ledger_accounts
                .iter()
                .find(|x| x.id == accounts[a["key"].as_str().unwrap()])
                .unwrap();
            assert_eq!(
                acct.institution.as_deref(),
                a["institution"].as_str(),
                "{}",
                case.id
            );
            assert_eq!(
                acct.masked_number.as_deref(),
                a["last4"].as_str(),
                "{}",
                case.id
            );
            assert_eq!(
                acct.kind,
                account_kind_of(a["kind"].as_str().unwrap()),
                "{}",
                case.id
            );
        }
        check_final_transactions(case, &u, &outcomes, &accounts, "import order");
    }
}

#[test]
fn td4_edge_cases_in_reverse_order() {
    for case in &edge_cases() {
        let mut order: Vec<String> = case.value["import_order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_owned())
            .collect();
        order.reverse();
        let (_dir, u, outcomes, accounts) = run_edge_case(case, &order);
        check_final_transactions(case, &u, &outcomes, &accounts, "reverse order");
    }
}

/// Review 2, item 4: the last four digits match only an account of the same kind.
#[test]
fn last_four_digits_need_the_same_kind_of_account() {
    let profiles = fm_import_csv::ProfileSet::with_builtin();
    let (_dir, mut u) = empty_ledger();
    let row: &[Row<'_>] = &[("20260105", "-4.50", "COFFEE")];
    let checking = import_file(
        &mut u,
        &request(
            &ofx("BANK A", &[("XXXX1234", row)]),
            "c.ofx",
            Target::Auto,
            &profiles,
        ),
    )
    .unwrap()
    .statements[0]
        .account_id;
    let card_ofx = String::from_utf8(ofx("BANK A", &[("XXXX1234", row)]))
        .unwrap()
        .replace("<BANKMSGSRSV1>", "<CREDITCARDMSGSRSV1>")
        .replace("</BANKMSGSRSV1>", "</CREDITCARDMSGSRSV1>")
        .replace("<STMTTRNRS><STMTRS>", "<CCSTMTTRNRS><CCSTMTRS>")
        .replace("</STMTRS></STMTTRNRS>", "</CCSTMTRS></CCSTMTTRNRS>")
        .replace(
            "<BANKACCTFROM><BANKID>999999001<ACCTID>XXXX1234<ACCTTYPE>CHECKING</BANKACCTFROM>",
            "<CCACCTFROM><ACCTID>XXXX1234</CCACCTFROM>",
        );
    let card = import_file(
        &mut u,
        &request(card_ofx.as_bytes(), "k.ofx", Target::Auto, &profiles),
    )
    .unwrap()
    .statements[0]
        .account_id;
    assert_ne!(checking, card);
    assert_eq!(
        u.account(card).unwrap().unwrap().kind,
        AccountKind::CreditCard
    );
    assert_eq!(
        u.account(checking).unwrap().unwrap().kind,
        AccountKind::Checking
    );
}
