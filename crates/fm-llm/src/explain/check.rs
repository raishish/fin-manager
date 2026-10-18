//! Checking an explanation's numbers against the computed values (SPEC constraint 6):
//! every number in the text must be one of the facts, a fact's evidence count, or a
//! derivation the caller allowed. Matching rules are in README.md.

use std::ops::Range;

use super::extract::{self, Kind, Mark, Number};
use super::facts::{Dec, Derivation, Fact, Value};

/// A number the text states that matches nothing it may state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unchecked {
    /// Byte offsets in the text.
    pub span: Range<usize>,
    /// What kind of number it was read as, e.g. `money` or `percent`.
    pub kind: &'static str,
}

/// The outcome of a check. Holds offsets and kinds, never the numbers themselves, so it
/// is safe to keep (SPEC §7).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// How many numbers the text states.
    pub found: usize,
    pub unchecked: Vec<Unchecked>,
}

impl Report {
    /// Every number in the text is accounted for.
    pub fn passed(&self) -> bool {
        self.unchecked.is_empty()
    }
}

/// The values a text may state, by kind.
#[derive(Debug, Default)]
struct Allowed {
    /// (currency code, absolute amount).
    money: Vec<(String, Dec)>,
    /// Absolute percents (12.34, not basis points).
    percents: Vec<Dec>,
    counts: Vec<i128>,
    years: Vec<i32>,
    months: Vec<(i32, u8)>,
    dates: Vec<(i32, u8, u8)>,
}

/// The value of a fact as a plain number, for derivations: money (with its currency),
/// a count, or a percent.
enum Amount {
    Money(String, Dec),
    Count(i128),
    Percent(Dec),
}

fn amount(fact: &Fact) -> Option<Amount> {
    match &fact.value {
        Value::Money(m) => Some(Amount::Money(m.currency.clone(), m.dec())),
        Value::Count(n) => Some(Amount::Count(i128::from(*n))),
        Value::Percent { basis_points } => {
            Some(Amount::Percent(Dec::new(i128::from(*basis_points), 2)))
        }
        _ => None,
    }
}

/// `num / den` as a percent, to 4 decimals (half away from zero).
fn percent_of(num: Dec, den: Dec) -> Option<Dec> {
    let scale = num.scale.max(den.scale);
    let (n, d) = (num.rescaled(scale)?, den.rescaled(scale)?);
    if d == 0 {
        return None;
    }
    // 100 × n / d with 4 decimals: n × 10^6 / d.
    let scaled = n.checked_mul(1_000_000)?;
    let (q, r) = (scaled / d, scaled % d);
    let q = if (r.abs() * 2) >= d.abs() {
        q + scaled.signum() * d.signum()
    } else {
        q
    };
    Some(Dec::new(q, 4))
}

impl Allowed {
    fn from(facts: &[Fact], derivations: &[Derivation]) -> Allowed {
        let mut a = Allowed::default();
        for f in facts {
            if let Some(e) = f.evidence {
                a.counts.push(i128::from(e));
            }
            match &f.value {
                Value::Money(m) => a.money.push((m.currency.clone(), m.dec().abs())),
                Value::Count(n) => a.counts.push(i128::from(*n)),
                Value::Percent { basis_points } => a
                    .percents
                    .push(Dec::new(i128::from(*basis_points), 2).abs()),
                Value::Date { year, month, day } => {
                    a.dates.push((*year, *month, *day));
                    a.months.push((*year, *month));
                    a.years.push(*year);
                }
                Value::Month { year, month } => {
                    a.months.push((*year, *month));
                    a.years.push(*year);
                }
                Value::Year(y) => a.years.push(*y),
                Value::Text(_) | Value::Texts(_) => {}
            }
        }
        let by_name = |n: &str| facts.iter().find(|f| f.name == n).and_then(amount);
        for d in derivations {
            match d {
                Derivation::Difference(x, y) => match (by_name(x), by_name(y)) {
                    (Some(Amount::Money(c1, v1)), Some(Amount::Money(c2, v2))) if c1 == c2 => {
                        if let Some(v) = v1.minus(v2) {
                            a.money.push((c1, v.abs()));
                        }
                    }
                    (Some(Amount::Count(v1)), Some(Amount::Count(v2))) => {
                        a.counts.push((v1 - v2).abs());
                    }
                    (Some(Amount::Percent(v1)), Some(Amount::Percent(v2))) => {
                        if let Some(v) = v1.minus(v2) {
                            a.percents.push(v.abs());
                        }
                    }
                    _ => {}
                },
                Derivation::Sum(names) => {
                    let parts: Vec<Amount> = names.iter().filter_map(|n| by_name(n)).collect();
                    if parts.len() != names.len() {
                        continue;
                    }
                    match parts.first() {
                        Some(Amount::Money(c, _)) => {
                            let c = c.clone();
                            let total = parts.iter().try_fold(Dec::new(0, 0), |acc, p| match p {
                                Amount::Money(c2, v) if *c2 == c => acc.plus(*v),
                                _ => None,
                            });
                            if let Some(t) = total {
                                a.money.push((c, t.abs()));
                            }
                        }
                        Some(Amount::Count(_)) => {
                            let total: Option<i128> = parts.iter().try_fold(0, |acc, p| match p {
                                Amount::Count(v) => Some(acc + v),
                                _ => None,
                            });
                            if let Some(t) = total {
                                a.counts.push(t.abs());
                            }
                        }
                        _ => {}
                    }
                }
                Derivation::Ratio(x, y) | Derivation::PercentChange(x, y) => {
                    let value = |am: Amount| match am {
                        Amount::Money(c, v) => Some((Some(c), v)),
                        Amount::Count(n) => Some((None, Dec::new(n, 0))),
                        Amount::Percent(_) => None,
                    };
                    let (Some((c1, v1)), Some((c2, v2))) =
                        (by_name(x).and_then(value), by_name(y).and_then(value))
                    else {
                        continue;
                    };
                    if c1 != c2 {
                        continue;
                    }
                    let num = if matches!(d, Derivation::Ratio(..)) {
                        Some(v1)
                    } else {
                        v1.minus(v2)
                    };
                    if let Some(p) = num.and_then(|n| percent_of(n, v2)) {
                        a.percents.push(p.abs());
                    }
                }
            }
        }
        a
    }
}

/// The currency codes a mark can mean.
fn mark_fits(mark: &Mark, code: &str) -> bool {
    match mark {
        Mark::Dollar => ["USD", "CAD", "AUD", "NZD", "SGD", "HKD", "MXN"].contains(&code),
        Mark::Euro => code == "EUR",
        Mark::Pound => code == "GBP",
        Mark::Yen => code == "JPY" || code == "CNY",
        Mark::Code(c) => c == code,
    }
}

/// The significant digits a whole or decimal number is written with: trailing zeros of a
/// whole number don't count ("1,200" has 2), every digit after a decimal point does.
fn significant(shown: Dec) -> u32 {
    let digits = shown.mantissa.abs().to_string();
    let trimmed = digits.trim_start_matches('0');
    if shown.scale == 0 {
        trimmed.trim_end_matches('0').len() as u32
    } else {
        trimmed.len() as u32
    }
}

/// Whether `value` (exact) is what the text shows as `shown`.
///
/// Plain: `value` rounded to as many decimals as `shown` has equals it, so "$1,234.56"
/// is exact, "$1,235" is right for 1,234.56, and "$1,234" isn't.
/// Hedged ("about", ...): `value` rounded to `shown`'s significant digits equals it, it
/// has at least two, and it is within 5%: "about $1,200" is right for 1,234.56;
/// "about $1,000" isn't (one significant digit).
fn shows(value: Dec, shown: Dec, hedged: bool) -> bool {
    let (value, shown) = (value.abs(), shown.abs());
    if value.rounded(shown.scale) == Some(shown.mantissa) {
        return true;
    }
    if !hedged {
        return false;
    }
    let sig = significant(shown);
    if sig < 2 {
        return false;
    }
    // The power of ten of the last significant digit shown.
    let magnitude = shown.mantissa.abs().to_string().len() as i32 - shown.scale as i32;
    let last = magnitude - sig as i32;
    let rounded = if last >= 0 {
        value
            .shifted(last as u32)
            .rounded(0)
            .and_then(|r| r.checked_mul(10i128.checked_pow(last as u32)?))
            .map(|r| Dec::new(r, 0))
    } else {
        value
            .rounded((-last) as u32)
            .map(|r| Dec::new(r, (-last) as u32))
    };
    let same = rounded
        .and_then(|r| r.minus(shown))
        .is_some_and(|d| d.mantissa == 0);
    let close = value.as_f64() == 0.0
        || ((value.as_f64() - shown.as_f64()).abs() / value.as_f64().abs()) <= 0.05;
    same && close
}

/// Checks every number in `text` against `facts` and the allowed `derivations`.
///
/// Facts' names and their text values are taken out of the text first, so a merchant
/// called "IN-N-OUT 592" or a label like "Q4 2025" isn't read as numbers.
pub fn check(text: &str, facts: &[Fact], derivations: &[Derivation]) -> Report {
    let allowed = Allowed::from(facts, derivations);
    let mut masks: Vec<String> = Vec::new();
    for f in facts {
        masks.push(f.name.clone());
        masks.push(super::template::label(&f.name));
        match &f.value {
            Value::Text(t) => masks.push(t.clone()),
            Value::Texts(ts) => masks.extend(ts.iter().cloned()),
            _ => {}
        }
    }
    // Longest first, so a longer name is masked whole before a shorter one inside it.
    masks.sort_by_key(|m| std::cmp::Reverse(m.len()));
    let numbers = extract::numbers(text, &masks);
    let mut report = Report {
        found: numbers.len(),
        unchecked: Vec::new(),
    };
    for Number { span, kind, hedged } in numbers {
        let (ok, what) = match kind {
            Kind::Money {
                mark,
                amount,
                power,
            } => (
                allowed.money.iter().any(|(code, v)| {
                    mark_fits(&mark, code) && shows(v.shifted(power), amount, hedged)
                }),
                "money",
            ),
            Kind::Percent { amount } => (
                allowed.percents.iter().any(|v| shows(*v, amount, hedged)),
                "percent",
            ),
            Kind::Plain {
                amount,
                power,
                year_like,
            } => {
                let count = allowed
                    .counts
                    .iter()
                    .any(|c| shows(Dec::new(*c, 0).shifted(power), amount, hedged));
                let year = year_like && allowed.years.contains(&(amount.mantissa as i32));
                (
                    count || year,
                    if year_like { "year or count" } else { "count" },
                )
            }
            Kind::Date { year, month, day } => (
                allowed
                    .dates
                    .iter()
                    .any(|&(y, m, d)| m == month && d == day && year.is_none_or(|yy| yy == y)),
                "date",
            ),
            Kind::Month { year, month } => (allowed.months.contains(&(year, month)), "month"),
        };
        if !ok {
            report.unchecked.push(Unchecked { span, kind: what });
        }
    }
    report
}
