use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::ModelError;

/// An ISO 4217 currency, with its minor-unit exponent (USD 2, JPY 0, KWD 3).
///
/// Only codes in this build's table are accepted (see [`Currency::new`]). Serialised as
/// the three-letter code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Currency {
    code: &'static str,
    exponent: u8,
}

/// ISO 4217 codes with their minor-unit exponents, sorted by code for binary search.
///
/// **Append-only.** Stored amounts are minor units next to a code, and the exponent is
/// looked up here, never stored. So a code is never removed and its exponent never
/// changes once shipped: either would change the meaning of amounts already in users'
/// ledgers. Add new codes only. `currency_table_is_append_only` pins today's entries. If
/// ISO ever changes an exponent, that is a new code plus a data migration, not an edit.
///
/// Active codes as of 2026, plus a few withdrawn since 2021 (ANG, BGN, CUC, HRK, SLL,
/// ZWL) because old statements still use them. Precious metals, SDRs and the testing
/// codes (XAU, XDR, XTS, XXX, ...) have no minor unit and are left out.
#[rustfmt::skip]
const CURRENCIES: &[(&str, u8)] = &[
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

impl Currency {
    pub const USD: Currency = Currency {
        code: "USD",
        exponent: 2,
    };
    pub const EUR: Currency = Currency {
        code: "EUR",
        exponent: 2,
    };
    pub const GBP: Currency = Currency {
        code: "GBP",
        exponent: 2,
    };
    pub const INR: Currency = Currency {
        code: "INR",
        exponent: 2,
    };
    pub const JPY: Currency = Currency {
        code: "JPY",
        exponent: 0,
    };

    /// The currency with this three-letter, uppercase ISO 4217 code.
    pub fn new(code: &str) -> Result<Currency, ModelError> {
        CURRENCIES
            .binary_search_by(|(c, _)| (*c).cmp(code))
            .map(|i| {
                let (code, exponent) = CURRENCIES[i];
                Currency { code, exponent }
            })
            .map_err(|_| ModelError::UnknownCurrency(code.to_owned()))
    }

    pub const fn code(self) -> &'static str {
        self.code
    }

    /// Digits after the decimal point: one major unit is `10^exponent` minor units.
    pub const fn minor_exponent(self) -> u8 {
        self.exponent
    }

    /// `10^exponent`. At most 10 000, so it always fits.
    const fn minor_per_major(self) -> i64 {
        10_i64.pow(self.exponent as u32)
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl FromStr for Currency {
    type Err = ModelError;

    fn from_str(s: &str) -> Result<Currency, ModelError> {
        Currency::new(s)
    }
}

serde_as_str!(Currency);

/// An exact amount: an integer count of the currency's minor unit (cents for USD).
///
/// **Sign convention:** from the account's point of view, negative means money left the
/// account and positive means money came in. This holds for every account type: a card
/// purchase is negative, a card payment received is positive. Importers normalise
/// bank-specific signs to this.
///
/// No floating point anywhere (SPEC constraint 1). Serialised as
/// `{"minor": -1234, "currency": "USD"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Money {
    minor: i64,
    currency: Currency,
}

impl Money {
    pub const fn new(minor: i64, currency: Currency) -> Money {
        Money { minor, currency }
    }

    pub const fn zero(currency: Currency) -> Money {
        Money { minor: 0, currency }
    }

    /// The amount in minor units (cents for USD).
    pub const fn minor(self) -> i64 {
        self.minor
    }

    pub const fn currency(self) -> Currency {
        self.currency
    }

    pub const fn is_negative(self) -> bool {
        self.minor < 0
    }

    pub const fn is_positive(self) -> bool {
        self.minor > 0
    }

    pub const fn is_zero(self) -> bool {
        self.minor == 0
    }

    pub fn checked_add(self, other: Money) -> Result<Money, ModelError> {
        self.same_currency(other)?;
        let minor = self
            .minor
            .checked_add(other.minor)
            .ok_or(ModelError::Overflow)?;
        Ok(Money::new(minor, self.currency))
    }

    pub fn checked_sub(self, other: Money) -> Result<Money, ModelError> {
        self.same_currency(other)?;
        let minor = self
            .minor
            .checked_sub(other.minor)
            .ok_or(ModelError::Overflow)?;
        Ok(Money::new(minor, self.currency))
    }

    pub fn checked_neg(self) -> Result<Money, ModelError> {
        let minor = self.minor.checked_neg().ok_or(ModelError::Overflow)?;
        Ok(Money::new(minor, self.currency))
    }

    /// The exact sum of `amounts`, all of which must be in `currency`; zero if empty.
    pub fn checked_sum(
        currency: Currency,
        amounts: impl IntoIterator<Item = Money>,
    ) -> Result<Money, ModelError> {
        amounts
            .into_iter()
            .try_fold(Money::zero(currency), Money::checked_add)
    }

    fn same_currency(self, other: Money) -> Result<(), ModelError> {
        if self.currency == other.currency {
            Ok(())
        } else {
            Err(ModelError::CurrencyMismatch {
                left: self.currency,
                right: other.currency,
            })
        }
    }

    /// Parses a plain decimal amount exactly: an optional `-` or `+`, ASCII digits, and
    /// optionally `.` followed by at most the currency's exponent digits (more are
    /// accepted only if the extra ones are zeros: `"1.230"` is 123 cents, `"1.235"` is an
    /// error). No thousands separators, spaces, symbols or exponents; importers strip
    /// those first.
    pub fn from_decimal_str(s: &str, currency: Currency) -> Result<Money, ModelError> {
        let (negative, unsigned) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        let (whole, fraction) = match unsigned.split_once('.') {
            Some((_, "")) => return Err(ModelError::InvalidAmount("nothing after the point")),
            Some((whole, fraction)) => (whole, fraction),
            None => (unsigned, ""),
        };
        let all_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
        if whole.is_empty() || !all_digits(whole) || !all_digits(fraction) {
            return Err(ModelError::InvalidAmount("not a plain decimal number"));
        }

        let exponent = usize::from(currency.minor_exponent());
        let (kept, dropped) = fraction.split_at(fraction.len().min(exponent));
        if dropped.bytes().any(|b| b != b'0') {
            return Err(ModelError::InvalidAmount(
                "more decimal places than the currency has",
            ));
        }

        let overflow = || ModelError::InvalidAmount("too large");
        let whole: i64 = whole.parse().map_err(|_| overflow())?;
        let mut fraction_minor: i64 = if kept.is_empty() {
            0
        } else {
            kept.parse().map_err(|_| overflow())?
        };
        for _ in kept.len()..exponent {
            fraction_minor *= 10;
        }
        let minor = whole
            .checked_mul(currency.minor_per_major())
            .and_then(|m| m.checked_add(fraction_minor))
            .ok_or_else(overflow)?;
        Ok(Money::new(if negative { -minor } else { minor }, currency))
    }

    /// The amount as a plain decimal string with exactly the currency's number of decimal
    /// places, e.g. `"-1234.50"` for USD or `"-1234"` for JPY. Inverse of
    /// [`Money::from_decimal_str`].
    pub fn to_decimal_string(self) -> String {
        let per_major = self.currency.minor_per_major().unsigned_abs();
        let abs = self.minor.unsigned_abs();
        let sign = if self.minor < 0 { "-" } else { "" };
        let whole = abs / per_major;
        match usize::from(self.currency.minor_exponent()) {
            0 => format!("{sign}{whole}"),
            width => format!("{sign}{whole}.{:0width$}", abs % per_major),
        }
    }
}

impl fmt::Display for Money {
    /// `-1234.50 USD`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.to_decimal_string(), self.currency)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd(s: &str) -> Result<i64, ModelError> {
        Money::from_decimal_str(s, Currency::USD).map(Money::minor)
    }

    #[test]
    fn currency_table_is_sorted_unique_and_well_formed() {
        for pair in CURRENCIES.windows(2) {
            assert!(pair[0].0 < pair[1].0, "{} before {}", pair[0].0, pair[1].0);
        }
        for (code, exponent) in CURRENCIES {
            assert!(code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase()));
            assert!(*exponent <= 4);
        }
    }

    /// The table is append-only (see [`CURRENCIES`]): every entry shipped so far must
    /// still be there with the same exponent. Adding codes is fine; never edit this list
    /// except to append to it.
    /// The importers' copy of the table (fm-import-types, Apache-2.0) is identical, so an
    /// amount an importer parsed has the exponent fm-core will read it back with.
    #[test]
    fn currency_table_matches_the_importers_copy() {
        assert_eq!(CURRENCIES, fm_import_types::CURRENCIES);
    }

    #[test]
    fn currency_table_is_append_only() {
        const SHIPPED: &str = "AED2 AFN2 ALL2 AMD2 ANG2 AOA2 ARS2 AUD2 AWG2 AZN2 BAM2 BBD2 BDT2 BGN2 BHD3 BIF0 BMD2 BND2 BOB2 BOV2 BRL2 BSD2 BTN2 BWP2 BYN2 BZD2 CAD2 CDF2 CHE2 CHF2 CHW2 CLF4 CLP0 CNY2 COP2 COU2 CRC2 CUC2 CUP2 CVE2 CZK2 DJF0 DKK2 DOP2 DZD2 EGP2 ERN2 ETB2 EUR2 FJD2 FKP2 GBP2 GEL2 GHS2 GIP2 GMD2 GNF0 GTQ2 GYD2 HKD2 HNL2 HRK2 HTG2 HUF2 IDR2 ILS2 INR2 IQD3 IRR2 ISK0 JMD2 JOD3 JPY0 KES2 KGS2 KHR2 KMF0 KPW2 KRW0 KWD3 KYD2 KZT2 LAK2 LBP2 LKR2 LRD2 LSL2 LYD3 MAD2 MDL2 MGA2 MKD2 MMK2 MNT2 MOP2 MRU2 MUR2 MVR2 MWK2 MXN2 MXV2 MYR2 MZN2 NAD2 NGN2 NIO2 NOK2 NPR2 NZD2 OMR3 PAB2 PEN2 PGK2 PHP2 PKR2 PLN2 PYG0 QAR2 RON2 RSD2 RUB2 RWF0 SAR2 SBD2 SCR2 SDG2 SEK2 SGD2 SHP2 SLE2 SLL2 SOS2 SRD2 SSP2 STN2 SVC2 SYP2 SZL2 THB2 TJS2 TMT2 TND3 TOP2 TRY2 TTD2 TWD2 TZS2 UAH2 UGX0 USD2 USN2 UYI0 UYU2 UYW4 UZS2 VED2 VES2 VND0 VUV0 WST2 XAF0 XCD2 XCG2 XOF0 XPF0 YER2 ZAR2 ZMW2 ZWG2 ZWL2";
        for entry in SHIPPED.split(' ') {
            let (code, exponent) = entry.split_at(3);
            let currency = Currency::new(code).unwrap_or_else(|_| panic!("{code} was removed"));
            assert_eq!(
                currency.minor_exponent().to_string(),
                exponent,
                "{code}'s exponent changed"
            );
        }
        assert!(CURRENCIES.len() >= SHIPPED.split(' ').count());
    }

    #[test]
    fn currency_constants_match_the_table() {
        for c in [
            Currency::USD,
            Currency::EUR,
            Currency::GBP,
            Currency::INR,
            Currency::JPY,
        ] {
            assert_eq!(Currency::new(c.code()), Ok(c));
        }
    }

    #[test]
    fn currency_exponents() {
        let exp = |c: &str| Currency::new(c).unwrap().minor_exponent();
        assert_eq!(exp("USD"), 2);
        assert_eq!(exp("JPY"), 0);
        assert_eq!(exp("KRW"), 0);
        assert_eq!(exp("KWD"), 3);
        assert_eq!(exp("BHD"), 3);
        assert_eq!(exp("CLF"), 4);
    }

    #[test]
    fn unknown_currencies_are_rejected() {
        for bad in ["", "usd", "US", "USDD", "XXX", "XAU", "ABC", " USD"] {
            assert!(Currency::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn decimal_parsing_is_exact() {
        assert_eq!(usd("0"), Ok(0));
        assert_eq!(usd("12"), Ok(1200));
        assert_eq!(usd("12.3"), Ok(1230));
        assert_eq!(usd("12.34"), Ok(1234));
        assert_eq!(usd("-12.34"), Ok(-1234));
        assert_eq!(usd("+12.34"), Ok(1234));
        assert_eq!(usd("0.01"), Ok(1));
        assert_eq!(usd("-0.01"), Ok(-1));
        assert_eq!(usd("12.340"), Ok(1234));
        assert_eq!(usd("007.10"), Ok(710));
        // 0.1 + 0.2 is exactly 0.3 here.
        let a = Money::from_decimal_str("0.1", Currency::USD).unwrap();
        let b = Money::from_decimal_str("0.2", Currency::USD).unwrap();
        assert_eq!(a.checked_add(b).unwrap().minor(), 30);

        assert_eq!(
            Money::from_decimal_str("-1500", Currency::JPY).map(Money::minor),
            Ok(-1500)
        );
        assert!(Money::from_decimal_str("15.5", Currency::JPY).is_err());
        let kwd = Currency::new("KWD").unwrap();
        assert_eq!(
            Money::from_decimal_str("1.234", kwd).map(Money::minor),
            Ok(1234)
        );
    }

    #[test]
    fn decimal_parsing_rejects_anything_ambiguous() {
        for bad in [
            "", "-", "+", ".", ".5", "5.", "12.345", "1,234.00", "1 234", "$12", "12.34USD", "1e3",
            "--1", "+-1", "١٢", "12.3.4", "12.-3", " 12", "12 ",
        ] {
            assert!(usd(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(usd("92233720368547758.08").is_err());
        assert_eq!(usd("92233720368547758.07"), Ok(i64::MAX));
    }

    #[test]
    fn decimal_formatting_round_trips() {
        for minor in [
            0,
            1,
            -1,
            9,
            10,
            99,
            100,
            -100,
            123_456,
            -123_456,
            i64::MAX,
            i64::MIN + 1,
        ] {
            for currency in [Currency::USD, Currency::JPY, Currency::new("KWD").unwrap()] {
                let m = Money::new(minor, currency);
                let s = m.to_decimal_string();
                assert_eq!(Money::from_decimal_str(&s, currency), Ok(m), "{s}");
            }
        }
        assert_eq!(Money::new(-5, Currency::USD).to_decimal_string(), "-0.05");
        assert_eq!(
            Money::new(123_450, Currency::USD).to_string(),
            "1234.50 USD"
        );
        assert_eq!(Money::new(-1500, Currency::JPY).to_string(), "-1500 JPY");
        assert_eq!(
            Money::new(i64::MIN, Currency::USD).to_decimal_string(),
            "-92233720368547758.08"
        );
    }

    #[test]
    fn arithmetic_checks_currency_and_overflow() {
        let a = Money::new(100, Currency::USD);
        let e = Money::new(100, Currency::EUR);
        assert_eq!(
            a.checked_add(e),
            Err(ModelError::CurrencyMismatch {
                left: Currency::USD,
                right: Currency::EUR
            })
        );
        assert_eq!(
            Money::new(i64::MAX, Currency::USD).checked_add(Money::new(1, Currency::USD)),
            Err(ModelError::Overflow)
        );
        assert_eq!(
            Money::new(i64::MIN, Currency::USD).checked_neg(),
            Err(ModelError::Overflow)
        );
        assert_eq!(a.checked_sub(a).unwrap(), Money::zero(Currency::USD));
        assert_eq!(
            Money::checked_sum(Currency::USD, [a, a, a.checked_neg().unwrap()]),
            Ok(a)
        );
        assert_eq!(
            Money::checked_sum(Currency::USD, []),
            Ok(Money::zero(Currency::USD))
        );
        assert!(Money::checked_sum(Currency::USD, [e]).is_err());
    }
}
