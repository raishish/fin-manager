//! Question → structured query with the local model (T4.2).
//!
//! ```text
//! question + today + the user's categories and accounts
//!   → prompt (language, examples) → model, decoding against the query schema's grammar
//!   → JSON query → the app deserialises it (fm_core::query::Query), shows it, runs it
//! ```
//!
//! - **Constrained output.** The model decodes against a grammar built from T4.1's
//!   query schema ([`gbnf`]), narrowed to this ledger ([`narrow`]): only its category
//!   keys, group keys, account ids and currencies can be written, a query has at most
//!   [`MAX_ITEMS`] items, and every scope names its period. Whatever comes out is a
//!   query of the right shape.
//! - **No fm-core here.** fm-core will call this crate (T2.5), so this module takes
//!   the schema text and the vocabulary as plain values and returns JSON text. The app
//!   turns it into a `Query`, renders it for the user (`fm_core::query::describe`) and
//!   runs it with the same `as_of`.
//! - **Today comes from the app.** `as_of` is written into the prompt so the model can
//!   read "in 2025" against it, and relative phrases become relative periods, which
//!   the executor resolves with the same `as_of`.
//! - **Deterministic.** Greedy decoding: the same model, question and ledger always
//!   give the same query.

mod complete;
mod examples;
pub mod gbnf;
mod narrow;
mod prompt;
#[cfg(test)]
mod tests;

use std::fmt;

pub use complete::{Added, Completion, RULES as COMPLETION_RULES, Rule, complete};
pub use examples::{Example, all as examples};
pub use gbnf::{GbnfError, Grammar};
pub use narrow::{MAX_ITEMS, narrow};
pub use prompt::PROMPT_VERSION;

use crate::{GenerateOptions, LlmError, Model};

/// The start of every answer. It goes into the prompt, so the model writes the items.
pub const PREFIX: &str = r#"{"items":["#;

/// A category the user can be asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    /// Stable key, e.g. `food.groceries`.
    pub key: String,
    /// Display name.
    pub name: String,
    /// Its group's key.
    pub group: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: i64,
    pub name: String,
    /// The account kind as the query language writes it (`checking`, `credit_card`).
    pub kind: String,
}

/// What this ledger has, for the prompt and the grammar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vocabulary {
    pub categories: Vec<Category>,
    pub groups: Vec<Group>,
    pub accounts: Vec<Account>,
    /// Currencies purchases were recorded in abroad (original amounts), not the
    /// accounts' own: what `original_currency` can name.
    pub currencies: Vec<String>,
    /// The merchant names the ledger shows (cleaned names). A query's merchant must be
    /// one of them, so the model completes "Whole Foods" to the name it is filed under.
    pub merchants: Vec<String>,
}

/// One question to interpret.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub question: &'a str,
    /// Today, `YYYY-MM-DD`, from the app.
    pub as_of: &'a str,
    /// The query schema (`fm_core::query::SCHEMA`).
    pub schema: &'a str,
    pub vocabulary: &'a Vocabulary,
    /// Text the model's answer starts with before the query, for models that expect
    /// one (an empty `<think></think>` block for a thinking model). Usually empty.
    pub answer_lead: &'a str,
    /// `as_of`'s weekday, filled in by [`interpret`].
    weekday: &'static str,
}

impl<'a> Request<'a> {
    pub fn new(
        question: &'a str,
        as_of: &'a str,
        schema: &'a str,
        vocabulary: &'a Vocabulary,
    ) -> Self {
        Request {
            question,
            as_of,
            schema,
            vocabulary,
            answer_lead: "",
            weekday: "",
        }
    }
}

/// The model's reading of a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interpretation {
    /// The query, as JSON text that the query schema accepts.
    pub json: String,
    /// How sure the model was of it (basis points): its probability over the model's
    /// whole vocabulary (T2.7), at most the figure renormalised over what the grammar
    /// allows, which `Model::generate_exact` gives.
    pub confidence_bp: u16,
    pub tokens: u32,
    pub model_id: String,
    pub prompt_version: &'static str,
}

/// Why a question couldn't be interpreted. Never carries the question text.
#[derive(Debug)]
pub enum NlQueryError {
    /// The schema text doesn't parse, or the grammar can't be built from it.
    Schema(GbnfError),
    /// `as_of` isn't a `YYYY-MM-DD` date.
    BadDate,
    /// The model failed.
    Llm(LlmError),
}

impl fmt::Display for NlQueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schema(e) => write!(f, "query schema: {e}"),
            Self::BadDate => f.write_str("today's date isn't YYYY-MM-DD"),
            Self::Llm(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for NlQueryError {}

impl From<GbnfError> for NlQueryError {
    fn from(e: GbnfError) -> Self {
        NlQueryError::Schema(e)
    }
}

impl From<LlmError> for NlQueryError {
    fn from(e: LlmError) -> Self {
        NlQueryError::Llm(e)
    }
}

/// The weekday of a `YYYY-MM-DD` date.
fn weekday(date: &str) -> Result<&'static str, NlQueryError> {
    let mut parts = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (Some(Some(y)), Some(Some(m)), Some(Some(d))) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(NlQueryError::BadDate);
    };
    if date.len() != 10 || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(NlQueryError::BadDate);
    }
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    const NAMES: [&str; 7] = [
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
    ];
    Ok(NAMES[usize::try_from(days.rem_euclid(7)).unwrap_or(0)])
}

/// The grammar for one ledger: the query schema narrowed to `vocabulary`, with the
/// root starting after [`PREFIX`].
pub fn grammar(schema: &str, vocabulary: &Vocabulary) -> Result<Grammar, GbnfError> {
    let schema: serde_json::Value =
        serde_json::from_str(schema).map_err(|_| GbnfError::Malformed("schema text".into()))?;
    let mut g = gbnf::from_schema(&narrow(&schema, vocabulary)?)?;
    let item = gbnf::Expr::Ref("Item".into());
    g.set(
        "root",
        gbnf::Expr::Seq(vec![
            item.clone(),
            gbnf::Expr::Repeat {
                expr: Box::new(gbnf::Expr::Seq(vec![gbnf::Expr::Lit(",".into()), item])),
                min: 0,
                max: u32::try_from(MAX_ITEMS - 1).unwrap_or(0),
            },
            gbnf::Expr::Lit("]}".into()),
        ]),
    );
    Ok(g)
}

/// The system part of the prompt for `request`: the language, today, the user's
/// categories and accounts, and the worked examples. It is the same for every question
/// on one day and ledger, so a caller with a prompt cache (T2.4's `Session`) can keep it
/// and encode only the question.
pub fn system_prompt(request: &Request<'_>) -> Result<String, NlQueryError> {
    let request = Request {
        weekday: weekday(request.as_of)?,
        ..*request
    };
    Ok(prompt::system(&request))
}

/// The prompt in `model`'s chat format, up to where the answer starts.
fn chat(model: &Model, request: &Request<'_>) -> Result<String, NlQueryError> {
    Ok(model.chat_prompt(&system_prompt(request)?, request.question)?)
}

/// The full prompt for `request`, ending with [`PREFIX`]: what the model continues.
pub fn prompt(model: &Model, request: &Request<'_>) -> Result<String, NlQueryError> {
    Ok(format!(
        "{}{}{PREFIX}",
        chat(model, request)?,
        request.answer_lead
    ))
}

/// Interprets `request.question` as a query.
pub fn interpret(
    model: &Model,
    request: &Request<'_>,
    options: &GenerateOptions,
) -> Result<Interpretation, NlQueryError> {
    let grammar = grammar(request.schema, request.vocabulary)?.render();
    let chat = format!("{}{}", chat(model, request)?, request.answer_lead);
    let complete = |text: &str| {
        text.ends_with("]}")
            && serde_json::from_str::<serde_json::Value>(&format!("{PREFIX}{text}")).is_ok()
    };
    let g = model.generate(&chat, PREFIX, &grammar, &complete, options)?;
    Ok(Interpretation {
        json: format!("{PREFIX}{}", g.text),
        confidence_bp: g.confidence_bp,
        tokens: g.tokens,
        model_id: model.id().to_owned(),
        prompt_version: PROMPT_VERSION,
    })
}
