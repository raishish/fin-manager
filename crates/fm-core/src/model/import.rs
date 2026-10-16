use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::hash::{decode_hex, write_hex};
use super::{AccountId, ImportId, ModelError, Sha256Hash, Timestamp};

/// Where a raw import came from.
///
/// Serialised in `snake_case`, e.g. `"simplefin"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSource {
    Csv,
    Ofx,
    Qfx,
    /// One SimpleFIN `/accounts` response (raw JSON body).
    #[serde(rename = "simplefin")]
    SimpleFin,
    /// Transactions the user typed in; `bytes` holds what was entered.
    Manual,
}

/// Exactly what came in, before any parsing: the original bytes of a file or a
/// SimpleFIN response, and their SHA-256 (SPEC decision 28: keep everything).
///
/// The hash is always the SHA-256 of the bytes. [`RawImport::new`] computes it, the
/// bytes can't be changed afterwards, and deserialising re-checks it and fails with
/// [`ModelError::HashMismatch`] if they differ.
///
/// Serialised with the bytes as lowercase hex under `"bytes"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImport {
    pub id: ImportId,
    pub source: ImportSource,
    /// Original file name, if it came from a file.
    pub file_name: Option<String>,
    /// The CSV mapping profile used to read it, if any (owned by the CSV importer).
    pub mapping_profile: Option<String>,
    /// The account the import is for, when the source covers one account (a CSV or OFX
    /// file usually does; a SimpleFIN response covers several).
    pub account_id: Option<AccountId>,
    /// When the app received it (UTC).
    pub imported_at: Timestamp,
    sha256: Sha256Hash,
    bytes: Vec<u8>,
}

impl RawImport {
    /// A raw import of `bytes`, with its SHA-256 computed here. Optional fields start as
    /// `None`; set them directly.
    pub fn new(
        id: ImportId,
        source: ImportSource,
        imported_at: Timestamp,
        bytes: Vec<u8>,
    ) -> RawImport {
        RawImport {
            id,
            source,
            file_name: None,
            mapping_profile: None,
            account_id: None,
            imported_at,
            sha256: Sha256Hash::of(&bytes),
            bytes,
        }
    }

    /// The SHA-256 of [`RawImport::bytes`]. Two imports of the same file share it.
    pub fn sha256(&self) -> Sha256Hash {
        self.sha256
    }

    /// The original bytes, unchanged.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Rebuilds a stored raw import, checking that `sha256` is the hash of `bytes`.
    pub fn from_parts(
        id: ImportId,
        source: ImportSource,
        imported_at: Timestamp,
        sha256: Sha256Hash,
        bytes: Vec<u8>,
    ) -> Result<RawImport, ModelError> {
        let raw = RawImport::new(id, source, imported_at, bytes);
        if raw.sha256 != sha256 {
            return Err(ModelError::HashMismatch {
                stated: sha256,
                actual: raw.sha256,
            });
        }
        Ok(raw)
    }
}

/// Formats bytes as lowercase hex for serialisation.
struct Hex<'a>(&'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(f, self.0)
    }
}

impl Serialize for Hex<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[derive(Serialize)]
struct RawImportOut<'a> {
    id: ImportId,
    source: ImportSource,
    file_name: &'a Option<String>,
    mapping_profile: &'a Option<String>,
    account_id: Option<AccountId>,
    imported_at: Timestamp,
    sha256: Sha256Hash,
    bytes: Hex<'a>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawImportIn {
    id: ImportId,
    source: ImportSource,
    file_name: Option<String>,
    mapping_profile: Option<String>,
    account_id: Option<AccountId>,
    imported_at: Timestamp,
    sha256: Sha256Hash,
    bytes: String,
}

impl Serialize for RawImport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RawImportOut {
            id: self.id,
            source: self.source,
            file_name: &self.file_name,
            mapping_profile: &self.mapping_profile,
            account_id: self.account_id,
            imported_at: self.imported_at,
            sha256: self.sha256,
            bytes: Hex(&self.bytes),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RawImport {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawImportIn::deserialize(deserializer)?;
        let bytes = decode_hex(&raw.bytes)
            .ok_or_else(|| D::Error::custom("raw import bytes are not lowercase hex"))?;
        let mut out = RawImport::from_parts(raw.id, raw.source, raw.imported_at, raw.sha256, bytes)
            .map_err(D::Error::custom)?;
        out.file_name = raw.file_name;
        out.mapping_profile = raw.mapping_profile;
        out.account_id = raw.account_id;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_computed_from_the_bytes() {
        let raw = RawImport::new(
            ImportId(1),
            ImportSource::Csv,
            Timestamp::from_unix_seconds(0),
            b"abc".to_vec(),
        );
        assert_eq!(raw.sha256(), Sha256Hash::of(b"abc"));
        assert_eq!(raw.bytes(), b"abc");
    }

    #[test]
    fn from_parts_rejects_a_wrong_hash() {
        let err = RawImport::from_parts(
            ImportId(1),
            ImportSource::Csv,
            Timestamp::from_unix_seconds(0),
            Sha256Hash::of(b"abc"),
            b"abd".to_vec(),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ModelError::HashMismatch {
                stated: Sha256Hash::of(b"abc"),
                actual: Sha256Hash::of(b"abd"),
            }
        );
    }
}
