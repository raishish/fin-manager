//! Exact money parsing shared by the importers.

use std::fmt;

/// ISO 4217 currencies and their minor-unit exponents (USD 2, JPY 0, KWD 3, CLF 4).
///
/// **A copy of `fm_core::model::money::CURRENCIES`, and append-only like it:** a code is
/// never removed and its exponent never changes once shipped, because stored amounts are
/// minor units whose meaning depends on it. `fm-core` has a test asserting the two
/// tables are identical, so they can't drift apart.
#[rustfmt::skip]
pub const CURRENCIES: &[(&str, u8)] = &[
    ("AED", 2), ("AFN", 2), ("ALL", 2), ("AMD", 2), ("ANG", 2), ("AOA", 2), ("ARS", 2),
    ("AUD", 2), ("AWG", 2), ("AZN", 2), ("BAM", 2), ("BBD", 2), ("BDT", 2), ("BGN", 2),
    ("BHD", 3), ("BIF", 0), ("BMD", 2), ("BND", 2), ("BOB", 2), ("BOV", 2), ("BRL", 2),
    ("BSD", 2), ("BTN", 2), ("BWP", 2), ("BYN", 2), ("BZD", 2), ("CAD", 2), ("CDF", 2),
    ("CHE", 2), ("CHF", 2), ("CHW", 2), ("CLF", 4), ("CLP", 0), ("CNY", 2), ("COP", 2),
    ("COU", 2), ("CRC", 2), ("CUC", 2), ("CUP", 2), ("CVE", 2), ("CZK", 2), ("DJF", 0),
    ("DKK", 2), ("DOP", 2), ("DZD", 2), ("EGP", 2), ("ERN", 2), ("ETB", 2), ("EUR", 2),
    ("FJD", 2), ("FKP", 2), ("GBP", 2), ("GEL", 2), ("GHS", 2), ("GIP", 2), ("GMD", 2),
    ("GNF", 0), ("GTQ", 2), ("GYD", 2), ("HKD", 2), ("HNL", 2), ("HRK", 2), ("HTG", 2),
    ("HUF", 2), ("IDR", 2), ("ILS", 2), ("INR", 2), ("IQD", 3), ("IRR", 2), ("ISK", 0),
    ("JMD", 2), ("JOD", 3), ("JPY", 0), ("KES", 2), ("KGS", 2), ("KHR", 2), ("KMF", 0),
    ("KPW", 2), ("KRW", 0), ("KWD", 3), ("KYD", 2), ("KZT", 2), ("LAK", 2), ("LBP", 2),
    ("LKR", 2), ("LRD", 2), ("LSL", 2), ("LYD", 3), ("MAD", 2), ("MDL", 2), ("MGA", 2),
    ("MKD", 2), ("MMK", 2), ("MNT", 2), ("MOP", 2), ("MRU", 2), ("MUR", 2), ("MVR", 2),
    ("MWK", 2), ("MXN", 2), ("MXV", 2), ("MYR", 2), ("MZN", 2), ("NAD", 2), ("NGN", 2),
    ("NIO", 2), ("NOK", 2), ("NPR", 2), ("NZD", 2), ("OMR", 3), ("PAB", 2), ("PEN", 2),
    ("PGK", 2), ("PHP", 2), ("PKR", 2), ("PLN", 2), ("PYG", 0), ("QAR", 2), ("RON", 2),
    ("RSD", 2), ("RUB", 2), ("RWF", 0), ("SAR", 2), ("SBD", 2), ("SCR", 2), ("SDG", 2),
    ("SEK", 2), ("SGD", 2), ("SHP", 2), ("SLE", 2), ("SLL", 2), ("SOS", 2), ("SRD", 2),
    ("SSP", 2), ("STN", 2), ("SVC", 2), ("SYP", 2), ("SZL", 2), ("THB", 2), ("TJS", 2),
    ("TMT", 2), ("TND", 3), ("TOP", 2), ("TRY", 2), ("TTD", 2), ("TWD", 2), ("TZS", 2),
    ("UAH", 2), ("UGX", 0), ("USD", 2), ("USN", 2), ("UYI", 0), ("UYU", 2), ("UYW", 4),
    ("UZS", 2), ("VED", 2), ("VES", 2), ("VND", 0), ("VUV", 0), ("WST", 2), ("XAF", 0),
    ("XCD", 2), ("XCG", 2), ("XOF", 0), ("XPF", 0), ("YER", 2), ("ZAR", 2), ("ZMW", 2),
    ("ZWG", 2), ("ZWL", 2),
];

/// The number of minor-unit digits for an ISO 4217 code (`"USD"` → 2, `"JPY"` → 0), or
/// `None` for a code that isn't in the table. Codes must be three uppercase letters; an
/// unknown code is never guessed.
pub fn minor_exponent(code: &str) -> Option<u8> {
    CURRENCIES.iter().find(|(c, _)| *c == code).map(|&(_, e)| e)
}

/// Why an amount was refused. Carries no input text, so it is safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmountError {
    /// Not a plain decimal number (or badly placed thousands separators).
    NotADecimal,
    /// More non-zero decimal places than the currency has.
    TooPrecise,
    /// Doesn't fit in an i64 of minor units.
    TooLarge,
}

impl fmt::Display for AmountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AmountError::NotADecimal => "not a plain decimal number",
            AmountError::TooPrecise => "more decimal places than the currency has",
            AmountError::TooLarge => "amount too large",
        })
    }
}

impl std::error::Error for AmountError {}

/// Parses a plain decimal (`"-67.79"`, `"+12"`, `"0.50"`) into signed minor units for a
/// currency with `exponent` decimal places, exactly and without floats.
///
/// The same rules as `fm_core::model::Money::from_decimal_str`: an optional leading `+` or
/// `-`, at least one digit before an optional `.`, at least one digit after it, extra
/// trailing zero decimals allowed (`"1.500"` in USD), more non-zero precision refused.
/// No spaces, thousands separators, currency symbols or parentheses: the importer strips
/// those first (see [`parse_grouped_decimal_minor`] for comma grouping).
pub fn parse_decimal_minor(text: &str, exponent: u8) -> Result<i64, AmountError> {
    let (negative, unsigned) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((_, "")) => return Err(AmountError::NotADecimal),
        Some((whole, fraction)) => (whole, fraction),
        None => (unsigned, ""),
    };
    let all_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || !all_digits(whole) || !all_digits(fraction) {
        return Err(AmountError::NotADecimal);
    }

    let exponent = usize::from(exponent);
    let (kept, dropped) = fraction.split_at(fraction.len().min(exponent));
    if dropped.bytes().any(|b| b != b'0') {
        return Err(AmountError::TooPrecise);
    }

    let per_major = 10_i64
        .checked_pow(exponent as u32)
        .ok_or(AmountError::TooLarge)?;
    let whole: i64 = whole.parse().map_err(|_| AmountError::TooLarge)?;
    let mut fraction_minor: i64 = if kept.is_empty() {
        0
    } else {
        kept.parse().map_err(|_| AmountError::TooLarge)?
    };
    for _ in kept.len()..exponent {
        fraction_minor *= 10;
    }
    let minor = whole
        .checked_mul(per_major)
        .and_then(|m| m.checked_add(fraction_minor))
        .ok_or(AmountError::TooLarge)?;
    Ok(if negative { -minor } else { minor })
}

/// Like [`parse_decimal_minor`], but also accepts comma thousands separators in the whole
/// part (`"1,234.56"`, `"-12,345,678"`), only in groups of exactly three digits after the
/// first group of one to three. `"1,23.45"` and `"1234,567"` are refused.
pub fn parse_grouped_decimal_minor(text: &str, exponent: u8) -> Result<i64, AmountError> {
    if !text.contains(',') {
        return parse_decimal_minor(text, exponent);
    }
    let (sign, rest) = match text.as_bytes().first() {
        Some(b'-') | Some(b'+') => text.split_at(1),
        _ => ("", text),
    };
    let (whole, fraction) = match rest.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (rest, None),
    };
    let mut groups = whole.split(',');
    let first = groups.next().unwrap_or("");
    let ok_first = (1..=3).contains(&first.len()) && first.bytes().all(|b| b.is_ascii_digit());
    let ok_rest = groups.all(|g| g.len() == 3 && g.bytes().all(|b| b.is_ascii_digit()));
    if !ok_first || !ok_rest {
        return Err(AmountError::NotADecimal);
    }
    let mut plain = String::with_capacity(text.len());
    plain.push_str(sign);
    plain.extend(whole.chars().filter(|&c| c != ','));
    if let Some(f) = fraction {
        plain.push('.');
        plain.push_str(f);
    }
    parse_decimal_minor(&plain, exponent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponents_come_from_the_table() {
        assert_eq!(minor_exponent("USD"), Some(2));
        assert_eq!(minor_exponent("JPY"), Some(0));
        assert_eq!(minor_exponent("KWD"), Some(3));
        assert_eq!(minor_exponent("CLF"), Some(4));
        assert_eq!(minor_exponent("usd"), None);
        assert_eq!(minor_exponent("XXX"), None);
        assert_eq!(minor_exponent(""), None);
    }

    #[test]
    fn table_is_sorted_unique_and_well_formed() {
        for pair in CURRENCIES.windows(2) {
            assert!(pair[0].0 < pair[1].0, "{} before {}", pair[0].0, pair[1].0);
        }
        for (code, exponent) in CURRENCIES {
            assert!(
                code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase()),
                "{code}"
            );
            assert!(*exponent <= 4, "{code}");
        }
    }

    #[test]
    fn plain_decimals_parse_exactly() {
        assert_eq!(parse_decimal_minor("-67.79", 2), Ok(-6779));
        assert_eq!(parse_decimal_minor("+12", 2), Ok(1200));
        assert_eq!(parse_decimal_minor("0.5", 2), Ok(50));
        assert_eq!(parse_decimal_minor("1.500", 2), Ok(150));
        assert_eq!(parse_decimal_minor("1500", 0), Ok(1500));
        assert_eq!(parse_decimal_minor("1.234", 3), Ok(1234));
        assert_eq!(parse_decimal_minor("-0.0001", 4), Ok(-1));
        assert_eq!(parse_decimal_minor("0", 2), Ok(0));
    }

    #[test]
    fn bad_decimals_are_refused() {
        assert_eq!(
            parse_decimal_minor("1.505", 2),
            Err(AmountError::TooPrecise)
        );
        assert_eq!(parse_decimal_minor("0.5", 0), Err(AmountError::TooPrecise));
        for bad in [
            "", "-", ".5", "5.", "1,000.00", "$5", "(5.00)", " 5", "5 ", "1e3", "--5", "5.0.0",
        ] {
            assert_eq!(
                parse_decimal_minor(bad, 2),
                Err(AmountError::NotADecimal),
                "{bad:?}"
            );
        }
        assert_eq!(
            parse_decimal_minor("92233720368547758.08", 2),
            Err(AmountError::TooLarge)
        );
        assert_eq!(parse_decimal_minor("92233720368547758.07", 2), Ok(i64::MAX));
    }

    #[test]
    fn grouped_decimals() {
        assert_eq!(parse_grouped_decimal_minor("1,234.56", 2), Ok(123_456));
        assert_eq!(
            parse_grouped_decimal_minor("-12,345,678", 0),
            Ok(-12_345_678)
        );
        assert_eq!(parse_grouped_decimal_minor("999.99", 2), Ok(99_999));
        for bad in [
            "1,23.45",
            "1234,567",
            ",123",
            "1,,234",
            "1,234,",
            "12345,678.00",
            "1,2a4",
        ] {
            assert_eq!(
                parse_grouped_decimal_minor(bad, 2),
                Err(AmountError::NotADecimal),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn errors_carry_no_input() {
        let text = AmountError::TooPrecise.to_string();
        assert!(!text.contains(char::is_numeric));
    }
}
