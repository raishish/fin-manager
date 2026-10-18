//! Without a model: the grammar accepts every TD.3 expected query and every worked
//! example, and rejects what the narrowing rules out; the examples don't leak TD.3.
//! With the CI test model (`FM_LLM_TEST_MODEL`): llama.cpp takes the grammar, and
//! whatever the model writes under it deserialises as a query.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value as Json;

use super::gbnf::canonical;
use super::*;
use crate::{CI_TEST_MODEL, Sampling};

fn read(rel: &str) -> Json {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// The default category list and TD.1's freelancer accounts.
fn vocabulary() -> Vocabulary {
    let t = read("../fm-core/assets/taxonomy/default-v1.json");
    let group_key = |id: &Json| {
        t["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| &g["id"] == id)
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let s = |v: &Json| v.as_str().unwrap().to_owned();
    Vocabulary {
        categories: t["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| Category {
                key: s(&c["key"]),
                name: s(&c["name"]),
                group: group_key(&c["group_id"]),
            })
            .collect(),
        groups: t["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| Group {
                key: s(&g["key"]),
                name: s(&g["name"]),
            })
            .collect(),
        accounts: [
            (1, "Share Draft Checking", "checking"),
            (2, "Regular Share Savings", "savings"),
            (3, "Capital One Venture", "credit_card"),
            (4, "Chase Freedom Flex", "credit_card"),
        ]
        .into_iter()
        .map(|(id, name, kind)| Account {
            id,
            name: name.into(),
            kind: kind.into(),
        })
        .collect(),
        currencies: vec!["EUR".into()],
        merchants: {
            let truth = read("../../testdata/synthetic/data/freelancer/truth.json");
            let mut m: Vec<String> = truth["transactions"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|t| t["merchant"].as_str().map(str::to_owned))
                .collect();
            m.sort();
            m.dedup();
            m
        },
    }
}

fn the_grammar(v: &Vocabulary) -> Grammar {
    grammar(fm_core::query::SCHEMA, v).unwrap()
}

/// What the model writes after [`PREFIX`] for `query`.
fn answer(query: &Json) -> String {
    let text = canonical(query);
    text.strip_prefix(PREFIX)
        .unwrap_or_else(|| panic!("not a query: {text}"))
        .to_owned()
}

fn td3() -> Json {
    read("../../testdata/questions/data/questions.json")
}

/// TD.3 and, once it exists, the held-out TD.7, in the same format.
fn test_sets() -> Vec<Json> {
    let holdout = "../../testdata/holdout-questions/data/questions.json";
    let mut sets = vec![td3()];
    if PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(holdout)
        .exists()
    {
        sets.push(read(holdout));
    }
    sets
}

#[test]
fn the_grammar_accepts_every_td3_expected_query() {
    let g = the_grammar(&vocabulary());
    let mut n = 0;
    for q in td3()["questions"].as_array().unwrap() {
        let query = &q["expected_query"];
        if query.is_null() {
            continue;
        }
        let id = q["id"].as_str().unwrap();
        assert!(g.accepts("root", &answer(query)), "{id}");
        n += 1;
    }
    assert_eq!(n, 98);
}

#[test]
fn the_grammar_holds_to_the_ledger() {
    let g = the_grammar(&vocabulary());
    let ok = r#"{"name":"total","ask":{"op":"sum","measure":"spending","scope":{"period":{"year":2025},"categories":["food.groceries"]}}}]}"#;
    assert!(g.accepts("root", ok));
    for bad in [
        // A category this ledger doesn't have.
        ok.replace("food.groceries", "food.grocery"),
        // No period.
        ok.replace(r#""period":{"year":2025},"#, ""),
        // An explicit null.
        ok.replace(r#""categories":["food.groceries"]"#, r#""merchant":null"#),
        // Seven items.
        format!("{}]}}", [&ok[..ok.len() - 2]; 7].join(",")),
        // An account that isn't there, and a currency that never occurs.
        ok.replace(
            r#""categories":["food.groceries"]"#,
            r#""accounts":{"ids":[9]}"#,
        ),
        ok.replace(
            r#""categories":["food.groceries"]"#,
            r#""original_currency":"GBP""#,
        ),
        // The account's own currency isn't an original currency.
        ok.replace(
            r#""categories":["food.groceries"]"#,
            r#""original_currency":"USD""#,
        ),
        // A merchant the ledger doesn't have, or a shortened name.
        ok.replace(
            r#""categories":["food.groceries"]"#,
            r#""merchant":"Lisbon""#,
        ),
        ok.replace(
            r#""categories":["food.groceries"]"#,
            r#""merchant":"Whole Foods""#,
        ),
        // Whitespace.
        ok.replace(r#""op":"sum""#, r#""op": "sum""#),
    ] {
        assert!(!g.accepts("root", &bad), "{bad}");
    }
    let six = format!("{}]}}", [&ok[..ok.len() - 2]; 6].join(","));
    assert!(g.accepts("root", &six));
    let named = ok.replace(
        r#""categories":["food.groceries"]"#,
        r#""merchant":"Whole Foods Market""#,
    );
    assert!(g.accepts("root", &named));
}

#[test]
fn every_example_is_a_valid_query_for_its_grammar() {
    let mut v = vocabulary();
    v.currencies.push("CAD".into());
    v.merchants
        .extend(["Costco", "Contoso", "Exxon", "Netflix"].map(String::from));
    let g = the_grammar(&v);
    for e in examples() {
        assert!(g.accepts("root", &answer(&e.query)), "{}", e.question);
        let q: fm_core::query::Query = serde_json::from_value(e.query.clone())
            .unwrap_or_else(|err| panic!("{}: {err}", e.question));
        assert!(!q.items.is_empty());
    }
}

#[test]
fn the_examples_dont_leak_the_test_sets() {
    // Compared as typed queries, so defaults written or left out don't hide a copy.
    // Failures name the example, never the test set's question.
    for set in test_sets() {
        no_leak(&set);
    }
}

fn no_leak(set: &Json) {
    use fm_core::query::{Ask, Query};
    let questions: BTreeSet<String> = set["questions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["question"].as_str().unwrap().to_lowercase())
        .collect();
    let queries: Vec<Query> = set["questions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|q| !q["expected_query"].is_null())
        .map(|q| serde_json::from_value(q["expected_query"].clone()).unwrap())
        .collect();
    let asks: Vec<&Ask> = queries
        .iter()
        .flat_map(|q| q.items.iter().map(|i| &i.ask))
        .filter(|a| !matches!(a, Ask::Derive { .. }))
        .collect();
    for e in examples() {
        assert!(
            !questions.contains(&e.question.to_lowercase()),
            "{}",
            e.question
        );
        let q: Query = serde_json::from_value(e.query.clone()).unwrap();
        assert!(!queries.contains(&q), "{}", e.question);
        for item in &q.items {
            assert!(
                !asks.contains(&&item.ask),
                "{}: item {} is one of TD.3's",
                e.question,
                item.name
            );
        }
    }
}

#[test]
fn weekdays() {
    for (d, w) in [
        ("2026-10-01", "Thursday"),
        ("1970-01-01", "Thursday"),
        ("2024-02-29", "Thursday"),
        ("2026-09-28", "Monday"),
        ("1999-12-31", "Friday"),
    ] {
        assert_eq!(weekday(d).unwrap(), w, "{d}");
    }
    for bad in ["2026-13-01", "2026-1-01", "yesterday", "2026-10-1"] {
        assert!(weekday(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_system_prompt_names_today_and_the_ledger_but_not_the_question() {
    let v = vocabulary();
    let q = "How much did I spend on pet food?";
    let text = system_prompt(&Request::new(q, "2026-10-01", fm_core::query::SCHEMA, &v)).unwrap();
    assert!(text.contains("Today is 2026-10-01 (Thursday)."));
    assert!(text.contains("food.groceries = Groceries"));
    assert!(text.contains("id 1: Share Draft Checking (checking)"));
    assert!(!text.contains(q));
    let other = system_prompt(&Request::new(
        "Something else?",
        "2026-10-01",
        fm_core::query::SCHEMA,
        &v,
    ))
    .unwrap();
    assert_eq!(text, other, "the same for every question");
    assert!(system_prompt(&Request::new(q, "2026-1-01", fm_core::query::SCHEMA, &v)).is_err());
}

#[test]
fn the_grammar_renders_with_root_first() {
    let text = the_grammar(&vocabulary()).render();
    assert!(text.starts_with("root ::= Item ("));
    assert!(text.contains(r#"\"food.groceries\""#));
}

// ---- With the CI test model.
//
// The two small ones (one short question on a tiny ledger, 128 tokens, greedy and one
// sampled run) run in CI since T2.7: the runtime samples each token first and checks
// only it against the grammar, instead of the whole vocabulary at every token (about
// 1 s per token on CI's macOS runners before). The full one stays ignored (minutes on
// a CPU); run it with
// `FM_LLM_TEST_MODEL=<gguf> cargo test -p fm-llm --lib nl_query -- --ignored`.
// The grammar and recogniser tests above cover validity without a model.

fn model() -> Option<Model> {
    let path = std::env::var_os("FM_LLM_TEST_MODEL").map(PathBuf::from);
    if path.is_none() {
        eprintln!("skipping real-model test: set FM_LLM_TEST_MODEL to the CI test model's path");
    }
    Some(Model::load(&path?, &CI_TEST_MODEL).expect("load the test model"))
}

fn options(sampling: Sampling, max_tokens: u32) -> GenerateOptions {
    GenerateOptions {
        sampling,
        max_tokens,
        threads: None,
    }
}

/// A ledger with a handful of keys, so the grammar and the prompt stay small.
fn tiny_vocabulary() -> Vocabulary {
    Vocabulary {
        categories: vec![
            Category {
                key: "food.groceries".into(),
                name: "Groceries".into(),
                group: "food".into(),
            },
            Category {
                key: "housing.rent".into(),
                name: "Rent".into(),
                group: "housing".into(),
            },
        ],
        groups: vec![
            Group {
                key: "food".into(),
                name: "Food".into(),
            },
            Group {
                key: "housing".into(),
                name: "Housing".into(),
            },
        ],
        accounts: vec![Account {
            id: 1,
            name: "Checking".into(),
            kind: "checking".into(),
        }],
        currencies: vec![],
        merchants: vec!["Corner Shop".into()],
    }
}

/// Whatever came out is a query, or the model hit the token cap (a tiny model can
/// wander); never anything malformed. Returns whether a query came out.
fn is_query_or_capped(r: Result<Interpretation, NlQueryError>) -> bool {
    match r {
        Ok(i) => {
            let q: fm_core::query::Query = serde_json::from_str(&i.json)
                .unwrap_or_else(|e| panic!("not a query ({e}): {}", i.json));
            assert!(!q.items.is_empty() && q.items.len() <= MAX_ITEMS as usize);
            assert_eq!(i.prompt_version, PROMPT_VERSION);
            true
        }
        Err(NlQueryError::Llm(LlmError::TooManyTokens { .. })) => false,
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn llama_takes_the_grammar_and_nothing_malformed_comes_out() {
    let Some(model) = model() else { return };
    let v = tiny_vocabulary();
    let request = Request::new("Rent in 2025?", "2026-10-01", fm_core::query::SCHEMA, &v);
    for sampling in [
        Sampling::Greedy,
        Sampling::Random {
            seed: 7,
            temperature: 1.0,
        },
    ] {
        is_query_or_capped(interpret(&model, &request, &options(sampling, 128)));
    }
}

#[test]
fn the_same_question_gives_the_same_reading() {
    let Some(model) = model() else { return };
    let v = tiny_vocabulary();
    let request = Request::new(
        "Groceries in 2024?",
        "2026-10-01",
        fm_core::query::SCHEMA,
        &v,
    );
    let a = interpret(&model, &request, &options(Sampling::Greedy, 128));
    let b = interpret(&model, &request, &options(Sampling::Greedy, 128));
    match (a, b) {
        (Ok(a), Ok(b)) => assert_eq!(a, b),
        (
            Err(NlQueryError::Llm(LlmError::TooManyTokens { .. })),
            Err(NlQueryError::Llm(LlmError::TooManyTokens { .. })),
        ) => {}
        (a, b) => panic!("different outcomes: {a:?} / {b:?}"),
    }
}

#[test]
#[ignore = "minutes on a CPU; see the note above"]
fn whatever_the_model_writes_is_a_query_full() {
    let Some(model) = model() else { return };
    let v = vocabulary();
    let mut written = 0;
    for (i, question) in [
        "How much did I spend on groceries in 2025?",
        "What were my top merchants last month?",
        "Did my rent go up?",
    ]
    .into_iter()
    .enumerate()
    {
        let mut runs = vec![Sampling::Greedy];
        for seed in 0..4 {
            runs.push(Sampling::Random {
                seed: 1_000 * i as u64 + seed,
                temperature: 1.0,
            });
        }
        for sampling in runs {
            let request = Request::new(question, "2026-10-01", fm_core::query::SCHEMA, &v);
            if is_query_or_capped(interpret(&model, &request, &options(sampling, 768))) {
                written += 1;
            }
        }
    }
    assert!(written > 0, "the model finished no query");
}
