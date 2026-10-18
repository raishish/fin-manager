//! Part (b), as a proxy until T4.1 defines the query type: each model sorts TD.3's
//! questions into TD.3's question types (constrained to the list), and each answer is
//! timed. The 20 questions typed `hard` are a difficulty bucket, not a kind of question,
//! so they are timed but not scored.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use fm_llm::{Choices, GenerateOptions, Model, SessionOptions};
use serde_json::{Value, json};

const TYPES: [(&str, &str); 5] = [
    (
        "totals",
        "a total for a named category, merchant or the whole household over an explicit period (a year, a quarter, a named month)",
    ),
    (
        "comparisons",
        "compares two periods or amounts, or asks which period was highest or lowest",
    ),
    (
        "merchants",
        "about specific merchants: spending there, top merchants, number of visits, average or biggest purchase",
    ),
    (
        "recurring",
        "about subscriptions or regular bills: current price, price changes, cancellations, totals paid to one",
    ),
    (
        "time_ranges",
        "a total over a period relative to today: last month, this year so far, the last few weeks",
    ),
];

pub fn run(model: &Model, run_dir: &Path, answer_lead: &str, threads: Option<u32>) -> Value {
    let data: Value = serde_json::from_slice(
        &fs::read(crate::td1::data_dir().join("questions/data/questions.json")).unwrap(),
    )
    .expect("TD.3 questions");
    let mut system = String::from(
        "You sort questions about a household's money by what kind of question they are. \
         Answer with exactly one key.\n\nKeys:\n",
    );
    for (key, meaning) in TYPES {
        system.push_str(&format!("{key}: {meaning}\n"));
    }
    const MARK: &str = "\u{1}";
    let full = model.chat_prompt(&system, MARK).expect("chat template");
    let (prefix, tail) = full.split_once(MARK).expect("user text in the prompt");
    let choices = Choices::new(TYPES.map(|(k, _)| k)).expect("distinct keys");
    let mut session = model
        .session(
            prefix,
            &SessionOptions {
                max_suffix_tokens: 128,
                max_answer_tokens: 16,
                threads,
            },
        )
        .expect("start a session");
    let options = GenerateOptions {
        max_tokens: 16,
        threads,
        ..GenerateOptions::default()
    };

    let mut lines = fs::File::create(run_dir.join("questions.jsonl")).expect("results file");
    let (mut scored, mut correct, mut total_ms) = (0, 0, 0.0);
    for q in data["questions"].as_array().unwrap() {
        let text = q["question"].as_str().unwrap();
        let expected = q["type"].as_str().unwrap();
        let suffix = format!("Question: {text}{tail}{answer_lead}");
        let t = Instant::now();
        let choice = session.choose(&suffix, &choices, &options);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        total_ms += ms;
        let (key, conf) = match &choice {
            Ok(c) => (c.key.clone(), c.confidence_bp),
            Err(e) => (format!("error: {e}"), 0),
        };
        if expected != "hard" {
            scored += 1;
            if key == expected {
                correct += 1;
            }
        }
        let record = json!({"id": q["id"], "expected": expected, "answer": key, "confidence_bp": conf, "ms": ms});
        writeln!(lines, "{record}").expect("write a result");
    }
    let n = data["questions"].as_array().unwrap().len();
    json!({"scored": scored, "correct": correct, "questions": n, "mean_ms": total_ms / n as f64})
}
