//! The Ask screen (T4.4): a question in, the interpreted query shown, the answers with
//! the transactions behind them, and an explanation whose every number is checked
//! (SPEC constraint 6).
//!
//! ```text
//! question → Interpreter → Query (+ what completion added) → query::run(ledger, as_of) → answers
//!                                    ↓ always shown                                     ↓
//!                              "How I read it"                       fm_llm::explain (check, template)
//! ```
//!
//! Two seams take the model (T4.5 installs both from `ask_model.rs`):
//! - [`Interpreter`] turns the question into a [`Query`]. Until the model is loaded,
//!   asking says the local model isn't ready (`NotReady`).
//! - [`Writer`] writes an explanation. Without one, the explanation is the template.
//!
//! The ledger is read under the session lock and then let go: the model runs for
//! seconds, and the auto-lock and other screens must not wait for it. If the ledger
//! locked meanwhile, the answer is dropped (`Locked`).
//!
//! `as_of` is the app's local date, checked as the dashboard checks it; the model never
//! gives it. Errors are fixed sentences that may name an item or a category key, never
//! an amount, a merchant or a description; nothing here logs.

use std::sync::Mutex;

use fm_core::model::{Date, Ledger};
use fm_core::query::{self, Answer, Context, Query, Value};
use fm_llm::explain::{self, Fact, Source};
use serde::Serialize;

use crate::ledger_view::{MoneyView, Result, local_today, locked, ui_error};
use crate::unlock::AppUnlock;

/// The longest question accepted, in characters.
const MAX_QUESTION: usize = 500;
/// How many times a writer may try before the template is used.
const ATTEMPTS: usize = 2;

/// What the user is told while no interpreter is installed.
pub const NOT_READY: &str =
    "Questions need the local model; it's still downloading or not set up yet.";

/// An item the app added that the question didn't ask for (T4.2's completion).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub name: String,
    /// The completion rule's stable id, e.g. `rank-amount`.
    pub rule: String,
    /// One line for the badge: why it was added.
    pub why: String,
}

/// A question read as a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interpretation {
    /// The completed query: the model's own items, then the added ones (appended, in
    /// `added` order, with unique names).
    pub query: Query,
    /// What completion added. Empty when nothing was.
    pub added: Vec<Added>,
}

impl Interpretation {
    fn added(&self, name: &str) -> Option<&Added> {
        self.added.iter().find(|a| a.name == name)
    }

    /// The model's own query: the completed one without the added items.
    fn own_query(&self) -> Query {
        Query {
            items: self
                .query
                .items
                .iter()
                .filter(|i| self.added(&i.name).is_none())
                .cloned()
                .collect(),
        }
    }
}

/// Why a question couldn't be read. Never repeats the question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpretError {
    /// The model isn't downloaded or loaded yet.
    NotReady,
    /// It isn't a question about this ledger the interpreter can turn into a query.
    NotUnderstood,
    /// The interpreter failed to run.
    Failed,
}

/// Turns a question into a query: T4.2's model (`ask_model.rs`), or a fixed table in
/// tests. `ledger` gives the names the question may use; `as_of` is today; `opened` is
/// the ledger's generation when it was read, so the model stops if it locks.
pub trait Interpreter: Send + Sync {
    fn interpret(
        &self,
        question: &str,
        ledger: &Ledger,
        as_of: Date,
        opened: u64,
    ) -> std::result::Result<Interpretation, InterpretError>;
}

/// Writes an explanation from a prompt (`ask_model.rs`): `attempt` counts from 1 for
/// one explanation, and `opened` is as for [`Interpreter`]. `None` when it can't.
pub trait Writer: Send + Sync {
    fn write(&self, prompt: &str, attempt: usize, opened: u64) -> Option<String>;
}

/// The Ask screen's model hooks, managed by the app.
#[derive(Default)]
pub struct AskEngine {
    interpreter: Mutex<Option<Box<dyn Interpreter>>>,
    writer: Mutex<Option<Box<dyn Writer>>>,
}

impl AskEngine {
    pub fn set_interpreter(&self, interpreter: Box<dyn Interpreter>) {
        *self.interpreter.lock().unwrap_or_else(|p| p.into_inner()) = Some(interpreter);
    }

    pub fn set_writer(&self, writer: Box<dyn Writer>) {
        *self.writer.lock().unwrap_or_else(|p| p.into_inner()) = Some(writer);
    }
}

/// The plain-English line for each item of `query` (T4.1's `describe`), without the
/// item's name in front: the screen shows the name as the label.
fn lines(ledger: &Ledger, query: &Query, as_of: Date) -> Vec<String> {
    query::describe(ledger, query, as_of)
        .into_iter()
        .zip(&query.items)
        .map(|(line, item)| {
            line.strip_prefix(&format!("{}: ", item.name))
                .map(str::to_owned)
                .unwrap_or(line)
        })
        .collect()
}

/// An item's name as words: `total_spent` is "Total spent".
fn label(name: &str) -> String {
    let words = name.replace(['_', '-'], " ");
    let mut chars = words.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// How the question was read, item by item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadItem {
    pub name: String,
    pub label: String,
    pub line: String,
    /// Added by the app, not asked for: the badge's text ("Added by the app: " and why).
    pub added: Option<String>,
    /// The completion rule that added it (a stable id).
    pub added_rule: Option<String>,
}

/// One answer, as the screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerView {
    pub name: String,
    pub label: String,
    /// The badge's text when the app added this answer.
    pub added: Option<String>,
    /// `money`, `count`, `text`, `texts`, `date` or `percent`.
    pub kind: &'static str,
    pub money: Option<MoneyView>,
    pub count: Option<i64>,
    pub texts: Vec<String>,
    /// `YYYY-MM-DD`.
    pub date: Option<String>,
    /// Basis points, and as the explanation writes them (`12.34%`).
    pub basis_points: Option<i64>,
    pub percent: Option<String>,
    /// The transactions behind a money or count answer, for the drill-down.
    pub transaction_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplanationView {
    pub text: String,
    /// `model` (checked against the answers) or `template` (written from them).
    pub source: &'static str,
    /// How many numbers the explanation states, all checked.
    pub numbers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskView {
    pub read: Vec<ReadItem>,
    /// Empty when the query couldn't run; then `problem` says why.
    pub answers: Vec<AnswerView>,
    pub explanation: Option<ExplanationView>,
    pub problem: Option<String>,
    /// The date the question was answered as of.
    pub as_of: String,
}

fn percent_text(basis_points: i64) -> String {
    let sign = if basis_points < 0 { "-" } else { "" };
    let bp = basis_points.unsigned_abs();
    let (whole, frac) = (bp / 100, bp % 100);
    if frac == 0 {
        format!("{sign}{whole}%")
    } else if frac.is_multiple_of(10) {
        format!("{sign}{whole}.{}%", frac / 10)
    } else {
        format!("{sign}{whole}.{frac:02}%")
    }
}

fn answer_view(a: &Answer, added: Option<String>) -> AnswerView {
    let mut v = AnswerView {
        name: a.name.clone(),
        label: label(&a.name),
        added,
        kind: "",
        money: None,
        count: None,
        texts: Vec::new(),
        date: None,
        basis_points: None,
        percent: None,
        transaction_ids: Vec::new(),
    };
    match &a.value {
        Value::Money {
            amount,
            transaction_ids,
        } => {
            v.kind = "money";
            v.money = Some(MoneyView::from(*amount));
            v.transaction_ids = transaction_ids.iter().map(|t| t.0).collect();
        }
        Value::Count {
            value,
            transaction_ids,
        } => {
            v.kind = "count";
            v.count = Some(*value);
            v.transaction_ids = transaction_ids.iter().map(|t| t.0).collect();
        }
        Value::Text { value } => {
            v.kind = "text";
            v.texts = vec![value.clone()];
        }
        Value::Texts { values } => {
            v.kind = "texts";
            v.texts = values.clone();
        }
        Value::Date { value } => {
            v.kind = "date";
            v.date = Some(value.to_string());
        }
        Value::Percent { basis_points } => {
            v.kind = "percent";
            v.basis_points = Some(*basis_points);
            v.percent = Some(percent_text(*basis_points));
        }
    }
    v
}

/// The answers as the explanation's facts: the values, and how many transactions each
/// money or count answer comes from.
pub fn facts(answers: &[Answer]) -> Vec<Fact> {
    answers
        .iter()
        .map(|a| {
            let (value, ids) = match &a.value {
                Value::Money {
                    amount,
                    transaction_ids,
                } => (
                    explain::Value::Money(explain::Money {
                        minor: amount.minor(),
                        currency: amount.currency().code().to_owned(),
                        minor_digits: amount.currency().minor_exponent(),
                    }),
                    transaction_ids.len(),
                ),
                Value::Count {
                    value,
                    transaction_ids,
                } => (explain::Value::Count(*value), transaction_ids.len()),
                Value::Text { value } => (explain::Value::Text(value.clone()), 0),
                Value::Texts { values } => (explain::Value::Texts(values.clone()), 0),
                Value::Date { value } => {
                    let (year, month, day) = value.ymd();
                    (explain::Value::Date { year, month, day }, 0)
                }
                Value::Percent { basis_points } => (
                    explain::Value::Percent {
                        basis_points: *basis_points,
                    },
                    0,
                ),
            };
            Fact {
                name: a.name.clone(),
                value,
                evidence: (ids > 0).then_some(ids as u64),
            }
        })
        .collect()
}

/// A question as the interpreter may see it: trimmed, 1 to [`MAX_QUESTION`] characters,
/// no control characters.
fn checked_question(question: &str) -> Result<&str> {
    let q = question.trim();
    if q.is_empty() || q.chars().count() > MAX_QUESTION {
        return Err(ui_error(
            "BadQuestion",
            format!("ask a question of 1 to {MAX_QUESTION} characters"),
        ));
    }
    if q.chars().any(char::is_control) {
        return Err(ui_error(
            "BadQuestion",
            "the question can't hold control characters",
        ));
    }
    Ok(q)
}

/// Answers `question` on `ledger` as of `as_of`, with `interpreter` and `writer`.
/// `opened` is the ledger's generation when it was read, and `open` says whether it
/// still is: after a lock nothing more is computed or written.
pub fn answer(
    ledger: &Ledger,
    question: &str,
    as_of: Date,
    (opened, open): (u64, &dyn Fn() -> bool),
    interpreter: &dyn Interpreter,
    writer: Option<&dyn Writer>,
) -> Result<AskView> {
    let interpretation = interpreter.interpret(question, ledger, as_of, opened);
    if !open() {
        return Err(locked());
    }
    let interpretation = interpretation.map_err(|e| match e {
        InterpretError::NotReady => ui_error("NotReady", NOT_READY),
        InterpretError::NotUnderstood => ui_error(
            "NotUnderstood",
            "that question couldn't be read as a question about your money; try rewording it",
        ),
        InterpretError::Failed => ui_error("InterpretFailed", "the question couldn't be read"),
    })?;
    let badge = |name: &str| {
        interpretation
            .added(name)
            .map(|a| format!("Added by the app: {}", a.why))
    };
    // The completed query first; if it can't run, the model's own query, without the
    // added items or their badges (an added item can fail where the asked ones don't).
    let completed = query::run(
        ledger,
        &interpretation.query,
        &Context {
            as_of,
            labeler: None,
        },
    );
    let (query, answers, badges) = match completed {
        Ok(answers) => (interpretation.query.clone(), Ok(answers), true),
        Err(e) if interpretation.added.is_empty() => (interpretation.query.clone(), Err(e), false),
        Err(_) => {
            let own = interpretation.own_query();
            let answers = query::run(
                ledger,
                &own,
                &Context {
                    as_of,
                    labeler: None,
                },
            );
            (own, answers, false)
        }
    };
    let read: Vec<ReadItem> = query
        .items
        .iter()
        .zip(lines(ledger, &query, as_of))
        .map(|(item, line)| ReadItem {
            name: item.name.clone(),
            label: label(&item.name),
            line,
            added: if badges { badge(&item.name) } else { None },
            added_rule: if badges {
                interpretation.added(&item.name).map(|a| a.rule.clone())
            } else {
                None
            },
        })
        .collect();
    let mut view = AskView {
        read,
        answers: Vec::new(),
        explanation: None,
        problem: None,
        as_of: as_of.to_string(),
    };
    let answers = match answers {
        Ok(a) => a,
        Err(e) => {
            // Shown with the interpretation, so the user sees what was asked.
            view.problem = Some(format!("This can't be answered: {e}."));
            return Ok(view);
        }
    };
    view.answers = answers
        .iter()
        .map(|a| answer_view(a, if badges { badge(&a.name) } else { None }))
        .collect();
    let facts = facts(&answers);
    // Each attempt only while the ledger is open: a lock ends the explanation (and the
    // check below then turns the answer away).
    let mut attempt = 0;
    let explanation = explain::explain(question, &facts, &[], ATTEMPTS, |prompt| {
        attempt += 1;
        writer
            .filter(|_| open())
            .and_then(|w| w.write(prompt, attempt, opened))
    });
    if !open() {
        return Err(locked());
    }
    let numbers = explain::check(&explanation.text, &facts, &[]).found;
    view.explanation = Some(ExplanationView {
        text: explanation.text,
        source: match explanation.source {
            Source::Model => "model",
            Source::Template => "template",
        },
        numbers,
    });
    Ok(view)
}

impl AppUnlock {
    pub fn ask(&self, engine: &AskEngine, question: &str, today: &str) -> Result<AskView> {
        let as_of = local_today(today, fm_core::model::Timestamp::now())?;
        let question = checked_question(question)?;
        let interpreter = engine.interpreter.lock().unwrap_or_else(|p| p.into_inner());
        let Some(interpreter) = interpreter.as_deref() else {
            return Err(ui_error("NotReady", NOT_READY));
        };
        let writer = engine.writer.lock().unwrap_or_else(|p| p.into_inner());
        // Read under the session lock, answered after it (see the module docs).
        let (ledger, generation) = self
            .with_ledger(|open| {
                open.ledger()
                    .load_ledger()
                    .map(|ledger| (ledger, self.generation()))
            })
            .ok_or_else(locked)??;
        let open = || self.generation() == generation;
        let view = answer(
            &ledger,
            question,
            as_of,
            (generation, &open),
            interpreter,
            writer.as_deref(),
        );
        drop(ledger);
        if !open() {
            return Err(locked());
        }
        view
    }
}

pub mod commands {
    use tauri::State;

    use super::{AskEngine, AskView};
    use crate::ledger_view::Result;
    use crate::unlock::AppUnlock;

    /// `today` is the UI's local date (`YYYY-MM-DD`), within a day of the system clock.
    #[tauri::command(async)]
    pub fn ask(
        unlock: State<'_, AppUnlock>,
        engine: State<'_, AskEngine>,
        question: String,
        today: String,
    ) -> Result<AskView> {
        unlock.ask(&engine, &question, &today)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use fm_core::model::{
        Assignment, Bucket, CategoryId, Decision, DecisionId, DecisionSource, DecisionState,
        Timestamp, TransactionId,
    };
    use serde_json::{Value as Json, json};
    use tauri::Manager;

    use super::*;
    use crate::tests::{Webview, create_confirmed, invoke, invoke_with, mock_app};

    fn testdata() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
    }

    fn read(path: PathBuf) -> Json {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    /// TD.1's freelancer, labelled as its truth says (the ledger TD.3 is computed on):
    /// one applied user decision per bucket and merchant.
    pub(crate) fn labelled_freelancer() -> Ledger {
        let dir = testdata().join("synthetic/data/freelancer");
        let mut ledger: Ledger =
            serde_json::from_slice(&std::fs::read(dir.join("ledger.json")).unwrap()).unwrap();
        let truth = read(dir.join("truth.json"));
        let by_key: HashMap<String, CategoryId> = ledger
            .taxonomy
            .categories
            .iter()
            .map(|c| (c.key.clone(), c.id))
            .collect();
        let tx = |v: &Json| v.as_i64().map(TransactionId);
        let mut decisions = Vec::new();
        for t in truth["transactions"].as_array().unwrap() {
            let b = &t["bucket"];
            let bucket = match b["kind"].as_str().unwrap() {
                "category" => Bucket::Category {
                    category_id: by_key[b["key"].as_str().unwrap()],
                },
                "refund" => Bucket::Refund {
                    category_id: by_key[b["key"].as_str().unwrap()],
                    original: tx(&b["original"]),
                },
                "transfer" => Bucket::Transfer {
                    counterpart: tx(&b["counterpart"]),
                },
                "income" => Bucket::Income,
                other => panic!("bucket {other}"),
            };
            let mut assignments = vec![Assignment::Bucket(bucket)];
            if let Some(m) = t["merchant"].as_str() {
                assignments.push(Assignment::Merchant(m.to_owned()));
            }
            for assignment in assignments {
                decisions.push(Decision {
                    id: DecisionId(decisions.len() as i64 + 1),
                    transaction_id: TransactionId(t["id"].as_i64().unwrap()),
                    assignment,
                    source: DecisionSource::User,
                    taxonomy_version: ledger.taxonomy.version.clone(),
                    decided_at: Timestamp::from_unix_seconds(1_760_000_000),
                    state: DecisionState::Applied,
                    seen_at: None,
                    replaces: None,
                });
            }
        }
        ledger.decisions = decisions;
        ledger
    }

    /// The stand-in: TD.3's questions and their expected queries, as a fixed table, in
    /// question-id order, so every test walks it the same way on every build.
    pub(crate) struct Table(pub(crate) Vec<(String, Query)>);

    impl Table {
        pub(crate) fn td3() -> Table {
            let set = read(testdata().join("questions/data/questions.json"));
            let mut rows: Vec<(String, String, Query)> = set["questions"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|q| !q["expected_query"].is_null())
                .map(|q| {
                    (
                        q["id"].as_str().unwrap().to_owned(),
                        q["question"].as_str().unwrap().to_owned(),
                        serde_json::from_value(q["expected_query"].clone()).unwrap(),
                    )
                })
                .collect();
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            Table(rows.into_iter().map(|(_, q, query)| (q, query)).collect())
        }
    }

    impl Interpreter for Table {
        fn interpret(
            &self,
            question: &str,
            _: &Ledger,
            _: Date,
            _: u64,
        ) -> std::result::Result<Interpretation, InterpretError> {
            let query = self
                .0
                .iter()
                .find(|(q, _)| q == question)
                .map(|(_, query)| query.clone())
                .ok_or(InterpretError::NotUnderstood)?;
            Ok(Interpretation {
                query,
                added: Vec::new(),
            })
        }
    }

    /// The table, with one item appended to every query, as T4.2's completion appends.
    struct Completing {
        table: Table,
        extra: query::Item,
    }

    impl Interpreter for Completing {
        fn interpret(
            &self,
            question: &str,
            ledger: &Ledger,
            as_of: Date,
            opened: u64,
        ) -> std::result::Result<Interpretation, InterpretError> {
            let mut i = self.table.interpret(question, ledger, as_of, opened)?;
            i.query.items.push(self.extra.clone());
            i.added.push(Added {
                name: self.extra.name.clone(),
                rule: "test-rule".into(),
                why: "to show how many transactions this covers".into(),
            });
            Ok(i)
        }
    }

    pub(crate) fn today() -> String {
        crate::ledger_view::utc_date(Timestamp::now()).to_string()
    }

    pub(crate) fn app_with_freelancer() -> (
        tempfile::TempDir,
        tauri::App<tauri::test::MockRuntime>,
        Webview,
        Ledger,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
        create_confirmed(&w);
        let ledger = labelled_freelancer();
        app.state::<AppUnlock>()
            .write_ledger(|open| open.ledger_mut().save_ledger(&ledger))
            .unwrap()
            .unwrap();
        (dir, app, w, ledger)
    }

    pub(crate) fn ask(w: &Webview, question: &str) -> std::result::Result<Json, Json> {
        invoke_with(w, "ask", json!({ "question": question, "today": today() }))
    }

    /// Done-when: every TD.3 question, end to end through IPC on TD.1's data with the
    /// stand-in interpreter, gives exactly what the query engine gives, with the
    /// interpretation shown and an explanation whose numbers all check.
    #[test]
    fn td3_questions_end_to_end_through_ipc() {
        let (_dir, app, w, _) = app_with_freelancer();
        let table = Table::td3();
        let questions: Vec<(String, Query)> = table.0.clone();
        app.state::<AskEngine>().set_interpreter(Box::new(table));
        let stored = app
            .state::<AppUnlock>()
            .with_ledger(|open| open.ledger().load_ledger().unwrap())
            .unwrap();
        let as_of: fm_core::model::Date = today().parse().unwrap();
        let (mut answered, mut problems, mut numbers) = (0, 0, 0);
        // The first answer, in question order, with transactions behind it.
        let mut drilled: Option<Vec<i64>> = None;
        for (question, query) in &questions {
            let view = ask(&w, question).unwrap_or_else(|e| panic!("{question}: {e}"));
            // The interpretation is always there, one line per item.
            let read = view["read"].as_array().unwrap();
            assert_eq!(read.len(), query.items.len());
            assert!(
                read.iter()
                    .all(|r| !r["line"].as_str().unwrap().is_empty() && r["added"].is_null())
            );
            assert_eq!(view["asOf"], today());
            match query::run(
                &stored,
                query,
                &Context {
                    as_of,
                    labeler: None,
                },
            ) {
                Ok(expected) => {
                    let got = view["answers"].as_array().unwrap();
                    assert_eq!(got.len(), expected.len(), "{question}");
                    for (g, e) in got.iter().zip(&expected) {
                        let want = serde_json::to_value(answer_view(e, None)).unwrap();
                        assert_eq!(g, &want, "{question}");
                    }
                    let explanation = &view["explanation"];
                    assert_eq!(explanation["source"], "template");
                    let text = explanation["text"].as_str().unwrap();
                    assert!(
                        explain::check(text, &facts(&expected), &[]).passed(),
                        "{question}"
                    );
                    numbers += explanation["numbers"].as_u64().unwrap();
                    if drilled.is_none() {
                        drilled = got
                            .iter()
                            .map(|a| a["transactionIds"].as_array().unwrap())
                            .find(|ids| !ids.is_empty())
                            .map(|ids| ids.iter().map(|i| i.as_i64().unwrap()).collect());
                    }
                    answered += 1;
                }
                Err(_) => {
                    assert!(view["problem"].is_string(), "{question}");
                    assert!(view["answers"].as_array().unwrap().is_empty());
                    problems += 1;
                }
            }
        }
        println!("Ask: {answered} answered, {problems} not answerable, {numbers} numbers checked");
        assert_eq!(answered + problems, questions.len());
        assert!(answered >= 90, "{answered} answered");

        // The drill-down: that answer's transactions resolve through `transactions`.
        let ids = drilled.expect("some answer has transactions behind it");
        let shown = invoke_with(&w, "transactions", json!({ "ids": ids })).unwrap();
        assert_eq!(shown.as_array().unwrap().len(), ids.len());
    }

    #[test]
    fn added_items_are_marked_and_a_failing_one_falls_back_to_the_asked_query() {
        let (_dir, app, w, _) = app_with_freelancer();
        let table = Table::td3();
        let (question, query) = table
            .0
            .iter()
            .find(|(_, q)| q.items.len() == 1 && matches!(q.items[0].ask, query::Ask::Sum { .. }))
            .map(|(q, query)| (q.clone(), query.clone()))
            .unwrap();
        let query::Ask::Sum { measure, scope } = query.items[0].ask.clone() else {
            unreachable!()
        };
        let engine = app.state::<AskEngine>();

        // A completion that works: the added count is badged, in the reading and the answer.
        engine.set_interpreter(Box::new(Completing {
            table: Table::td3(),
            extra: query::Item {
                name: "how_many".into(),
                ask: query::Ask::Count { measure, scope },
            },
        }));
        let view = ask(&w, &question).unwrap();
        let read = view["read"].as_array().unwrap();
        assert_eq!(read.len(), 2);
        assert!(read[0]["added"].is_null());
        assert_eq!(
            read[1]["added"],
            "Added by the app: to show how many transactions this covers"
        );
        assert_eq!(read[1]["addedRule"], "test-rule");
        let answers = view["answers"].as_array().unwrap();
        assert_eq!(answers.len(), 2);
        assert!(answers[0]["added"].is_null());
        assert_eq!(answers[1]["added"], read[1]["added"]);
        assert_eq!(answers[1]["kind"], "count");

        // A completion that can't run: the asked query's answers, and no badges.
        engine.set_interpreter(Box::new(Completing {
            table: Table::td3(),
            extra: query::Item {
                name: "broken".into(),
                ask: query::Ask::Derive {
                    formula: query::DeriveOp::Difference,
                    of: vec!["nothing".into(), "here".into()],
                },
            },
        }));
        let view = ask(&w, &question).unwrap();
        assert_eq!(view["read"].as_array().unwrap().len(), 1);
        assert!(view["read"][0]["added"].is_null());
        assert_eq!(view["answers"].as_array().unwrap().len(), 1);
        assert!(view["answers"][0]["added"].is_null());
        assert!(view["problem"].is_null());
    }

    #[test]
    fn asking_needs_an_interpreter_a_question_a_date_and_an_unlocked_ledger() {
        let (_dir, app, w, _) = app_with_freelancer();
        // No interpreter yet: the calm "not ready" message.
        let e = ask(&w, "How much did I spend?").unwrap_err();
        assert_eq!(
            (e["code"].as_str(), e["message"].as_str()),
            (Some("NotReady"), Some(NOT_READY))
        );
        app.state::<AskEngine>()
            .set_interpreter(Box::new(Table::td3()));
        for (question, code) in [("   ", "BadQuestion"), ("a\u{7}b", "BadQuestion")] {
            assert_eq!(ask(&w, question).unwrap_err()["code"], code);
        }
        assert_eq!(
            ask(&w, &"x".repeat(501)).unwrap_err()["code"],
            "BadQuestion"
        );
        assert_eq!(
            ask(&w, "What is the meaning of life?").unwrap_err()["code"],
            "NotUnderstood"
        );
        let e = invoke_with(
            &w,
            "ask",
            json!({ "question": "How much?", "today": "1999-01-01" }),
        )
        .unwrap_err();
        assert_eq!(e["code"], "InvalidDate");
        invoke(&w, "lock").unwrap();
        assert_eq!(
            ask(&w, "How much did I spend?").unwrap_err(),
            json!(crate::unlock::LOCKED)
        );
    }

    /// Locks the ledger while it "thinks", as the auto-lock would during a slow model.
    struct LocksMidway(tauri::AppHandle<tauri::test::MockRuntime>);

    impl Interpreter for LocksMidway {
        fn interpret(
            &self,
            question: &str,
            ledger: &Ledger,
            as_of: Date,
            opened: u64,
        ) -> std::result::Result<Interpretation, InterpretError> {
            // The ledger isn't held while the interpreter runs, so locking doesn't wait.
            self.0.state::<AppUnlock>().lock();
            Table::td3().interpret(question, ledger, as_of, opened)
        }
    }

    /// The model runs outside the session lock; an answer finished after the ledger
    /// locked is dropped, not shown.
    #[test]
    fn an_answer_finished_after_the_ledger_locked_is_dropped() {
        let (_dir, app, w, _) = app_with_freelancer();
        let question = Table::td3().0[0].0.clone();
        app.state::<AskEngine>()
            .set_interpreter(Box::new(LocksMidway(app.handle().clone())));
        assert_eq!(ask(&w, &question).unwrap_err()["code"], "Locked");
    }

    /// Locks the ledger while writing, and writes a text that fails the number check,
    /// so `explain` would ask again if nothing stopped it.
    struct LocksWhileWriting {
        app: tauri::AppHandle<tauri::test::MockRuntime>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl Writer for LocksWhileWriting {
        fn write(&self, _: &str, attempt: usize, _: u64) -> Option<String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(attempt, 1);
            self.app.state::<AppUnlock>().lock();
            Some("It was 123456789.".into())
        }
    }

    /// Review finding 2: after a lock nothing more is written, and the answer is dropped.
    #[test]
    fn a_lock_while_explaining_stops_the_explanation() {
        let (_dir, app, w, _) = app_with_freelancer();
        let question = Table::td3().0[0].0.clone();
        let engine = app.state::<AskEngine>();
        engine.set_interpreter(Box::new(Table::td3()));
        let writer = std::sync::Arc::new(LocksWhileWriting {
            app: app.handle().clone(),
            calls: Default::default(),
        });
        struct Shared(std::sync::Arc<LocksWhileWriting>);
        impl Writer for Shared {
            fn write(&self, prompt: &str, attempt: usize, opened: u64) -> Option<String> {
                self.0.write(prompt, attempt, opened)
            }
        }
        engine.set_writer(Box::new(Shared(std::sync::Arc::clone(&writer))));
        assert_eq!(ask(&w, &question).unwrap_err()["code"], "Locked");
        assert_eq!(
            writer.calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "no second attempt after the lock"
        );
    }

    #[test]
    fn each_line_is_the_query_described_without_the_items_name() {
        let ledger = labelled_freelancer();
        let query: Query = serde_json::from_value(json!({ "items": [
            { "name": "visits", "ask": { "op": "count", "measure": "purchases",
              "scope": { "period": { "year": 2025 } } } },
            { "name": "change", "ask": { "op": "derive", "formula": "percent_change",
              "of": ["visits", "visits"] } }
        ]}))
        .unwrap();
        let as_of = Date::from_ymd(2026, 10, 1).unwrap();
        let described = query::describe(&ledger, &query, as_of);
        let lines = lines(&ledger, &query, as_of);
        assert_eq!(lines.len(), 2);
        for ((line, full), item) in lines.iter().zip(&described).zip(&query.items) {
            assert_eq!(full, &format!("{}: {line}", item.name));
            assert!(!line.is_empty());
        }
    }
}
