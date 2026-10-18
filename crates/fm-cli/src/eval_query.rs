//! Part (b): question → query (T4.1's `fm_core::query`), scored on TD.3.
//!
//! Each model writes one query per question, constrained only to well-formed JSON (a
//! generic JSON grammar; schema-constrained decoding is T4.2's job). The JSON is read as
//! a [`Query`], run on TD.1's freelancer ledger labelled from its truth, and its answers
//! compared with TD.3's expected answers: values exactly, and for sums and counts the
//! transaction ids behind them. Answers are matched by value, not by item name, so the
//! model gets no hint of how many answers a question wants or what they are called.
//!
//! This is a **floor**: T4.2's schema-constrained decoding can only make more outputs
//! valid. The same prompt, the same few-shot pairs ([`FEW_SHOT`], fixed before any
//! model ran) and the same grammar for every model.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use fm_core::model::{
    Assignment, Decision, DecisionId, DecisionSource, DecisionState, Ledger, Timestamp,
    TransactionId,
};
use fm_core::query::{self, Context, Query, Value as Got};
use fm_core::recurring::{Label, LabelKind, Labeler, SeriesSketch};
use fm_llm::{GenerateOptions, Model, SessionOptions};
use serde_json::{Value, json};

use crate::td1;

/// The worked examples in the prompt, chosen before any model ran, one per kind of
/// query. They aren't scored.
pub const FEW_SHOT: [&str; 8] = [
    "q002", // sum over a category and a year
    "q022", // month-on-month comparison (derive percent_change)
    "q035", // median per month over relative months
    "q037", // rank: top merchants
    "q040", // pick: largest purchase
    "q041", // distinct count
    "q051", // recurring: latest price
    "q068", // relative period: last three months
];

/// TD.3's known gaps (T4.1): the right query doesn't give the answer yet.
const KNOWN_GAPS: [&str; 2] = ["q062", "q065"];

const MAX_ANSWER_TOKENS: u32 = 400;

/// A generic JSON object grammar (llama.cpp's json.gbnf, whitespace limited).
const JSON_GBNF: &str = r#"root ::= object
value ::= object | array | string | number | ("true" | "false" | "null") ws
object ::= "{" ws ( string ":" ws value ("," ws string ":" ws value)* )? "}" ws
array ::= "[" ws ( value ("," ws value)* )? "]" ws
string ::= "\"" ( [^"\\\x7F\x00-\x1F] | "\\" (["\\bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F]) )* "\"" ws
number ::= ("-"? ([0-9] | [1-9] [0-9]{0,15})) ("." [0-9]+)? ws
ws ::= | " " | "\n" [ \t]{0,8}
"#;

/// TD.1's freelancer with one applied user decision per truth bucket and merchant, as
/// T4.1's done-when test builds it.
fn labelled() -> (Ledger, Value) {
    let h = td1::load("freelancer");
    let mut ledger = h.ledger;
    let mut decisions = Vec::new();
    let mut ids: Vec<&TransactionId> = h.truth.keys().collect();
    ids.sort();
    for id in ids {
        let mut assignments = vec![Assignment::Bucket(h.truth[id])];
        if let Some(m) = h.merchant.get(id) {
            assignments.push(Assignment::Merchant(m.clone()));
        }
        for assignment in assignments {
            decisions.push(Decision {
                id: DecisionId(decisions.len() as i64 + 1),
                transaction_id: *id,
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
    let truth: Value = serde_json::from_slice(
        &fs::read(td1::data_dir().join("synthetic/data/freelancer/truth.json")).unwrap(),
    )
    .expect("TD.1 truth");
    (ledger, truth)
}

/// Names series the way TD.1's truth does (T4.1's stand-in for T3.2's labeler).
struct TruthLabeler {
    by_description: HashMap<String, Label>,
}

impl TruthLabeler {
    fn new(ledger: &Ledger, truth: &Value) -> TruthLabeler {
        let description: HashMap<TransactionId, &str> = ledger
            .transactions
            .iter()
            .map(|t| (t.id, t.description.as_str()))
            .collect();
        let mut by_description = HashMap::new();
        for s in truth["recurring"].as_array().into_iter().flatten() {
            let kind = match s["kind"].as_str().unwrap_or("") {
                "subscription" => LabelKind::Subscription,
                "bill" => LabelKind::Bill,
                "income" => LabelKind::Income,
                "interest" => LabelKind::Interest,
                "transfer" => LabelKind::Transfer,
                _ => LabelKind::Other,
            };
            for id in s["transaction_ids"].as_array().into_iter().flatten() {
                let id = TransactionId(id.as_i64().unwrap_or(0));
                if let Some(d) = description.get(&id) {
                    by_description.insert(
                        (*d).to_owned(),
                        Label {
                            name: s["merchant"].as_str().unwrap_or("").to_owned(),
                            kind,
                        },
                    );
                }
            }
        }
        TruthLabeler { by_description }
    }
}

impl Labeler for TruthLabeler {
    fn label(&self, series: &SeriesSketch<'_>) -> Option<Label> {
        series
            .descriptions
            .iter()
            .find_map(|d| self.by_description.get(d).cloned())
    }
}

/// `v` without nulls, so the examples are short (every scope field defaults).
fn compact(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), compact(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(compact).collect()),
        v => v.clone(),
    }
}

fn instructions(ledger: &Ledger) -> String {
    let groups: Vec<&str> = ledger
        .taxonomy
        .groups
        .iter()
        .map(|g| g.key.as_str())
        .collect();
    let categories: Vec<&str> = ledger
        .taxonomy
        .selectable()
        .map(|c| c.key.as_str())
        .collect();
    format!(
        "You turn a question about a household's money into a query, written as one JSON \
         object. The app runs the query; you never compute numbers.\n\n\
         A query is {{\"items\": [item, ...]}}. Each item is {{\"name\": snake_case, \"ask\": ask}} \
         and gives one answer. An ask has \"op\":\n\
         - sum, count: {{\"op\", \"measure\", \"scope\"}}\n\
         - average, median: also \"per\": \"transaction\" or \"month\"\n\
         - rank: {{\"op\", \"measure\", \"scope\", \"by\": category|group|merchant|month|account, \
         \"order\": highest|lowest, \"metric\": amount|count, then \"n\" (top n labels) or \
         \"position\" with \"output\": label|amount|count}}\n\
         - keys, distinct: {{\"op\", \"measure\", \"scope\", \"by\"}}\n\
         - pick: {{\"op\", \"measure\", \"scope\", \"order\": largest|smallest|latest|earliest, \
         \"output\": merchant|amount|date}}\n\
         - recurring: {{\"op\", \"select\": {{\"name\", \"categories\", \"kinds\", \"cadences\", \
         \"status\": active|ended|cancelled|any, \"period\"}}, \"output\": names|count|latest|\
         latest_total|price_from|price_before|price_after|member_count|member_total}}\n\
         - derive: {{\"op\", \"formula\": difference|sum|percent_change|share|larger, \"of\": \
         [item names]}}\n\
         Measures: spending (net of refunds), purchases, refunds, income, transfers_in, \
         transfers_out, money_in, money_out, pending.\n\
         Scope (all optional): period, categories (keys), groups (keys), merchant, accounts, \
         counterpart_accounts, description, original_currency, amounts: account|original.\n\
         Periods: {{\"year\": 2025}}, {{\"month\": \"2025-07\"}}, {{\"months\": {{\"from\", \"to\"}}}}, \
         {{\"quarter\": {{\"year\", \"quarter\"}}}}, {{\"range\": {{\"from\", \"to\"}}}} (dates), \
         {{\"last_months\": n}}, {{\"last_days\": n}}, \"last_week\", \"last_quarter\", \
         \"year_to_date\", \"all_time\". Relative periods are counted from today, which the app \
         knows.\n\
         Group keys: {}\nCategory keys: {}\n",
        groups.join(", "),
        categories.join(", ")
    )
}

/// Whether `got` is the expected answer: the value, and the evidence ids for sums and
/// counts (as T4.1's done-when test checks).
fn matches(expected: &Value, got: &Got) -> bool {
    let traced = expected["evidence"]["rule"].as_str() != Some("derived");
    let ids = |v: &Value| -> Vec<i64> {
        let mut ids: Vec<i64> = v
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .collect();
        ids.sort_unstable();
        ids
    };
    let got_ids = |t: &[TransactionId]| t.iter().map(|t| t.0).collect::<Vec<_>>();
    match (expected["kind"].as_str(), got) {
        (
            Some("money"),
            Got::Money {
                amount,
                transaction_ids,
            },
        ) => {
            expected["amount"]["minor"].as_i64() == Some(amount.minor())
                && expected["amount"]["currency"].as_str() == Some(amount.currency().code())
                && (!traced
                    || ids(&expected["evidence"]["transaction_ids"]) == got_ids(transaction_ids))
        }
        (
            Some("count"),
            Got::Count {
                value,
                transaction_ids,
            },
        ) => {
            expected["value"].as_i64() == Some(*value)
                && (!traced
                    || ids(&expected["evidence"]["transaction_ids"]) == got_ids(transaction_ids))
        }
        (Some("percent"), Got::Percent { basis_points }) => {
            expected["basis_points"].as_i64() == Some(*basis_points)
        }
        (Some("date"), Got::Date { value }) => {
            expected["value"].as_str() == Some(value.to_string().as_str())
        }
        (Some("text"), Got::Text { value }) => expected["value"].as_str() == Some(value.as_str()),
        (Some("texts"), Got::Texts { values }) => {
            expected["values"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                == Some(values.iter().map(String::as_str).collect())
        }
        _ => false,
    }
}

/// Every expected answer is matched by a different answer the query gave.
fn all_match(expected: &[Value], got: &[fm_core::query::Answer]) -> bool {
    let mut used = HashSet::new();
    expected.iter().all(|e| {
        let hit = got
            .iter()
            .enumerate()
            .find(|(i, a)| !used.contains(i) && matches(e, &a.value));
        match hit {
            Some((i, _)) => {
                used.insert(i);
                true
            }
            None => false,
        }
    })
}

/// Writes a query for one question: the JSON text and the tokens it took.
type Writer<'a> = dyn FnMut(&Value) -> Result<(String, u32), String> + 'a;

/// Runs part (b) with `model`.
pub fn run(model: &Model, run_dir: &Path, answer_lead: &str, threads: Option<u32>) -> Value {
    let data = questions();
    let (ledger, _) = labelled();
    let questions = data["questions"].as_array().expect("questions");
    let mut system = instructions(&ledger);
    system.push_str("\nExamples:\n");
    for id in FEW_SHOT {
        let q = questions
            .iter()
            .find(|q| q["id"] == id)
            .expect("few-shot question exists");
        system.push_str(&format!(
            "\nQuestion: {}\nQuery: {}\n",
            q["question"].as_str().unwrap_or(""),
            compact(&q["expected_query"])
        ));
    }
    const MARK: &str = "\u{1}";
    let full = model.chat_prompt(&system, MARK).expect("chat template");
    let (prefix, tail) = full.split_once(MARK).expect("user text in the prompt");
    let mut session = model
        .session(
            prefix,
            &SessionOptions {
                max_suffix_tokens: 160,
                max_answer_tokens: MAX_ANSWER_TOKENS,
                threads,
            },
        )
        .expect("start a session");
    let options = GenerateOptions {
        max_tokens: MAX_ANSWER_TOKENS,
        threads,
        ..GenerateOptions::default()
    };
    let is_complete = |text: &str| serde_json::from_str::<Value>(text).is_ok_and(|v| v.is_object());
    let mut write = |q: &Value| {
        let suffix = format!(
            "Question: {}{tail}{answer_lead}",
            q["question"].as_str().unwrap_or("")
        );
        session
            .generate(&suffix, "", JSON_GBNF, &is_complete, &options)
            .map(|g| (g.text, g.tokens))
            .map_err(|e| e.to_string())
    };
    score(run_dir, &mut write)
}

/// Runs the scoring with TD.3's own expected queries in place of a model: every
/// question but the known gaps must come out exact, which checks the scoring itself.
pub fn oracle(run_dir: &Path) -> Value {
    let mut write = |q: &Value| Ok((compact(&q["expected_query"]).to_string(), 0));
    score(run_dir, &mut write)
}

fn questions() -> Value {
    serde_json::from_slice(
        &fs::read(td1::data_dir().join("questions/data/questions.json")).unwrap(),
    )
    .expect("TD.3 questions")
}

/// Scores every question but the few-shot ones and those without a query.
fn score(run_dir: &Path, write: &mut Writer<'_>) -> Value {
    let data = questions();
    let as_of = data["today"]
        .as_str()
        .and_then(|d| d.parse().ok())
        .expect("TD.3 has a date");
    let (ledger, truth) = labelled();
    let labeler = TruthLabeler::new(&ledger, &truth);
    let ctx = Context {
        as_of,
        labeler: Some(&labeler),
    };
    let questions = data["questions"].as_array().expect("questions");

    let mut lines = fs::File::create(run_dir.join("queries.jsonl")).expect("results file");
    for q in questions {
        let id = q["id"].as_str().unwrap_or("");
        if FEW_SHOT.contains(&id) || q["expected_query"].is_null() {
            continue;
        }
        let t = Instant::now();
        let generated = write(q);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let (json_ok, query_ok, ran, exact, error) = match &generated {
            Err(e) => (false, false, false, false, Some(e.clone())),
            Ok((text, _)) => match serde_json::from_str::<Query>(text) {
                Err(_) => (true, false, false, false, Some("not a query".to_string())),
                Ok(query) => match query::run(&ledger, &query, &ctx) {
                    Err(e) => (true, true, false, false, Some(e.to_string())),
                    Ok(answers) => {
                        let expected = q["answers"].as_array().cloned().unwrap_or_default();
                        (true, true, true, all_match(&expected, &answers), None)
                    }
                },
            },
        };
        let record = json!({
            "id": id, "type": q["type"], "ms": ms,
            "tokens": generated.as_ref().map(|g| g.1).unwrap_or(0),
            "json": json_ok, "query": query_ok, "ran": ran, "exact": exact,
            "proxy": !q["proxy"].is_null(), "known_gap": KNOWN_GAPS.contains(&id),
            "error": error,
        });
        writeln!(lines, "{record}").expect("write a result");
    }
    json!({"few_shot": FEW_SHOT, "known_gaps": KNOWN_GAPS})
}
