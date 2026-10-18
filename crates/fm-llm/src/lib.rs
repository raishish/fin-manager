//! Embedded llama.cpp runtime, model download and verification, constrained output.
//!
//! - **Download** ([`ensure_model`], [`download`]): a model file comes from a URL
//!   pinned to a fixed revision, through fm-netguard (purpose
//!   [`DOWNLOAD_PURPOSE`]), and is kept only if its size and SHA-256 match its
//!   [`ModelSpec`]. [`Model::load`] checks them again before loading.
//! - **Constrained output** ([`Choices`], [`Model::choose`]): the model writes
//!   `{"category":"<key>"}` with `<key>` from the caller's closed list or
//!   [`UNCATEGORISED`] (SPEC constraint 3). A llama.cpp grammar masks every
//!   other token at each step, so nothing else can come out.
//!   [`Model::generate`] runs any GBNF grammar the same way.
//! - **Confidence**: the probability the model gave its own answer, within
//!   what was allowed. At each step the disallowed tokens are masked, the
//!   remaining logits are renormalised with a softmax (temperature 1), and the
//!   chosen token's probability is taken. The confidence is the product over
//!   the generated tokens, in basis points (0 to 10 000). It is computed from
//!   the output, not asked of the model. The fixed `{"category":"` prefix is
//!   part of the prompt, so it doesn't count.
//!
//!   In [`Model::choose`] the mask is the grammar *and* each key's canonical
//!   tokenisation (the tokens the model's tokenizer uses for that answer), so
//!   the confidence is the probability of that key among the listed keys:
//!   they sum to 1, and a key that is the only option scores 10 000. Without
//!   the second mask, other spellings of the same text in tokens take some
//!   probability (about 6% for a forced `uncategorised` on the test model).
//!   [`Model::generate`] masks with the grammar alone, so its figure can be
//!   below the text's total probability in that way.
//! - **Hardware**: Metal on Apple Silicon (all layers offloaded), CPU elsewhere.
//!   Drop every [`Model`] before the process exits: ggml's Metal backend
//!   aborts at exit if a model is still loaded, so never keep one in a
//!   `static`.
//!
//! - **Explanations** ([`explain`]): a model's explanation of an answer is kept only if
//!   every number in it is one of the computed values the caller passed in (SPEC
//!   constraint 6); otherwise a template is used.
//!
//! The answer is greedy by default, so the same model, prompt and list always
//! give the same key and confidence.
//!
//! This crate has no dependency on fm-core: it returns plain values (the key,
//! confidence in basis points, the model id and the GGUF SHA-256), and the
//! categorisation code maps them to its decision types.

mod download;
pub mod explain;
mod grammar;
pub mod nl_query;
mod runtime;

pub use download::{
    CI_TEST_MODEL, DOWNLOAD_ORIGINS, DOWNLOAD_PURPOSE, DownloadError, ModelSpec, download,
    download_with, ensure_model, ensure_model_with, hex, sha256_file, sha256_hex, verify_file,
};
pub use grammar::{CHOICE_PREFIX, Choices, ChoicesError, UNCATEGORISED};
pub use runtime::{
    Choice, GenerateOptions, Generation, LlmError, MAX_BASIS_POINTS, Model, Sampling,
    gpu_offload_supported,
};

/// Crate name, used by the workspace smoke test.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_set() {
        assert_eq!(super::NAME, "fm-llm");
    }
}
