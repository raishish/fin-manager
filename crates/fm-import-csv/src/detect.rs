//! Recognising which known layout a file uses.

use crate::parse::{ImportError, find_header, parse, text};
use crate::profile::{Profile, Role};

/// The profile a file matches, or why none can be chosen.
///
/// A profile with a header matches when its header row appears (cells compared
/// trimmed, ignoring case) in the first lines. A headerless profile matches when the
/// file parses with it and its fixed-value columns hold their value. If several
/// profiles match, the ones that read the whole file without error win; if more than
/// one is still left, the file is ambiguous and needs a manual choice.
pub fn detect<'a>(bytes: &[u8], profiles: &'a [Profile]) -> Result<&'a Profile, ImportError> {
    let text = text(bytes)?;
    let mut by_header: Vec<&Profile> = profiles
        .iter()
        .filter(|p| p.csv.has_header && find_header(text, &p.csv).is_some())
        .collect();
    if by_header.is_empty() {
        by_header = profiles
            .iter()
            .filter(|p| !p.csv.has_header && headerless_fits(bytes, p))
            .collect();
    }
    match by_header.len() {
        0 => Err(ImportError::UnknownLayout),
        1 => Ok(by_header[0]),
        _ => {
            let parsed: Vec<&Profile> = by_header
                .iter()
                .copied()
                .filter(|p| parse(bytes, p, "USD").is_ok())
                .collect();
            match parsed.len() {
                1 => Ok(parsed[0]),
                0 => Err(ImportError::UnknownLayout),
                _ => Err(ImportError::Ambiguous(
                    parsed.iter().map(|p| p.id.clone()).collect(),
                )),
            }
        }
    }
}

fn headerless_fits(bytes: &[u8], p: &Profile) -> bool {
    let Ok(parsed) = parse(bytes, p, "USD") else {
        return false;
    };
    if parsed.iter().all(|s| s.records.is_empty()) {
        return false;
    }
    // Fixed-value columns (Wells Fargo's `*`) must hold their value on every row.
    let fixed: Vec<(usize, &str)> = p
        .csv
        .columns
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match &c.role {
            Role::Fixed { value } if !value.is_empty() => Some((i, value.as_str())),
            _ => None,
        })
        .collect();
    if fixed.is_empty() {
        return true;
    }
    let Ok(text) = text(bytes) else { return false };
    let body: String = text.split_inclusive('\n').skip(p.csv.skip_lines).collect();
    crate::csv::records(&body, p.csv.delimiter, 1)
        .iter()
        .all(|r| {
            fixed
                .iter()
                .all(|(i, v)| r.fields.get(*i).is_some_and(|f| f.trim() == *v))
        })
}
