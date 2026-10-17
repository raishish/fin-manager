//! Shared record types for the bank-export importers (`fm-import-csv`, `fm-import-ofx`).
//!
//! Both importers turn a bank's file into the same shape: a list of [`Statement`]s, each
//! with account-level [`StatementInfo`] and normalised [`Record`]s. The import pipeline in
//! `fm-core` (T1.3) maps these into its ledger model, so it handles one input shape no
//! matter which format a file came in.
//!
//! The crate is Apache-2.0 and deliberately small: no dependency on `fm-core`, only
//! `serde`. It also owns the two pieces of money handling both importers need, so neither
//! duplicates them: the ISO 4217 minor-unit table ([`minor_exponent`]) and an exact,
//! float-free decimal parser ([`parse_decimal_minor`]).
//!
//! Conventions every importer follows:
//! - **Amounts** are signed integers in the currency's minor unit, **negative = money out
//!   of the account**, for every account type (card purchases are negative on the card).
//! - **Dates** are calendar dates exactly as the bank wrote them, never converted to UTC.
//! - **Raw text** (description, payee, memo) is kept as written; only layouts that pad
//!   fields get trimmed.
//! - **Errors never contain financial data**: positions and codes only, never field values.

mod date;
mod money;
mod record;

pub use date::{Date, DateError};
pub use money::{
    AmountError, CURRENCIES, minor_exponent, parse_decimal_minor, parse_grouped_decimal_minor,
};
pub use record::{AccountKind, Institution, Record, Statement, StatementInfo, Status};
