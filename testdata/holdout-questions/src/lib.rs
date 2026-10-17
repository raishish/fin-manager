//! A held-out question set for T4.2 (TD.7).
//!
//! 25 questions about the same household as TD.3 (TD.1's freelancer), written apart from
//! T4.2's work so they measure how question answering does on phrasings it was never
//! tuned on. Same file format, conventions and checks as TD.3: every answer is computed
//! from TD.1's committed ledger and ground truth with TD.3's helpers, never typed by
//! hand, and every question has the structured query a perfect interpreter would write.
//! `README.md` documents the set.

mod queries;
mod questions;

use fm_core::model::Sha256Hash;
use fm_questions::{Book, QuestionSet, facts};

/// Bumped whenever a change alters the answers.
pub const VERSION: u32 = 1;

/// Builds the whole set from TD.1's committed data.
pub fn build() -> QuestionSet {
    let dir = facts::synthetic_data_dir();
    let (ledger, truth) = facts::load(&dir);
    let book = Book {
        facts: facts::facts(&ledger, &truth),
        truth,
    };
    let manifest = std::fs::read(dir.join("manifest.json")).expect("TD.1 manifest is readable");
    let mut questions = questions::all(&book);
    for (i, q) in questions.iter_mut().enumerate() {
        q.id = format!("h{:02}", i + 1);
        match queries::expected(&q.id) {
            queries::Expected::Query(query) => q.expected_query = Some(query),
            queries::Expected::Proxy(query, why) => {
                q.expected_query = Some(query);
                q.proxy = Some(why.to_string());
            }
        }
    }
    QuestionSet {
        version: VERSION,
        persona: facts::PERSONA.to_string(),
        today: facts::today(),
        currency: facts::CURRENCY.to_string(),
        // TD.3's counting rules, word for word: the held-out set is answered the same way.
        conventions: fm_questions::build().conventions,
        synthetic_manifest_sha256: Sha256Hash::of(&manifest),
        questions,
    }
}

/// `questions.json` bytes.
pub fn render() -> Vec<u8> {
    fm_synth::json::to_lines(&build())
}
