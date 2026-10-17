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
    CHOICE_PREFIX, CI_TEST_MODEL, Choices, GenerateOptions, LlmError, MAX_BASIS_POINTS, Model,
    Sampling, UNCATEGORISED,
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
fn model() -> Option<Model> {
    let path = model_path()?;
    Some(Model::load(&path, &CI_TEST_MODEL).expect("load the test model"))
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
