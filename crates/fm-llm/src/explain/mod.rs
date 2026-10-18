//! Explaining an answer in plain words, with every number checked (SPEC constraint 6;
//! see README.md).
//!
//! The caller passes the computed values as [`Fact`]s: this crate never sees fm-core's
//! types. A model may write the explanation, through [`prompt`] and a generator the
//! caller supplies; [`explain`] keeps a text only if [`check`] finds every number in it
//! among the facts, their evidence counts and the [`Derivation`]s allowed. Otherwise it
//! asks again, and after the last attempt it uses the [`template`].

mod check;
mod extract;
mod facts;
mod template;
#[cfg(test)]
mod tests;

pub use check::{Report, Unchecked, check};
pub use facts::{Derivation, Fact, Money, Value};
pub use template::template;

/// Who wrote an explanation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Model,
    Template,
}

/// An explanation the user can be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation {
    pub text: String,
    pub source: Source,
    /// The checks of the model's texts that were turned down, in order.
    pub rejected: Vec<Report>,
}

/// The instructions for a model asked to explain `facts`, in answer to `question`.
///
/// It holds the user's question and the facts, written as the template writes them
/// (amounts, dates, names), and nothing else from the ledger: no transaction
/// descriptions beyond the facts' own text values.
pub fn prompt(question: &str, facts: &[Fact], derivations: &[Derivation]) -> String {
    let mut out = String::from(
        "Explain the answer to the user's question in one or two short sentences.\n\
         Use only the numbers listed below, written exactly as they are written here. \
         Do not compute, round or invent any other number.\n",
    );
    if !derivations.is_empty() {
        out.push_str("You may also state these, computed from the listed values:\n");
        for d in derivations {
            let line = match d {
                Derivation::Difference(a, b) => {
                    format!(
                        "- the difference between {} and {}\n",
                        template::label(a),
                        template::label(b)
                    )
                }
                Derivation::Sum(names) => format!(
                    "- the total of {}\n",
                    names
                        .iter()
                        .map(|n| template::label(n))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Derivation::Ratio(a, b) => {
                    format!(
                        "- {} as a share of {}\n",
                        template::label(a),
                        template::label(b)
                    )
                }
                Derivation::PercentChange(a, b) => {
                    format!(
                        "- the change from {} to {} in percent\n",
                        template::label(b),
                        template::label(a)
                    )
                }
            };
            out.push_str(&line);
        }
    }
    out.push_str("\nQuestion: ");
    out.push_str(question.trim());
    out.push_str("\nAnswer:\n");
    for f in facts {
        out.push_str("- ");
        out.push_str(&template::sentence(f));
        out.push('\n');
    }
    out.push_str("\nExplanation:");
    out
}

/// An explanation of `facts`: the first of up to `attempts` texts from `generate` that
/// passes [`check`], or else the [`template`]. `generate` gets the [`prompt`] and returns
/// `None` when it can't write (no model loaded); then the template is used at once.
pub fn explain(
    question: &str,
    facts: &[Fact],
    derivations: &[Derivation],
    attempts: usize,
    mut generate: impl FnMut(&str) -> Option<String>,
) -> Explanation {
    let prompt = prompt(question, facts, derivations);
    let mut rejected = Vec::new();
    for _ in 0..attempts {
        let Some(text) = generate(&prompt) else {
            break;
        };
        let text = text.trim().to_owned();
        let report = check(&text, facts, derivations);
        if report.passed() && !text.is_empty() {
            return Explanation {
                text,
                source: Source::Model,
                rejected,
            };
        }
        rejected.push(report);
    }
    Explanation {
        text: template(facts),
        source: Source::Template,
        rejected,
    }
}
