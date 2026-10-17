//! Opening the SQLCipher database with the data key, and the schema migration runner.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::config::DbConfig;
use rusqlite::functions::FunctionFlags;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
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
    /// Run with foreign keys off, as SQLite's table-rebuild procedure requires (a
    /// `DROP TABLE` would otherwise cascade). `foreign_key_check` must then come back
    /// empty before the migration commits.
    pub foreign_keys_off: bool,
}

/// The ledger schema, in order.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "meta",
        sql: "CREATE TABLE meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL) STRICT;",
        foreign_keys_off: false,
    },
    Migration {
        version: 2,
        name: "ledger",
        sql: include_str!("schema/v2_ledger.sql"),
        foreign_keys_off: false,
    },
    Migration {
        version: 3,
        name: "source facts and the import pipeline source",
        sql: include_str!("schema/v3_source_facts.sql"),
        foreign_keys_off: true,
    },
    Migration {
        version: 4,
        name: "rows deleted out of kept imports",
        sql: include_str!("schema/v4_deleted_rows.sql"),
        foreign_keys_off: false,
    },
    Migration {
        version: 5,
        name: "applied decisions the user has seen may be dismissed",
        sql: include_str!("schema/v5_reject_state.sql"),
        foreign_keys_off: false,
    },
];

/// What the Rust side can temporarily allow on a connection. Each is off except while
/// the one API call that needs it runs (see [`Permits::grant`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Permit {
    /// `fm_permit('delete')`: delete imports and everything derived from them.
    Delete,
    /// `fm_permit('post_pending')`: turn a pending transaction into the posted one.
    PostPending,
    /// Schema changes and PRAGMA writes, otherwise refused by the authorizer: only
    /// migrations and rekeying.
    Schema,
}

/// The switches behind `fm_permit()` and the authorizer on one connection.
#[derive(Debug, Default)]
pub(crate) struct Permits {
    delete: AtomicBool,
    post_pending: AtomicBool,
    schema: AtomicBool,
}

impl Permits {
    fn flag(&self, permit: Permit) -> &AtomicBool {
        match permit {
            Permit::Delete => &self.delete,
            Permit::PostPending => &self.post_pending,
            Permit::Schema => &self.schema,
        }
    }

    fn granted(&self, permit: Permit) -> bool {
        self.flag(permit).load(Ordering::SeqCst)
    }

    /// Turns `permit` on until the returned guard drops, even on an early return.
    pub(crate) fn grant(&self, permit: Permit) -> PermitGuard<'_> {
        let flag = self.flag(permit);
        flag.store(true, Ordering::SeqCst);
        PermitGuard(flag)
    }
}

pub(crate) struct PermitGuard<'a>(&'a AtomicBool);

impl Drop for PermitGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// PRAGMAs that take an argument but only read. Every other PRAGMA with an argument is
/// a write and is refused (outside [`Permit::Schema`]).
const READ_ONLY_PRAGMAS_WITH_ARGUMENTS: &[&str] = &[
    "foreign_key_check",
    "foreign_key_list",
    "index_info",
    "index_list",
    "index_xinfo",
    "table_info",
    "table_xinfo",
];

/// The connection's authorizer: SQL can read and write rows (the triggers police
/// those), but can't change the schema, drop or add triggers, attach other files, or
/// write PRAGMAs (foreign_keys, recursive_triggers, writable_schema, key, ...).
/// `defer_foreign_keys` stays allowed: it only moves the check to commit.
fn authorize(permits: &Permits, ctx: AuthContext<'_>) -> Authorization {
    if permits.granted(Permit::Schema) {
        return Authorization::Allow;
    }
    match ctx.action {
        AuthAction::Pragma {
            pragma_name,
            pragma_value,
        } => {
            let name = pragma_name.to_ascii_lowercase();
            let read_only =
                pragma_value.is_none() || READ_ONLY_PRAGMAS_WITH_ARGUMENTS.contains(&name.as_str());
            if read_only || name == "defer_foreign_keys" {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        AuthAction::CreateIndex { .. }
        | AuthAction::CreateTable { .. }
        | AuthAction::CreateTrigger { .. }
        | AuthAction::CreateTempTrigger { .. }
        | AuthAction::CreateView { .. }
        | AuthAction::CreateVtable { .. }
        | AuthAction::DropIndex { .. }
        | AuthAction::DropTable { .. }
        | AuthAction::DropTrigger { .. }
        | AuthAction::DropTempTrigger { .. }
        | AuthAction::DropView { .. }
        | AuthAction::DropVtable { .. }
        | AuthAction::AlterTable { .. }
        | AuthAction::Attach { .. }
        | AuthAction::Detach { .. } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

/// Opens (or creates) the database at `path` with `key`, checks the key is right, and
/// sets the connection up: foreign keys, recursive triggers (so REPLACE's implicit
/// delete fires DELETE triggers), secure delete, SQLite's defensive mode,
/// `fm_permit()`, and the authorizer. Returns the connection's [`Permits`].
///
/// SQLCipher 4 defaults: AES-256-CBC pages with HMAC-SHA512, 4 KiB pages. The key is
/// given raw, so SQLCipher's own PBKDF2 is skipped (Argon2id already ran on the password).
pub(crate) fn open_encrypted(
    path: &Path,
    key: &DataKey,
    create: bool,
) -> Result<(Connection, Arc<Permits>), StorageError> {
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
    conn.execute_batch(
        "PRAGMA foreign_keys = ON; PRAGMA recursive_triggers = ON; PRAGMA secure_delete = ON;",
    )?;
    // Defensive mode: no writable_schema, no writes to shadow tables, no direct edits of
    // the schema table.
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;

    let permits = Arc::new(Permits::default());
    let for_function = Arc::clone(&permits);
    conn.create_scalar_function("fm_permit", 1, FunctionFlags::SQLITE_UTF8, move |ctx| {
        Ok(match ctx.get_raw(0).as_str().unwrap_or("") {
            "delete" => for_function.granted(Permit::Delete),
            "post_pending" => for_function.granted(Permit::PostPending),
            _ => false,
        })
    })?;
    let for_authorizer = Arc::clone(&permits);
    conn.authorizer(Some(move |ctx: AuthContext<'_>| {
        authorize(&for_authorizer, ctx)
    }))?;
    Ok((conn, permits))
}

/// Re-encrypts every page of the open database under `new_key` (SQLCipher `PRAGMA
/// rekey`). Runs in SQLCipher's own transaction: on failure the file keeps the old key.
pub(crate) fn rekey(
    conn: &Connection,
    permits: &Permits,
    new_key: &DataKey,
) -> Result<(), StorageError> {
    let _schema = permits.grant(Permit::Schema);
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

/// Foreign keys switched off for one migration, and back on when dropped.
struct ForeignKeysOff<'c> {
    conn: &'c Connection,
}

impl<'c> ForeignKeysOff<'c> {
    fn new(conn: &'c Connection) -> rusqlite::Result<ForeignKeysOff<'c>> {
        let refused = |why: &str| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
                Some(why.into()),
            )
        };
        if !conn.is_autocommit() {
            return Err(refused(
                "can't switch foreign keys off inside a transaction",
            ));
        }
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
        let guard = ForeignKeysOff { conn };
        let on: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
        if on != 0 {
            return Err(refused("foreign keys didn't switch off"));
        }
        Ok(guard)
    }
}

impl Drop for ForeignKeysOff<'_> {
    fn drop(&mut self) {
        // Can't fail outside a transaction; if it somehow did, the next open turns
        // foreign keys on again (open_encrypted sets them on every connection).
        let _ = self.conn.execute_batch("PRAGMA foreign_keys = ON;");
    }
}

/// Brings the database up to the last of `migrations`, each in its own transaction that
/// also records the new version, so a failed migration leaves the previous version
/// intact. Refuses a database newer than the app. Returns the resulting version.
pub(crate) fn migrate(
    conn: &mut Connection,
    permits: &Permits,
    migrations: &[Migration],
) -> Result<u32, StorageError> {
    let _schema = permits.grant(Permit::Schema);
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
        // `unchecked_transaction` takes `&Connection`, so the foreign-keys guard below can
        // hold the connection too; `migrate` holds `&mut`, so nothing else is in one.
        let apply = |conn: &Connection| -> rusqlite::Result<()> {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(m.sql)?;
            if m.foreign_keys_off {
                let violations: i64 =
                    tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                        r.get(0)
                    })?;
                if violations != 0 {
                    return Err(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY),
                        Some("migration left foreign key violations".into()),
                    ));
                }
            }
            // PRAGMA can't take parameters; the version is our own integer.
            tx.execute_batch(&format!("PRAGMA user_version = {};", m.version))?;
            tx.commit()
        };
        // PRAGMA foreign_keys has no effect inside a transaction, so it is switched
        // around the migration's transaction, checked to have taken effect (otherwise a
        // table rebuild's DROP TABLE would cascade), and switched back by a guard
        // whatever happens.
        let _foreign_keys = if m.foreign_keys_off {
            Some(
                ForeignKeysOff::new(conn).map_err(|source| StorageError::Migration {
                    version: m.version,
                    source,
                })?,
            )
        } else {
            None
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
