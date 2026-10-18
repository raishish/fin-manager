//! The fallback explanation: one plain sentence per fact, written only from the facts,
//! so it always passes [`check`](super::check).

use super::facts::{Fact, Money, Value};

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A fact's name as words: `total_spent` is "Total spent".
pub(crate) fn label(name: &str) -> String {
    let words = name.replace(['_', '-'], " ");
    let mut chars = words.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn grouped(whole: u128) -> String {
    let digits = whole.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `$1,234.56`, `€12.30`, `¥500`, or `CHF 12.30` for currencies without a symbol here.
pub(crate) fn money(m: &Money) -> String {
    let abs = m.minor.unsigned_abs() as u128;
    let digits = u32::from(m.minor_digits);
    let unit = 10u128.pow(digits);
    let mut number = grouped(abs / unit);
    if digits > 0 {
        number.push_str(&format!(".{:0width$}", abs % unit, width = digits as usize));
    }
    let sign = if m.minor < 0 { "-" } else { "" };
    match m.currency.as_str() {
        "USD" => format!("{sign}${number}"),
        "EUR" => format!("{sign}€{number}"),
        "GBP" => format!("{sign}£{number}"),
        "JPY" => format!("{sign}¥{number}"),
        code => format!("{sign}{code} {number}"),
    }
}

/// `12.34%`, `28%`.
pub(crate) fn percent(basis_points: i64) -> String {
    let sign = if basis_points < 0 { "-" } else { "" };
    let bp = basis_points.unsigned_abs();
    let (whole, frac) = (bp / 100, bp % 100);
    if frac == 0 {
        format!("{sign}{whole}%")
    } else if frac.is_multiple_of(10) {
        format!("{sign}{whole}.{}%", frac / 10)
    } else {
        format!("{sign}{whole}.{frac:02}%")
    }
}

fn month(year: i32, m: u8) -> String {
    let name = MONTH_NAMES
        .get(usize::from(m).wrapping_sub(1))
        .unwrap_or(&"?");
    format!("{name} {year}")
}

/// The sentence for one fact.
pub(crate) fn sentence(fact: &Fact) -> String {
    let name = label(&fact.name);
    let value = match &fact.value {
        Value::Money(m) => money(m),
        Value::Count(n) => n.to_string(),
        Value::Percent { basis_points } => percent(*basis_points),
        Value::Date {
            year,
            month: m,
            day,
        } => {
            let name = MONTH_NAMES
                .get(usize::from(*m).wrapping_sub(1))
                .unwrap_or(&"?");
            format!("{name} {day}, {year}")
        }
        Value::Month { year, month: m } => month(*year, *m),
        Value::Year(y) => y.to_string(),
        Value::Text(t) => format!("\"{t}\""),
        Value::Texts(ts) => ts
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(", "),
    };
    match fact.evidence {
        Some(1) => format!("{name}: {value}, from 1 transaction."),
        Some(n) => format!("{name}: {value}, from {n} transactions."),
        None => format!("{name}: {value}."),
    }
}

/// The fallback explanation for `facts`.
pub fn template(facts: &[Fact]) -> String {
    facts.iter().map(sentence).collect::<Vec<_>>().join(" ")
}
