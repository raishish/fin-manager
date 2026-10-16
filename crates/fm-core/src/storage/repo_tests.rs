//! Repository tests: every T0.5 type through a real encrypted database, append-only
//! decisions, and cascading import deletion (SPEC decision 28).

use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use super::*;
use crate::model::round_trip_tests::sample_ledger;
use crate::model::*;

const P: KdfParams = KdfParams::INSECURE_TEST;
const PASSWORD: &[u8] = b"correct horse battery staple";

fn new_ledger() -> (TempDir, Unlocked) {
    let dir = tempfile::tempdir().unwrap();
    let u = Vault::at(&dir.path().join("ledger"))
        .create_with_params(PASSWORD, P)
        .unwrap();
    (dir, u)
}

fn with_sample() -> (TempDir, Unlocked) {
    let (dir, mut u) = new_ledger();
    u.save_ledger(&sample_ledger()).unwrap();
    (dir, u)
}

fn td1_ledgers() -> Vec<(String, Ledger)> {
    let data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/synthetic/data");
    let mut out = Vec::new();
    for entry in fs::read_dir(&data).unwrap() {
        let path = entry.unwrap().path().join("ledger.json");
        if path.exists() {
            let ledger = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            out.push((path.display().to_string(), ledger));
        }
    }
    assert!(
        out.len() >= 4,
        "TD.1 ledgers not found under {}",
        data.display()
    );
    out
}

fn sql_fails(u: &Unlocked, sql: &str) -> bool {
    u.connection().execute_batch(sql).is_err()
}

#[test]
fn sample_ledger_round_trips_through_the_database() {
    let sample = sample_ledger();
    let (_dir, mut u) = new_ledger();
    u.save_ledger(&sample).unwrap();
    assert_eq!(u.load_ledger().unwrap(), sample);

    // And again after closing and reopening the encrypted file.
    let vault = u.lock();
    let u = vault.unlock(PASSWORD).unwrap();
    assert_eq!(u.load_ledger().unwrap(), sample);
}

#[test]
fn td1_ledgers_round_trip_through_the_database() {
    for (name, ledger) in td1_ledgers() {
        let (_dir, mut u) = new_ledger();
        u.save_ledger(&ledger).unwrap();
        assert_eq!(u.load_ledger().unwrap(), ledger, "{name}");
    }
}

#[test]
fn single_row_reads_and_queries() {
    let sample = sample_ledger();
    let (_dir, u) = with_sample();
    for a in &sample.accounts {
        assert_eq!(u.account(a.id).unwrap().as_ref(), Some(a));
    }
    for r in &sample.imports {
        assert_eq!(u.raw_import(r.id).unwrap().as_ref(), Some(r));
        assert!(u.imports_with_sha256(r.sha256()).unwrap().contains(&r.id));
    }
    for t in &sample.transactions {
        assert_eq!(u.transaction(t.id).unwrap().as_ref(), Some(t));
    }
    for d in &sample.decisions {
        assert_eq!(u.decision(d.id).unwrap().as_ref(), Some(d));
    }
    assert_eq!(u.account(AccountId(999)).unwrap(), None);
    assert_eq!(u.transaction(TransactionId(999)).unwrap(), None);

    let card: Vec<_> = u.transactions_for_account(AccountId(2)).unwrap();
    assert!(card.iter().all(|t| t.account_id == AccountId(2)));
    assert!(
        card.windows(2)
            .all(|w| w[0].posted_date <= w[1].posted_date)
    );
    let about_2: Vec<_> = u
        .decisions_for_transaction(TransactionId(2))
        .unwrap()
        .into_iter()
        .map(|d| d.id.0)
        .collect();
    assert_eq!(about_2, [2, 3, 4]);

    assert_eq!(u.next_id(Table::Transactions).unwrap(), 8);
    assert_eq!(u.next_id(Table::Decisions).unwrap(), 12);
    assert_eq!(u.next_id(Table::Rules).unwrap(), 4);
}

#[test]
fn new_ledgers_start_with_the_default_taxonomy() {
    let (_dir, u) = new_ledger();
    let t = u.taxonomy().unwrap();
    assert_eq!(t, default_taxonomy());
    t.validate().unwrap();
    assert_eq!((t.groups.len(), t.categories.len()), (17, 66));
    assert_eq!(t.version.to_string(), "1+r0");
}

#[test]
fn save_ledger_needs_an_empty_ledger() {
    let (_dir, mut u) = with_sample();
    assert!(matches!(
        u.save_ledger(&sample_ledger()),
        Err(StorageError::AlreadyExists)
    ));
}

#[test]
fn taxonomy_edits_are_saved_and_referenced_categories_are_kept() {
    let (_dir, mut u) = new_ledger();
    let mut t = u.taxonomy().unwrap();
    let groceries = t.categories[0].id;
    let restaurants = t.categories[1].id;

    // Rename, hide, merge and bump the revision.
    t.version.revision += 1;
    t.categories[0].name = "Supermarket".into();
    t.groups[0].hidden = true;
    t.categories[1].merged_into = Some(groceries);
    u.save_taxonomy(&t).unwrap();
    assert_eq!(u.taxonomy().unwrap(), t);

    // Swapping two keys works (keys are unique, so this needs the placeholder step).
    let mut swapped = t.clone();
    let (k0, k2) = (
        swapped.categories[0].key.clone(),
        swapped.categories[2].key.clone(),
    );
    swapped.categories[0].key = k2;
    swapped.categories[2].key = k0;
    u.save_taxonomy(&swapped).unwrap();
    assert_eq!(u.taxonomy().unwrap(), swapped);

    // Removing a category nothing uses works.
    let mut fewer = swapped.clone();
    fewer.categories.retain(|c| c.id != restaurants);
    u.save_taxonomy(&fewer).unwrap();
    assert_eq!(u.taxonomy().unwrap(), fewer);

    // Removing one that a decision uses fails, and nothing changes.
    let sample = sample_ledger();
    let mut l = sample.clone();
    l.taxonomy = fewer.clone();
    for d in &mut l.decisions {
        if let Assignment::Bucket(
            Bucket::Category { category_id } | Bucket::Refund { category_id, .. },
        ) = &mut d.assignment
        {
            *category_id = groceries;
        }
    }
    for r in &mut l.rules {
        if let Some(Bucket::Refund { category_id, .. }) = &mut r.action.bucket {
            *category_id = groceries;
        }
    }
    u.save_ledger(&l).unwrap();
    let mut without_groceries = fewer.clone();
    without_groceries
        .categories
        .retain(|c| c.id != groceries && c.merged_into != Some(groceries));
    assert!(u.save_taxonomy(&without_groceries).is_err());
    assert_eq!(u.taxonomy().unwrap(), fewer);

    // Invalid lists are refused before anything is written.
    let mut invalid = fewer.clone();
    invalid.categories[0].group_id = CategoryGroupId(999);
    assert!(matches!(
        u.save_taxonomy(&invalid),
        Err(StorageError::Model(ModelError::InvalidTaxonomy(_)))
    ));
    assert_eq!(u.taxonomy().unwrap(), fewer);
}

#[test]
fn decisions_move_only_along_their_lifecycle() {
    let (_dir, u) = with_sample();
    let state = |id| u.decision(DecisionId(id)).unwrap().unwrap().state;

    // Allowed: applied → superseded; suggested → applied | dismissed | superseded.
    u.set_decision_state(DecisionId(1), DecisionState::Superseded)
        .unwrap();
    assert_eq!(state(1), DecisionState::Superseded);
    u.set_decision_state(DecisionId(10), DecisionState::Applied)
        .unwrap();
    assert_eq!(state(10), DecisionState::Applied);
    // "Moving" to the state it is already in is refused: an update must change something.
    assert!(matches!(
        u.set_decision_state(DecisionId(10), DecisionState::Applied),
        Err(StorageError::ForbiddenChange(_))
    ));

    // Everything else is refused, at the database level.
    for (id, to) in [
        (1, DecisionState::Applied),    // superseded → applied
        (10, DecisionState::Suggested), // applied → suggested
        (10, DecisionState::Dismissed), // applied → dismissed
        (11, DecisionState::Applied),   // dismissed → applied
        (11, DecisionState::Suggested), // dismissed → suggested
        (2, DecisionState::Applied),    // superseded → applied
    ] {
        let before = state(id);
        assert!(
            matches!(
                u.set_decision_state(DecisionId(id), to),
                Err(StorageError::ForbiddenChange(_))
            ),
            "decision {id}: {before:?} → {to:?} allowed"
        );
        assert_eq!(state(id), before);
    }
    assert!(matches!(
        u.set_decision_state(DecisionId(999), DecisionState::Superseded),
        Err(StorageError::NotFound)
    ));

    // seen_at is set once.
    u.mark_decision_seen(DecisionId(4), Timestamp::from_unix_seconds(1_800_000_000))
        .unwrap();
    assert!(matches!(
        u.mark_decision_seen(DecisionId(4), Timestamp::from_unix_seconds(1_800_000_001)),
        Err(StorageError::ForbiddenChange(_))
    ));
    assert_eq!(
        u.decision(DecisionId(4)).unwrap().unwrap().seen_at,
        Some(Timestamp::from_unix_seconds(1_800_000_000))
    );
}

#[test]
fn decision_rows_cannot_be_edited_or_deleted_directly() {
    let (_dir, u) = with_sample();
    let before = u.decisions().unwrap();
    for sql in [
        "UPDATE decisions SET bucket = 'income', bucket_category_id = NULL WHERE id = 3",
        "UPDATE decisions SET merchant = 'Someone else' WHERE id = 4",
        "UPDATE decisions SET source = 'user', model_id = NULL, model_gguf_sha256 = NULL, \
         prompt_version = NULL, confidence_bp = NULL WHERE id = 9",
        "UPDATE decisions SET confidence_bp = 10000 WHERE id = 9",
        "UPDATE decisions SET transaction_id = 1 WHERE id = 3",
        "UPDATE decisions SET decided_at = 0 WHERE id = 3",
        "UPDATE decisions SET taxonomy_revision = 7 WHERE id = 3",
        "UPDATE decisions SET replaces = NULL WHERE id = 3",
        "UPDATE decisions SET seen_at = NULL WHERE id = 2",
        "UPDATE decisions SET id = 100 WHERE id = 3",
        "DELETE FROM decisions WHERE id = 3",
        "DELETE FROM decisions",
        // Deleting what a decision hangs off also goes through the trigger.
        "DELETE FROM transactions WHERE id = 1",
        "DELETE FROM raw_imports WHERE id = 1",
    ] {
        assert!(sql_fails(&u, sql), "allowed: {sql}");
    }
    assert_eq!(u.decisions().unwrap(), before);
}

#[test]
fn raw_import_bytes_are_immutable() {
    let (_dir, u) = with_sample();
    assert!(sql_fails(
        &u,
        "UPDATE raw_imports SET bytes = x'00' WHERE id = 1"
    ));
    assert!(sql_fails(
        &u,
        "UPDATE raw_imports SET sha256 = zeroblob(32) WHERE id = 1"
    ));
    assert!(sql_fails(&u, "UPDATE raw_imports SET id = 99 WHERE id = 3"));
    // Metadata isn't part of the raw layer.
    u.connection()
        .execute_batch("UPDATE raw_imports SET file_name = 'renamed.csv' WHERE id = 1")
        .unwrap();
}

#[test]
fn deleting_an_import_cascades_to_everything_derived_from_it() {
    let (_dir, mut u) = with_sample();
    // A rule learned from import 2 (rule 1, from correcting transaction 2) also made a
    // decision on transaction 1 (import 1), which the user then replaced.
    u.insert_decision(&Decision {
        id: DecisionId(12),
        transaction_id: TransactionId(1),
        assignment: Assignment::Merchant("Acme payroll".into()),
        source: DecisionSource::Rule { rule_id: RuleId(1) },
        taxonomy_version: TaxonomyVersion {
            base: "1".into(),
            revision: 0,
        },
        decided_at: Timestamp::from_unix_seconds(1_760_003_000),
        state: DecisionState::Superseded,
        seen_at: None,
        replaces: None,
    })
    .unwrap();
    let user_fix = Decision {
        id: DecisionId(13),
        source: DecisionSource::User,
        assignment: Assignment::Merchant("ACME Corp".into()),
        state: DecisionState::Applied,
        replaces: Some(DecisionId(12)),
        ..u.decision(DecisionId(12)).unwrap().unwrap()
    };
    u.insert_decision(&user_fix).unwrap();

    // Import 2 holds transactions 2, 4, 5, 6 and 7 (the card).
    let counts = u.delete_import(ImportId(2)).unwrap();
    assert_eq!(
        counts,
        DeletedCounts {
            transactions: 5,
            // 2, 3, 4, 6, 7, 8, 9, 10, 11 are about deleted transactions; 5 is a transfer
            // whose counterpart (4) is gone; 12 was made by the deleted rule.
            decisions: 11,
            rules: 1,
        }
    );
    let tx_ids: Vec<_> = u.transactions().unwrap().iter().map(|t| t.id.0).collect();
    assert_eq!(tx_ids, [1, 3]);
    let decision_ids: Vec<_> = u.decisions().unwrap().iter().map(|d| d.id.0).collect();
    assert_eq!(decision_ids, [1, 13]);
    let rule_ids: Vec<_> = u.rules().unwrap().iter().map(|r| r.id.0).collect();
    assert_eq!(rule_ids, [2, 3], "the correction-learned rule survived");
    assert!(u.raw_import(ImportId(2)).unwrap().is_none());

    // The user's decision stands; only its pointer to the deleted one is cleared.
    let survivor = u.decision(DecisionId(13)).unwrap().unwrap();
    assert_eq!(
        survivor,
        Decision {
            replaces: None,
            ..user_fix
        }
    );

    // The switch is off again, so direct deletes are blocked again.
    assert!(sql_fails(&u, "DELETE FROM decisions WHERE id = 1"));
    let switch: bool = u
        .connection()
        .query_row("SELECT fm_permit('delete')", [], |r| r.get(0))
        .unwrap();
    assert!(!switch);

    // Unknown imports change nothing.
    assert!(matches!(
        u.delete_import(ImportId(2)),
        Err(StorageError::NotFound)
    ));
    assert_eq!(u.decisions().unwrap().len(), 2);
}

#[test]
fn deleting_a_whole_td1_import_leaves_no_orphans() {
    let (_, ledger) = td1_ledgers().remove(0);
    let (_dir, mut u) = new_ledger();
    u.save_ledger(&ledger).unwrap();
    let import = ledger.imports[0].id;
    let expected = ledger
        .transactions
        .iter()
        .filter(|t| t.source.import_id == import)
        .count() as u64;
    let counts = u.delete_import(import).unwrap();
    assert_eq!(counts.transactions, expected);
    assert!(
        u.transactions()
            .unwrap()
            .iter()
            .all(|t| t.source.import_id != import)
    );
    let violations: i64 = u
        .connection()
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
}

#[test]
fn rows_that_break_model_invariants_are_reported() {
    let (_dir, u) = with_sample();
    // Simulate a file edited outside the app: drop the protection (only possible under
    // the schema permit, which nothing outside migrations gets) and corrupt rows.
    let _outside = u.permits.grant(db::Permit::Schema);
    u.connection()
        .execute_batch("DROP TRIGGER transactions_are_immutable")
        .unwrap();
    // 'XXX' passes the column's length check but isn't a currency.
    u.connection()
        .execute_batch("UPDATE transactions SET currency = 'XXX' WHERE id = 1")
        .unwrap();
    assert!(matches!(
        u.transactions(),
        Err(StorageError::InvalidRow {
            table: "transactions",
            reason: "unknown currency"
        })
    ));
    u.connection()
        .execute_batch(
            "UPDATE transactions SET currency = 'USD', posted_date = '2026-02-30' WHERE id = 1",
        )
        .unwrap();
    assert!(matches!(
        u.transaction(TransactionId(1)),
        Err(StorageError::InvalidRow {
            reason: "invalid date",
            ..
        })
    ));
    assert_eq!(
        StorageError::InvalidRow {
            table: "transactions",
            reason: "invalid date"
        }
        .to_string(),
        "invalid row in transactions: invalid date"
    );
}

#[test]
fn schema_rejects_malformed_rows() {
    let (_dir, u) = with_sample();
    for sql in [
        // A category bucket without a category.
        "INSERT INTO decisions (id, transaction_id, assignment, bucket, source, taxonomy_base, \
         taxonomy_revision, decided_at, state) VALUES (50, 1, 'bucket', 'category', 'user', '1', 0, 0, 'applied')",
        // A model decision without its provenance.
        "INSERT INTO decisions (id, transaction_id, assignment, bucket, source, taxonomy_base, \
         taxonomy_revision, decided_at, state) VALUES (51, 1, 'bucket', 'income', 'model', '1', 0, 0, 'applied')",
        // A merchant assignment that also names a bucket.
        "INSERT INTO decisions (id, transaction_id, assignment, bucket, merchant, source, taxonomy_base, \
         taxonomy_revision, decided_at, state) VALUES (52, 1, 'merchant', 'income', 'X', 'user', '1', 0, 0, 'applied')",
        // An unknown state.
        "INSERT INTO decisions (id, transaction_id, assignment, bucket, source, taxonomy_base, \
         taxonomy_revision, decided_at, state) VALUES (53, 1, 'bucket', 'income', 'user', '1', 0, 0, 'maybe')",
        // A transaction for an account that doesn't exist.
        "INSERT INTO transactions (id, account_id, import_id, source_index, status, posted_date, \
         amount_minor, currency, description) VALUES (60, 99, 1, 9, 'posted', '2026-01-01', 1, 'USD', 'x')",
        // Two transactions at the same place in one import.
        "INSERT INTO transactions (id, account_id, import_id, source_index, status, posted_date, \
         amount_minor, currency, description) VALUES (61, 1, 1, 0, 'posted', '2026-01-01', 1, 'USD', 'x')",
        // A foreign amount without its currency.
        "INSERT INTO transactions (id, account_id, import_id, source_index, status, posted_date, \
         amount_minor, currency, original_amount_minor, description) \
         VALUES (62, 1, 1, 9, 'posted', '2026-01-01', 1, 'USD', 5, 'x')",
        // A correction-learned rule that doesn't say what it was learned from.
        "INSERT INTO rules (id, match_kind, match_text, merchant, origin, priority, enabled, created_at) \
         VALUES (70, 'contains', 'X', 'X', 'correction', 0, 1, 0)",
        // A rule that does nothing.
        "INSERT INTO rules (id, match_kind, match_text, origin, priority, enabled, created_at) \
         VALUES (71, 'contains', 'X', 'user', 0, 1, 0)",
    ] {
        assert!(sql_fails(&u, sql), "accepted: {sql}");
    }
}

#[test]
fn touch_id_hooks_unlock_with_the_raw_key() {
    let (_dir, u) = with_sample();
    let key = u.data_key_copy();
    let vault = u.lock();

    let u = vault.unlock_with_data_key(&key).unwrap();
    assert_eq!(u.load_ledger().unwrap(), sample_ledger());
    // Same lock as a password unlock.
    assert!(matches!(
        vault.unlock_with_data_key(&key),
        Err(StorageError::InUse)
    ));
    drop(u);

    let mut wrong = *key;
    wrong[0] ^= 1;
    assert!(matches!(
        vault.unlock_with_data_key(&wrong),
        Err(StorageError::DatabaseKeyRejected)
    ));

    // After a rotation the old copy no longer works.
    let mut u = vault.unlock(PASSWORD).unwrap();
    u.rotate_data_key_with_params(PASSWORD, None, P).unwrap();
    assert_ne!(*u.data_key_copy(), *key);
    let vault = u.lock();
    assert!(matches!(
        vault.unlock_with_data_key(&key),
        Err(StorageError::DatabaseKeyRejected)
    ));
}

#[test]
fn reset_password_with_the_recovery_key() {
    let (_dir, mut u) = new_ledger();
    u.set_recovery_key_with_params(PASSWORD, b"RECOVERY-KEY", P)
        .unwrap();
    let vault = u.lock();

    // The user forgot the password and unlocks with the recovery key.
    let mut u = vault.unlock_with_recovery_key(b"RECOVERY-KEY").unwrap();
    let keyfile_before = fs::read(vault.keyfile_path()).unwrap();
    for wrong in [&b"RECOVERY-KEX"[..], PASSWORD] {
        assert!(matches!(
            u.reset_password_with_params(wrong, b"new password", P),
            Err(StorageError::WrongSecret)
        ));
        assert_eq!(fs::read(vault.keyfile_path()).unwrap(), keyfile_before);
    }
    u.reset_password_with_params(b"RECOVERY-KEY", b"new password", P)
        .unwrap();
    let vault = u.lock();

    assert!(matches!(
        vault.unlock(PASSWORD),
        Err(StorageError::WrongSecret)
    ));
    vault.unlock(b"new password").unwrap().lock();
    vault.unlock_with_recovery_key(b"RECOVERY-KEY").unwrap();

    // Without a recovery key set up, there is nothing to reset with.
    let (_dir2, mut u) = new_ledger();
    assert!(matches!(
        u.reset_password_with_params(b"anything", b"new", P),
        Err(StorageError::NoSuchSlot(SlotKind::RecoveryKey))
    ));
}

/// No SQL can switch the deletion guard on, and a connection without the app's function
/// (any other tool) can't change decisions at all.
#[test]
fn decision_protection_fails_closed() {
    let (_dir, u) = with_sample();
    // There is no table or setting to flip: the function only reads the Rust switch,
    // and SQL can't define or replace functions.
    for kind in ["delete", "post_pending", "anything"] {
        let on: bool = u
            .connection()
            .query_row("SELECT fm_permit(?1)", [kind], |r| r.get(0))
            .unwrap();
        assert!(!on, "{kind}");
    }
    assert!(sql_fails(&u, "DELETE FROM decisions WHERE id = 1"));

    // A raw connection to the same file, with the right key but without the function.
    let raw = rusqlite::Connection::open(u.vault().db_path()).unwrap();
    raw.execute_batch(&u.key.sqlcipher_pragma(keyfile::Pragma::Key))
        .unwrap();
    raw.execute_batch("PRAGMA foreign_keys = ON; PRAGMA recursive_triggers = ON;")
        .unwrap();
    for sql in [
        "UPDATE decisions SET state = 'superseded' WHERE id = 1",
        "DELETE FROM decisions WHERE id = 1",
        "DELETE FROM raw_imports WHERE id = 1",
        "DELETE FROM transactions WHERE id = 1",
        "UPDATE transactions SET amount_minor = 1 WHERE id = 1",
    ] {
        assert!(
            raw.execute_batch(sql).is_err(),
            "raw connection allowed: {sql}"
        );
    }
    drop(raw);
    // The app's own connection still works through the API.
    u.set_decision_state(DecisionId(1), DecisionState::Superseded)
        .unwrap();
}

/// Review 1, item 1: REPLACE's implicit delete goes through the DELETE triggers, so it
/// can't rewrite decisions, swap import bytes or delete transactions.
#[test]
fn replace_cannot_bypass_the_triggers() {
    let (_dir, u) = with_sample();
    let ledger_before = u.load_ledger().unwrap();
    // Decision 4 and import 3 are referenced by nothing, so no foreign-key cascade is
    // involved: only recursive triggers stand between REPLACE and these rows (see the
    // control test below).
    let decision_copy = "SELECT id, 1, 'bucket', 'income', NULL, NULL, NULL, 'user', NULL, NULL, NULL, \
         NULL, NULL, NULL, NULL, '1', 0, 0, 'applied', NULL, NULL FROM decisions WHERE id = 4";
    for sql in [
        format!("REPLACE INTO decisions {decision_copy}"),
        format!("INSERT OR REPLACE INTO decisions {decision_copy}"),
        "REPLACE INTO raw_imports (id, source, imported_at, sha256, bytes) \
         VALUES (3, 'csv', 0, zeroblob(32), x'00')"
            .into(),
        // Import 1 has transactions: replacing it would cascade-delete them.
        "REPLACE INTO raw_imports (id, source, imported_at, sha256, bytes) \
         VALUES (1, 'csv', 0, zeroblob(32), x'00')"
            .into(),
        "INSERT OR REPLACE INTO raw_imports (id, source, imported_at, sha256, bytes) \
         VALUES (2, 'csv', 0, zeroblob(32), x'00')"
            .into(),
        "REPLACE INTO transactions (id, account_id, import_id, source_index, status, posted_date, \
         amount_minor, currency, description) VALUES (1, 1, 1, 0, 'posted', '2026-01-01', 1, 'USD', 'x')"
            .into(),
        // A conflicting UNIQUE (import_id, source_index) also replaces under OR REPLACE.
        "INSERT OR REPLACE INTO transactions (id, account_id, import_id, source_index, status, \
         posted_date, amount_minor, currency, description) \
         VALUES (90, 1, 1, 0, 'posted', '2026-01-01', 1, 'USD', 'x')"
            .into(),
        "UPDATE OR REPLACE transactions SET source_index = 0 WHERE id = 3".into(),
    ] {
        assert!(sql_fails(&u, &sql), "allowed: {sql}");
    }
    assert_eq!(u.load_ledger().unwrap(), ledger_before);
}

/// Review 1, item 2: SQL on the connection can't change the schema, the triggers, the
/// connection's settings or attach other files.
#[test]
fn the_authorizer_refuses_schema_and_setting_changes() {
    let (_dir, u) = with_sample();
    for sql in [
        "DROP TRIGGER decisions_are_append_only",
        "DROP TRIGGER decisions_are_never_deleted",
        "CREATE TRIGGER t BEFORE DELETE ON decisions BEGIN SELECT 1; END",
        "CREATE TEMP TRIGGER t BEFORE DELETE ON decisions BEGIN SELECT 1; END",
        "DROP TABLE decisions",
        "ALTER TABLE decisions RENAME TO old_decisions",
        "ALTER TABLE decisions ADD COLUMN extra TEXT",
        "CREATE TABLE extra (x INTEGER)",
        "CREATE INDEX extra ON decisions (merchant)",
        "DROP INDEX decisions_by_transaction",
        "CREATE VIEW v AS SELECT 1",
        "PRAGMA foreign_keys = OFF",
        "PRAGMA recursive_triggers = OFF",
        "PRAGMA writable_schema = ON",
        "PRAGMA secure_delete = OFF",
        "PRAGMA user_version = 99",
        "PRAGMA key = 'x'",
        "PRAGMA rekey = 'x'",
        "PRAGMA journal_mode = OFF",
        "ATTACH DATABASE ':memory:' AS other",
    ] {
        assert!(sql_fails(&u, sql), "allowed: {sql}");
    }
    // Settings are still on.
    let pragma = |name: &str| -> i64 {
        u.connection()
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(pragma("foreign_keys"), 1);
    assert_eq!(pragma("recursive_triggers"), 1);
    assert_eq!(pragma("user_version"), 2);
    // Reads, temporary tables and defer_foreign_keys are fine.
    u.connection()
        .execute_batch(
            "PRAGMA defer_foreign_keys = ON; CREATE TEMP TABLE scratch (x INTEGER); \
             SELECT * FROM pragma_table_info('decisions');",
        )
        .unwrap();
    // Defensive mode is on.
    assert!(
        u.connection()
            .db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE)
            .unwrap()
    );
}

/// Review 1, item 3: a transaction never changes, except pending → posted through
/// post_pending.
#[test]
fn transactions_are_immutable_except_pending_to_posted() {
    let (_dir, u) = with_sample();
    let before = u.transactions().unwrap();
    for sql in [
        "UPDATE transactions SET amount_minor = 1 WHERE id = 1",
        "UPDATE transactions SET import_id = 2 WHERE id = 1",
        "UPDATE transactions SET description = 'x' WHERE id = 1",
        "UPDATE transactions SET status = 'posted' WHERE id = 6",
        "UPDATE transactions SET id = 99 WHERE id = 1",
    ] {
        assert!(sql_fails(&u, sql), "allowed: {sql}");
    }
    assert_eq!(u.transactions().unwrap(), before);

    // Transaction 6 is pending on the card; the posted record arrives in import 1.
    let pending = u.transaction(TransactionId(6)).unwrap().unwrap();
    assert_eq!(pending.status, TransactionStatus::Pending);
    let posted = Transaction {
        status: TransactionStatus::Posted,
        posted_date: "2026-10-02".parse().unwrap(),
        amount: Money::new(-525, Currency::USD),
        description: "COFFEE ROASTERS".into(),
        source: SourceRef {
            import_id: ImportId(1),
            index: 7,
            external_id: Some("POSTED-6".into()),
        },
        ..pending.clone()
    };
    u.post_pending(&posted).unwrap();
    assert_eq!(u.transaction(TransactionId(6)).unwrap().unwrap(), posted);
    // Only once, and only to posted, and only on its own account.
    assert!(matches!(
        u.post_pending(&posted),
        Err(StorageError::ForbiddenChange(_))
    ));
    let posted_tx1 = Transaction {
        status: TransactionStatus::Posted,
        ..u.transaction(TransactionId(1)).unwrap().unwrap()
    };
    assert!(matches!(
        u.post_pending(&posted_tx1),
        Err(StorageError::ForbiddenChange(_))
    ));
    let mut other_account = posted.clone();
    other_account.id = TransactionId(5);
    other_account.account_id = AccountId(1);
    assert!(matches!(
        u.post_pending(&other_account),
        Err(StorageError::NotFound)
    ));
}

/// Review 1, item 3: once a rule has made decisions, its matcher and action are fixed.
#[test]
fn rules_are_frozen_once_they_have_made_decisions() {
    let (_dir, u) = with_sample();
    // Rule 1 made decision 7; rule 2 made none.
    for sql in [
        "UPDATE rules SET match_text = 'SOMETHING ELSE' WHERE id = 1",
        "UPDATE rules SET bucket = 'income', bucket_category_id = NULL WHERE id = 1",
        "UPDATE rules SET merchant = 'Other' WHERE id = 1",
        "UPDATE rules SET account_id = NULL WHERE id = 1",
        "UPDATE rules SET id = 50 WHERE id = 1",
    ] {
        assert!(sql_fails(&u, sql), "allowed: {sql}");
    }
    u.connection()
        .execute_batch("UPDATE rules SET priority = 99, enabled = 0 WHERE id = 1")
        .unwrap();
    u.connection()
        .execute_batch("UPDATE rules SET match_text = 'NETFLIX' WHERE id = 2")
        .unwrap();
    let rules = u.rules().unwrap();
    assert_eq!((rules[0].priority, rules[0].enabled), (99, false));
    assert_eq!(
        rules[1].matcher.description,
        TextMatch::Equals("NETFLIX".into())
    );
}

/// Review 1, item 4: rules and the category list are validated when read back.
#[test]
fn rules_and_taxonomy_are_validated_on_read() {
    let (_dir, u) = with_sample();
    // A blank pattern passes the column checks but not Rule::validate (rule 2 is unused,
    // so it can still be edited).
    u.connection()
        .execute_batch("UPDATE rules SET match_text = '   ' WHERE id = 2")
        .unwrap();
    assert!(matches!(
        u.rules(),
        Err(StorageError::InvalidRow { table: "rules", .. })
    ));

    // A merge chain passes the foreign keys but not Taxonomy::validate.
    u.connection()
        .execute_batch(
            "INSERT INTO categories (id, group_id, key, name, hidden, merged_into) \
             VALUES (3, 1, 'food.tea', 'Tea', 0, 2)",
        )
        .unwrap();
    assert!(matches!(
        u.taxonomy(),
        Err(StorageError::InvalidRow {
            table: "categories",
            ..
        })
    ));
}

/// Review 1, item 5: an update must change something; seen_at is set exactly once.
#[test]
fn same_value_decision_updates_are_refused() {
    let (_dir, u) = with_sample();
    let at = Timestamp::from_unix_seconds(1_800_000_000);
    u.mark_decision_seen(DecisionId(1), at).unwrap();
    assert!(matches!(
        u.mark_decision_seen(DecisionId(1), at),
        Err(StorageError::ForbiddenChange(_))
    ));
    for sql in [
        "UPDATE decisions SET state = state WHERE id = 1",
        "UPDATE decisions SET seen_at = seen_at WHERE id = 1",
        "UPDATE decisions SET merchant = merchant WHERE id = 4",
    ] {
        assert!(sql_fails(&u, sql), "allowed: {sql}");
    }
    // A transition and the first seen_at together are fine.
    u.connection()
        .execute_batch("UPDATE decisions SET state = 'applied', seen_at = 5 WHERE id = 10")
        .unwrap();
}

/// Control for `replace_cannot_bypass_the_triggers`: with recursive triggers off (the
/// SQLite default), the same REPLACE does rewrite a decision. So the protection really
/// rests on the setting the connection turns on, and the authorizer keeps it on.
#[test]
fn replace_bypass_needs_recursive_triggers_off() {
    let (_dir, u) = with_sample();
    let _schema = u.permits.grant(db::Permit::Schema);
    u.connection()
        .execute_batch("PRAGMA recursive_triggers = OFF")
        .unwrap();
    u.connection()
        .execute_batch(
            "REPLACE INTO decisions SELECT id, 1, 'bucket', 'income', NULL, NULL, NULL, 'user', \
             NULL, NULL, NULL, NULL, NULL, NULL, NULL, '1', 0, 0, 'applied', NULL, NULL \
             FROM decisions WHERE id = 4;
             REPLACE INTO raw_imports (id, source, imported_at, sha256, bytes)
             VALUES (3, 'csv', 0, zeroblob(32), x'00');",
        )
        .unwrap();
    // The bypass the review found: a decision moved to another transaction, and an
    // import's bytes swapped.
    assert_eq!(
        u.decision(DecisionId(4)).unwrap().unwrap().transaction_id,
        TransactionId(1)
    );
    let swapped: Vec<u8> = u
        .connection()
        .query_row("SELECT bytes FROM raw_imports WHERE id = 3", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(swapped, [0]);
}
