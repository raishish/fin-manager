use std::fmt;

use super::SlotKind;

/// Why a storage operation failed.
///
/// Messages never include passwords, keys or ledger contents.
#[derive(Debug)]
#[non_exhaustive]
pub enum StorageError {
    /// The password or recovery key is wrong, or the slot holding it was tampered with.
    /// Deliberately one error for every cause, so a failed unlock reveals nothing.
    WrongSecret,
    /// The keyfile has no slot of this kind (for example, no recovery key set up yet).
    NoSuchSlot(SlotKind),
    /// The keyfile is not one this build can read; the reason says what is wrong.
    KeyfileInvalid(&'static str),
    /// The keyfile unlocked, but the database didn't accept the key: the database and
    /// keyfile come from different ledgers, or the database is damaged.
    DatabaseKeyRejected,
    /// The database rejected the key while it was being applied. The message is fixed
    /// and never includes the statement, which contains the key.
    KeySetup,
    /// A stored row breaks a model invariant (the database was edited outside the app,
    /// or is damaged). Names the table and what is wrong, never the row's contents.
    InvalidRow {
        table: &'static str,
        reason: &'static str,
    },
    /// A change the ledger never allows, such as editing a recorded decision.
    ForbiddenChange(&'static str),
    /// A model value broke its own invariant (for example an invalid taxonomy).
    Model(crate::model::ModelError),
    /// Argon2id parameters below the minimum for protecting a new slot.
    WeakKdfParams,
    /// There is no ledger at this location.
    NotFound,
    /// A keyfile exists but the database was never created (a crash during create).
    /// [`Vault::finish_create`](super::Vault::finish_create) completes it.
    IncompleteCreate,
    /// Another instance of the app has this ledger open.
    InUse,
    /// A ledger already exists at this location.
    AlreadyExists,
    /// The database was written by a newer version of the app.
    NewerSchema {
        found: u32,
        supported: u32,
    },
    /// Migration `version` failed and was rolled back.
    Migration {
        version: u32,
        source: rusqlite::Error,
    },
    Database(rusqlite::Error),
    Io(std::io::Error),
    /// The operating system's random number generator failed.
    Random(getrandom::Error),
    /// Key derivation failed (invalid Argon2 parameters).
    Kdf(argon2::Error),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongSecret => f.write_str("wrong password or recovery key"),
            Self::NoSuchSlot(kind) => write!(f, "no {kind} is set up for this ledger"),
            Self::KeyfileInvalid(why) => write!(f, "unreadable keyfile: {why}"),
            Self::DatabaseKeyRejected => {
                f.write_str("the database does not match its keyfile, or is damaged")
            }
            Self::InvalidRow { table, reason } => write!(f, "invalid row in {table}: {reason}"),
            Self::ForbiddenChange(what) => write!(f, "not allowed: {what}"),
            Self::Model(e) => write!(f, "{e}"),
            Self::KeySetup => f.write_str("the database could not apply its key"),
            Self::WeakKdfParams => f.write_str(
                "key derivation parameters are below the minimum (19 MiB memory, 2 passes)",
            ),
            Self::NotFound => f.write_str("no ledger found at this location"),
            Self::IncompleteCreate => {
                f.write_str("ledger creation was interrupted; finish creating it to continue")
            }
            Self::InUse => f.write_str("the ledger is open in another instance of the app"),
            Self::AlreadyExists => f.write_str("a ledger already exists at this location"),
            Self::NewerSchema { found, supported } => write!(
                f,
                "the ledger has schema version {found}, but this app supports up to {supported}; update the app"
            ),
            Self::Migration { version, .. } => {
                write!(f, "schema migration {version} failed and was rolled back")
            }
            Self::Database(e) => write!(f, "database error: {e}"),
            Self::Io(e) => write!(f, "file error: {e}"),
            Self::Random(e) => write!(f, "random number generator failed: {e}"),
            Self::Kdf(e) => write!(f, "key derivation failed: {e}"),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Migration { source, .. } => Some(source),
            Self::Database(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Random(e) => Some(e),
            Self::Kdf(e) => Some(e),
            Self::Model(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e)
    }
}

impl From<crate::model::ModelError> for StorageError {
    fn from(e: crate::model::ModelError) -> Self {
        Self::Model(e)
    }
}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<getrandom::Error> for StorageError {
    fn from(e: getrandom::Error) -> Self {
        Self::Random(e)
    }
}

impl From<argon2::Error> for StorageError {
    fn from(e: argon2::Error) -> Self {
        Self::Kdf(e)
    }
}
