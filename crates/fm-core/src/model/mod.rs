//! Data model v0: the types every other part of the core works on.
//!
//! The schema, units and conventions are documented in `README.md` next to this file.
//! In short:
//! - Amounts are exact integers in the currency's minor unit, with an ISO 4217 code
//!   ([`Money`], [`Currency`]). Negative means money left the account.
//! - Original import bytes are kept with their SHA-256 ([`RawImport`]), and every
//!   [`Transaction`] points back to the import it came from.
//! - Every categorisation or merchant clean-up is a [`Decision`] that records its
//!   [`DecisionSource`]: the user, a rule, built-in fixed logic, or a model (with model id,
//!   GGUF hash, prompt version and confidence).
//! - Categories are versioned ([`Taxonomy`]); the special buckets Transfer, Income,
//!   Refund and Needs review sit outside the category list ([`Bucket`]).
//!
//! Primitive types (dates, currencies, hashes, confidences, raw imports) check their
//! invariants on construction and on deserialisation. Composite types that refer to each
//! other ([`Taxonomy`], [`Rule`]) have a `validate` method.

/// Implements `Display`-based `Serialize` and `FromStr`-based `Deserialize` for a type,
/// so it appears in JSON as a plain string that is validated when read back.
macro_rules! serde_as_str {
    ($ty:ty) => {
        impl serde::Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

mod account;
mod category;
mod date;
mod decision;
mod error;
mod hash;
mod ids;
mod import;
mod ledger;
mod money;
mod rule;
mod transaction;

#[cfg(test)]
pub(crate) mod round_trip_tests;

pub use account::{Account, AccountKind};
pub use category::{Bucket, Category, CategoryGroup, Taxonomy, TaxonomyVersion};
pub use date::{Date, Timestamp, YearMonth};
pub use decision::{
    Assignment, BuiltinLogic, Confidence, Decision, DecisionSource, DecisionState, ModelProvenance,
};
pub use error::ModelError;
pub use hash::Sha256Hash;
pub use ids::{
    AccountId, CategoryGroupId, CategoryId, DecisionId, ImportId, RuleId, TransactionId,
};
pub use import::{ImportSource, RawImport};
pub use ledger::Ledger;
pub use money::{Currency, Money};
pub use rule::{AmountMatch, Rule, RuleAction, RuleMatcher, RuleOrigin, TextMatch};
pub use transaction::{SourceRef, Transaction, TransactionStatus};
