//! Tests against a real model: the pinned CI test model ([`CI_TEST_MODEL`]).
//!
//! They run only when `FM_LLM_TEST_MODEL` holds the path of that GGUF file,
//! and are skipped with a message otherwise. To fetch it:
//!
//! ```text
//! curl -fL -o SmolLM2-135M-Instruct-Q8_0.gguf \
//!   https://huggingface.co/bartowski/SmolLM2-135M-Instruct-GGUF/resolve/09816acd5d99df7be770d85ea30822623dab342c/SmolLM2-135M-Instruct-Q8_0.gguf
//! echo "5a1395716f7913741cc51d98581b9b1228d80987a9f7d3664106742eb06bba83  SmolLM2-135M-Instruct-Q8_0.gguf" | shasum -a 256 -c
//! FM_LLM_TEST_MODEL=$PWD/SmolLM2-135M-Instruct-Q8_0.gguf cargo test -p fm-llm
//! ```

use std::path::PathBuf;

use fm_llm::{
    CHOICE_PREFIX, CI_TEST_MODEL, Choices, GenerateOptions, LlmError, LoadOptions,
    MAX_BASIS_POINTS, Model, Sampling, SessionOptions, UNCATEGORISED,
};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

const ENV: &str = "FM_LLM_TEST_MODEL";

fn model_path() -> Option<PathBuf> {
    let path = std::env::var_os(ENV).map(PathBuf::from);
    if path.is_none() {
        eprintln!("skipping real-model test: set {ENV} to the CI test model's path");
    }
    path
}

/// The test model, or `None` when not configured. Each test loads its own and
/// drops it before the process exits: ggml's Metal device asserts at exit if
/// a model is still alive, so a model must never live in a `static`.
/// `FM_LLM_TEST_CPU=1` loads it on the CPU only, e.g. where the GPU is too slow
/// to use (CI's macOS virtual machines).
fn model() -> Option<Model> {
    let path = model_path()?;
    let gpu = std::env::var_os("FM_LLM_TEST_CPU").is_none();
    Some(
        Model::load_with(&path, &CI_TEST_MODEL, &LoadOptions { gpu }).expect("load the test model"),
    )
}

/// Category keys from the default list (fm-core's taxonomy asset).
fn default_keys() -> Vec<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fm-core/assets/taxonomy/default-v1.json"
    );
    let taxonomy: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    taxonomy["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["key"].as_str().unwrap().to_string())
        .collect()
}

fn categorise_prompt(model: &Model, choices: &Choices, description: &str) -> String {
    // Keys go in JSON-encoded: raw keys may hold control characters, and a NUL
    // can't pass through llama.cpp's C strings.
    let system = format!(
        "You categorise bank transactions. Answer with JSON: {{\"category\": <key>}}. \
         Keys: {}.",
        serde_json::to_string(choices.keys()).unwrap()
    );
    model
        .chat_prompt(&system, &format!("Transaction: {description}"))
        .unwrap()
}

#[test]
fn the_model_file_is_the_pinned_one() {
    let Some(model) = model() else { return };
    assert_eq!(model.id(), CI_TEST_MODEL.id);
    assert_eq!(model.gguf_sha256(), CI_TEST_MODEL.sha256);
}

#[test]
fn a_tampered_file_is_refused() {
    let Some(path) = model_path() else { return };
    let mut spec = CI_TEST_MODEL.clone();
    spec.sha256[31] ^= 1;
    let err = Model::load(&path, &spec).unwrap_err();
    assert!(matches!(err, LlmError::File(_)), "{err}");
}

#[test]
fn runs_on_metal_on_apple_silicon() {
    let Some(_model) = model() else { return };
    let gpu = fm_llm::gpu_offload_supported().unwrap();
    eprintln!("GPU offload supported: {gpu}");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        assert!(gpu, "Metal backend missing on Apple Silicon");
    }
}

#[test]
fn picks_a_key_from_the_default_list_with_a_confidence() {
    let Some(model) = model() else { return };
    let choices = Choices::new(default_keys()).unwrap();
    for description in [
        "WHOLE FOODS MARKET #10234 AUSTIN TX",
        "SHELL OIL 57444 HOUSTON",
        "NETFLIX.COM 866-579-7172 CA",
        "Zelle payment from J SMITH",
    ] {
        let prompt = categorise_prompt(&model, &choices, description);
        let choice = model
            .choose(&prompt, &choices, &GenerateOptions::default())
            .unwrap();
        eprintln!(
            "{description:?} -> {} ({} bp)",
            choice.key, choice.confidence_bp
        );
        assert!(choices.keys().contains(&choice.key));
        assert!(choice.confidence_bp <= MAX_BASIS_POINTS);
    }
}

#[test]
fn greedy_answers_repeat_exactly() {
    let Some(model) = model() else { return };
    let choices = Choices::new(default_keys()).unwrap();
    let prompt = categorise_prompt(&model, &choices, "UBER *TRIP HELP.UBER.COM");
    let first = model
        .choose(&prompt, &choices, &GenerateOptions::default())
        .unwrap();
    for _ in 0..3 {
        let again = model
            .choose(&prompt, &choices, &GenerateOptions::default())
            .unwrap();
        assert_eq!(again, first);
    }
}

#[test]
fn a_single_option_is_certain() {
    let Some(model) = model() else { return };
    // Only `uncategorised` is offered: every token is forced.
    let choices = Choices::new(Vec::<String>::new()).unwrap();
    let prompt = categorise_prompt(&model, &choices, "ANYTHING");
    let choice = model
        .choose(&prompt, &choices, &GenerateOptions::default())
        .unwrap();
    assert_eq!(choice.key, UNCATEGORISED);
    assert_eq!(choice.confidence_bp, MAX_BASIS_POINTS);
}

#[test]
fn generate_stops_at_the_token_limit() {
    let Some(model) = model() else { return };
    let choices = Choices::new(["a.very.long.category.key.that.takes.many.tokens"]).unwrap();
    let prompt = categorise_prompt(&model, &choices, "X");
    let options = GenerateOptions {
        max_tokens: 1,
        ..GenerateOptions::default()
    };
    // Whatever the model picks, one token can't finish `<key>"}`.
    let err = model.choose(&prompt, &choices, &options).unwrap_err();
    assert!(
        matches!(err, LlmError::TooManyTokens { max_tokens: 1 }),
        "{err}"
    );
}

/// Keys built to stress the grammar: JSON escapes, GBNF metacharacters,
/// control characters, multi-byte UTF-8, and keys that are prefixes of others.
fn awkward_key() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z_]{1,8}(\\.[a-z_]{1,8})?",
        "[\"\\\\{}\\[\\]|()*+?:a-z ]{1,8}",
        "[\\x00-\\x1f\\x7fa-z]{1,6}",
        "[é✓🙂中文a-z]{1,6}",
        "\\PC{1,10}",
    ]
}

fn awkward_choices() -> impl Strategy<Value = Choices> {
    proptest::collection::vec(awkward_key(), 1..8).prop_filter_map("valid list", |keys| {
        let mut unique = Vec::new();
        for key in keys {
            // Shared prefixes on purpose: add each key's first half too.
            let half: String = key.chars().take(key.chars().count() / 2).collect();
            for k in [half, key] {
                if !k.is_empty() && k != UNCATEGORISED && !unique.contains(&k) {
                    unique.push(k);
                }
            }
        }
        Choices::new(unique).ok()
    })
}

/// SPEC constraint 3: whatever the model does, the answer is one of the keys.
/// High-temperature random sampling drives the model down unlikely paths
/// through the grammar, which greedy decoding would never take.
#[test]
fn output_never_breaks_the_schema() {
    let Some(model) = model() else { return };
    let cases = std::env::var("FM_LLM_PROPTEST_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(48);
    let mut runner = TestRunner::new(Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    });
    let strategy = (
        awkward_choices(),
        any::<u64>(),
        prop_oneof![Just(1.0_f32), Just(3.0_f32), Just(20.0_f32)],
        "\\PC{0,40}",
    );
    runner
        .run(&strategy, |(choices, seed, temperature, description)| {
            let prompt = categorise_prompt(&model, &choices, &description);
            let options = GenerateOptions {
                sampling: Sampling::Random { seed, temperature },
                max_tokens: 512,
                threads: None,
            };
            // The raw generated text, not a re-encoding of the parsed key. A
            // grammar that let anything else through would never complete and
            // fail with `TooManyTokens`.
            let generation = model
                .generate(
                    &prompt,
                    CHOICE_PREFIX,
                    &choices.grammar(),
                    &|text| choices.parse(text).is_some(),
                    &options,
                )
                .map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert!(generation.confidence_bp <= MAX_BASIS_POINTS);

            // The full answer is exactly the JSON object the schema describes.
            let answer = format!("{CHOICE_PREFIX}{}", generation.text);
            let parsed: serde_json::Value = serde_json::from_str(&answer)
                .map_err(|e| TestCaseError::fail(format!("{answer:?}: {e}")))?;
            let object = parsed.as_object().expect("a JSON object");
            prop_assert_eq!(object.len(), 1);
            let key = object["category"].as_str().expect("a string");
            prop_assert!(
                choices.keys().iter().any(|k| k == key),
                "{key:?} not offered"
            );

            // `choose` adds the canonical-token mask on top of the grammar:
            // its raw answer must be a listed key too.
            let choice = model
                .choose(&prompt, &choices, &options)
                .map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert!(choices.keys().contains(&choice.key));
            prop_assert!(choice.confidence_bp <= MAX_BASIS_POINTS);
            Ok(())
        })
        .unwrap();
}

/// A session's prefix and per-transaction suffix, split where a categorisation prompt
/// splits: after the shared system part, before the transaction.
fn split_prompt(model: &Model, choices: &Choices, description: &str) -> (String, String) {
    let full = categorise_prompt(model, choices, description);
    let at = full
        .find("Transaction: ")
        .expect("the prompt names the transaction");
    (full[..at].to_string(), full[at..].to_string())
}

#[test]
fn a_session_answers_exactly_like_a_fresh_context_and_keeps_no_state_between_calls() {
    let Some(model) = model() else { return };
    let choices = Choices::new(default_keys()).unwrap();
    let descriptions = [
        "WHOLE FOODS MARKET #10234 AUSTIN TX",
        "SHELL OIL 57444 HOUSTON",
        "NETFLIX.COM 866-579-7172 CA",
        "Zelle payment from J SMITH",
    ];
    let (prefix, _) = split_prompt(&model, &choices, descriptions[0]);
    let options = GenerateOptions::default();

    // Each answer from a brand-new session: nothing evaluated before it but the prefix.
    let fresh: Vec<_> = descriptions
        .iter()
        .map(|d| {
            let (p, suffix) = split_prompt(&model, &choices, d);
            assert_eq!(p, prefix);
            let mut s = model.session(&p, &SessionOptions::default()).unwrap();
            s.choose(&suffix, &choices, &options).unwrap()
        })
        .collect();

    // One session reused for all of them, in an order that would show leakage: the same
    // transaction before and after others.
    let mut session = model.session(&prefix, &SessionOptions::default()).unwrap();
    for i in [0, 1, 2, 3, 0, 3, 1, 2] {
        let (_, suffix) = split_prompt(&model, &choices, descriptions[i]);
        let reused = session.choose(&suffix, &choices, &options).unwrap();
        assert_eq!(reused, fresh[i], "call for transaction {i}");
    }

    // And the same as evaluating the whole prompt at once.
    for (i, d) in descriptions.iter().enumerate() {
        let whole = model
            .choose(&categorise_prompt(&model, &choices, d), &choices, &options)
            .unwrap();
        assert_eq!(whole.key, fresh[i].key, "transaction {i}");
        assert!(
            whole.confidence_bp.abs_diff(fresh[i].confidence_bp) <= 50,
            "transaction {i}: {} vs {}",
            whole.confidence_bp,
            fresh[i].confidence_bp
        );
    }
}

#[test]
fn on_the_cpu_a_session_answers_exactly_like_the_whole_prompt() {
    // On the CPU, batching the prompt differently (prefix then suffix, or all at once)
    // must not change any answer, not even by a basis point: a CPU-only machine
    // applies an answer when its confidence crosses the threshold.
    let Some(path) = model_path() else { return };
    let model = Model::load_with(&path, &CI_TEST_MODEL, &LoadOptions { gpu: false }).unwrap();
    let choices = Choices::new(default_keys()).unwrap();
    let options = GenerateOptions::default();
    let (prefix, _) = split_prompt(&model, &choices, "");
    let mut session = model.session(&prefix, &SessionOptions::default()).unwrap();
    for d in [
        "WHOLE FOODS MARKET #10234 AUSTIN TX",
        "SHELL OIL 57444 HOUSTON",
        "NETFLIX.COM 866-579-7172 CA",
        "Zelle payment from J SMITH",
    ] {
        let (_, suffix) = split_prompt(&model, &choices, d);
        let whole = model
            .choose(&categorise_prompt(&model, &choices, d), &choices, &options)
            .unwrap();
        assert_eq!(session.choose(&suffix, &choices, &options).unwrap(), whole);
    }
}

#[test]
fn a_session_split_inside_a_token_answers_like_the_whole_prompt() {
    // "Pick on" + "e." tokenises differently from "Pick one.": the session must give
    // the model the whole prompt's tokens, not the suffix tokenised on its own.
    let Some(model) = model() else { return };
    let choices = Choices::new(["a", "b"]).unwrap();
    let options = GenerateOptions::default();
    let whole = model.choose("Pick one.\n", &choices, &options).unwrap();
    let mut session = model
        .session("Pick on", &SessionOptions::default())
        .unwrap();
    assert_eq!(session.choose("e.\n", &choices, &options).unwrap(), whole);
    // And again, from the shorter cache.
    assert_eq!(session.choose("e.\n", &choices, &options).unwrap(), whole);
}

#[test]
fn a_session_refuses_a_suffix_or_answer_over_its_limits() {
    let Some(model) = model() else { return };
    let choices = Choices::new(["a", "b"]).unwrap();
    let options = SessionOptions {
        max_suffix_tokens: 8,
        max_answer_tokens: 16,
        threads: None,
    };
    let mut session = model.session("Pick one.\n", &options).unwrap();
    let long = "word ".repeat(50);
    let err = session
        .choose(&long, &choices, &GenerateOptions::default())
        .unwrap_err();
    assert!(matches!(err, LlmError::PromptTooLong { .. }), "{err}");
    let err = session
        .choose(
            "x",
            &choices,
            &GenerateOptions {
                max_tokens: 17,
                ..GenerateOptions::default()
            },
        )
        .unwrap_err();
    assert!(matches!(err, LlmError::PromptTooLong { .. }), "{err}");
    // Still usable afterwards.
    let ok = session.choose(
        "x",
        &choices,
        &GenerateOptions {
            max_tokens: 16,
            ..GenerateOptions::default()
        },
    );
    assert!(ok.is_ok(), "{ok:?}");
}

/// T2.7: sampling first and checking only that token gives the same greedy text as
/// checking every token, on the categorisation grammar and on T4.2's query grammar; the
/// fast confidence is the model's probability over its whole vocabulary, so it is at
/// most the exact one.
#[test]
fn the_grammar_fast_path_writes_what_the_full_check_writes() {
    let Some(model) = model() else { return };
    let options = GenerateOptions {
        max_tokens: 96,
        ..GenerateOptions::default()
    };
    let mut compared = 0;

    // The categorisation grammar, over the default category list.
    let choices = Choices::new(default_keys()).unwrap();
    for description in [
        "TRADER JOE'S #552",
        "SHELL OIL 57444",
        "NETFLIX.COM",
        "PAYROLL ACME",
    ] {
        let prompt = categorise_prompt(&model, &choices, description);
        let done = |text: &str| choices.parse(text).is_some();
        let fast = model.generate(&prompt, CHOICE_PREFIX, &choices.grammar(), &done, &options);
        let exact =
            model.generate_exact(&prompt, CHOICE_PREFIX, &choices.grammar(), &done, &options);
        match (fast, exact) {
            (Ok(f), Ok(e)) => {
                assert_eq!((&f.text, f.tokens), (&e.text, e.tokens), "{description}");
                assert!(f.confidence_bp <= e.confidence_bp, "{description}");
            }
            (Err(f), Err(e)) => assert_eq!(f.to_string(), e.to_string()),
            (f, e) => panic!("{description}: {f:?} / {e:?}"),
        }
        compared += 1;
    }

    // T4.2's query grammar (large: every op, measure and period) on a small ledger.
    let vocabulary = fm_llm::nl_query::Vocabulary {
        categories: vec![fm_llm::nl_query::Category {
            key: "food.groceries".into(),
            name: "Groceries".into(),
            group: "food".into(),
        }],
        groups: vec![fm_llm::nl_query::Group {
            key: "food".into(),
            name: "Food".into(),
        }],
        accounts: vec![],
        currencies: vec![],
        merchants: vec![],
    };
    let grammar = fm_llm::nl_query::grammar(fm_core::query::SCHEMA, &vocabulary)
        .unwrap()
        .render();
    let done = |text: &str| serde_json::from_str::<serde_json::Value>(text).is_ok();
    for question in ["Groceries in 2025?", "How much did I spend last month?"] {
        let prompt = model
            .chat_prompt("Write the query as JSON.", question)
            .unwrap();
        let options = GenerateOptions {
            max_tokens: 64,
            ..GenerateOptions::default()
        };
        let fast = model.generate(&prompt, "", &grammar, &done, &options);
        let exact = model.generate_exact(&prompt, "", &grammar, &done, &options);
        match (fast, exact) {
            (Ok(f), Ok(e)) => {
                assert_eq!((&f.text, f.tokens), (&e.text, e.tokens), "{question}");
                assert!(f.confidence_bp <= e.confidence_bp, "{question}");
            }
            (Err(f), Err(e)) => assert_eq!(f.to_string(), e.to_string(), "{question}"),
            (f, e) => panic!("{question}: {f:?} / {e:?}"),
        }
        compared += 1;
    }
    assert_eq!(compared, 6);
}
