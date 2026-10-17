//! Backup file names: `fin-manager-<ledger id>-YYYYMMDDTHHMMSSZ.fmbackup`, with `-N`
//! before the extension when two backups of one ledger fall in the same second.
//!
//! The ledger id (10 lowercase hex digits, random, kept in the ledger's `meta` table)
//! tells ledgers apart when several share a folder: two Macs syncing the same Dropbox
//! folder, say. Rotation and deletion only ever touch files with this ledger's id.
//! The time is UTC and in the name, so listing and rotation never open a file.
//! Anything that doesn't parse is not ours and is never touched.

use crate::model::Date;

const PREFIX: &str = "fin-manager-";
pub(crate) const EXTENSION: &str = ".fmbackup";
/// A backup being written. Hidden, so sync tools and file browsers skip it.
const TMP_PREFIX: &str = ".fin-manager-";
const TMP_SUFFIX: &str = ".fmbackup.tmp";
pub(crate) const ID_LEN: usize = 10;

/// A backup's ledger id and time, from its file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Parsed {
    pub ledger_id: String,
    pub unix_secs: i64,
}

pub(crate) fn is_ledger_id(s: &str) -> bool {
    s.len() == ID_LEN
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The name for a backup of ledger `id` taken at `unix_secs`, with collision counter
/// `n` (0 = none).
pub(crate) fn backup_name(id: &str, unix_secs: i64, n: u32) -> String {
    format!("{PREFIX}{}{EXTENSION}", stem(id, unix_secs, n))
}

pub(crate) fn tmp_name(id: &str, unix_secs: i64, n: u32) -> String {
    format!("{TMP_PREFIX}{}{TMP_SUFFIX}", stem(id, unix_secs, n))
}

fn stem(id: &str, unix_secs: i64, n: u32) -> String {
    debug_assert!(is_ledger_id(id));
    let stamp = stamp(unix_secs);
    if n == 0 {
        format!("{id}-{stamp}")
    } else {
        format!("{id}-{stamp}-{n}")
    }
}

/// The ledger id and time of a backup from its file name, or `None` if the name isn't
/// one of ours.
pub(crate) fn parse_backup_name(name: &str) -> Option<Parsed> {
    parse_stem(name.strip_prefix(PREFIX)?.strip_suffix(EXTENSION)?)
}

/// True for a temporary file this app writes while making a backup.
pub(crate) fn is_tmp_name(name: &str) -> bool {
    name.strip_prefix(TMP_PREFIX)
        .and_then(|s| s.strip_suffix(TMP_SUFFIX))
        .and_then(parse_stem)
        .is_some()
}

fn parse_stem(stem: &str) -> Option<Parsed> {
    let mut parts = stem.splitn(3, '-');
    let id = parts.next()?;
    let stamp = parts.next()?;
    if let Some(n) = parts.next()
        && (n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    if !is_ledger_id(id) {
        return None;
    }
    Some(Parsed {
        ledger_id: id.to_string(),
        unix_secs: parse_stamp(stamp)?,
    })
}

/// `YYYYMMDDTHHMMSSZ` in UTC.
fn stamp(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs = unix_secs.rem_euclid(86_400);
    let (y, m, d) = i32::try_from(days)
        .ok()
        .and_then(Date::from_days_since_epoch)
        .map_or((1970, 1, 1), Date::ymd);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

fn parse_stamp(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 16 || b[8] != b'T' || b[15] != b'Z' {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        let part = s.get(r)?;
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let year = i32::try_from(digits(0..4)?).ok()?;
    let month = u8::try_from(digits(4..6)?).ok()?;
    let day = u8::try_from(digits(6..8)?).ok()?;
    let (h, m, sec) = (digits(9..11)?, digits(11..13)?, digits(13..15)?);
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    let date = Date::from_ymd(year, month, day).ok()?;
    Some(i64::from(date.days_since_epoch()) * 86_400 + i64::from(h * 3600 + m * 60 + sec))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0a1b2c3d4e";

    #[test]
    fn names_round_trip() {
        let t = 1_792_196_603; // 2026-10-17T00:23:23Z
        assert_eq!(
            backup_name(ID, t, 0),
            "fin-manager-0a1b2c3d4e-20261017T002323Z.fmbackup"
        );
        assert_eq!(
            backup_name(ID, t, 2),
            "fin-manager-0a1b2c3d4e-20261017T002323Z-2.fmbackup"
        );
        let parsed = Parsed {
            ledger_id: ID.into(),
            unix_secs: t,
        };
        assert_eq!(
            parse_backup_name(&backup_name(ID, t, 0)),
            Some(parsed.clone())
        );
        assert_eq!(parse_backup_name(&backup_name(ID, t, 2)), Some(parsed));
        assert_eq!(
            tmp_name(ID, t, 0),
            ".fin-manager-0a1b2c3d4e-20261017T002323Z.fmbackup.tmp"
        );
        assert!(is_tmp_name(&tmp_name(ID, t, 3)));
        assert_eq!(parse_backup_name(&tmp_name(ID, t, 0)), None);
    }

    #[test]
    fn other_files_are_not_ours() {
        for name in [
            "fin-manager-0a1b2c3d4e-20261017T002323Z.fmbackup.bak",
            "fin-manager-0a1b2c3d4e-20261017T002323Z.zip",
            "copy of fin-manager-0a1b2c3d4e-20261017T002323Z.fmbackup",
            "fin-manager-0a1b2c3d4e-20261017T002323Z (1).fmbackup",
            "fin-manager-20261017T002323Z.fmbackup",
            "fin-manager-0A1B2C3D4E-20261017T002323Z.fmbackup",
            "fin-manager-0a1b2c3d-20261017T002323Z.fmbackup",
            "fin-manager-0a1b2c3d4g-20261017T002323Z.fmbackup",
            "fin-manager-0a1b2c3d4e-20261332T000000Z.fmbackup",
            "fin-manager-0a1b2c3d4e-20261016T246000Z.fmbackup",
            "fin-manager-0a1b2c3d4e-20261017T002323Z-.fmbackup",
            "fin-manager-0a1b2c3d4e-20261017T002323Z-x.fmbackup",
            "fin-manager-0a1b2c3d4e-20261017T002323Z-1-2.fmbackup",
            "notes.txt",
            ".fin-manager-notes.fmbackup.tmp",
        ] {
            assert_eq!(parse_backup_name(name), None, "{name}");
            assert!(!is_tmp_name(name), "{name}");
        }
    }
}
