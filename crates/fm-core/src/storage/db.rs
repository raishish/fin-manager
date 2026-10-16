//! Opening the SQLCipher database with the data key, and the schema migration runner.

use std::path::Path;

use rusqlite::{Connection, ErrorCode, OpenFlags};

use super::StorageError;
use super::keyfile::{DataKey, Pragma};

/// One schema change. Versions start at 1 and go up by one; a migration is never edited
/// once released (add a new one instead).
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// The ledger schema, in order. The ledger tables arrive in T0.8.
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "meta",
    sql: "CREATE TABLE meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL) STRICT;",
}];

/// Opens (or creates) the database at `path` with `key`, and checks the key is right.
///
/// SQLCipher 4 defaults: AES-256-CBC pages with HMAC-SHA512, 4 KiB pages. The key is
/// given raw, so SQLCipher's own PBKDF2 is skipped (Argon2id already ran on the password).
pub(crate) fn open_encrypted(
    path: &Path,
    key: &DataKey,
    create: bool,
) -> Result<Connection, StorageError> {
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let conn = Connection::open_with_flags(path, flags)?;
    conn.execute_batch(&key.sqlcipher_pragma(Pragma::Key))
        .map_err(key_statement_error)?;
    // The key is only checked when the first page is read.
    match conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    }) {
        Ok(_) => {}
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::NotADatabase => {
            return Err(StorageError::DatabaseKeyRejected);
        }
        Err(e) => return Err(e.into()),
    }
    // secure_delete overwrites deleted content inside the file, which matters for
    // selective deletion (T5.3).
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON;")?;
    Ok(conn)
}

/// Re-encrypts every page of the open database under `new_key` (SQLCipher `PRAGMA
/// rekey`). Runs in SQLCipher's own transaction: on failure the file keeps the old key.
pub(crate) fn rekey(conn: &Connection, new_key: &DataKey) -> Result<(), StorageError> {
    conn.execute_batch(&new_key.sqlcipher_pragma(Pragma::Rekey))
        .map_err(key_statement_error)
}

/// Errors from a statement holding the key become a fixed message: SQLite error text
/// can quote the statement (`near "x'…'": syntax error`), and that would leak the key.
fn key_statement_error(_: rusqlite::Error) -> StorageError {
    StorageError::KeySetup
}

/// The schema version stored in the database (0 for a new one).
pub(crate) fn schema_version(conn: &Connection) -> Result<u32, StorageError> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Brings the database up to the last of `migrations`, each in its own transaction that
/// also records the new version, so a failed migration leaves the previous version
/// intact. Refuses a database newer than the app. Returns the resulting version.
pub(crate) fn migrate(
    conn: &mut Connection,
    migrations: &[Migration],
) -> Result<u32, StorageError> {
    debug_assert!(
        migrations.iter().zip(1..).all(|(m, v)| m.version == v),
        "migration versions must be 1, 2, 3, ..."
    );
    let latest = migrations.last().map_or(0, |m| m.version);
    let current = schema_version(conn)?;
    if current > latest {
        return Err(StorageError::NewerSchema {
            found: current,
            supported: latest,
        });
    }
    for m in migrations.iter().filter(|m| m.version > current) {
        let apply = |conn: &mut Connection| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            tx.execute_batch(m.sql)?;
            // PRAGMA can't take parameters; the version is our own integer.
            tx.execute_batch(&format!("PRAGMA user_version = {};", m.version))?;
            tx.commit()
        };
        apply(conn).map_err(|source| StorageError::Migration {
            version: m.version,
            source,
        })?;
    }
    Ok(latest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_statement_errors_never_echo_the_statement() {
        let leaky = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some("near \"x'ABCDEF0123'\": syntax error".into()),
        );
        let message = key_statement_error(leaky).to_string();
        assert_eq!(message, "the database could not apply its key");
        assert!(!message.contains("ABCDEF"));
    }
}
