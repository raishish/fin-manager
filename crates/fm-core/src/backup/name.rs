//! Backup file names: `fin-manager-YYYYMMDDTHHMMSSZ.fmbackup`, or with `-N` before the
//! extension when two backups fall in the same second. The time is UTC and in the name,
//! so listing and rotation never open a file. Anything else in the folder is not ours
//! and is never touched.

use crate::model::Date;

const PREFIX: &str = "fin-manager-";
pub(crate) const EXTENSION: &str = ".fmbackup";
/// A backup being written. Hidden, so sync tools and file browsers skip it.
pub(crate) const TMP_PREFIX: &str = ".fin-manager-";
pub(crate) const TMP_SUFFIX: &str = ".fmbackup.tmp";

/// The name for a backup taken at `unix_secs`, with collision counter `n` (0 = none).
pub(crate) fn backup_name(unix_secs: i64, n: u32) -> String {
    let stamp = stamp(unix_secs);
    if n == 0 {
        format!("{PREFIX}{stamp}{EXTENSION}")
    } else {
        format!("{PREFIX}{stamp}-{n}{EXTENSION}")
    }
}

pub(crate) fn tmp_name(unix_secs: i64, n: u32) -> String {
    let name = backup_name(unix_secs, n);
    let stem = name
        .strip_prefix(PREFIX)
        .and_then(|s| s.strip_suffix(EXTENSION))
        .expect("built above");
    format!("{TMP_PREFIX}{stem}{TMP_SUFFIX}")
}

/// The time of a backup from its file name, or `None` if the name isn't one of ours.
pub(crate) fn parse_backup_name(name: &str) -> Option<i64> {
    let stem = name.strip_prefix(PREFIX)?.strip_suffix(EXTENSION)?;
    let stamp = match stem.split_once('-') {
        Some((stamp, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => stamp,
        Some(_) => return None,
        None => stem,
    };
    parse_stamp(stamp)
}

/// True for a temporary file this app writes while making a backup.
pub(crate) fn is_tmp_name(name: &str) -> bool {
    name.strip_prefix(TMP_PREFIX)
        .and_then(|s| s.strip_suffix(TMP_SUFFIX))
        .is_some_and(|stem| {
            let stamp = stem.split_once('-').map_or(stem, |(s, _)| s);
            parse_stamp(stamp).is_some()
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

    #[test]
    fn names_round_trip() {
        let t = 1_792_196_603; // 2026-10-17T00:23:23Z
        assert_eq!(backup_name(t, 0), "fin-manager-20261017T002323Z.fmbackup");
        assert_eq!(backup_name(t, 2), "fin-manager-20261017T002323Z-2.fmbackup");
        assert_eq!(parse_backup_name(&backup_name(t, 0)), Some(t));
        assert_eq!(parse_backup_name(&backup_name(t, 2)), Some(t));
        assert_eq!(tmp_name(t, 0), ".fin-manager-20261017T002323Z.fmbackup.tmp");
        assert!(is_tmp_name(&tmp_name(t, 3)));
        assert_eq!(parse_backup_name(&tmp_name(t, 0)), None);
    }

    #[test]
    fn other_files_are_not_ours() {
        for name in [
            "fin-manager-20261017T002323Z.fmbackup.bak",
            "fin-manager-20261017T002323Z.zip",
            "copy of fin-manager-20261017T002323Z.fmbackup",
            "fin-manager-20261017T002323Z (1).fmbackup",
            "fin-manager-2026-10-16.fmbackup",
            "fin-manager-20261332T000000Z.fmbackup",
            "fin-manager-20261016T246000Z.fmbackup",
            "fin-manager-20261017T002323Z-.fmbackup",
            "fin-manager-20261017T002323Z-x.fmbackup",
            "notes.txt",
            ".fin-manager-notes.fmbackup.tmp",
        ] {
            assert_eq!(parse_backup_name(name), None, "{name}");
            assert!(!is_tmp_name(name), "{name}");
        }
    }
}
