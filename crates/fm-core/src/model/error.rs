use std::fmt;

use super::{Currency, Sha256Hash};

/// A value that breaks one of the model's invariants.
///
/// Messages never repeat amounts, descriptions or file contents, because errors can end up
/// in logs and crash reports (SPEC §7). Dates and currency codes are echoed, since they
/// are needed to find the bad input and identify no one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// Not a valid `YYYY-MM-DD` date between 0001-01-01 and 9999-12-31.
    InvalidDate(String),
    /// Not a valid `YYYY-MM` month between 0001-01 and 9999-12.
    InvalidYearMonth(String),
    /// Not an ISO 4217 code this build knows.
    UnknownCurrency(String),
    /// A decimal amount that can't be represented exactly; the reason says why.
    InvalidAmount(&'static str),
    /// Arithmetic on two amounts in different currencies.
    CurrencyMismatch { left: Currency, right: Currency },
    /// Arithmetic overflowed `i64` minor units (or a date left the supported range).
    Overflow,
    /// Not a 64-character lowercase hex SHA-256.
    InvalidHash,
    /// Raw bytes whose SHA-256 differs from the hash stored with them.
    HashMismatch {
        stated: Sha256Hash,
        actual: Sha256Hash,
    },
    /// A confidence outside 0..=10000 basis points (or a probability outside 0..=1).
    InvalidConfidence,
    /// A taxonomy that refers to missing or duplicate entries; the message says which.
    InvalidTaxonomy(String),
    /// A rule that can't do anything or can't match anything; the message says why.
    InvalidRule(&'static str),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDate(s) => write!(f, "invalid date {s:?}: expected YYYY-MM-DD"),
            Self::InvalidYearMonth(s) => write!(f, "invalid month {s:?}: expected YYYY-MM"),
            Self::UnknownCurrency(s) => write!(f, "unknown ISO 4217 currency code {s:?}"),
            Self::InvalidAmount(why) => write!(f, "invalid amount: {why}"),
            Self::CurrencyMismatch { left, right } => {
                write!(f, "currency mismatch: {left} and {right}")
            }
            Self::Overflow => f.write_str("arithmetic overflow"),
            Self::InvalidHash => f.write_str("invalid SHA-256: expected 64 lowercase hex digits"),
            Self::HashMismatch { stated, actual } => {
                write!(
                    f,
                    "raw import hash mismatch: stated {stated}, bytes hash to {actual}"
                )
            }
            Self::InvalidConfidence => {
                f.write_str("invalid confidence: expected 0..=10000 basis points")
            }
            Self::InvalidTaxonomy(why) => write!(f, "invalid taxonomy: {why}"),
            Self::InvalidRule(why) => write!(f, "invalid rule: {why}"),
        }
    }
}

impl std::error::Error for ModelError {}
