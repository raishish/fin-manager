//! CSV bank-export parsing with per-bank mapping profiles.
//!
//! - [`import`] recognises the layout of a file among known [`Profile`]s and reads it
//!   into a [`Statement`] of normalised [`Record`]s (the record types are shared with
//!   the OFX importer, in `fm-import-types`): signed amounts in minor units (negative =
//!   money out, whatever the bank's convention), dates as written, pending status, and
//!   the bank's own reference, check number, type, balance and category where the file
//!   has them.
//! - The built-in profiles are the documented layouts of 18 US banks and card issuers
//!   (the CSV part of `testdata/bank-exports/data/layouts.json`).
//! - [`suggest`] guesses a mapping for an unknown layout; once the user confirms it,
//!   [`ProfileSet::add`] keeps it and [`ProfileSet::user_profiles_json`] saves it.
//!
//! Errors say where a file is broken but never repeat its contents.

mod csv;
mod detect;
mod parse;
pub mod profile;
mod suggest;

pub use detect::detect;
pub use fm_import_types::{
    AccountKind, Date, Institution, Record, Statement, StatementInfo, Status,
};
pub use parse::{ImportError, parse};
pub use profile::{Profile, ProfileSet};
pub use suggest::suggest;

/// Crate name, used by the workspace smoke test.
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Recognises the file's layout among `profiles` and reads it. `currency` is the
/// account's ISO 4217 code. A CSV file holds one statement; the result is a list to
/// match the OFX importer, which can return several.
pub fn import(
    bytes: &[u8],
    profiles: &ProfileSet,
    currency: &str,
) -> Result<Vec<Statement>, ImportError> {
    let profile = detect(bytes, profiles.profiles())?;
    parse(bytes, profile, currency)
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_set() {
        assert_eq!(super::NAME, "fm-import-csv");
    }
}
