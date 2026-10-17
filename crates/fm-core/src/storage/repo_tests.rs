//! Repository tests: every T0.5 type through a real encrypted database, append-only
//! decisions, and cascading import deletion (SPEC decision 28).

use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use super::repo::{DeletionPlan, DeletionScope};
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

/// T0.13 (SPEC decision 46): a user may reject an applied decision they have acted on
/// (seen), which dismisses it. An unseen one can't be dismissed, and the database
/// itself refuses it, whatever SQL asks.
#[test]
fn a_seen_applied_decision_can_be_dismissed_and_only_that() {
    let (_dir, u) = with_sample();
    let decision = |id| u.decision(DecisionId(id)).unwrap().unwrap();
    let seen = Timestamp::from_unix_seconds(1_800_000_000);
    // Decisions 1 and 3 are applied and unseen.
    assert_eq!(decision(1).state, DecisionState::Applied);
    assert_eq!(decision(1).seen_at, None);

    // Unseen: refused through the API and through any SQL, including setting seen_at
    // in the same statement.
    assert!(matches!(
        u.set_decision_state(DecisionId(1), DecisionState::Dismissed),
        Err(StorageError::ForbiddenChange(_))
    ));
    assert!(sql_fails(
        &u,
        "UPDATE decisions SET state = 'dismissed' WHERE id = 1"
    ));
    assert!(sql_fails(
        &u,
        "UPDATE decisions SET state = 'dismissed', seen_at = 1800000000 WHERE id = 1"
    ));
    assert_eq!(decision(1).state, DecisionState::Applied);

    // Seen: allowed, and seen_at stays as it was.
    u.mark_decision_seen(DecisionId(1), seen).unwrap();
    u.set_decision_state(DecisionId(1), DecisionState::Dismissed)
        .unwrap();
    let dismissed = decision(1);
    assert_eq!(dismissed.state, DecisionState::Dismissed);
    assert_eq!(dismissed.seen_at, Some(seen));

    // A dismissed decision goes nowhere, and dismissing can't touch seen_at.
    for to in [
        DecisionState::Applied,
        DecisionState::Suggested,
        DecisionState::Superseded,
        DecisionState::Dismissed,
    ] {
        assert!(matches!(
            u.set_decision_state(DecisionId(1), to),
            Err(StorageError::ForbiddenChange(_))
        ));
    }
    u.mark_decision_seen(DecisionId(3), seen).unwrap();
    assert!(sql_fails(
        &u,
        "UPDATE decisions SET state = 'dismissed', seen_at = 1800000001 WHERE id = 3"
    ));
    assert!(sql_fails(
        &u,
        "UPDATE decisions SET state = 'dismissed', seen_at = NULL WHERE id = 3"
    ));
    // Nor can it change anything else on the way.
    assert!(sql_fails(
        &u,
        "UPDATE decisions SET state = 'dismissed', decided_at = 1 WHERE id = 3"
    ));
    assert_eq!(decision(3).state, DecisionState::Applied);
    u.connection()
        .execute_batch("UPDATE decisions SET state = 'dismissed' WHERE id = 3")
        .unwrap();
    assert_eq!(decision(3).state, DecisionState::Dismissed);

    // The other moves are unchanged: applied → superseded still works, seen or not.
    u.set_decision_state(DecisionId(4), DecisionState::Superseded)
        .unwrap();
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
        (10, DecisionState::Dismissed), // applied → dismissed, never seen
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
            imports: 1,
            accounts: 0,
            recorded_rows: 0,
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
    assert_eq!(pragma("user_version"), MIGRATIONS.len() as i64);
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

/// Migration 3 on a schema-2 file that already holds decisions, a correction-learned
/// rule and the decisions that rule made: nothing is lost or cascaded away by the
/// decisions table rebuild, the protections still hold, and the new columns and decision
/// source work.
#[test]
fn migration_3_keeps_existing_decisions_and_rules() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::at(&dir.path().join("ledger"));
    fs::create_dir_all(vault.dir()).unwrap();
    let key = keyfile::DataKey::generate().unwrap();
    let mut kf = keyfile::Keyfile::new().unwrap();
    kf.set_slot(SlotKind::Password, PASSWORD, P, &key).unwrap();
    kf.write_atomic(vault.keyfile_path()).unwrap();
    let (mut conn, permits) = db::open_encrypted(vault.db_path(), &key, true).unwrap();
    assert_eq!(
        db::migrate(&mut conn, &permits, &MIGRATIONS[..2]).unwrap(),
        2
    );
    repo::seed_default_taxonomy(&mut conn).unwrap();
    // A schema-2 ledger, written with schema-2 columns: two transactions, a user
    // correction, a rule learned from it, a decision by that rule, and a later user
    // decision replacing the rule's.
    conn.execute_batch(
        "INSERT INTO accounts (id, name, kind, currency, own_account, created_at)
             VALUES (1, 'Card', 'credit_card', 'USD', 1, 0);
         INSERT INTO raw_imports (id, source, imported_at, sha256, bytes)
             VALUES (1, 'manual', 0, x'6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d', x'00');
         INSERT INTO transactions (id, account_id, import_id, source_index, status, posted_date,
             amount_minor, currency, description)
             VALUES (1, 1, 1, 0, 'posted', '2026-01-02', -500, 'USD', 'COFFEE'),
                    (2, 1, 1, 1, 'posted', '2026-01-03', -600, 'USD', 'COFFEE');
         INSERT INTO decisions (id, transaction_id, assignment, bucket, bucket_category_id, source,
             taxonomy_base, taxonomy_revision, decided_at, state)
             VALUES (1, 1, 'bucket', 'category', 1, 'user', '1', 0, 10, 'applied');
         INSERT INTO rules (id, match_kind, match_text, bucket, bucket_category_id, origin,
             origin_transaction_id, origin_decision_id, priority, enabled, created_at)
             VALUES (1, 'contains', 'COFFEE', 'category', 1, 'correction', 1, 1, 0, 1, 11);
         INSERT INTO decisions (id, transaction_id, assignment, bucket, bucket_category_id, source,
             rule_id, taxonomy_base, taxonomy_revision, decided_at, state)
             VALUES (2, 2, 'bucket', 'category', 1, 'rule', 1, '1', 0, 12, 'superseded');
         INSERT INTO decisions (id, transaction_id, assignment, bucket, source, taxonomy_base,
             taxonomy_revision, decided_at, state, replaces)
             VALUES (3, 2, 'bucket', 'needs_review', 'user', '1', 0, 13, 'applied', 2);",
    )
    .unwrap();
    let before: Vec<(i64, String)> = conn
        .prepare("SELECT id, state FROM decisions ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    drop(conn);

    let u = vault.unlock(PASSWORD).unwrap();
    assert_eq!(u.schema_version().unwrap(), MIGRATIONS.len() as u32);
    let l = u.load_ledger().unwrap();
    let after: Vec<(i64, String)> = l
        .decisions
        .iter()
        .map(|d| {
            (
                d.id.0,
                serde_json::to_value(d.state)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        })
        .collect();
    assert_eq!(after, before);
    assert_eq!(l.rules.len(), 1, "the rebuild cascaded into rules");
    assert_eq!(l.decisions[2].replaces, Some(DecisionId(2)));
    assert!(
        l.transactions
            .iter()
            .all(|t| t.trn_type.is_none() && t.check_number.is_none())
    );

    // The rebuilt table is protected exactly as before.
    assert!(sql_fails(
        &u,
        "UPDATE decisions SET merchant = 'x' WHERE id = 1"
    ));
    assert!(sql_fails(&u, "DELETE FROM decisions WHERE id = 1"));
    assert!(sql_fails(
        &u,
        "UPDATE rules SET match_text = 'TEA' WHERE id = 1"
    ));
    let fks: i64 = u
        .connection()
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(fks, 0);
    // The new decision source is accepted.
    let mut d = l.decisions[0].clone();
    d.id = DecisionId(4);
    d.source = DecisionSource::Builtin {
        logic: BuiltinLogic::ImportPipeline,
        version: 1,
    };
    d.state = DecisionState::Suggested;
    u.insert_decision(&d).unwrap();
    assert_eq!(u.decision(DecisionId(4)).unwrap().unwrap(), d);
}

// ---- Planned deletion (T0.10, SPEC decisions 28, 42 and 43).

fn d(text: &str) -> Date {
    text.parse().unwrap()
}

fn scope(account: Option<i64>, from: Option<&str>, to: Option<&str>) -> DeletionScope {
    DeletionScope {
        account: account.map(AccountId),
        from: from.map(d),
        to: to.map(d),
    }
}

fn db_bytes(dir: &TempDir) -> Vec<u8> {
    fs::read(dir.path().join("ledger").join(DB_FILE)).unwrap()
}

/// Everything a deletion could touch: the whole ledger and the deleted-row records.
fn snapshot(u: &Unlocked) -> (Ledger, Vec<(i64, i64)>) {
    let mut statement = u
        .connection()
        .prepare("SELECT import_id, source_index FROM deleted_rows ORDER BY 1, 2")
        .unwrap();
    let records = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    (u.load_ledger().unwrap(), records)
}

fn no_foreign_key_violations(u: &Unlocked) {
    let violations: i64 = u
        .connection()
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
}

/// The sample ledger without the given rows, as a deletion should leave it.
fn sample_without(transactions: &[i64], decisions: &[i64], rules: &[i64]) -> Ledger {
    let mut ledger = sample_ledger();
    ledger
        .transactions
        .retain(|t| !transactions.contains(&t.id.0));
    ledger.decisions.retain(|x| !decisions.contains(&x.id.0));
    ledger.rules.retain(|r| !rules.contains(&r.id.0));
    ledger
}

#[test]
fn deleting_a_scope_cascades_and_records_rows_of_kept_imports() {
    let (_dir, mut u) = with_sample();
    // The card (account 2) up to 10 September: transaction 2 (bought 2 September,
    // posted 3 September) and 5 (10 September). Transaction 4 (16 September) stays.
    let plan = DeletionPlan {
        transactions: Some(scope(Some(2), None, Some("2026-09-10"))),
        ..DeletionPlan::default()
    };
    let counts = u.delete_planned(&plan, false).unwrap();
    assert_eq!(
        counts,
        DeletedCounts {
            transactions: 2,
            // 2, 3, 4 are about transaction 2 and 9, 10, 11 about transaction 5; 7 is
            // a refund pointing at 2, made by rule 1, which was learned from 2.
            decisions: 7,
            rules: 1,
            imports: 0,
            accounts: 0,
            recorded_rows: 2,
        }
    );
    // Out of scope: every other row is exactly as it was.
    assert_eq!(
        u.load_ledger().unwrap(),
        sample_without(&[2, 5], &[2, 3, 4, 7, 9, 10, 11], &[1])
    );
    // Import 2 is kept, so its deleted rows (indexes 0 and 3) are recorded.
    assert_eq!(u.deleted_rows(ImportId(2)).unwrap(), [0, 3]);
    assert!(u.deleted_rows(ImportId(1)).unwrap().is_empty());
    no_foreign_key_violations(&u);

    // Deleting the import afterwards takes its records with it.
    u.delete_import(ImportId(2)).unwrap();
    assert!(u.deleted_rows(ImportId(2)).unwrap().is_empty());
    assert_eq!(snapshot(&u).1, []);
}

#[test]
fn a_scope_dates_transactions_by_purchase_date_when_given() {
    // Transaction 2 was bought on 2 September and posted on 3 September.
    for (from, to, expected) in [
        ("2026-09-02", "2026-09-02", vec![2]),
        ("2026-09-03", "2026-09-03", vec![]),
        ("2026-09-01", "2026-09-02", vec![1, 2]),
        ("2026-09-15", "2026-09-16", vec![3, 4]),
    ] {
        let (_dir, mut u) = with_sample();
        let all: Vec<i64> = u.transactions().unwrap().iter().map(|t| t.id.0).collect();
        let plan = DeletionPlan {
            transactions: Some(scope(None, Some(from), Some(to))),
            ..DeletionPlan::default()
        };
        let counts = u.delete_planned(&plan, false).unwrap();
        let left: Vec<i64> = u.transactions().unwrap().iter().map(|t| t.id.0).collect();
        let gone: Vec<i64> = all.into_iter().filter(|id| !left.contains(id)).collect();
        assert_eq!(gone, expected, "{from}..={to}");
        assert_eq!(counts.transactions, expected.len() as u64);
        no_foreign_key_violations(&u);
    }
}

#[test]
fn listed_imports_go_whole_and_record_nothing() {
    let (_dir, mut u) = with_sample();
    // The scope covers import 2's first rows, but import 2 is deleted whole too.
    let plan = DeletionPlan {
        transactions: Some(scope(Some(2), None, Some("2026-09-10"))),
        imports: vec![ImportId(2)],
        ..DeletionPlan::default()
    };
    let counts = u.delete_planned(&plan, false).unwrap();
    assert_eq!(
        counts,
        DeletedCounts {
            transactions: 5,
            // As for delete_import, without the extra decision 12 that test adds.
            decisions: 10,
            rules: 1,
            imports: 1,
            accounts: 0,
            recorded_rows: 0,
        }
    );
    assert!(u.raw_import(ImportId(2)).unwrap().is_none());
    assert_eq!(snapshot(&u).1, []);
    no_foreign_key_violations(&u);
}

#[test]
fn listed_accounts_go_last_with_their_rules() {
    let (_dir, mut u) = with_sample();
    // A rule scoped to the unused account 11 that has made one decision.
    let mut rule = u.rules().unwrap().remove(1);
    rule.id = RuleId(4);
    rule.matcher.account_id = Some(AccountId(11));
    u.insert_rule(&rule).unwrap();
    let mut by_rule = u.decision(DecisionId(1)).unwrap().unwrap();
    by_rule.id = DecisionId(14);
    by_rule.source = DecisionSource::Rule { rule_id: RuleId(4) };
    by_rule.state = DecisionState::Suggested;
    u.insert_decision(&by_rule).unwrap();
    let plan = DeletionPlan {
        accounts: vec![AccountId(11)],
        ..DeletionPlan::default()
    };
    assert_eq!(
        u.delete_planned(&plan, false).unwrap(),
        DeletedCounts {
            decisions: 1,
            rules: 1,
            accounts: 1,
            ..DeletedCounts::default()
        }
    );
    assert!(u.account(AccountId(11)).unwrap().is_none());

    // The checking account, with its transactions and the import that names it.
    let plan = DeletionPlan {
        transactions: Some(scope(Some(1), None, None)),
        imports: vec![ImportId(1)],
        accounts: vec![AccountId(1)],
    };
    let counts = u.delete_planned(&plan, false).unwrap();
    assert_eq!(
        counts,
        DeletedCounts {
            transactions: 2,
            // 1 is about transaction 1, 5 about 3, and 6 is 4's transfer pointing at 3.
            decisions: 3,
            rules: 0,
            imports: 1,
            accounts: 1,
            recorded_rows: 0,
        }
    );
    assert!(u.account(AccountId(1)).unwrap().is_none());
    no_foreign_key_violations(&u);
}

#[test]
fn an_account_still_in_use_is_refused_and_nothing_changes() {
    for plan in [
        // The card still has transactions.
        DeletionPlan {
            accounts: vec![AccountId(2)],
            ..DeletionPlan::default()
        },
        // The checking account's transactions go first, but import 1 still names it:
        // the scope's deletions are rolled back too.
        DeletionPlan {
            transactions: Some(scope(Some(1), None, None)),
            accounts: vec![AccountId(1)],
            ..DeletionPlan::default()
        },
    ] {
        let (dir, mut u) = with_sample();
        let (before, bytes) = (snapshot(&u), db_bytes(&dir));
        assert!(matches!(
            u.delete_planned(&plan, false),
            Err(StorageError::AccountInUse)
        ));
        assert_eq!(snapshot(&u), before);
        assert_eq!(db_bytes(&dir), bytes);
    }
}

#[test]
fn an_error_mid_plan_rolls_everything_back() {
    let (dir, mut u) = with_sample();
    let (before, bytes) = (snapshot(&u), db_bytes(&dir));
    for plan in [
        DeletionPlan {
            transactions: Some(scope(None, None, None)),
            imports: vec![ImportId(2), ImportId(99)],
            ..DeletionPlan::default()
        },
        DeletionPlan {
            transactions: Some(scope(Some(2), Some("2026-09-01"), None)),
            imports: vec![ImportId(1)],
            accounts: vec![AccountId(99)],
        },
    ] {
        assert!(matches!(
            u.delete_planned(&plan, false),
            Err(StorageError::NotFound)
        ));
        assert_eq!(snapshot(&u), before);
        assert_eq!(db_bytes(&dir), bytes);
    }
    // The permit is off again afterwards.
    assert!(sql_fails(&u, "DELETE FROM decisions WHERE id = 1"));
}

#[test]
fn malformed_plans_are_refused() {
    let (_dir, mut u) = with_sample();
    let before = snapshot(&u);
    for plan in [
        DeletionPlan::default(),
        DeletionPlan {
            transactions: Some(scope(None, Some("2026-09-10"), Some("2026-09-09"))),
            ..DeletionPlan::default()
        },
        DeletionPlan {
            imports: vec![ImportId(1), ImportId(1)],
            ..DeletionPlan::default()
        },
        DeletionPlan {
            accounts: vec![AccountId(11), AccountId(11)],
            ..DeletionPlan::default()
        },
    ] {
        for dry_run in [false, true] {
            assert!(
                matches!(
                    u.delete_planned(&plan, dry_run),
                    Err(StorageError::InvalidRequest(_))
                ),
                "{plan:?}"
            );
        }
    }
    assert_eq!(snapshot(&u), before);
}

#[test]
fn a_dry_run_counts_exactly_what_the_real_run_removes_and_changes_nothing() {
    let plans = [
        DeletionPlan {
            transactions: Some(scope(Some(2), None, Some("2026-09-10"))),
            ..DeletionPlan::default()
        },
        DeletionPlan {
            transactions: Some(scope(None, Some("2026-09-02"), Some("2026-09-15"))),
            imports: vec![ImportId(3)],
            ..DeletionPlan::default()
        },
        DeletionPlan {
            transactions: Some(scope(Some(1), None, None)),
            imports: vec![ImportId(1)],
            accounts: vec![AccountId(1), AccountId(11)],
        },
        DeletionPlan {
            imports: vec![ImportId(2)],
            ..DeletionPlan::default()
        },
    ];
    for plan in plans {
        let (dir, mut u) = with_sample();
        let (before, bytes) = (snapshot(&u), db_bytes(&dir));
        let dry = u.delete_planned(&plan, true).unwrap();
        assert_eq!(snapshot(&u), before, "{plan:?}");
        assert_eq!(db_bytes(&dir), bytes, "{plan:?}: the file changed");
        assert_eq!(u.delete_planned(&plan, false).unwrap(), dry, "{plan:?}");
        assert_ne!(snapshot(&u), before);
    }
}

#[test]
fn deleting_a_td1_account_month_leaves_the_rest_intact() {
    let (_, ledger) = td1_ledgers().remove(0);
    let (dir, mut u) = new_ledger();
    u.save_ledger(&ledger).unwrap();
    let account = ledger.transactions[0].account_id;
    let (from, to) = (d("2025-03-01"), d("2025-03-31"));
    let in_scope = |t: &Transaction| {
        let date = t.transaction_date.unwrap_or(t.posted_date);
        t.account_id == account && from <= date && date <= to
    };
    let expected = ledger.transactions.iter().filter(|t| in_scope(t)).count() as u64;
    assert!(expected > 0, "the scope should hold transactions");
    let plan = DeletionPlan {
        transactions: Some(DeletionScope {
            account: Some(account),
            from: Some(from),
            to: Some(to),
        }),
        ..DeletionPlan::default()
    };

    let (before, bytes) = (snapshot(&u), db_bytes(&dir));
    let dry = u.delete_planned(&plan, true).unwrap();
    assert_eq!(snapshot(&u), before);
    assert_eq!(db_bytes(&dir), bytes);
    let counts = u.delete_planned(&plan, false).unwrap();
    assert_eq!(counts, dry);
    assert_eq!(counts.transactions, expected);
    assert_eq!(counts.recorded_rows, expected, "every import is kept");

    let kept: Vec<_> = ledger
        .transactions
        .iter()
        .filter(|t| !in_scope(t))
        .cloned()
        .collect();
    assert_eq!(u.transactions().unwrap(), kept);
    let recorded: usize = ledger
        .imports
        .iter()
        .map(|i| u.deleted_rows(i.id).unwrap().len())
        .sum();
    assert_eq!(recorded as u64, expected);
    no_foreign_key_violations(&u);
}

#[test]
fn deleted_row_records_are_written_only_by_deletion() {
    let (_dir, mut u) = with_sample();
    assert!(sql_fails(
        &u,
        "INSERT INTO deleted_rows (import_id, source_index) VALUES (1, 7)"
    ));
    let plan = DeletionPlan {
        transactions: Some(scope(Some(2), None, Some("2026-09-10"))),
        ..DeletionPlan::default()
    };
    u.delete_planned(&plan, false).unwrap();
    assert!(sql_fails(
        &u,
        "UPDATE deleted_rows SET source_index = 9 WHERE import_id = 2"
    ));
    assert!(sql_fails(&u, "DELETE FROM deleted_rows"));
    assert!(sql_fails(
        &u,
        "INSERT OR REPLACE INTO deleted_rows (import_id, source_index) VALUES (2, 0)"
    ));
    assert_eq!(u.deleted_rows(ImportId(2)).unwrap(), [0, 3]);
}
