//! Finding every number in an explanation: money, percents, plain and written-out
//! numbers, dates, months and years, each with where it is and how precisely it is
//! written (README.md lists the forms).

use std::ops::Range;

use super::facts::Dec;

/// How the text marks a currency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mark {
    /// `$`: the US dollar and the other dollars.
    Dollar,
    Euro,
    Pound,
    Yen,
    /// An ISO code: `USD 1,234`, `12.30 EUR`.
    Code(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `amount × 10^power` in a currency (`$1.2k` is 1.2 with power 3).
    Money {
        mark: Mark,
        amount: Dec,
        power: u32,
    },
    Percent {
        amount: Dec,
    },
    /// A number with no unit: a count, or a year.
    Plain {
        amount: Dec,
        power: u32,
        year_like: bool,
    },
    /// A day, with its year when the text gives one.
    Date {
        year: Option<i32>,
        month: u8,
        day: u8,
    },
    Month {
        year: i32,
        month: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Number {
    pub(crate) span: Range<usize>,
    pub(crate) kind: Kind,
    /// Preceded by a hedge word ("about", "roughly", ...): see README.md.
    pub(crate) hedged: bool,
}

/// The words that loosen the rounding a number must match (README.md, "Rounding").
/// "over", "under", "more than" and the like are not here: they state a bound, not the
/// value, and a bound is not checked, so a number after one must match exactly.
pub(crate) const HEDGES: [&str; 7] = [
    "about",
    "around",
    "roughly",
    "approximately",
    "approx",
    "nearly",
    "almost",
];

const MONTHS: [(&str, u8); 13] = [
    ("january", 1),
    ("february", 2),
    ("march", 3),
    ("april", 4),
    ("may", 5),
    ("june", 6),
    ("july", 7),
    ("august", 8),
    ("september", 9),
    ("sept", 9),
    ("october", 10),
    ("november", 11),
    ("december", 12),
];

const CODES: [&str; 16] = [
    "USD", "EUR", "GBP", "JPY", "CAD", "AUD", "CHF", "NZD", "SEK", "NOK", "DKK", "MXN", "INR",
    "CNY", "HKD", "SGD",
];

const UNITS: [(&str, u64); 20] = [
    ("zero", 0),
    ("one", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("six", 6),
    ("seven", 7),
    ("eight", 8),
    ("nine", 9),
    ("ten", 10),
    ("eleven", 11),
    ("twelve", 12),
    ("thirteen", 13),
    ("fourteen", 14),
    ("fifteen", 15),
    ("sixteen", 16),
    ("seventeen", 17),
    ("eighteen", 18),
    ("nineteen", 19),
];

const TENS: [(&str, u64); 8] = [
    ("twenty", 20),
    ("thirty", 30),
    ("forty", 40),
    ("fifty", 50),
    ("sixty", 60),
    ("seventy", 70),
    ("eighty", 80),
    ("ninety", 90),
];

/// Uses of "one" that aren't a count.
const NOT_ONE: [&str; 3] = ["one-off", "one off", "one-time"];

/// `text` with every occurrence of each of `masks` (case-insensitively) blanked out, at
/// the same byte offsets, so numbers inside names aren't read as numbers.
pub(crate) fn masked(text: &str, masks: &[String]) -> String {
    let lower = text.to_lowercase();
    // Lower-casing can change byte lengths outside ASCII; then mask the text as it is.
    let folded = lower.len() == text.len();
    let comparable = if folded { lower.as_str() } else { text };
    let mut blank = vec![false; text.len()];
    for m in masks.iter().filter(|m| !m.trim().is_empty()) {
        let m = if folded { m.to_lowercase() } else { m.clone() };
        let mut from = 0;
        while let Some(at) = comparable[from..].find(&m) {
            let start = from + at;
            blank[start..start + m.len()]
                .iter_mut()
                .for_each(|b| *b = true);
            from = start + m.len().max(1);
        }
    }
    text.char_indices()
        .map(|(i, c)| {
            if blank[i] {
                " ".repeat(c.len_utf8())
            } else {
                c.to_string()
            }
        })
        .collect()
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// Digits starting at `i`, with `,` thousands groups when they are well formed, and an
/// optional fraction: the value and the end.
fn digits(s: &[u8], i: usize) -> Option<(Dec, usize)> {
    let mut j = i;
    while j < s.len() && s[j].is_ascii_digit() {
        j += 1;
    }
    if j == i {
        return None;
    }
    let mut whole: String = std::str::from_utf8(&s[i..j]).ok()?.to_owned();
    // Thousands groups: ,ddd repeated, only after 1 to 3 leading digits.
    if j - i <= 3 {
        while j + 3 < s.len()
            && s[j] == b','
            && s[j + 1..j + 4].iter().all(u8::is_ascii_digit)
            && !s.get(j + 4).is_some_and(u8::is_ascii_digit)
        {
            whole.push_str(std::str::from_utf8(&s[j + 1..j + 4]).ok()?);
            j += 4;
        }
    }
    let mut scale = 0;
    if j + 1 < s.len() && s[j] == b'.' && s[j + 1].is_ascii_digit() {
        let mut k = j + 1;
        while k < s.len() && s[k].is_ascii_digit() {
            k += 1;
        }
        whole.push_str(std::str::from_utf8(&s[j + 1..k]).ok()?);
        scale = (k - j - 1) as u32;
        j = k;
    }
    Some((Dec::new(whole.parse().ok()?, scale), j))
}

/// The word (ASCII letters) starting at `i`, lower-cased.
fn word_at(s: &[u8], i: usize) -> Option<(String, usize)> {
    let mut j = i;
    while j < s.len() && s[j].is_ascii_alphabetic() {
        j += 1;
    }
    (j > i).then(|| (String::from_utf8_lossy(&s[i..j]).to_ascii_lowercase(), j))
}

fn month_at(s: &[u8], i: usize) -> Option<(u8, usize)> {
    if i > 0 && s[i - 1].is_ascii_alphabetic() {
        return None;
    }
    let (w, end) = word_at(s, i)?;
    let month = MONTHS.iter().find_map(|&(name, n)| {
        (w == name || (w.len() == 3 && name.starts_with(&w))).then_some(n)
    })?;
    // An abbreviation may end with a dot.
    let end = if w.len() <= 4 && s.get(end) == Some(&b'.') {
        end + 1
    } else {
        end
    };
    Some((month, end))
}

fn spaces(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && s[i] == b' ' {
        i += 1;
    }
    i
}

/// A day of the month (1 to 31), with an optional ordinal ending.
fn day_at(s: &[u8], i: usize) -> Option<(u8, usize)> {
    let (d, mut j) = digits(s, i)?;
    if d.scale != 0 || !(1..=31).contains(&d.mantissa) || j - i > 2 {
        return None;
    }
    if let Some((w, end)) = word_at(s, j)
        && ["st", "nd", "rd", "th"].contains(&w.as_str())
    {
        j = end;
    }
    (!s.get(j).copied().is_some_and(is_word)).then_some((d.mantissa as u8, j))
}

fn year_at(s: &[u8], i: usize) -> Option<(i32, usize)> {
    let (d, j) = digits(s, i)?;
    (d.scale == 0
        && j - i == 4
        && (1900..=2100).contains(&d.mantissa)
        && !s.get(j).copied().is_some_and(is_word))
    .then_some((d.mantissa as i32, j))
}

fn date_at(s: &[u8], i: usize) -> Option<(Kind, usize)> {
    if i > 0 && is_word(s[i - 1]) {
        return None;
    }
    // ISO: 2026-03-03, or a month: 2026-03.
    if let Some((y, j)) = digits(s, i)
        && y.scale == 0
        && j - i == 4
        && s.get(j) == Some(&b'-')
        && let Some((m, k)) = digits(s, j + 1)
        && k - j - 1 == 2
        && (1..=12).contains(&m.mantissa)
    {
        let (year, month) = (y.mantissa as i32, m.mantissa as u8);
        if s.get(k) == Some(&b'-')
            && let Some((d, e)) = digits(s, k + 1)
            && e - k - 1 == 2
            && (1..=31).contains(&d.mantissa)
            && !s.get(e).copied().is_some_and(is_word)
        {
            return Some((
                Kind::Date {
                    year: Some(year),
                    month,
                    day: d.mantissa as u8,
                },
                e,
            ));
        }
        if !s.get(k).copied().is_some_and(|b| is_word(b) || b == b'-') {
            return Some((Kind::Month { year, month }, k));
        }
    }
    // US: 03/03/2026.
    if let Some((m, j)) = digits(s, i)
        && s.get(j) == Some(&b'/')
        && let Some((d, k)) = digits(s, j + 1)
        && s.get(k) == Some(&b'/')
        && let Some((y, e)) = year_at(s, k + 1)
        && (1..=12).contains(&m.mantissa)
        && (1..=31).contains(&d.mantissa)
    {
        return Some((
            Kind::Date {
                year: Some(y),
                month: m.mantissa as u8,
                day: d.mantissa as u8,
            },
            e,
        ));
    }
    // March 3, 2026 / March 3 / March 2026.
    if let Some((month, j)) = month_at(s, i) {
        let k = spaces(s, j);
        if k > j {
            if let Some((day, e)) = day_at(s, k) {
                let mut after = e;
                if s.get(after) == Some(&b',') {
                    after += 1;
                }
                let y0 = spaces(s, after);
                if y0 > e
                    && let Some((year, end)) = year_at(s, y0)
                {
                    return Some((
                        Kind::Date {
                            year: Some(year),
                            month,
                            day,
                        },
                        end,
                    ));
                }
                return Some((
                    Kind::Date {
                        year: None,
                        month,
                        day,
                    },
                    e,
                ));
            }
            if let Some((year, end)) = year_at(s, k) {
                return Some((Kind::Month { year, month }, end));
            }
        }
    }
    // 3 March 2026 / 3rd of March.
    if let Some((day, j)) = day_at(s, i) {
        let mut k = spaces(s, j);
        if s[k..].starts_with(b"of ") {
            k = spaces(s, k + 3);
        }
        if k > j
            && let Some((month, e)) = month_at(s, k)
        {
            let y0 = spaces(s, e);
            if y0 > e
                && let Some((year, end)) = year_at(s, y0)
            {
                return Some((
                    Kind::Date {
                        year: Some(year),
                        month,
                        day,
                    },
                    end,
                ));
            }
            return Some((
                Kind::Date {
                    year: None,
                    month,
                    day,
                },
                e,
            ));
        }
    }
    None
}

/// The currency mark ending just before `i` (`$`, `USD `), and where it starts.
fn mark_before(text: &str, i: usize) -> Option<(Mark, usize)> {
    let before = &text[..i];
    for (sym, mark) in [
        ("$", Mark::Dollar),
        ("€", Mark::Euro),
        ("£", Mark::Pound),
        ("¥", Mark::Yen),
    ] {
        if let Some(rest) = before.strip_suffix(sym) {
            return Some((mark, rest.len()));
        }
    }
    let trimmed = before.trim_end_matches(' ');
    if trimmed.len() + 1 >= before.len() && trimmed.len() >= 3 {
        let code = &trimmed[trimmed.len() - 3..];
        let start = trimmed.len() - 3;
        if CODES.contains(&code)
            && !trimmed[..start]
                .bytes()
                .last()
                .is_some_and(|b| b.is_ascii_alphabetic())
        {
            return Some((Mark::Code(code.to_owned()), start));
        }
    }
    None
}

/// Whether the word before byte `i` is a hedge.
fn hedged_before(text: &str, i: usize) -> bool {
    let before = text[..i].trim_end_matches(' ');
    if before.ends_with('~') {
        return true;
    }
    let word: String = before
        .chars()
        .rev()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .to_ascii_lowercase();
    HEDGES.contains(&word.as_str())
}

/// What follows a number: a scale word or letter, then a unit.
fn suffix(s: &[u8], mut j: usize) -> Option<(u32, Option<Unit>, usize)> {
    let mut power = 0;
    // 12k, 1.2M, 3bn; or 1.2 million.
    if let Some((w, end)) = word_at(s, j) {
        power = match w.as_str() {
            "k" => 3,
            "m" | "mn" => 6,
            "b" | "bn" => 9,
            _ => return None,
        };
        j = end;
    } else {
        let k = spaces(s, j);
        if k > j
            && let Some((w, end)) = word_at(s, k)
        {
            let p = match w.as_str() {
                "thousand" => 3,
                "million" => 6,
                "billion" => 9,
                _ => 0,
            };
            if p > 0 {
                power = p;
                j = end;
            }
        }
    }
    let k = spaces(s, j);
    if s.get(k) == Some(&b'%') {
        return Some((power, Some(Unit::Percent), k + 1));
    }
    if let Some((w, end)) = word_at(s, k) {
        if k > j || power == 0 {
            match w.as_str() {
                "percent" | "pct" => return Some((power, Some(Unit::Percent), end)),
                "per" if s[end..].starts_with(b" cent") => {
                    return Some((power, Some(Unit::Percent), end + 5));
                }
                "percentage" => {
                    let p = spaces(s, end);
                    let end = word_at(s, p)
                        .filter(|(w, _)| w == "point" || w == "points")
                        .map_or(end, |(_, e)| e);
                    return Some((power, Some(Unit::Percent), end));
                }
                "dollar" | "dollars" | "bucks" => {
                    return Some((power, Some(Unit::Money(Mark::Dollar)), end));
                }
                "euro" | "euros" => return Some((power, Some(Unit::Money(Mark::Euro)), end)),
                "pound" | "pounds" => return Some((power, Some(Unit::Money(Mark::Pound)), end)),
                "yen" => return Some((power, Some(Unit::Money(Mark::Yen)), end)),
                _ => {}
            }
            let upper = w.to_ascii_uppercase();
            if k > j && CODES.contains(&upper.as_str()) && end - k == 3 {
                return Some((power, Some(Unit::Money(Mark::Code(upper))), end));
            }
        }
        if k == j {
            // Letters straight after the digits: an ordinal (3rd) or part of a word.
            return None;
        }
    }
    Some((power, None, j))
}

enum Unit {
    Percent,
    Money(Mark),
}

/// A written-out whole number starting at `i`: "twelve", "twenty-five", "a hundred".
fn words_at(lower: &str, i: usize) -> Option<(u64, usize)> {
    let s = lower.as_bytes();
    if i > 0 && s[i - 1].is_ascii_alphabetic() {
        return None;
    }
    if NOT_ONE.iter().any(|n| lower[i..].starts_with(n)) {
        return None;
    }
    let (w, mut end) = word_at(s, i)?;
    let mut value;
    if w == "a" || w == "an" {
        let k = spaces(s, end);
        let (next, e) = word_at(s, k)?;
        value = match next.as_str() {
            "dozen" => 12,
            "hundred" => 100,
            "thousand" => 1000,
            _ => return None,
        };
        return Some((value, e));
    }
    if let Some(&(_, t)) = TENS.iter().find(|(n, _)| *n == w) {
        value = t;
        if matches!(s.get(end), Some(b'-' | b' '))
            && let Some((u, e)) = word_at(s, end + 1)
            && let Some(&(_, n)) = UNITS
                .iter()
                .find(|(name, n)| *name == u && (1..=9).contains(n))
        {
            value += n;
            end = e;
        }
    } else if let Some(&(_, n)) = UNITS.iter().find(|(name, _)| *name == w) {
        value = n;
    } else if w == "dozen" {
        value = 12;
    } else {
        return None;
    }
    let k = spaces(s, end);
    if k > end
        && let Some((scale, e)) = word_at(s, k)
    {
        let factor = match scale.as_str() {
            "hundred" => 100,
            "thousand" => 1000,
            "million" => 1_000_000,
            _ => 1,
        };
        if factor > 1 {
            value *= factor;
            end = e;
        }
    }
    Some((value, end))
}

/// Every number in `text` that isn't inside one of `masks` (names and labels).
pub(crate) fn numbers(text: &str, masks: &[String]) -> Vec<Number> {
    let text = masked(text, masks);
    let s = text.as_bytes();
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if let Some((kind, end)) = date_at(s, i) {
            out.push(Number {
                span: i..end,
                kind,
                hedged: hedged_before(&text, i),
            });
            i = end;
            continue;
        }
        if s[i].is_ascii_digit()
            && !(i > 0 && (s[i - 1].is_ascii_alphabetic() || s[i - 1].is_ascii_digit()))
        {
            let Some((amount, j)) = digits(s, i) else {
                i += 1;
                continue;
            };
            let Some((power, unit, end)) = suffix(s, j) else {
                // Part of a word or an ordinal: skip the whole run.
                let mut k = j;
                while k < s.len() && is_word(s[k]) {
                    k += 1;
                }
                i = k;
                continue;
            };
            let mark = mark_before(&text, i);
            let mut start = mark.as_ref().map_or(i, |(_, at)| *at);
            if start > 0 && (s[start - 1] == b'-' || text[..start].ends_with('\u{2212}')) {
                start -= if s[start - 1] == b'-' {
                    1
                } else {
                    '\u{2212}'.len_utf8()
                };
            }
            let hedged = hedged_before(&text, start);
            let kind = match (mark, unit) {
                (Some((m, _)), _) | (None, Some(Unit::Money(m))) => Kind::Money {
                    mark: m,
                    amount,
                    power,
                },
                (None, Some(Unit::Percent)) => Kind::Percent {
                    amount: amount.shifted(0),
                },
                (None, None) => Kind::Plain {
                    amount,
                    power,
                    year_like: power == 0
                        && amount.scale == 0
                        && j - i == 4
                        && (1900..=2100).contains(&amount.mantissa),
                },
            };
            let kind = match kind {
                Kind::Percent { amount } if power > 0 => Kind::Percent {
                    amount: Dec::new(amount.mantissa * 10i128.pow(power), amount.scale),
                },
                k => k,
            };
            out.push(Number {
                span: start..end,
                kind,
                hedged,
            });
            i = end;
            continue;
        }
        if s[i].is_ascii_alphabetic() {
            if let Some((value, end)) = words_at(&lower, i) {
                out.push(Number {
                    span: i..end,
                    kind: Kind::Plain {
                        amount: Dec::new(i128::from(value), 0),
                        power: 0,
                        year_like: false,
                    },
                    hedged: hedged_before(&text, i),
                });
                i = end;
                continue;
            }
            // Skip the rest of the word.
            while i < s.len() && s[i].is_ascii_alphabetic() {
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    out
}
