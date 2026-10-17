use std::fs;
use std::time::Instant;

use rusqlite::{Connection, ErrorCode, params};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;

const P: KdfParams = KdfParams::INSECURE_TEST;
const PASSWORD: &[u8] = b"correct horse battery staple";
const MARKER: &str = "PLAINTEXT-MARKER-7f3a";

fn new_vault() -> (TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::at(&dir.path().join("ledger"));
    (dir, vault)
}

fn put(u: &Unlocked, key: &str, value: &str) {
    u.connection()
        .execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .unwrap();
}

fn get(u: &Unlocked, key: &str) -> Option<String> {
    u.connection()
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .ok()
}

fn file_hash(path: &Path) -> Vec<u8> {
    Sha256::digest(fs::read(path).unwrap()).to_vec()
}

#[test]
fn create_write_lock_unlock_read() {
    let (_dir, vault) = new_vault();
    let u = vault.create_with_params(PASSWORD, P).unwrap();
    assert_eq!(u.schema_version().unwrap(), MIGRATIONS.len() as u32);
    put(&u, "greeting", MARKER);
    let vault = u.lock();

    let u = vault.unlock(PASSWORD).unwrap();
    assert_eq!(get(&u, "greeting").as_deref(), Some(MARKER));
}

#[test]
fn wrong_password_fails_without_revealing_why() {
    let (_dir, vault) = new_vault();
    vault.create_with_params(PASSWORD, P).unwrap().lock();
    for wrong in [
        &b"correct horse battery stapl"[..],
        b"",
        b"CORRECT HORSE BATTERY STAPLE",
    ] {
        let err = vault.unlock(wrong).unwrap_err();
        assert!(matches!(err, StorageError::WrongSecret), "{err:?}");
        assert_eq!(err.to_string(), "wrong password or recovery key");
    }
    // The right one still works afterwards (nothing was damaged or rate-limited here).
    vault.unlock(PASSWORD).unwrap();
}

#[test]
fn database_file_is_encrypted_and_unreadable_without_the_key() {
    let (_dir, vault) = new_vault();
    let u = vault.create_with_params(PASSWORD, P).unwrap();
    put(&u, "greeting", MARKER);
    let vault = u.lock();

    let bytes = fs::read(vault.db_path()).unwrap();
    assert!(
        !bytes.starts_with(b"SQLite format 3\0"),
        "plain SQLite header"
    );
    assert!(
        !bytes.windows(MARKER.len()).any(|w| w == MARKER.as_bytes()),
        "plaintext found in the database file"
    );

    // Opening with no key at all: SQLite sees random bytes.
    let plain = Connection::open(vault.db_path()).unwrap();
    let err = plain
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap_err();
    assert_eq!(
        err.sqlite_error_code(),
        Some(ErrorCode::NotADatabase),
        "{err}"
    );

    // Nor is the key in the keyfile in readable form.
    let keyfile = fs::read(vault.keyfile_path()).unwrap();
    assert!(!keyfile.windows(PASSWORD.len()).any(|w| w == PASSWORD));
}

#[test]
fn raw_key_from_another_keyfile_is_rejected() {
    let (_dir_a, a) = new_vault();
    let (_dir_b, b) = new_vault();
    a.create_with_params(PASSWORD, P).unwrap().lock();
    let unlocked_b = b.create_with_params(PASSWORD, P).unwrap();

    // B's data key against A's database.
    let err = db::open_encrypted(a.db_path(), &unlocked_b.key, false).unwrap_err();
    assert!(matches!(err, StorageError::DatabaseKeyRejected), "{err:?}");

    // B's keyfile copied next to A's database: the password opens the keyfile, but the
    // database refuses the key it yields.
    fs::copy(b.keyfile_path(), a.keyfile_path()).unwrap();
    let err = a.unlock(PASSWORD).unwrap_err();
    assert!(matches!(err, StorageError::DatabaseKeyRejected), "{err:?}");
}

#[test]
fn lock_wipes_the_key_and_closes_the_connection() {
    let (_dir, vault) = new_vault();
    let mut u = vault.create_with_params(PASSWORD, P).unwrap();
    assert!(!u.key.is_wiped());
    u.wipe();
    assert!(u.key.is_wiped(), "data key still in memory after lock");
    assert!(u.conn.is_none(), "connection still open after lock");
    drop(u);

    // `lock()` goes through the same path and leaves a working locked ledger.
    let vault = vault.unlock(PASSWORD).unwrap().lock();
    vault.unlock(PASSWORD).unwrap();
}

/// The app's migrations plus one more, as a later build would have them.
fn with_extra_migration(name: &'static str, sql: &'static str) -> Vec<Migration> {
    let mut all = MIGRATIONS.to_vec();
    all.push(Migration {
        version: MIGRATIONS.len() as u32 + 1,
        name,
        sql,
    });
    all
}

#[test]
fn migration_runs_on_an_existing_file() {
    let latest = MIGRATIONS.len() as u32;
    let next = with_extra_migration(
        "test: notes",
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL) STRICT;
         INSERT INTO meta (key, value) VALUES ('migrated', 'yes');",
    );
    let (_dir, vault) = new_vault();
    let u = vault.create_with_params(PASSWORD, P).unwrap();
    put(&u, "greeting", MARKER);

    // Reopen the same file through the same key and run a newer migration list.
    let (mut conn, permits) = db::open_encrypted(vault.db_path(), &u.key, false).unwrap();
    assert_eq!(db::schema_version(&conn).unwrap(), latest);
    assert_eq!(db::migrate(&mut conn, &permits, &next).unwrap(), latest + 1);
    assert_eq!(db::schema_version(&conn).unwrap(), latest + 1);
    conn.execute("INSERT INTO notes (body) VALUES ('x')", [])
        .unwrap();
    // Running it again is a no-op.
    assert_eq!(db::migrate(&mut conn, &permits, &next).unwrap(), latest + 1);
    drop(conn);

    // Existing data survived, and the migration's own changes are there.
    assert_eq!(get(&u, "greeting").as_deref(), Some(MARKER));
    assert_eq!(get(&u, "migrated").as_deref(), Some("yes"));
    drop(u);

    // This build doesn't know the newer version, so it refuses the file.
    let err = vault.unlock(PASSWORD).unwrap_err();
    assert!(
        matches!(err, StorageError::NewerSchema { found, supported } if found == latest + 1 && supported == latest),
        "{err:?}"
    );
}

/// A ledger made by T0.3 (schema version 1, no ledger tables) gains the ledger tables
/// and the default category list when this build opens it.
#[test]
fn version_1_ledger_upgrades_to_the_ledger_schema() {
    let (_dir, vault) = new_vault();
    fs::create_dir_all(vault.dir()).unwrap();
    let key = keyfile::DataKey::generate().unwrap();
    let mut kf = keyfile::Keyfile::new().unwrap();
    kf.set_slot(SlotKind::Password, PASSWORD, P, &key).unwrap();
    kf.write_atomic(vault.keyfile_path()).unwrap();
    let (mut conn, permits) = db::open_encrypted(vault.db_path(), &key, true).unwrap();
    assert_eq!(
        db::migrate(&mut conn, &permits, &MIGRATIONS[..1]).unwrap(),
        1
    );
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('greeting', ?1)",
        [MARKER],
    )
    .unwrap();
    drop(conn);

    let u = vault.unlock(PASSWORD).unwrap();
    assert_eq!(u.schema_version().unwrap(), 2);
    assert_eq!(get(&u, "greeting").as_deref(), Some(MARKER));
    assert_eq!(u.taxonomy().unwrap(), default_taxonomy());
}

#[test]
fn failed_migration_rolls_back() {
    let latest = MIGRATIONS.len() as u32;
    let broken = with_extra_migration(
        "test: broken",
        "CREATE TABLE half (x INTEGER) STRICT; SELECT * FROM no_such_table;",
    );
    let (_dir, vault) = new_vault();
    let u = vault.create_with_params(PASSWORD, P).unwrap();
    let (mut conn, permits) = db::open_encrypted(vault.db_path(), &u.key, false).unwrap();
    let err = db::migrate(&mut conn, &permits, &broken).unwrap_err();
    assert!(
        matches!(err, StorageError::Migration { version, .. } if version == latest + 1),
        "{err:?}"
    );
    assert_eq!(db::schema_version(&conn).unwrap(), latest);
    let half: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'half'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(half, 0, "partial migration left behind");
}

#[test]
fn changing_the_password_rewraps_without_touching_the_database() {
    let (_dir, vault) = new_vault();
    let mut u = vault.create_with_params(PASSWORD, P).unwrap();
    put(&u, "greeting", MARKER);
    let db_before = file_hash(vault.db_path());
    let keyfile_before = file_hash(vault.keyfile_path());

    // The current password is required (L1).
    assert!(matches!(
        u.set_password_with_params(b"not the password", b"a new password", P),
        Err(StorageError::WrongSecret)
    ));
    assert_eq!(file_hash(vault.keyfile_path()), keyfile_before);

    u.set_password_with_params(PASSWORD, b"a new password", P)
        .unwrap();
    assert_eq!(
        file_hash(vault.db_path()),
        db_before,
        "database was rewritten"
    );
    assert_ne!(file_hash(vault.keyfile_path()), keyfile_before);
    let vault = u.lock();

    assert!(matches!(
        vault.unlock(PASSWORD),
        Err(StorageError::WrongSecret)
    ));
    let u = vault.unlock(b"a new password").unwrap();
    assert_eq!(get(&u, "greeting").as_deref(), Some(MARKER));
}

#[test]
fn recovery_key_unlocks_alongside_the_password() {
    let (_dir, vault) = new_vault();
    let u = vault.create_with_params(PASSWORD, P).unwrap();
    assert!(!u.has_recovery_key());
    put(&u, "greeting", MARKER);
    let vault = u.lock();
    assert!(matches!(
        vault.unlock_with_recovery_key(b"anything"),
        Err(StorageError::NoSuchSlot(SlotKind::RecoveryKey))
    ));

    let mut u = vault.unlock(PASSWORD).unwrap();
    assert!(matches!(
        u.set_recovery_key_with_params(b"wrong", b"ABCD-EFGH-IJKL-MNOP", P),
        Err(StorageError::WrongSecret)
    ));
    assert!(!u.has_recovery_key());
    u.set_recovery_key_with_params(PASSWORD, b"ABCD-EFGH-IJKL-MNOP", P)
        .unwrap();
    assert!(u.has_recovery_key());
    let vault = u.lock();

    let u = vault
        .unlock_with_recovery_key(b"ABCD-EFGH-IJKL-MNOP")
        .unwrap();
    assert_eq!(get(&u, "greeting").as_deref(), Some(MARKER));
    let vault = u.lock();
    assert!(matches!(
        vault.unlock_with_recovery_key(PASSWORD),
        Err(StorageError::WrongSecret)
    ));
    vault.unlock(PASSWORD).unwrap();
}

#[test]
fn create_and_unlock_check_what_is_on_disk() {
    let (_dir, vault) = new_vault();
    assert_eq!(vault.state(), VaultState::Missing);
    assert!(matches!(
        vault.unlock(PASSWORD),
        Err(StorageError::NotFound)
    ));
    vault.create_with_params(PASSWORD, P).unwrap().lock();
    assert_eq!(vault.state(), VaultState::Ready);
    assert!(matches!(
        vault.create_with_params(PASSWORD, P),
        Err(StorageError::AlreadyExists)
    ));

    // A garbage keyfile is reported as such.
    fs::write(vault.keyfile_path(), b"not a keyfile").unwrap();
    assert!(matches!(
        vault.unlock(PASSWORD),
        Err(StorageError::KeyfileInvalid(_))
    ));

    // A database without its keyfile can never be opened, and isn't overwritten.
    fs::remove_file(vault.keyfile_path()).unwrap();
    assert_eq!(vault.state(), VaultState::Orphaned);
    assert!(matches!(
        vault.unlock(PASSWORD),
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        vault.create_with_params(PASSWORD, P),
        Err(StorageError::AlreadyExists)
    ));
}

/// L5: a crash between writing the keyfile and creating the database leaves a keyfile
/// alone; it can be finished with the password instead of being stuck.
#[test]
fn interrupted_create_can_be_finished() {
    let (_dir, vault) = new_vault();
    vault.create_with_params(PASSWORD, P).unwrap().lock();
    let keyfile_before = fs::read(vault.keyfile_path()).unwrap();

    for leftover in ["missing", "empty"] {
        fs::remove_file(vault.db_path()).unwrap();
        if leftover == "empty" {
            fs::write(vault.db_path(), b"").unwrap();
        }
        assert_eq!(vault.state(), VaultState::Incomplete, "{leftover}");
        assert!(matches!(
            vault.unlock(PASSWORD),
            Err(StorageError::IncompleteCreate)
        ));
        assert!(matches!(
            vault.create_with_params(PASSWORD, P),
            Err(StorageError::IncompleteCreate)
        ));
        assert!(matches!(
            vault.finish_create(b"wrong"),
            Err(StorageError::WrongSecret)
        ));

        let u = vault.finish_create(PASSWORD).unwrap();
        put(&u, "greeting", MARKER);
        assert_eq!(fs::read(vault.keyfile_path()).unwrap(), keyfile_before);
        let vault = u.lock();
        assert_eq!(vault.state(), VaultState::Ready);
        assert_eq!(
            get(&vault.unlock(PASSWORD).unwrap(), "greeting").as_deref(),
            Some(MARKER)
        );
        assert!(matches!(
            vault.finish_create(PASSWORD),
            Err(StorageError::AlreadyExists)
        ));
    }
}

/// L4: a second instance can't open (or change) the ledger while it is unlocked.
#[test]
fn only_one_instance_at_a_time() {
    let (_dir, vault) = new_vault();
    let u = vault.create_with_params(PASSWORD, P).unwrap();
    assert!(matches!(vault.unlock(PASSWORD), Err(StorageError::InUse)));
    assert!(matches!(vault.shred_keys(), Err(StorageError::InUse)));
    let vault = u.lock();
    let u = vault.unlock(PASSWORD).unwrap();
    assert!(matches!(vault.unlock(PASSWORD), Err(StorageError::InUse)));
    drop(u);
    vault.unlock(PASSWORD).unwrap();
}

/// M1: rotation re-encrypts under a new key, so an old keyfile copy (snapshot, backup)
/// plus the old password no longer opens the current database.
#[test]
fn rotating_the_data_key_retires_old_keyfile_copies() {
    let (_dir, vault) = new_vault();
    let mut u = vault.create_with_params(b"old password", P).unwrap();
    u.set_recovery_key_with_params(b"old password", b"OLD-RECOVERY", P)
        .unwrap();
    put(&u, "greeting", MARKER);
    let old_keyfile = fs::read(vault.keyfile_path()).unwrap();
    let old_id = u.keyfile.id_for_test();
    let db_before = file_hash(vault.db_path());

    // Password changed after a suspected compromise, then the key rotated.
    u.set_password_with_params(b"old password", PASSWORD, P)
        .unwrap();
    assert!(matches!(
        u.rotate_data_key_with_params(b"old password", None, P),
        Err(StorageError::WrongSecret)
    ));
    u.rotate_data_key_with_params(PASSWORD, Some(b"NEW-RECOVERY"), P)
        .unwrap();
    assert_ne!(u.keyfile.id_for_test(), old_id);
    assert_ne!(
        file_hash(vault.db_path()),
        db_before,
        "database not re-encrypted"
    );
    assert_eq!(
        get(&u, "greeting").as_deref(),
        Some(MARKER),
        "open connection keeps working"
    );
    let vault = u.lock();
    assert!(!vault.next_keyfile_path.exists());

    // The current keyfile works with the password and the new recovery key only.
    assert_eq!(
        get(&vault.unlock(PASSWORD).unwrap(), "greeting").as_deref(),
        Some(MARKER)
    );
    vault.unlock_with_recovery_key(b"NEW-RECOVERY").unwrap();
    assert!(matches!(
        vault.unlock_with_recovery_key(b"OLD-RECOVERY"),
        Err(StorageError::WrongSecret)
    ));

    // The old keyfile restored from a "backup", with the old password: rejected.
    let current_keyfile = fs::read(vault.keyfile_path()).unwrap();
    fs::write(vault.keyfile_path(), &old_keyfile).unwrap();
    assert!(matches!(
        vault.unlock(b"old password"),
        Err(StorageError::DatabaseKeyRejected)
    ));
    assert!(matches!(
        vault.unlock_with_recovery_key(b"OLD-RECOVERY"),
        Err(StorageError::DatabaseKeyRejected)
    ));
    fs::write(vault.keyfile_path(), &current_keyfile).unwrap();

    // Rotating without a new recovery key drops the recovery slot.
    let mut u = vault.unlock(PASSWORD).unwrap();
    u.rotate_data_key_with_params(PASSWORD, None, P).unwrap();
    assert!(!u.has_recovery_key());
}

/// M1: an interrupted rotation is completed or discarded on the next unlock.
#[test]
fn interrupted_rotation_recovers_on_unlock() {
    let (_dir, vault) = new_vault();
    let mut u = vault.create_with_params(PASSWORD, P).unwrap();
    put(&u, "greeting", MARKER);
    let old_keyfile = fs::read(vault.keyfile_path()).unwrap();
    u.rotate_data_key_with_params(PASSWORD, None, P).unwrap();
    let vault = u.lock();
    let new_keyfile = fs::read(vault.keyfile_path()).unwrap();

    // Crash after re-encrypting, before promoting the new keyfile.
    fs::write(vault.keyfile_path(), &old_keyfile).unwrap();
    fs::write(&vault.next_keyfile_path, &new_keyfile).unwrap();
    let u = vault.unlock(PASSWORD).unwrap();
    assert_eq!(get(&u, "greeting").as_deref(), Some(MARKER));
    assert_eq!(
        fs::read(vault.keyfile_path()).unwrap(),
        new_keyfile,
        "not promoted"
    );
    assert!(!vault.next_keyfile_path.exists());
    drop(u);

    // Crash before re-encrypting: the database still uses the current key, and the
    // half-made next keyfile is discarded.
    let (_other_dir, other) = new_vault();
    other.create_with_params(PASSWORD, P).unwrap().lock();
    fs::copy(other.keyfile_path(), &vault.next_keyfile_path).unwrap();
    let u = vault.unlock(PASSWORD).unwrap();
    assert_eq!(get(&u, "greeting").as_deref(), Some(MARKER));
    assert_eq!(fs::read(vault.keyfile_path()).unwrap(), new_keyfile);
    assert!(!vault.next_keyfile_path.exists());
}

/// M1: shredding destroys every keyfile, including leftovers of interrupted writes.
#[test]
fn shredding_keys_removes_every_keyfile() {
    let (_dir, vault) = new_vault();
    let u = vault.create_with_params(PASSWORD, P).unwrap();
    put(&u, "greeting", MARKER);
    let vault = u.lock();
    let leftovers = [
        vault.next_keyfile_path.clone(),
        keyfile::Keyfile::tmp_path(vault.keyfile_path()),
        keyfile::Keyfile::tmp_path(&vault.next_keyfile_path),
    ];
    for path in &leftovers {
        fs::copy(vault.keyfile_path(), path).unwrap();
    }

    vault.shred_keys().unwrap();
    assert!(!vault.keyfile_path().exists());
    for path in &leftovers {
        assert!(!path.exists(), "{} survived", path.display());
    }
    assert_eq!(vault.state(), VaultState::Orphaned);
    assert!(matches!(
        vault.unlock(PASSWORD),
        Err(StorageError::NotFound)
    ));
    // Shredding again is harmless.
    vault.shred_keys().unwrap();
}

/// M2: the folder is owner-only (0700) and every file in it 0600, including journals and
/// a keyfile after it is rewritten, whatever the process umask.
#[cfg(unix)]
#[test]
fn files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;

    let (_dir, vault) = new_vault();
    let mut u = vault.create_with_params(PASSWORD, P).unwrap();
    assert_eq!(mode(vault.dir()), 0o700, "folder");
    assert_eq!(mode(vault.db_path()), 0o600, "database");
    assert_eq!(mode(vault.keyfile_path()), 0o600, "keyfile");
    assert_eq!(mode(&vault.lock_path), 0o600, "lock file");

    // Keep the rollback journal around after a write so its mode can be checked.
    {
        // A PRAGMA write: refused by the authorizer outside the schema permit.
        let _schema = u.permits.grant(db::Permit::Schema);
        u.connection()
            .execute_batch("PRAGMA journal_mode = PERSIST;")
            .unwrap();
    }
    put(&u, "greeting", MARKER);
    let journal = vault.dir().join("ledger.db-journal");
    assert_eq!(mode(&journal), 0o600, "journal");

    // Rewritten keyfiles (rename over the old one) stay owner-only.
    u.set_password_with_params(PASSWORD, b"another password", P)
        .unwrap();
    assert_eq!(
        mode(vault.keyfile_path()),
        0o600,
        "keyfile after password change"
    );
    u.rotate_data_key_with_params(b"another password", None, P)
        .unwrap();
    assert_eq!(mode(vault.keyfile_path()), 0o600, "keyfile after rotation");
    let vault = u.lock();

    // Loosened permissions are repaired on unlock.
    fs::set_permissions(vault.db_path(), fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(vault.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    vault.unlock(b"another password").unwrap();
    assert_eq!(mode(vault.db_path()), 0o600);
    assert_eq!(mode(vault.dir()), 0o700);
}

/// L2: debug output shows slot kinds and parameters, never salts, nonces, wrapped keys
/// or the data key.
#[test]
fn debug_output_has_no_key_material() {
    let (_dir, vault) = new_vault();
    let mut u = vault.create_with_params(PASSWORD, P).unwrap();
    u.set_recovery_key_with_params(PASSWORD, b"RECOVERY", P)
        .unwrap();
    let printed = format!("{u:?} {:?}", u.keyfile);
    assert!(
        printed.contains("Password") && printed.contains("RecoveryKey"),
        "{printed}"
    );
    assert!(printed.contains("memory_kib"), "{printed}");
    for secret_field in ["salt", "nonce", "wrapped", "tag", "id:", "key:"] {
        assert!(
            !printed.contains(secret_field),
            "{secret_field} in {printed}"
        );
    }
    // No byte arrays, which print as `[12, 34, ...]`.
    assert!(
        !printed
            .as_bytes()
            .windows(2)
            .any(|w| w[0] == b'[' && w[1].is_ascii_digit()),
        "{printed}"
    );
}

/// L3: new slots need at least 19 MiB and 2 passes outside unit tests.
#[test]
fn write_floor_for_kdf_params() {
    assert!(KdfParams::DEFAULT.meets_write_floor());
    let floor = KdfParams {
        memory_kib: 19 * 1024,
        iterations: 2,
        parallelism: 1,
    };
    assert!(floor.meets_write_floor());
    assert!(
        !KdfParams {
            memory_kib: 19 * 1024 - 1,
            ..floor
        }
        .meets_write_floor()
    );
    assert!(
        !KdfParams {
            iterations: 1,
            ..floor
        }
        .meets_write_floor()
    );
    assert!(!P.meets_write_floor());
    assert_eq!(
        StorageError::WeakKdfParams.to_string(),
        "key derivation parameters are below the minimum (19 MiB memory, 2 passes)"
    );
}

/// Documents the PRAGMA key-copy gap (README): `cipher_memory_security` stays at its
/// default, off. Pins the cipher settings (I1) and the crypto backend chosen for T0.3
/// (vendored OpenSSL).
#[test]
fn sqlcipher_settings() {
    let (_dir, vault) = new_vault();
    let u = vault.create_with_params(PASSWORD, P).unwrap();
    let pragma = |name: &str| -> String {
        u.connection()
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(pragma("cipher_memory_security"), "0");
    assert_eq!(pragma("cipher_page_size"), "4096");
    assert_eq!(pragma("cipher_hmac_algorithm"), "HMAC_SHA512");
    assert_eq!(pragma("cipher_use_hmac"), "1");
    assert_eq!(pragma("cipher_provider"), "openssl");
    assert!(pragma("cipher_provider_version").starts_with("OpenSSL 3.6."));
    assert!(pragma("cipher_version").starts_with("4.14."));
    let fk: i64 = u
        .connection()
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1);
    let secure_delete: i64 = u
        .connection()
        .query_row("PRAGMA secure_delete", [], |r| r.get(0))
        .unwrap();
    assert_eq!(secure_delete, 1);
}

/// One run with the real default parameters (64 MiB, t=3, p=4). Prints the time; run
/// with `cargo test --release -- --nocapture default_params` for a meaningful number.
#[test]
fn default_params_create_and_unlock() {
    let (_dir, vault) = new_vault();
    vault.create(PASSWORD).unwrap().lock();
    let start = Instant::now();
    let u = vault.unlock(PASSWORD).unwrap();
    println!(
        "unlock with default Argon2id params took {:?}",
        start.elapsed()
    );
    assert_eq!(
        u.keyfile.slots_for_test()[0],
        (SlotKind::Password, KdfParams::DEFAULT)
    );
}

/// T0.9: the restore lock creates the folder and excludes every other way in while held.
#[cfg(unix)]
#[test]
fn restore_lock_excludes_other_instances_until_dropped() {
    use std::os::unix::fs::PermissionsExt;

    // On a ledger that doesn't exist yet: the folder is created, owner-only.
    let (_dir, fresh) = new_vault();
    let guard = fresh.lock_for_restore().unwrap();
    assert_eq!(
        fs::metadata(fresh.dir()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(matches!(fresh.lock_for_restore(), Err(StorageError::InUse)));
    assert!(matches!(
        fresh.create_with_params(PASSWORD, P),
        Err(StorageError::InUse)
    ));
    drop(guard);
    fresh.create_with_params(PASSWORD, P).unwrap().lock();

    // On an existing ledger.
    let (_dir, vault) = new_vault();
    vault.create_with_params(PASSWORD, P).unwrap().lock();
    let guard = vault.lock_for_restore().unwrap();
    assert!(matches!(vault.lock_for_restore(), Err(StorageError::InUse)));
    assert!(matches!(vault.unlock(PASSWORD), Err(StorageError::InUse)));
    assert!(matches!(vault.shred_keys(), Err(StorageError::InUse)));
    drop(guard);
    let u = vault.unlock(PASSWORD).unwrap();
    // And the other way round: an unlocked ledger blocks a restore.
    assert!(matches!(vault.lock_for_restore(), Err(StorageError::InUse)));
    drop(u);
    drop(vault.lock_for_restore().unwrap());
}
