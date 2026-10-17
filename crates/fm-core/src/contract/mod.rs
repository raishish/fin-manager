//! The output contract's hard constraints (SPEC §6, decision 20), checked over a
//! [`Ledger`]. A violation is a bug: the checker runs in tests and in the app.
//!
//! This module provides the framework and constraints 1 (numbers are exact),
//! 4 (everything adds up) and 8 (every decision shows its source). The others plug in as
//! later tasks add them: 2 (user inputs win, T2.2), 3 (closed list, T2.5), 5 (admit
//! uncertainty, T2.5), 6 (explanations match the numbers, T4.3), 7 (no silent changes,
//! T2.5). See `README.md` for what each check means.
//!
//! ```
//! # use fm_core::{contract, model::Ledger};
//! # fn run(ledger: &Ledger) {
//! let report = contract::check(ledger);
//! assert!(report.is_ok(), "{report}");
//! # }
//! ```

mod adds_up;
mod exact_numbers;
mod index;
mod source_shown;

#[cfg(test)]
mod tests;

use std::fmt;

use crate::model::{AccountId, DecisionId, Ledger, RuleId, TransactionId};

pub use index::LedgerIndex;

/// A hard constraint from SPEC §6, by its number there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Constraint {
    /// 1. Numbers are exact.
    ExactNumbers,
    /// 2. User inputs always win.
    UserInputsWin,
    /// 3. Models pick from a closed list.
    ClosedList,
    /// 4. Everything adds up.
    EverythingAddsUp,
    /// 5. Admit uncertainty.
    AdmitUncertainty,
    /// 6. Explanations match the numbers.
    ExplanationsMatch,
    /// 7. No silent changes.
    NoSilentChanges,
    /// 8. Every decision shows its source.
    SourceShown,
}

impl Constraint {
    /// Its number in SPEC §6.
    pub const fn number(self) -> u8 {
        match self {
            Self::ExactNumbers => 1,
            Self::UserInputsWin => 2,
            Self::ClosedList => 3,
            Self::EverythingAddsUp => 4,
            Self::AdmitUncertainty => 5,
            Self::ExplanationsMatch => 6,
            Self::NoSilentChanges => 7,
            Self::SourceShown => 8,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::ExactNumbers => "numbers are exact",
            Self::UserInputsWin => "user inputs always win",
            Self::ClosedList => "models pick from a closed list",
            Self::EverythingAddsUp => "everything adds up",
            Self::AdmitUncertainty => "admit uncertainty",
            Self::ExplanationsMatch => "explanations match the numbers",
            Self::NoSilentChanges => "no silent changes",
            Self::SourceShown => "every decision shows its source",
        }
    }
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "constraint {} ({})", self.number(), self.name())
    }
}

/// What a violation is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subject {
    Ledger,
    Account(AccountId),
    Transaction(TransactionId),
    Decision(DecisionId),
    Rule(RuleId),
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ledger => f.write_str("ledger"),
            Self::Account(id) => write!(f, "account {}", id.0),
            Self::Transaction(id) => write!(f, "transaction {}", id.0),
            Self::Decision(id) => write!(f, "decision {}", id.0),
            Self::Rule(id) => write!(f, "rule {}", id.0),
        }
    }
}

/// One broken constraint.
///
/// Holds ids, a stable code and a fixed message, never amounts, descriptions or
/// merchant names, so a report can go into logs and crash reports (SPEC §7).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Violation {
    pub constraint: Constraint,
    pub subject: Subject,
    /// Stable machine code, e.g. `"no_bucket"`. Tests and dashboards match on it.
    pub code: &'static str,
    /// What is wrong, in plain words.
    pub message: &'static str,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}: {} [{}]",
            self.constraint, self.subject, self.message, self.code
        )
    }
}

/// One constraint's check. Each constraint module provides one; [`Checker`] runs them.
pub trait Check: Send + Sync {
    fn constraint(&self) -> Constraint;

    /// Appends every violation found in `ledger` to `out`.
    fn check(&self, ledger: &LedgerIndex<'_>, out: &mut Vec<Violation>);
}

/// The result of checking a ledger: every violation, sorted by constraint, subject and
/// code, so the same ledger always gives the same report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The constraints that were checked.
    pub checked: Vec<Constraint>,
    pub violations: Vec<Violation>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }

    /// The violations of one constraint.
    pub fn of(&self, constraint: Constraint) -> impl Iterator<Item = &Violation> {
        self.violations
            .iter()
            .filter(move |v| v.constraint == constraint)
    }

    /// Whether some violation has this code (and, if given, this subject).
    pub fn has(&self, code: &str, subject: Option<Subject>) -> bool {
        self.violations
            .iter()
            .any(|v| v.code == code && subject.is_none_or(|s| v.subject == s))
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let checked: Vec<String> = self
            .checked
            .iter()
            .map(|c| c.number().to_string())
            .collect();
        writeln!(
            f,
            "contract check (constraints {}): {} violation(s)",
            checked.join(", "),
            self.violations.len()
        )?;
        for v in &self.violations {
            writeln!(f, "  {v}")?;
        }
        Ok(())
    }
}

/// A set of constraint checks to run over ledgers.
pub struct Checker {
    checks: Vec<Box<dyn Check>>,
}

impl Checker {
    /// No checks; add them with [`Checker::with`].
    pub fn empty() -> Checker {
        Checker { checks: Vec::new() }
    }

    /// Every constraint implemented so far: 1, 4 and 8.
    pub fn all() -> Checker {
        Checker::empty()
            .with(exact_numbers::ExactNumbers)
            .with(adds_up::EverythingAddsUp)
            .with(source_shown::SourceShown)
    }

    pub fn with(mut self, check: impl Check + 'static) -> Checker {
        self.checks.push(Box::new(check));
        self
    }

    pub fn run(&self, ledger: &Ledger) -> Report {
        let index = LedgerIndex::new(ledger);
        let mut violations = Vec::new();
        for check in &self.checks {
            check.check(&index, &mut violations);
        }
        violations.sort();
        violations.dedup();
        let mut checked: Vec<Constraint> = self.checks.iter().map(|c| c.constraint()).collect();
        checked.sort();
        checked.dedup();
        Report {
            checked,
            violations,
        }
    }
}

impl Default for Checker {
    fn default() -> Checker {
        Checker::all()
    }
}

/// Checks `ledger` against every constraint implemented so far.
pub fn check(ledger: &Ledger) -> Report {
    Checker::all().run(ledger)
}
