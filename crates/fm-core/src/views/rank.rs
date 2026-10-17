//! Row order and the top-`n` cut, shared by every view.

use super::{GroupBy, Key};

pub(super) type Keyed<T> = (Key, String, T);

/// Sorts rows by `score`, largest first (ties by key), and splits off everything after
/// the first `top` for the caller to add up into an `other` row. Month rows then go
/// back into calendar order.
pub(super) fn rank<T>(
    mut rows: Vec<Keyed<T>>,
    by: GroupBy,
    top: Option<usize>,
    score: impl Fn(&T) -> i128,
) -> (Vec<Keyed<T>>, Vec<Keyed<T>>) {
    rows.sort_by(|a, b| score(&b.2).cmp(&score(&a.2)).then_with(|| a.0.cmp(&b.0)));
    let rest = match top {
        Some(n) if n < rows.len() => rows.split_off(n),
        _ => Vec::new(),
    };
    if by == GroupBy::Month {
        rows.sort_by(|a, b| a.0.cmp(&b.0));
    }
    (rows, rest)
}
