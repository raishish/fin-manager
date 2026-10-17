//! The backup container. All integers are little-endian.
//!
//! ```text
//! magic        8 bytes   "FMBACKUP"
//! version      u16       1
//! reserved     u16       0
//! created      i64       Unix seconds (UTC)
//! keyfile_len  u64
//! db_len       u64
//! app_len      u16       length of the app version string (≤ 64)
//! app_version  app_len bytes, UTF-8
//! keyfile      keyfile_len bytes: ledger.keys, as on disk
//! database     db_len bytes: ledger.db, as on disk (already encrypted)
//! sha256       32 bytes over everything above
//! ```
//!
//! Nothing is re-encrypted: both files are already protected (SPEC §8). The hash catches
//! truncation and corruption, for example a half-synced cloud copy. Tampering is caught
//! anyway, by the keyfile's AEAD and SQLCipher's page HMACs. Everything is streamed
//! through a fixed buffer, so a backup never has to fit in memory.

use std::io::{self, Read, Write};

use sha2::{Digest, Sha256};

use super::BackupError;

pub(crate) const MAGIC: &[u8; 8] = b"FMBACKUP";
pub(crate) const VERSION: u16 = 1;
/// Real keyfiles are a few hundred bytes; anything bigger isn't one.
const MAX_KEYFILE: u64 = 1 << 20;
const MAX_APP_VERSION: usize = 64;
const BUF: usize = 64 * 1024;
const FIXED: usize = 8 + 2 + 2 + 8 + 8 + 8 + 2;
const HASH: u64 = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Header {
    pub created_unix: i64,
    pub app_version: String,
    pub keyfile_len: u64,
    pub db_len: u64,
}

impl Header {
    fn encode(&self) -> Vec<u8> {
        let app = self.app_version.as_bytes();
        let app = &app[..app.len().min(MAX_APP_VERSION)];
        let mut out = Vec::with_capacity(FIXED + app.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.created_unix.to_le_bytes());
        out.extend_from_slice(&self.keyfile_len.to_le_bytes());
        out.extend_from_slice(&self.db_len.to_le_bytes());
        out.extend_from_slice(&(app.len() as u16).to_le_bytes());
        out.extend_from_slice(app);
        out
    }

    /// Reads a header, feeding its bytes to `hash`. Returns it and its encoded length.
    fn decode(r: &mut impl Read, hash: &mut Sha256) -> Result<(Header, u64), BackupError> {
        let mut fixed = [0u8; FIXED];
        // The magic first, so a short file that isn't a backup says so rather than
        // "truncated".
        let (magic, rest) = fixed.split_at_mut(8);
        if r.read_exact(magic).is_err() || magic != MAGIC {
            return Err(BackupError::NotABackup);
        }
        read_exact(r, rest)?;
        hash.update(fixed);
        let u16_at = |i: usize| u16::from_le_bytes([fixed[i], fixed[i + 1]]);
        let u64_at = |i: usize| u64::from_le_bytes(fixed[i..i + 8].try_into().expect("8 bytes"));
        let version = u16_at(8);
        if version != VERSION {
            return Err(BackupError::UnsupportedVersion(version));
        }
        let created_unix = i64::from_le_bytes(fixed[12..20].try_into().expect("8 bytes"));
        let keyfile_len = u64_at(20);
        let db_len = u64_at(28);
        let app_len = usize::from(u16_at(36));
        if keyfile_len == 0 || keyfile_len > MAX_KEYFILE || app_len > MAX_APP_VERSION {
            return Err(BackupError::Corrupt);
        }
        let mut app = vec![0u8; app_len];
        read_exact(r, &mut app)?;
        hash.update(&app);
        let app_version = String::from_utf8(app).map_err(|_| BackupError::Corrupt)?;
        let header = Header {
            created_unix,
            app_version,
            keyfile_len,
            db_len,
        };
        Ok((header, (FIXED + app_len) as u64))
    }
}

/// Writes a backup: the header, `keyfile_len` bytes of `keyfile`, `db_len` bytes of
/// `db`, then the hash. Fails if either source is shorter than its length.
pub(crate) fn write(
    out: &mut impl Write,
    header: &Header,
    keyfile: &mut impl Read,
    db: &mut impl Read,
    cancel: &dyn Fn() -> bool,
) -> Result<(), BackupError> {
    let mut hash = Sha256::new();
    let head = header.encode();
    hash.update(&head);
    out.write_all(&head)?;
    let mut buf = vec![0u8; BUF];
    copy_exact(
        keyfile,
        out,
        header.keyfile_len,
        &mut hash,
        &mut buf,
        cancel,
    )?;
    copy_exact(db, out, header.db_len, &mut hash, &mut buf, cancel)?;
    out.write_all(&hash.finalize())?;
    Ok(())
}

/// Reads a backup of `total_len` bytes, streaming the keyfile and the database to the
/// two writers, and checks the hash at the end. On any error the writers hold partial
/// data and must be discarded.
pub(crate) fn read(
    r: &mut impl Read,
    total_len: u64,
    keyfile_out: &mut impl Write,
    db_out: &mut impl Write,
    cancel: &dyn Fn() -> bool,
) -> Result<Header, BackupError> {
    let mut hash = Sha256::new();
    let (header, head_len) = Header::decode(r, &mut hash)?;
    let expected = head_len
        .checked_add(header.keyfile_len)
        .and_then(|n| n.checked_add(header.db_len))
        .and_then(|n| n.checked_add(HASH));
    match expected {
        Some(n) if n == total_len => {}
        Some(n) if n > total_len => return Err(BackupError::Truncated),
        _ => return Err(BackupError::Corrupt),
    }
    let mut buf = vec![0u8; BUF];
    copy_exact(
        r,
        keyfile_out,
        header.keyfile_len,
        &mut hash,
        &mut buf,
        cancel,
    )?;
    copy_exact(r, db_out, header.db_len, &mut hash, &mut buf, cancel)?;
    let mut stored = [0u8; 32];
    read_exact(r, &mut stored)?;
    if hash.finalize().as_slice() != stored {
        return Err(BackupError::Corrupt);
    }
    Ok(header)
}

/// Checks a backup without keeping its contents.
pub(crate) fn verify(
    r: &mut impl Read,
    total_len: u64,
    cancel: &dyn Fn() -> bool,
) -> Result<Header, BackupError> {
    read(r, total_len, &mut io::sink(), &mut io::sink(), cancel)
}

fn copy_exact(
    src: &mut impl Read,
    dst: &mut impl Write,
    mut len: u64,
    hash: &mut Sha256,
    buf: &mut [u8],
    cancel: &dyn Fn() -> bool,
) -> Result<(), BackupError> {
    while len > 0 {
        if cancel() {
            return Err(BackupError::Interrupted);
        }
        let want = usize::try_from(len).map_or(buf.len(), |l| l.min(buf.len()));
        let n = match src.read(&mut buf[..want]) {
            Ok(0) => return Err(BackupError::Truncated),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        hash.update(&buf[..n]);
        dst.write_all(&buf[..n])?;
        len -= n as u64;
    }
    Ok(())
}

fn read_exact(r: &mut impl Read, buf: &mut [u8]) -> Result<(), BackupError> {
    r.read_exact(buf).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => BackupError::Truncated,
        _ => e.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(keyfile: &[u8], db: &[u8]) -> Header {
        Header {
            created_unix: 1_792_196_603,
            app_version: "0.0.0".into(),
            keyfile_len: keyfile.len() as u64,
            db_len: db.len() as u64,
        }
    }

    fn backup(keyfile: &[u8], db: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        write(
            &mut out,
            &header(keyfile, db),
            &mut &keyfile[..],
            &mut &db[..],
            &|| false,
        )
        .unwrap();
        out
    }

    #[test]
    fn round_trip() {
        let keyfile = b"KEYFILE".to_vec();
        let db: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let file = backup(&keyfile, &db);
        let (mut k, mut d) = (Vec::new(), Vec::new());
        let h = read(&mut &file[..], file.len() as u64, &mut k, &mut d, &|| false).unwrap();
        assert_eq!(h, header(&keyfile, &db));
        assert_eq!((k, d), (keyfile, db));
    }

    #[test]
    fn any_flipped_byte_is_caught() {
        let file = backup(b"KEYFILE", b"DATABASE PAGES");
        for i in 0..file.len() {
            let mut bad = file.clone();
            bad[i] ^= 0x01;
            assert!(
                verify(&mut &bad[..], bad.len() as u64, &|| false).is_err(),
                "flip at byte {i} not caught"
            );
        }
    }

    #[test]
    fn truncation_is_caught() {
        let file = backup(b"KEYFILE", b"DATABASE PAGES");
        for cut in [0, 5, FIXED, file.len() - 33, file.len() - 1] {
            let short = &file[..cut];
            let err = verify(&mut &short[..], short.len() as u64, &|| false).unwrap_err();
            assert!(
                matches!(err, BackupError::Truncated | BackupError::NotABackup),
                "cut at {cut}: {err:?}"
            );
        }
        // Extra bytes on the end don't pass either.
        let mut long = file.clone();
        long.push(0);
        assert!(verify(&mut &long[..], long.len() as u64, &|| false).is_err());
    }

    #[test]
    fn other_files_are_refused_early() {
        let text = b"just some text that is long enough for a header....";
        assert!(matches!(
            verify(&mut &text[..], text.len() as u64, &|| false),
            Err(BackupError::NotABackup)
        ));
        let mut future = backup(b"K", b"D");
        future[8] = 9;
        assert!(matches!(
            verify(&mut &future[..], future.len() as u64, &|| false),
            Err(BackupError::UnsupportedVersion(9))
        ));
    }

    #[test]
    fn a_short_source_fails_the_write() {
        let mut h = header(b"KEYFILE", b"DATABASE");
        h.db_len += 10;
        let mut out = Vec::new();
        assert!(matches!(
            write(
                &mut out,
                &h,
                &mut &b"KEYFILE"[..],
                &mut &b"DATABASE"[..],
                &|| false
            ),
            Err(BackupError::Truncated)
        ));
    }

    #[test]
    fn huge_declared_lengths_are_refused_before_reading() {
        let mut file = backup(b"K", b"D");
        file[28..36].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(verify(&mut &file[..], file.len() as u64, &|| false).is_err());
        let mut file = backup(b"K", b"D");
        file[20..28].copy_from_slice(&(MAX_KEYFILE + 1).to_le_bytes());
        assert!(matches!(
            verify(&mut &file[..], file.len() as u64, &|| false),
            Err(BackupError::Corrupt)
        ));
    }

    #[test]
    fn cancelling_stops_between_chunks() {
        let db = vec![7u8; 3 * BUF];
        let mut out = Vec::new();
        let calls = std::cell::Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() > 2
        };
        let err = write(
            &mut out,
            &header(b"K", &db),
            &mut &b"K"[..],
            &mut &db[..],
            &cancel,
        )
        .unwrap_err();
        assert!(matches!(err, BackupError::Interrupted));
        assert!(out.len() < db.len(), "stopped early");
    }
}
