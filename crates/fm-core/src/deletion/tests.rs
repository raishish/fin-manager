//! Deletion tests on real encrypted ledgers.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, UNIX_EPOCH};

use zeroize::Zeroizing;

use super::*;
use crate::model::round_trip_tests::sample_ledger;
use crate::model::{Ledger, Transaction};
use crate::storage::KdfParams;
use crate::unlock::{RecoveryToConfirm, TouchIdError};

const P: KdfParams = KdfParams::INSECURE_TEST;
const PASSWORD: &str = "velvet anchor mosaic tundra";

fn ledger_with(data: &Ledger) -> (tempfile::TempDir, Unlocked) {
    let dir = tempfile::tempdir().unwrap();
    let mut u = Vault::at(&dir.path().join("ledger"))
        .create_with_params(PASSWORD.as_bytes(), P)
        .unwrap();
    u.save_ledger(data).unwrap();
    (dir, u)
}

fn td1_ledgers() -> Vec<Ledger> {
    let data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/synthetic/data");
    let mut out = Vec::new();
    for entry in fs::read_dir(&data).unwrap() {
        let path = entry.unwrap().path().join("ledger.json");
        if path.exists() {
            out.push(serde_json::from_slice(&fs::read(&path).unwrap()).unwrap());
        }
    }
    assert!(out.len() >= 4, "TD.1 ledgers not found");
    out
}

// ---- the plan: an independent oracle over the ledger data -----------------------------

fn date_of(t: &Transaction) -> Date {
    t.transaction_date.unwrap_or(t.posted_date)
}

fn in_scope(t: &Transaction, scope: Scope) -> bool {
    scope.account.is_none_or(|a| t.account_id == a)
        && scope.from.is_none_or(|f| date_of(t) >= f)
        && scope.to.is_none_or(|to| date_of(t) <= to)
}

/// What `plan` should return, computed in Rust from the ledger value.
fn expected(data: &Ledger, target: Target, partial: PartialFiles) -> (Plan, Vec<PartialFile>) {
    let (scope, account) = match target {
        Target::Account(a) => (
            Scope {
                account: Some(a),
                ..Scope::default()
            },
            Some(a),
        ),
        Target::Range { account, from, to } => (
            Scope {
                account,
                from: Some(from),
                to: Some(to),
            },
            None,
        ),
        Target::Import(_) => unreachable!(),
    };
    let mut imports = Vec::new();
    let mut partials = Vec::new();
    for import in &data.imports {
        let txs: Vec<&Transaction> = data
            .transactions
            .iter()
            .filter(|t| t.source.import_id == import.id)
            .collect();
        let inside = txs.iter().filter(|t| in_scope(t, scope)).count() as u64;
        let outside = txs.len() as u64 - inside;
        let owned = account.is_some() && import.account_id == account;
        if inside == 0 && !owned {
            continue;
        }
        let whole = outside == 0;
        let forced = !whole && owned;
        if !whole {
            partials.push(PartialFile {
                import: import.id,
                file_name: import.file_name.clone(),
                in_scope: inside,
                out_of_scope: outside,
                forced,
            });
        }
        if whole || forced || partial == PartialFiles::DeleteWhole {
            imports.push(import.id);
        }
    }
    imports.sort();
    partials.sort_by_key(|p| p.import);
    let plan = Plan {
        transactions: Some(scope),
        imports,
        accounts: account.into_iter().collect(),
    };
    (plan, partials)
}

fn targets(data: &Ledger) -> Vec<Target> {
    let mut dates: Vec<Date> = data.transactions.iter().map(date_of).collect();
    dates.sort();
    let mut out: Vec<Target> = data
        .accounts
        .iter()
        .map(|a| Target::Account(a.id))
        .collect();
    if let (Some(&first), Some(&last)) = (dates.first(), dates.last()) {
        let mid = dates[dates.len() / 2];
        let quarter = dates[dates.len() / 4];
        out.push(Target::Range {
            account: None,
            from: first,
            to: last,
        });
        out.push(Target::Range {
            account: None,
            from: quarter,
            to: mid,
        });
        out.push(Target::Range {
            account: None,
            from: mid,
            to: mid,
        });
        for a in &data.accounts {
            out.push(Target::Range {
                account: Some(a.id),
                from: quarter,
                to: mid,
            });
        }
    }
    out
}

#[test]
fn the_plan_matches_an_independent_computation() {
    let mut ledgers = td1_ledgers();
    ledgers.push(sample_ledger());
    let mut checked = 0;
    let mut partial_seen = 0;
    for data in &ledgers {
        let (_dir, u) = ledger_with(data);
        for target in targets(data) {
            for partial in [PartialFiles::Keep, PartialFiles::DeleteWhole] {
                let (mut plan, mut partials) = plan(&u, target, partial).unwrap();
                plan.imports.sort();
                partials.sort_by_key(|p| p.import);
                partial_seen += partials.len();
                assert_eq!(
                    (plan, partials),
                    expected(data, target, partial),
                    "{target:?} {partial:?}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 40, "{checked}");
    assert!(
        partial_seen > 0,
        "the cases include files only partly in scope"
    );
}

#[test]
fn a_whole_range_takes_every_file_and_no_partials() {
    let data = sample_ledger();
    let (_dir, u) = ledger_with(&data);
    let dates: BTreeSet<Date> = data.transactions.iter().map(date_of).collect();
    let target = Target::Range {
        account: None,
        from: *dates.first().unwrap(),
        to: *dates.last().unwrap(),
    };
    let (plan, partials) = plan(&u, target, PartialFiles::Keep).unwrap();
    assert!(partials.is_empty());
    let with_rows: BTreeSet<ImportId> = data
        .transactions
        .iter()
        .map(|t| t.source.import_id)
        .collect();
    assert_eq!(
        plan.imports.iter().copied().collect::<BTreeSet<_>>(),
        with_rows
    );
}

#[test]
fn unknown_targets_and_empty_ranges_are_refused() {
    let (_dir, u) = ledger_with(&sample_ledger());
    assert!(matches!(
        plan(&u, Target::Import(ImportId(999)), PartialFiles::Keep),
        Err(DeletionError::NotFound)
    ));
    assert!(matches!(
        plan(&u, Target::Account(AccountId(999)), PartialFiles::Keep),
        Err(DeletionError::NotFound)
    ));
    let d = |s: &str| s.parse::<Date>().unwrap();
    assert!(matches!(
        plan(
            &u,
            Target::Range {
                account: None,
                from: d("2026-02-01"),
                to: d("2026-01-01")
            },
            PartialFiles::Keep
        ),
        Err(DeletionError::EmptyRange)
    ));
    let (plan, partials) = plan(&u, Target::Import(ImportId(1)), PartialFiles::Keep).unwrap();
    assert_eq!(plan.imports, [ImportId(1)]);
    assert!(plan.transactions.is_none() && partials.is_empty());
}

// ---- delete everything ------------------------------------------------------------------

#[derive(Clone, Default)]
struct Keychain {
    item: Rc<RefCell<Option<[u8; 32]>>>,
    fail_delete: Rc<RefCell<bool>>,
    /// Like an unsigned build: can't see items a signed build stored.
    blind: Rc<RefCell<bool>>,
}

impl TouchIdKeychain for Keychain {
    fn available(&self) -> bool {
        true
    }
    fn store(&self, key: &[u8; 32]) -> Result<(), TouchIdError> {
        *self.item.borrow_mut() = Some(*key);
        Ok(())
    }
    fn load(&self, _: &str) -> Result<Zeroizing<[u8; 32]>, TouchIdError> {
        self.item
            .borrow()
            .map(Zeroizing::new)
            .ok_or(TouchIdError::Unavailable)
    }
    fn delete(&self) -> Result<(), TouchIdError> {
        if *self.fail_delete.borrow() {
            return Err(TouchIdError::Other("locked keychain".into()));
        }
        if !*self.blind.borrow() {
            *self.item.borrow_mut() = None;
        }
        Ok(())
    }
    fn delete_verified(&self) -> Result<KeychainDeletion, TouchIdError> {
        self.delete()?;
        Ok(if *self.blind.borrow() {
            KeychainDeletion::Unverifiable
        } else {
            KeychainDeletion::Done
        })
    }
}

struct Install {
    root: PathBuf,
    _tmp: tempfile::TempDir,
    keychain: Keychain,
    session: Session<VaultStore, Keychain>,
}

fn now() -> Now {
    Now::current()
}

fn confirm(session: &mut Session<VaultStore, Keychain>, shown: &RecoveryToConfirm) {
    let answers: Vec<String> = shown
        .challenge
        .iter()
        .map(|&i| shown.groups[i].to_string())
        .collect();
    let answers: Vec<&str> = answers.iter().map(String::as_str).collect();
    session.confirm_recovery_key(&answers).unwrap();
}

/// An unlocked install with data, a Touch ID copy of the key, a backup folder with one
/// backup, and the recovery key's text.
fn install() -> (Install, String) {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let keychain = Keychain::default();
    let vault = Vault::at(&root.join("ledger"));
    let mut session = Session::new(
        VaultStore::insecure_for_tests(vault),
        keychain.clone(),
        now(),
    );
    let shown = session.create(PASSWORD, now()).unwrap();
    let groups: Vec<&str> = shown.groups.iter().map(|g| g.as_str()).collect();
    let recovery = groups.join("-");
    confirm(&mut session, &shown);
    let ledger = session.ledger().unwrap().ledger();
    ledger
        .connection()
        .execute(
            "INSERT INTO meta (key, value) VALUES ('note', 'PLAINTEXT-MARKER-5e7c')",
            [],
        )
        .unwrap();
    let folder = root.join("backups");
    fs::create_dir(&folder).unwrap();
    backup::set_folder(ledger, &folder).unwrap();
    backup::back_up(
        ledger,
        &folder,
        UNIX_EPOCH + Duration::from_secs(1_792_152_000),
    )
    .unwrap();
    assert!(keychain.item.borrow().is_some(), "Touch ID copy stored");
    (
        Install {
            root,
            _tmp: tmp,
            keychain,
            session,
        },
        recovery,
    )
}

#[test]
fn deleting_everything_destroys_the_keys_first_and_leftovers_stay_unreadable() {
    let (mut inst, recovery) = install();
    let vault = inst.session.store().vault().clone();
    let leftover = fs::read(vault.db_path()).unwrap(); // e.g. a snapshot or synced copy
    STEPS.with(|s| s.borrow_mut().clear());

    let done = delete_everything(&mut inst.session, now(), false).unwrap();

    assert_eq!(
        STEPS.with(|s| s.borrow().clone()),
        ["forget_touch_id", "lock", "shred_keys", "remove_files"],
        "keys before files"
    );
    assert!(inst.keychain.item.borrow().is_none(), "Touch ID copy gone");
    assert!(!vault.dir().exists(), "ledger folder gone");
    assert_eq!(inst.session.status(), Status::NoLedger);

    // The backup offer was read before the ledger went.
    let offer = done.backups.expect("one backup to offer");
    assert_eq!(offer.count, 1);
    assert!(offer.message().contains("older backups still contain"));
    assert_eq!(delete_backups(&offer).unwrap(), 1);

    // A leftover copy of the database can't be opened by anything the user still has.
    fs::create_dir(vault.dir()).unwrap();
    fs::write(vault.db_path(), &leftover).unwrap();
    assert_eq!(vault.state(), crate::storage::VaultState::Orphaned);
    assert!(vault.unlock(PASSWORD.as_bytes()).is_err());
    let key = crate::unlock::RecoveryKey::parse(&recovery).unwrap();
    assert!(vault.unlock_with_recovery_key(key.as_bytes()).is_err());
    let marker = b"PLAINTEXT-MARKER-5e7c";
    assert!(
        !leftover.windows(marker.len()).any(|w| w == marker),
        "the database file is encrypted"
    );
    let _ = inst.root;
}

#[test]
fn nothing_is_destroyed_if_the_keychain_copy_cant_be_removed() {
    let (mut inst, _) = install();
    *inst.keychain.fail_delete.borrow_mut() = true;
    let err = delete_everything(&mut inst.session, now(), false).unwrap_err();
    assert!(matches!(err, DeletionError::KeychainNotCleared));
    let vault = inst.session.store().vault().clone();
    assert_eq!(vault.state(), crate::storage::VaultState::Ready);
    assert_eq!(inst.session.status(), Status::Unlocked, "still open");
    // Once the Keychain cooperates, it works.
    *inst.keychain.fail_delete.borrow_mut() = false;
    delete_everything(&mut inst.session, now(), false).unwrap();
    assert!(!vault.dir().exists());
}

#[test]
fn deleting_everything_needs_an_open_ledger() {
    let (mut inst, _) = install();
    inst.session.lock(now());
    assert!(matches!(
        delete_everything(&mut inst.session, now(), false),
        Err(DeletionError::NotUnlocked)
    ));
    assert!(inst.session.store().vault().dir().exists());
}

#[test]
fn the_backup_offer_counts_only_this_ledgers_backups() {
    let (inst, _) = install();
    let ledger = inst.session.ledger().unwrap().ledger();
    let offer = backup_offer(ledger).unwrap();
    assert_eq!(offer.count, 1);
    assert_eq!(offer.ledger_id, backup::ledger_id(ledger).unwrap());
    // Another ledger's backup in the same folder isn't counted or deleted.
    let other_dir = inst.root.join("other");
    let other = Vault::at(&other_dir)
        .create_with_params(PASSWORD.as_bytes(), P)
        .unwrap();
    backup::back_up(
        &other,
        &offer.folder,
        UNIX_EPOCH + Duration::from_secs(1_792_160_000),
    )
    .unwrap();
    assert_eq!(backup_offer(ledger).unwrap().count, 1);
    assert_eq!(delete_backups(&offer).unwrap(), 1);
    assert_eq!(
        backup::list(&offer.folder).unwrap().len(),
        1,
        "the other's stays"
    );
}

/// The coordinator's case: a signed build stored the Touch ID item; an unsigned build
/// can't see it. Nothing is destroyed until the user, told so, chooses to go ahead.
#[test]
fn an_unverifiable_keychain_needs_the_users_go_ahead() {
    let (mut inst, _) = install();
    *inst.keychain.blind.borrow_mut() = true;
    let vault = inst.session.store().vault().clone();

    let err = delete_everything(&mut inst.session, now(), false).unwrap_err();
    assert!(matches!(err, DeletionError::KeychainUnverifiable));
    assert!(err.to_string().contains("Keychain Access"));
    assert_eq!(
        vault.state(),
        crate::storage::VaultState::Ready,
        "nothing destroyed"
    );
    assert_eq!(inst.session.status(), Status::Unlocked);

    let done = delete_everything(&mut inst.session, now(), true).unwrap();
    assert!(done.keychain_unverified);
    assert!(!vault.dir().exists());
    assert!(
        inst.keychain.item.borrow().is_some(),
        "the item this build couldn't see is still there, as the user was told"
    );
}

// ---- the cascade (T0.10 underneath): done when ---------------------------------------

/// Ids of every row, by table, read straight from the database.
#[derive(Debug, Default, PartialEq, Eq)]
struct Rows {
    transactions: BTreeSet<i64>,
    decisions: BTreeSet<i64>,
    rules: BTreeSet<i64>,
    imports: BTreeSet<i64>,
    accounts: BTreeSet<i64>,
}

fn ids(u: &Unlocked, table: &str) -> BTreeSet<i64> {
    let mut stmt = u
        .connection()
        .prepare(&format!("SELECT id FROM {table}"))
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn rows(u: &Unlocked) -> Rows {
    Rows {
        transactions: ids(u, "transactions"),
        decisions: ids(u, "decisions"),
        rules: ids(u, "rules"),
        imports: ids(u, "raw_imports"),
        accounts: ids(u, "accounts"),
    }
}

fn gone(before: &BTreeSet<i64>, after: &BTreeSet<i64>) -> BTreeSet<i64> {
    before.difference(after).copied().collect()
}

fn ids_where(u: &Unlocked, sql: &str) -> BTreeSet<i64> {
    let mut stmt = u.connection().prepare(sql).unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn list(ids: &BTreeSet<i64>) -> String {
    let v: Vec<String> = ids.iter().map(i64::to_string).collect();
    format!("({})", v.join(","))
}

/// Checks one deletion against an untouched copy of the same ledger:
/// - the preview's counts are exactly what the deletion removed, and the preview
///   changed nothing;
/// - every transaction in scope, and every transaction of a file deleted whole, is
///   gone; every other transaction is still there, unchanged;
/// - every removed decision and rule was derived from removed data, and nothing
///   derived from removed data is left (the foreign-key check passes);
/// - surviving rows are unchanged, apart from a `replaces` pointing at a removed
///   decision being cleared;
/// - rows deleted from kept files are recorded (decision 43).
fn check_deletion(data: &Ledger, target: Target, partial: PartialFiles) -> Seen {
    let (_ref_dir, reference) = ledger_with(data);
    let (_dir, mut u) = ledger_with(data);
    let before = u.load_ledger().unwrap();

    let preview = preview(&mut u, target, partial).unwrap();
    assert_eq!(
        u.load_ledger().unwrap(),
        before,
        "the preview changed nothing"
    );
    let (plan, partial_files) = plan(&u, target, partial).unwrap();
    let deleted = delete(&mut u, target, partial).unwrap();
    assert_eq!(
        deleted.counts, preview.counts,
        "{target:?}: preview == deletion"
    );

    let (b, a) = (rows(&reference), rows(&u));
    let tx_gone = gone(&b.transactions, &a.transactions);
    let decisions_gone = gone(&b.decisions, &a.decisions);
    let rules_gone = gone(&b.rules, &a.rules);
    let c = deleted.counts;
    assert_eq!(c.transactions as usize, tx_gone.len());
    assert_eq!(c.decisions as usize, decisions_gone.len());
    assert_eq!(c.rules as usize, rules_gone.len());
    assert_eq!(c.imports as usize, gone(&b.imports, &a.imports).len());
    assert_eq!(c.accounts as usize, gone(&b.accounts, &a.accounts).len());
    assert_eq!(
        gone(&b.imports, &a.imports),
        plan.imports.iter().map(|i| i.0).collect()
    );

    // Transactions: exactly those in scope or in a file deleted whole.
    let whole: BTreeSet<ImportId> = plan.imports.iter().copied().collect();
    let expected_gone: BTreeSet<i64> = data
        .transactions
        .iter()
        .filter(|t| {
            plan.transactions.is_some_and(|s| in_scope(t, s)) || whole.contains(&t.source.import_id)
        })
        .map(|t| t.id.0)
        .collect();
    assert_eq!(tx_gone, expected_gone, "{target:?}");

    // Every removed decision and rule came from removed data.
    if !decisions_gone.is_empty() || !rules_gone.is_empty() {
        let (tx, rules) = (list(&tx_gone), list(&rules_gone));
        let accounts = list(&gone(&b.accounts, &a.accounts));
        let derived_rules = ids_where(
            &reference,
            &format!(
                "SELECT id FROM rules WHERE origin_transaction_id IN {tx}
                 OR bucket_transaction_id IN {tx} OR account_id IN {accounts}
                 OR origin_decision_id IN {}",
                list(&decisions_gone)
            ),
        );
        assert!(rules_gone.is_subset(&derived_rules), "{target:?}: rules");
        let derived_decisions = ids_where(
            &reference,
            &format!(
                "SELECT id FROM decisions WHERE transaction_id IN {tx}
                 OR bucket_transaction_id IN {tx} OR rule_id IN {rules}"
            ),
        );
        assert!(
            decisions_gone.is_subset(&derived_decisions),
            "{target:?}: decisions"
        );
    }
    let dangling: i64 = u
        .connection()
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(dangling, 0, "nothing derived from removed data is left");

    // Survivors are unchanged (a `replaces` to a removed decision is cleared).
    let after = u.load_ledger().unwrap();
    for t in &after.transactions {
        assert_eq!(Some(t), before.transactions.iter().find(|x| x.id == t.id));
    }
    for r in &after.rules {
        assert_eq!(Some(r), before.rules.iter().find(|x| x.id == r.id));
    }
    for i in &after.imports {
        assert_eq!(Some(i), before.imports.iter().find(|x| x.id == i.id));
    }
    for acc in &after.accounts {
        assert_eq!(Some(acc), before.accounts.iter().find(|x| x.id == acc.id));
    }
    let mut cleared = 0;
    for d in &after.decisions {
        let was = before.decisions.iter().find(|x| x.id == d.id).unwrap();
        let replaces_cleared = was.replaces.is_some_and(|r| decisions_gone.contains(&r.0));
        cleared += u64::from(replaces_cleared);
        let expected = crate::model::Decision {
            replaces: if replaces_cleared { None } else { was.replaces },
            ..was.clone()
        };
        assert_eq!(*d, expected);
    }

    // Kept files remember their deleted rows, so they never come back (decision 43).
    let mut recorded = 0;
    for p in &partial_files {
        if whole.contains(&p.import) {
            continue;
        }
        let deleted_rows = u.deleted_rows(p.import).unwrap();
        let expected: Vec<u32> = {
            let mut v: Vec<u32> = data
                .transactions
                .iter()
                .filter(|t| t.source.import_id == p.import && tx_gone.contains(&t.id.0))
                .map(|t| t.source.index)
                .collect();
            v.sort_unstable();
            v
        };
        assert_eq!(
            deleted_rows, expected,
            "{target:?}: rows recorded for kept file"
        );
        recorded += deleted_rows.len() as u64;
    }
    assert_eq!(c.recorded_rows, recorded);
    Seen {
        decisions: c.decisions,
        rules: c.rules,
        recorded_rows: c.recorded_rows,
        replaces_cleared: cleared,
    }
}

/// What the cases exercised, so a test can't pass by never meeting the hard parts.
#[derive(Debug, Default)]
struct Seen {
    decisions: u64,
    rules: u64,
    recorded_rows: u64,
    replaces_cleared: u64,
}

impl Seen {
    fn add(&mut self, o: Seen) {
        self.decisions += o.decisions;
        self.rules += o.rules;
        self.recorded_rows += o.recorded_rows;
        self.replaces_cleared += o.replaces_cleared;
    }
}

#[test]
fn cascades_on_the_td1_ledgers() {
    let mut cases = 0;
    let mut seen = Seen::default();
    for data in td1_ledgers() {
        for target in targets(&data) {
            for partial in [PartialFiles::Keep, PartialFiles::DeleteWhole] {
                seen.add(check_deletion(&data, target, partial));
                cases += 1;
            }
        }
        for import in data.imports.iter().take(3) {
            seen.add(check_deletion(
                &data,
                Target::Import(import.id),
                PartialFiles::Keep,
            ));
            cases += 1;
        }
    }
    assert!(cases > 40, "{cases}");
    // TD.1 holds transactions and files only (no decisions or rules): these cases
    // cover the transaction, file and recorded-row parts; the sample covers the rest.
    assert!(seen.recorded_rows > 0, "{seen:?}");
}

#[test]
fn cascades_on_the_sample_ledger() {
    // The sample has a transfer pair across accounts, a refund, rules learned from
    // corrections, and a decision that replaces another.
    let data = sample_ledger();
    let mut seen = Seen::default();
    for target in targets(&data) {
        for partial in [PartialFiles::Keep, PartialFiles::DeleteWhole] {
            seen.add(check_deletion(&data, target, partial));
        }
    }
    for import in &data.imports {
        seen.add(check_deletion(
            &data,
            Target::Import(import.id),
            PartialFiles::Keep,
        ));
    }
    assert!(seen.decisions > 0 && seen.rules > 0, "{seen:?}");
}

#[test]
fn deleting_one_side_of_a_transfer_keeps_the_other() {
    let data = sample_ledger();
    let (_dir, mut u) = ledger_with(&data);
    // Transfer decisions point at their counterpart transaction.
    let pairs: Vec<(i64, i64)> = {
        let mut stmt = u
            .connection()
            .prepare(
                "SELECT d.transaction_id, d.bucket_transaction_id FROM decisions d
                 WHERE d.bucket = 'transfer' AND d.bucket_transaction_id IS NOT NULL",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let &(this, other) = pairs.first().expect("the sample has a transfer pair");
    let other_account: i64 = u
        .connection()
        .query_row(
            "SELECT account_id FROM transactions WHERE id = ?1",
            [other],
            |r| r.get(0),
        )
        .unwrap();
    delete(
        &mut u,
        Target::Account(AccountId(other_account)),
        PartialFiles::Keep,
    )
    .unwrap();
    let left = rows(&u);
    assert!(!left.transactions.contains(&other));
    assert!(left.transactions.contains(&this), "the other side stays");
    let pointing: i64 = u
        .connection()
        .query_row(
            "SELECT count(*) FROM decisions WHERE bucket_transaction_id = ?1",
            [other],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pointing, 0, "no decision points at the deleted side");
}

#[test]
fn a_deletion_offers_to_delete_old_backups() {
    let data = sample_ledger();
    let (dir, mut u) = ledger_with(&data);
    let folder = fs::canonicalize(dir.path()).unwrap().join("backups");
    fs::create_dir(&folder).unwrap();
    backup::set_folder(&u, &folder).unwrap();
    backup::back_up(&u, &folder, UNIX_EPOCH + Duration::from_secs(1_792_152_000)).unwrap();
    let deleted = delete(&mut u, Target::Import(ImportId(1)), PartialFiles::Keep).unwrap();
    let offer = deleted.backups.expect("an offer");
    assert_eq!(offer.count, 1);
    // Without backups there is nothing to offer.
    let (_d2, mut bare) = ledger_with(&data);
    let deleted = delete(&mut bare, Target::Import(ImportId(1)), PartialFiles::Keep).unwrap();
    assert!(deleted.backups.is_none());
}

/// A decision on a kept transaction that replaced one made by a rule learned from a
/// deleted file: the rule and its decision go, the user's decision stays with its
/// `replaces` cleared (the sample's own chains stay within one transaction).
#[test]
fn a_replaces_chain_across_the_deletion_is_cleared_not_deleted() {
    use crate::model::{
        Assignment, Decision, DecisionId, DecisionSource, DecisionState, RuleId, Timestamp,
        TransactionId,
    };
    let mut data = sample_ledger();
    let taxonomy_version = data.decisions[0].taxonomy_version.clone();
    let by_rule = Decision {
        id: DecisionId(12),
        transaction_id: TransactionId(1),
        assignment: Assignment::Merchant("Acme payroll".into()),
        source: DecisionSource::Rule { rule_id: RuleId(1) },
        taxonomy_version,
        decided_at: Timestamp::from_unix_seconds(1_760_003_000),
        state: DecisionState::Superseded,
        seen_at: None,
        replaces: None,
    };
    let by_user = Decision {
        id: DecisionId(13),
        source: DecisionSource::User,
        assignment: Assignment::Merchant("ACME Corp".into()),
        state: DecisionState::Applied,
        replaces: Some(DecisionId(12)),
        ..by_rule.clone()
    };
    data.decisions.push(by_rule);
    data.decisions.push(by_user);
    // Rule 1 was learned from a transaction in import 2.
    let seen = check_deletion(&data, Target::Import(ImportId(2)), PartialFiles::Keep);
    assert!(seen.replaces_cleared > 0, "{seen:?}");
}
