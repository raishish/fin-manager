//! Mapping profiles: how one bank's CSV layout maps to [`Record`](crate::Record) fields.
//!
//! The format is the CSV part of the bank-export corpus spec
//! (`testdata/bank-exports/data/layouts.json`), so built-in profiles are exactly the
//! documented layouts, and a profile the user creates for an unknown bank is saved in
//! the same form.

use serde::{Deserialize, Serialize};

/// What a column holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Role {
    PostedDate,
    TransactionDate,
    Description,
    Payee,
    Memo,
    /// One signed amount. `money_out` is the sign a purchase has: `negative` or `positive`.
    Amount {
        money_out: String,
    },
    /// Money out (absolute value is used).
    Debit,
    /// Money in (absolute value is used; some banks print it negative).
    Credit {
        #[serde(default)]
        negative_printed: bool,
    },
    /// An unsigned amount; `Indicator` gives the direction.
    AbsAmount,
    Indicator {
        debit: String,
        credit: String,
    },
    Balance,
    CheckNumber,
    /// The bank's id for the transaction, without the `wrapped_in` characters.
    Reference {
        #[serde(default)]
        wrapped_in: String,
    },
    Status {
        pending: String,
        posted: String,
    },
    BankCategory,
    BankSubcategory,
    Type,
    AccountNumber,
    Fixed {
        value: String,
    },
    Extra {
        what: String,
    },
}

/// Padding a writer adds; readers only need to know to trim (see [`CsvSpec::trim`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Pad {
    #[default]
    None,
    Trailing(usize),
    Width(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    /// Header text (empty when the file has no header).
    pub header: String,
    #[serde(default)]
    pub quoted: bool,
    #[serde(default)]
    pub pad: Pad,
    #[serde(flatten)]
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmountFormat {
    pub thousands_separator: bool,
    pub currency_symbol: bool,
    /// `minus` or `parentheses`.
    pub negative: String,
}

impl Default for AmountFormat {
    fn default() -> AmountFormat {
        AmountFormat {
            thousands_separator: false,
            currency_symbol: false,
            negative: "minus".to_string(),
        }
    }
}

fn comma() -> char {
    ','
}

fn lf() -> String {
    "lf".to_string()
}

fn none() -> String {
    "none".to_string()
}

fn yes() -> bool {
    true
}

/// The layout of one CSV export. Writing-side fields (quoting, line endings) are kept
/// so profiles match the corpus spec exactly; the reader is lenient about them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsvSpec {
    #[serde(default = "comma")]
    pub delimiter: char,
    #[serde(default)]
    pub quote_all: bool,
    #[serde(default)]
    pub quote_header: bool,
    #[serde(default)]
    pub quote_empty: bool,
    #[serde(default = "lf")]
    pub line_ending: String,
    #[serde(default)]
    pub utf8_bom: bool,
    /// Lines before the header (or the first row, without a header).
    #[serde(default)]
    pub skip_lines: usize,
    #[serde(default = "yes")]
    pub has_header: bool,
    pub columns: Vec<Column>,
    /// `MM/DD/YYYY`, `M/D/YYYY`, `MM/DD/YY` or `YYYY-MM-DD`.
    pub date_format: String,
    #[serde(default)]
    pub amount_format: AmountFormat,
    /// Rows without an amount are summaries, not transactions.
    #[serde(default)]
    pub skip_rows_without_amount: bool,
    /// `none`, `rows` (data rows only) or `all`.
    #[serde(default = "none")]
    pub trailing_delimiter: String,
    /// Trim spaces around every field.
    #[serde(default)]
    pub trim: bool,
}

/// A named layout: built in for known banks, or created for a new one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Stable id, e.g. `chase_card_csv`.
    pub id: String,
    pub institution: String,
    /// `checking`, `savings`, `credit_card`.
    #[serde(default)]
    pub account_kinds: Vec<String>,
    pub csv: CsvSpec,
}

/// Why a profile can't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    EmptyId,
    /// No posted- or transaction-date column.
    NoDate,
    NoDescription,
    /// Amount columns don't form one of: `amount`; `debit` + `credit`;
    /// `abs_amount` + `indicator`.
    AmountColumns,
    /// `money_out` isn't `negative` or `positive`.
    MoneyOut,
    DateFormat,
    /// A role that may appear once appears more than once.
    Duplicate(&'static str),
    /// Another profile already has this id.
    DuplicateId,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyId => f.write_str("profile id is empty"),
            Self::NoDate => f.write_str("profile has no date column"),
            Self::NoDescription => f.write_str("profile has no description column"),
            Self::AmountColumns => {
                f.write_str("profile needs an amount column, debit and credit columns, or an unsigned amount and an indicator")
            }
            Self::MoneyOut => f.write_str("money_out must be `negative` or `positive`"),
            Self::DateFormat => f.write_str("unknown date format"),
            Self::Duplicate(role) => write!(f, "more than one {role} column"),
            Self::DuplicateId => f.write_str("a profile with this id already exists"),
        }
    }
}

impl std::error::Error for ProfileError {}

type RoleTest = fn(&Role) -> bool;

pub(crate) const DATE_FORMATS: [&str; 4] = ["MM/DD/YYYY", "M/D/YYYY", "MM/DD/YY", "YYYY-MM-DD"];

impl CsvSpec {
    pub(crate) fn position(&self, f: impl Fn(&Role) -> bool) -> Option<usize> {
        self.columns.iter().position(|c| f(&c.role))
    }

    fn count(&self, f: impl Fn(&Role) -> bool) -> usize {
        self.columns.iter().filter(|c| f(&c.role)).count()
    }
}

impl Profile {
    /// Checks that the profile can produce records.
    pub fn validate(&self) -> Result<(), ProfileError> {
        let s = &self.csv;
        if self.id.trim().is_empty() {
            return Err(ProfileError::EmptyId);
        }
        if !DATE_FORMATS.contains(&s.date_format.as_str()) {
            return Err(ProfileError::DateFormat);
        }
        let once: [(&'static str, RoleTest); 8] = [
            ("posted date", |r| matches!(r, Role::PostedDate)),
            ("transaction date", |r| matches!(r, Role::TransactionDate)),
            ("description", |r| matches!(r, Role::Description)),
            ("amount", |r| matches!(r, Role::Amount { .. })),
            ("debit", |r| matches!(r, Role::Debit)),
            ("credit", |r| matches!(r, Role::Credit { .. })),
            ("balance", |r| matches!(r, Role::Balance)),
            ("status", |r| matches!(r, Role::Status { .. })),
        ];
        for (name, f) in once {
            if s.count(f) > 1 {
                return Err(ProfileError::Duplicate(name));
            }
        }
        if s.count(|r| matches!(r, Role::PostedDate | Role::TransactionDate)) == 0 {
            return Err(ProfileError::NoDate);
        }
        if s.count(|r| matches!(r, Role::Description)) == 0 {
            return Err(ProfileError::NoDescription);
        }
        let amount = s.count(|r| matches!(r, Role::Amount { .. }));
        let debit = s.count(|r| matches!(r, Role::Debit));
        let credit = s.count(|r| matches!(r, Role::Credit { .. }));
        let abs = s.count(|r| matches!(r, Role::AbsAmount));
        let ind = s.count(|r| matches!(r, Role::Indicator { .. }));
        let ok = matches!(
            (amount, debit, credit, abs, ind),
            (1, 0, 0, 0, 0) | (0, 1, 1, 0, 0) | (0, 0, 0, 1, 1)
        );
        if !ok {
            return Err(ProfileError::AmountColumns);
        }
        for c in &s.columns {
            if let Role::Amount { money_out } = &c.role
                && money_out != "negative"
                && money_out != "positive"
            {
                return Err(ProfileError::MoneyOut);
            }
        }
        Ok(())
    }
}

/// The built-in profiles: every CSV layout in the bank-export corpus spec.
pub fn builtin() -> Vec<Profile> {
    serde_json::from_str(include_str!("../profiles/builtin.json")).expect("built-in profiles parse")
}

/// Built-in profiles plus the ones the user has saved.
#[derive(Debug, Clone)]
pub struct ProfileSet {
    profiles: Vec<Profile>,
    builtin_count: usize,
}

impl Default for ProfileSet {
    fn default() -> ProfileSet {
        ProfileSet::with_builtin()
    }
}

impl ProfileSet {
    pub fn with_builtin() -> ProfileSet {
        let profiles = builtin();
        ProfileSet {
            builtin_count: profiles.len(),
            profiles,
        }
    }

    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    /// Adds a profile the user created (for example from [`suggest`](crate::suggest)).
    pub fn add(&mut self, profile: Profile) -> Result<(), ProfileError> {
        profile.validate()?;
        if self.profiles.iter().any(|p| p.id == profile.id) {
            return Err(ProfileError::DuplicateId);
        }
        self.profiles.push(profile);
        Ok(())
    }

    /// The user's profiles as JSON, for the caller to store.
    pub fn user_profiles_json(&self) -> String {
        serde_json::to_string_pretty(&self.profiles[self.builtin_count..])
            .expect("profiles serialise")
    }

    /// Adds profiles saved earlier with [`user_profiles_json`](Self::user_profiles_json).
    pub fn load_user_profiles(&mut self, json: &str) -> Result<usize, LoadError> {
        let loaded: Vec<Profile> = serde_json::from_str(json).map_err(|_| LoadError::Json)?;
        let n = loaded.len();
        for p in loaded {
            self.add(p).map_err(LoadError::Profile)?;
        }
        Ok(n)
    }
}

/// Saved profiles that couldn't be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    Json,
    Profile(ProfileError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json => f.write_str("saved profiles are not valid JSON"),
            Self::Profile(e) => write!(f, "a saved profile is invalid: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_profiles_are_valid_and_unique() {
        let all = builtin();
        assert!(all.len() >= 20);
        let mut set = ProfileSet {
            profiles: Vec::new(),
            builtin_count: 0,
        };
        for p in all {
            let id = p.id.clone();
            set.add(p).unwrap_or_else(|e| panic!("{id}: {e}"));
        }
    }

    #[test]
    fn user_profiles_round_trip() {
        let mut set = ProfileSet::with_builtin();
        let mut p = builtin().remove(0);
        p.id = "my_credit_union".into();
        set.add(p.clone()).unwrap();
        assert_eq!(set.add(p.clone()), Err(ProfileError::DuplicateId));
        let json = set.user_profiles_json();
        let mut again = ProfileSet::with_builtin();
        assert_eq!(again.load_user_profiles(&json), Ok(1));
        assert_eq!(again.profiles().last(), Some(&p));
    }

    #[test]
    fn invalid_profiles_are_rejected() {
        let mut p = builtin().remove(0);
        p.csv
            .columns
            .retain(|c| !matches!(c.role, Role::Amount { .. }));
        assert_eq!(p.validate(), Err(ProfileError::AmountColumns));
        let mut p = builtin().remove(0);
        p.csv.date_format = "DD.MM.YYYY".into();
        assert_eq!(p.validate(), Err(ProfileError::DateFormat));
    }
}
