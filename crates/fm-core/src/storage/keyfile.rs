//! The keyfile: the random data key, wrapped once per way of unlocking (SPEC decision 16).
//!
//! Layout (all integers little-endian):
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 8 | magic `FMLEDKEY` |
//! | 8 | 2 | format version (1) |
//! | 10 | 16 | keyfile id (random) |
//! | 26 | 1 | slot count (1–8) |
//! | 27 | 101 × n | slots |
//!
//! Each slot: kind (1) · Argon2id memory KiB (4) · iterations (4) · parallelism (4) ·
//! salt (16) · nonce (24) · wrapped data key (32) · Poly1305 tag (16).
//!
//! The wrapping key (KEK) is Argon2id(secret, salt) with the slot's parameters. The data
//! key is sealed with XChaCha20-Poly1305 under the KEK. The additional data is the file
//! header plus the slot's kind, parameters and salt, so changing any of them, or moving
//! a slot into another keyfile, makes it fail to open.

use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Version};
use chacha20poly1305::aead::AeadInOut;
use chacha20poly1305::{KeyInit, Tag, XChaCha20Poly1305, XNonce};
use zeroize::{Zeroize, Zeroizing};

use super::StorageError;

const MAGIC: &[u8; 8] = b"FMLEDKEY";
const FORMAT_VERSION: u16 = 1;
const HEADER_LEN: usize = 8 + 2 + 16;
const MAX_SLOTS: usize = 8;
const SLOT_AAD_LEN: usize = 1 + 12 + 16;
const SLOT_LEN: usize = SLOT_AAD_LEN + 24 + 32 + 16;

/// The random 256-bit key that encrypts the database. It exists only in memory while the
/// ledger is unlocked and is wiped when dropped. It can't be cloned or printed.
pub(crate) struct DataKey([u8; 32]);

impl Zeroize for DataKey {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for DataKey {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl DataKey {
    pub(crate) fn generate() -> Result<DataKey, StorageError> {
        let mut key = DataKey([0; 32]);
        getrandom::fill(&mut key.0)?;
        Ok(key)
    }

    /// A data key from raw bytes (the Keychain copy T5.1 keeps for Touch ID).
    pub(crate) fn from_bytes(bytes: &[u8; 32]) -> DataKey {
        DataKey(*bytes)
    }

    /// A copy of the raw key, wiped when dropped.
    pub(crate) fn copy_bytes(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.0)
    }

    /// `PRAGMA key = "x'<hex>'";` (or `rekey`), which hands SQLCipher the raw key
    /// (skipping its own password KDF). Built in a buffer of exactly the right size, so it is never
    /// reallocated, and wiped when dropped.
    ///
    /// Known gap: SQLite keeps a transient copy of this text while it parses the PRAGMA
    /// and frees it without wiping (unless `cipher_memory_security` is on, which slows
    /// every query). Closing it needs `sqlite3_key_v2` over FFI; deferred (see README).
    pub(crate) fn sqlcipher_pragma(&self, pragma: Pragma) -> Zeroizing<String> {
        let prefix = match pragma {
            Pragma::Key => "PRAGMA key = \"x'",
            Pragma::Rekey => "PRAGMA rekey = \"x'",
        };
        const SUFFIX: &str = "'\";";
        const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
        let mut sql = Zeroizing::new(String::with_capacity(prefix.len() + 64 + SUFFIX.len()));
        sql.push_str(prefix);
        for b in self.0 {
            sql.push(char::from(DIGITS[usize::from(b >> 4)]));
            sql.push(char::from(DIGITS[usize::from(b & 0x0f)]));
        }
        sql.push_str(SUFFIX);
        sql
    }

    /// Whether every byte is zero (true after [`Zeroize::zeroize`]).
    #[cfg(test)]
    pub(crate) fn is_wiped(&self) -> bool {
        self.0.iter().all(|&b| b == 0)
    }
}

/// Which SQLCipher statement [`DataKey::sqlcipher_pragma`] builds.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Pragma {
    /// Open an existing database with this key (or set it on a new one).
    Key,
    /// Re-encrypt the open database under this key.
    Rekey,
}

impl fmt::Debug for DataKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DataKey(<redacted>)")
    }
}

/// What a key slot is unlocked with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlotKind {
    /// The master password.
    Password,
    /// The recovery key shown once at setup (T5.1).
    RecoveryKey,
}

impl SlotKind {
    const fn code(self) -> u8 {
        match self {
            Self::Password => 1,
            Self::RecoveryKey => 2,
        }
    }

    const fn from_code(code: u8) -> Option<SlotKind> {
        match code {
            1 => Some(Self::Password),
            2 => Some(Self::RecoveryKey),
            _ => None,
        }
    }
}

impl fmt::Display for SlotKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Password => "password",
            Self::RecoveryKey => "recovery key",
        })
    }
}

/// Argon2id cost parameters, stored in each slot so they can be raised later without a
/// migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes, 4 lanes: the second recommended setting in RFC 9106 §4. About
    /// 0.1 s on an Apple M2 (release build); uses 64 MiB once per unlock.
    pub const DEFAULT: KdfParams = KdfParams {
        memory_kib: 64 * 1024,
        iterations: 3,
        parallelism: 4,
    };

    /// Upper bounds accepted when reading a keyfile, so a crafted file can't make an
    /// unlock allocate or spin without limit (4× the default memory, ~5× its passes).
    const MAX_MEMORY_KIB: u32 = 256 * 1024;
    const MAX_ITERATIONS: u32 = 16;
    const MAX_PARALLELISM: u32 = 16;

    /// Lower bounds for writing a new slot: OWASP's Argon2id minimum (19 MiB, 2 passes).
    /// Reading still accepts anything within the upper bounds.
    const MIN_WRITE_MEMORY_KIB: u32 = 19 * 1024;
    const MIN_WRITE_ITERATIONS: u32 = 2;

    /// Cheap parameters for unit tests only. Never use them for a real ledger.
    #[cfg(test)]
    pub(crate) const INSECURE_TEST: KdfParams = KdfParams {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };

    /// Whether these parameters are strong enough to protect a new slot.
    pub(crate) fn meets_write_floor(self) -> bool {
        self.memory_kib >= Self::MIN_WRITE_MEMORY_KIB
            && self.iterations >= Self::MIN_WRITE_ITERATIONS
    }

    fn check(self) -> Result<argon2::Params, StorageError> {
        let in_bounds = (1..=Self::MAX_PARALLELISM).contains(&self.parallelism)
            && (1..=Self::MAX_ITERATIONS).contains(&self.iterations)
            && self.memory_kib <= Self::MAX_MEMORY_KIB
            && self.memory_kib >= 8 * self.parallelism;
        if !in_bounds {
            return Err(StorageError::KeyfileInvalid(
                "key derivation parameters out of bounds",
            ));
        }
        Ok(argon2::Params::new(
            self.memory_kib,
            self.iterations,
            self.parallelism,
            Some(32),
        )?)
    }

    fn derive(self, secret: &[u8], salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>, StorageError> {
        let params = self.check()?;
        let mut kek = Zeroizing::new([0u8; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params).hash_password_into(
            secret,
            salt,
            kek.as_mut(),
        )?;
        Ok(kek)
    }
}

/// Unit tests may write slots below the floor (debug-build Argon2 is slow); nothing else may.
const ALLOW_WEAK_PARAMS: bool = cfg!(test);

#[derive(Clone, PartialEq, Eq)]
struct KeySlot {
    kind: SlotKind,
    kdf: KdfParams,
    salt: [u8; 16],
    nonce: [u8; 24],
    wrapped: [u8; 32],
    tag: [u8; 16],
}

/// Kind and parameters only: salt, nonce and wrapped key stay out of logs.
impl fmt::Debug for KeySlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeySlot")
            .field("kind", &self.kind)
            .field("kdf", &self.kdf)
            .finish_non_exhaustive()
    }
}

impl KeySlot {
    /// The slot's part of the additional data: kind, parameters and salt.
    fn aad_fields(&self) -> [u8; SLOT_AAD_LEN] {
        let mut out = [0u8; SLOT_AAD_LEN];
        out[0] = self.kind.code();
        out[1..5].copy_from_slice(&self.kdf.memory_kib.to_le_bytes());
        out[5..9].copy_from_slice(&self.kdf.iterations.to_le_bytes());
        out[9..13].copy_from_slice(&self.kdf.parallelism.to_le_bytes());
        out[13..29].copy_from_slice(&self.salt);
        out
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.aad_fields());
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.wrapped);
        out.extend_from_slice(&self.tag);
    }

    fn decode(b: &[u8; SLOT_LEN]) -> Result<KeySlot, StorageError> {
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let kind =
            SlotKind::from_code(b[0]).ok_or(StorageError::KeyfileInvalid("unknown slot kind"))?;
        let kdf = KdfParams {
            memory_kib: u32_at(1),
            iterations: u32_at(5),
            parallelism: u32_at(9),
        };
        kdf.check()?;
        let mut slot = KeySlot {
            kind,
            kdf,
            salt: [0; 16],
            nonce: [0; 24],
            wrapped: [0; 32],
            tag: [0; 16],
        };
        slot.salt.copy_from_slice(&b[13..29]);
        slot.nonce.copy_from_slice(&b[29..53]);
        slot.wrapped.copy_from_slice(&b[53..85]);
        slot.tag.copy_from_slice(&b[85..101]);
        Ok(slot)
    }
}

/// The data key, wrapped by each secret that can unlock the ledger. Lives in its own file
/// next to the database, so destroying it destroys access to the data (crypto-shred,
/// SPEC decision 28) and changing the password rewrites only this file.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Keyfile {
    id: [u8; 16],
    slots: Vec<KeySlot>,
}

/// Slots only (each printing kind and parameters); the id is left out too.
impl fmt::Debug for Keyfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keyfile")
            .field("slots", &self.slots)
            .finish_non_exhaustive()
    }
}

impl Keyfile {
    /// An empty keyfile with a fresh random id. Add a slot before saving it.
    pub(crate) fn new() -> Result<Keyfile, StorageError> {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id)?;
        Ok(Keyfile {
            id,
            slots: Vec::new(),
        })
    }

    fn header(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[..8].copy_from_slice(MAGIC);
        out[8..10].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        out[10..26].copy_from_slice(&self.id);
        out
    }

    fn aad(&self, slot: &KeySlot) -> [u8; HEADER_LEN + SLOT_AAD_LEN] {
        let mut out = [0u8; HEADER_LEN + SLOT_AAD_LEN];
        out[..HEADER_LEN].copy_from_slice(&self.header());
        out[HEADER_LEN..].copy_from_slice(&slot.aad_fields());
        out
    }

    #[cfg(test)]
    pub(crate) fn id_for_test(&self) -> [u8; 16] {
        self.id
    }

    /// Each slot's kind and parameters, for tests.
    #[cfg(test)]
    pub(crate) fn slots_for_test(&self) -> Vec<(SlotKind, KdfParams)> {
        self.slots.iter().map(|s| (s.kind, s.kdf)).collect()
    }

    pub(crate) fn has_slot(&self, kind: SlotKind) -> bool {
        self.slots.iter().any(|s| s.kind == kind)
    }

    /// Wraps `key` under `secret` in a slot of this kind, replacing any existing one.
    pub(crate) fn set_slot(
        &mut self,
        kind: SlotKind,
        secret: &[u8],
        kdf: KdfParams,
        key: &DataKey,
    ) -> Result<(), StorageError> {
        if !ALLOW_WEAK_PARAMS && !kdf.meets_write_floor() {
            return Err(StorageError::WeakKdfParams);
        }
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut salt)?;
        getrandom::fill(&mut nonce)?;
        // Derive first: once the plaintext key is copied into the slot below, nothing
        // can fail before it is encrypted in place.
        let kek = kdf.derive(secret, &salt)?;
        let mut slot = KeySlot {
            kind,
            kdf,
            salt,
            nonce,
            wrapped: key.0,
            tag: [0; 16],
        };
        let aad = self.aad(&slot);
        let tag = cipher(&kek)
            .encrypt_inout_detached(
                &XNonce::from(slot.nonce),
                &aad,
                slot.wrapped.as_mut_slice().into(),
            )
            .map_err(|_| StorageError::KeyfileInvalid("could not seal the key"))?;
        slot.tag = tag.into();

        self.slots.retain(|s| s.kind != kind);
        self.slots.push(slot);
        Ok(())
    }

    /// The data key, if `secret` opens this keyfile's slot of `kind`. Any failure to open
    /// the slot (wrong secret, edited parameters, slot from another keyfile) is
    /// [`StorageError::WrongSecret`].
    pub(crate) fn unwrap_key(
        &self,
        kind: SlotKind,
        secret: &[u8],
    ) -> Result<DataKey, StorageError> {
        let slot = self
            .slots
            .iter()
            .find(|s| s.kind == kind)
            .ok_or(StorageError::NoSuchSlot(kind))?;
        let kek = slot.kdf.derive(secret, &slot.salt)?;
        let mut key = DataKey(slot.wrapped);
        cipher(&kek)
            .decrypt_inout_detached(
                &XNonce::from(slot.nonce),
                &self.aad(slot),
                key.0.as_mut_slice().into(),
                &Tag::from(slot.tag),
            )
            .map_err(|_| StorageError::WrongSecret)?;
        Ok(key)
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + 1 + SLOT_LEN * self.slots.len());
        out.extend_from_slice(&self.header());
        // At most MAX_SLOTS (one per kind), so this can't truncate.
        out.push(self.slots.len() as u8);
        for slot in &self.slots {
            slot.encode(&mut out);
        }
        out
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Keyfile, StorageError> {
        let invalid = StorageError::KeyfileInvalid;
        if bytes.len() < HEADER_LEN + 1 || &bytes[..8] != MAGIC {
            return Err(invalid("not a fin-manager keyfile"));
        }
        if u16::from_le_bytes([bytes[8], bytes[9]]) != FORMAT_VERSION {
            return Err(invalid("unsupported keyfile version"));
        }
        let mut id = [0u8; 16];
        id.copy_from_slice(&bytes[10..26]);
        let count = usize::from(bytes[26]);
        if count == 0 || count > MAX_SLOTS {
            return Err(invalid("bad slot count"));
        }
        let (slots, rest) = bytes[HEADER_LEN + 1..].as_chunks::<SLOT_LEN>();
        if slots.len() != count || !rest.is_empty() {
            return Err(invalid("wrong length for its slot count"));
        }
        let slots = slots
            .iter()
            .map(KeySlot::decode)
            .collect::<Result<Vec<_>, _>>()?;
        for (i, s) in slots.iter().enumerate() {
            if slots[..i].iter().any(|t| t.kind == s.kind) {
                return Err(invalid("two slots of the same kind"));
            }
        }
        Ok(Keyfile { id, slots })
    }

    pub(crate) fn read(path: &Path) -> Result<Keyfile, StorageError> {
        Keyfile::decode(&fs::read(path)?)
    }

    /// The temporary file [`Keyfile::write_atomic`] writes `path` through.
    pub(crate) fn tmp_path(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push(".tmp");
        PathBuf::from(name)
    }

    /// Writes the keyfile so that a crash leaves either the old file or the new one,
    /// never a mix: write a temporary file (owner-only, 0600), flush it to disk, rename
    /// it over the old one. The rename keeps the 0600 mode. Callers hold the ledger lock,
    /// so a leftover temporary file is from a crash and is replaced.
    pub(crate) fn write_atomic(&self, path: &Path) -> Result<(), StorageError> {
        let tmp = Keyfile::tmp_path(path);
        let result = (|| {
            match fs::remove_file(&tmp) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut f = options.open(&tmp)?;
            f.write_all(&self.encode())?;
            f.sync_all()?;
            fs::rename(&tmp, path)?;
            if let Some(dir) = path.parent() {
                // Make the rename itself durable. Not supported everywhere, so best effort.
                let _ = fs::File::open(dir).and_then(|d| d.sync_all());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

fn cipher(kek: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(&(*kek).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: KdfParams = KdfParams::INSECURE_TEST;

    fn keyfile_with_password(key: &DataKey) -> Keyfile {
        let mut kf = Keyfile::new().unwrap();
        kf.set_slot(SlotKind::Password, b"hunter2", P, key).unwrap();
        kf
    }

    #[test]
    fn wraps_and_unwraps() {
        let key = DataKey::generate().unwrap();
        let kf = keyfile_with_password(&key);
        let back = Keyfile::decode(&kf.encode()).unwrap();
        assert_eq!(back, kf);
        assert_eq!(
            back.unwrap_key(SlotKind::Password, b"hunter2").unwrap().0,
            key.0
        );
        // The stored bytes don't contain the key.
        assert!(!kf.encode().windows(32).any(|w| w == key.0));
    }

    #[test]
    fn wrong_secret_fails_without_saying_why() {
        let key = DataKey::generate().unwrap();
        let kf = keyfile_with_password(&key);
        for wrong in [&b"hunter3"[..], b"", b"Hunter2", b"hunter2 "] {
            let err = kf.unwrap_key(SlotKind::Password, wrong).unwrap_err();
            assert!(matches!(err, StorageError::WrongSecret), "{err:?}");
            assert_eq!(err.to_string(), "wrong password or recovery key");
        }
        assert!(matches!(
            kf.unwrap_key(SlotKind::RecoveryKey, b"hunter2"),
            Err(StorageError::NoSuchSlot(SlotKind::RecoveryKey))
        ));
    }

    #[test]
    fn slots_are_independent_and_replaceable() {
        let key = DataKey::generate().unwrap();
        let mut kf = keyfile_with_password(&key);
        kf.set_slot(SlotKind::RecoveryKey, b"RECOVERY-0000", P, &key)
            .unwrap();
        assert_eq!(
            kf.unwrap_key(SlotKind::RecoveryKey, b"RECOVERY-0000")
                .unwrap()
                .0,
            key.0
        );
        assert!(kf.unwrap_key(SlotKind::RecoveryKey, b"hunter2").is_err());

        kf.set_slot(SlotKind::Password, b"new password", P, &key)
            .unwrap();
        assert_eq!(kf.slots.len(), 2);
        assert!(kf.unwrap_key(SlotKind::Password, b"hunter2").is_err());
        assert_eq!(
            kf.unwrap_key(SlotKind::Password, b"new password")
                .unwrap()
                .0,
            key.0
        );
        assert_eq!(Keyfile::decode(&kf.encode()).unwrap(), kf);
    }

    /// Flipping any bit of a slot, or of the header it is bound to, stops it opening.
    #[test]
    fn every_bit_of_header_and_slot_is_authenticated() {
        let key = DataKey::generate().unwrap();
        let bytes = keyfile_with_password(&key).encode();
        for i in 10..bytes.len() {
            if i == 26 {
                continue; // slot count: changes the layout, covered by decode tests
            }
            if (28..32).contains(&i) {
                // Argon2 memory: a high-bit flip asks for up to 1 GiB, far too slow in a
                // debug test. Covered by tampered_params_fail_as_wrong_secret.
                continue;
            }
            for bit in 0..8 {
                let mut tampered = bytes.clone();
                tampered[i] ^= 1 << bit;
                let opened = Keyfile::decode(&tampered)
                    .and_then(|kf| kf.unwrap_key(SlotKind::Password, b"hunter2"));
                assert!(opened.is_err(), "flip at byte {i} bit {bit} not detected");
            }
        }
    }

    #[test]
    fn tampered_params_fail_as_wrong_secret() {
        let key = DataKey::generate().unwrap();
        let kf = keyfile_with_password(&key);
        let mut tampered = kf.clone();
        tampered.slots[0].kdf.iterations = 2;
        let back = Keyfile::decode(&tampered.encode()).unwrap();
        assert!(matches!(
            back.unwrap_key(SlotKind::Password, b"hunter2"),
            Err(StorageError::WrongSecret)
        ));
    }

    #[test]
    fn slot_moved_to_another_keyfile_fails() {
        let key = DataKey::generate().unwrap();
        let a = keyfile_with_password(&key);
        let mut b = keyfile_with_password(&key);
        b.slots[0] = a.slots[0].clone();
        assert!(matches!(
            b.unwrap_key(SlotKind::Password, b"hunter2"),
            Err(StorageError::WrongSecret)
        ));
    }

    #[test]
    fn out_of_bounds_params_are_rejected() {
        let key = DataKey::generate().unwrap();
        for bad in [
            KdfParams {
                memory_kib: 256 * 1024 + 1,
                ..P
            },
            KdfParams {
                memory_kib: u32::MAX,
                ..P
            },
            KdfParams { memory_kib: 7, ..P },
            KdfParams { iterations: 0, ..P },
            KdfParams {
                iterations: 17,
                ..P
            },
            KdfParams {
                parallelism: 0,
                ..P
            },
            KdfParams {
                parallelism: 17,
                memory_kib: 1024,
                ..P
            },
            KdfParams {
                parallelism: 4,
                memory_kib: 31,
                ..P
            },
        ] {
            let mut kf = keyfile_with_password(&key);
            kf.slots[0].kdf = bad;
            assert!(
                matches!(
                    Keyfile::decode(&kf.encode()),
                    Err(StorageError::KeyfileInvalid(_))
                ),
                "{bad:?} accepted on read"
            );
            assert!(
                Keyfile::new()
                    .unwrap()
                    .set_slot(SlotKind::Password, b"x", bad, &key)
                    .is_err(),
                "{bad:?} accepted on create"
            );
        }
    }

    /// L6: the read caps are 256 MiB and 16 passes, inclusive (decode only; deriving at
    /// 256 MiB would be slow in a debug build).
    #[test]
    fn read_caps_are_inclusive() {
        let key = DataKey::generate().unwrap();
        let mut kf = keyfile_with_password(&key);
        kf.slots[0].kdf = KdfParams {
            memory_kib: 256 * 1024,
            iterations: 16,
            parallelism: 16,
        };
        assert!(Keyfile::decode(&kf.encode()).is_ok());
    }

    #[test]
    fn malformed_files_are_rejected() {
        let key = DataKey::generate().unwrap();
        let good = keyfile_with_password(&key).encode();
        let mut two_passwords = keyfile_with_password(&key);
        two_passwords.slots.push(two_passwords.slots[0].clone());
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("bad magic", [b"XXXXXXXX", &good[8..]].concat()),
            (
                "future version",
                [&good[..8], &[2, 0], &good[10..]].concat(),
            ),
            ("truncated", good[..good.len() - 1].to_vec()),
            ("trailing byte", [&good[..], &[0]].concat()),
            ("zero slots", [&good[..26], &[0]].concat()),
            ("count too high", [&good[..26], &[2], &good[27..]].concat()),
            ("unknown kind", {
                let mut b = good.clone();
                b[27] = 9;
                b
            }),
            ("duplicate kind", two_passwords.encode()),
        ];
        for (name, bytes) in cases {
            assert!(
                matches!(
                    Keyfile::decode(&bytes),
                    Err(StorageError::KeyfileInvalid(_))
                ),
                "{name} accepted"
            );
        }
    }

    #[test]
    fn pragma_is_the_hex_key() {
        let key = DataKey([0xab; 32]);
        assert!(
            key.sqlcipher_pragma(Pragma::Rekey)
                .starts_with("PRAGMA rekey = \"x'ABAB")
        );
        let pragma = key.sqlcipher_pragma(Pragma::Key);
        assert_eq!(
            pragma.as_str(),
            format!("PRAGMA key = \"x'{}'\";", "AB".repeat(32))
        );
        // Sized exactly, so building it never reallocated (no stray copies).
        assert_eq!(pragma.len(), pragma.capacity());
        assert_eq!(format!("{key:?}"), "DataKey(<redacted>)");
    }

    #[test]
    fn zeroize_wipes_the_key() {
        let mut key = DataKey::generate().unwrap();
        assert!(!key.is_wiped());
        key.zeroize();
        assert!(key.is_wiped());
    }

    #[test]
    fn atomic_write_round_trips_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.keys");
        let key = DataKey::generate().unwrap();
        let kf = keyfile_with_password(&key);
        kf.write_atomic(&path).unwrap();
        let mut kf2 = kf.clone();
        kf2.set_slot(SlotKind::Password, b"other", P, &key).unwrap();
        kf2.write_atomic(&path).unwrap();
        assert_eq!(Keyfile::read(&path).unwrap(), kf2);
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["ledger.keys"]);
    }
}
