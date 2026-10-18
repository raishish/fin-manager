//! The categorisation fallback's model (T2.5): an [`fm_llm::Session`] behind fm-core's
//! [`Chooser`], so fm-core never depends on llama.cpp.
//!
//! The instructions (category list, conventions, the user's own corrections) go in the
//! system turn and are evaluated once per run, as the session's prefix; each transaction
//! is a user turn after it.

use fm_core::categorize::{Chooser, ChooserError};
use fm_llm::{Choices, GenerateOptions, LlmError, Model, Session, SessionOptions};

/// The longest transaction turn, in tokens: a description, amount, date and account kind
/// (about 70 tokens in TD.1).
const MAX_SUFFIX_TOKENS: u32 = 256;
/// The longest answer: a key and the closing of `{"category":"…"}`.
const MAX_ANSWER_TOKENS: u32 = 48;

/// Marks where the user's turn goes in the chat template, to split the prompt there.
const MARK: &str = "\u{1}";

pub struct SessionChooser<'m> {
    model: &'m Model,
    /// Put before the answer, e.g. an empty thinking block for a thinking model.
    answer_lead: &'static str,
    threads: Option<u32>,
    /// The session over the instructions, and what the chat template puts after the
    /// user's text (the start of the assistant's turn).
    session: Option<(Session<'m>, String)>,
    /// The closed list for the last `keys`, kept while they don't change.
    choices: Option<(Vec<String>, Choices)>,
}

impl<'m> SessionChooser<'m> {
    pub fn new(model: &'m Model, answer_lead: &'static str, threads: Option<u32>) -> Self {
        Self {
            model,
            answer_lead,
            threads,
            session: None,
            choices: None,
        }
    }
}

/// Errors carry llama.cpp's reason and never the prompt (no financial data).
fn model_error(e: LlmError) -> ChooserError {
    ChooserError(e.to_string())
}

/// llama.cpp takes C strings, so a NUL can't pass; and [`MARK`] must appear only
/// where this module puts it.
fn clean(text: &str) -> String {
    text.replace(['\0', '\u{1}'], " ")
}

impl Chooser for SessionChooser<'_> {
    fn model_id(&self) -> &str {
        self.model.id()
    }

    fn gguf_sha256(&self) -> [u8; 32] {
        self.model.gguf_sha256()
    }

    fn begin(&mut self, instructions: &str) -> Result<(), ChooserError> {
        let full = self
            .model
            .chat_prompt(&clean(instructions), MARK)
            .map_err(model_error)?;
        let (prefix, tail) = full
            .rsplit_once(MARK)
            .ok_or_else(|| ChooserError("the chat template dropped the user's turn".into()))?;
        let session = self
            .model
            .session(
                prefix,
                &SessionOptions {
                    max_suffix_tokens: MAX_SUFFIX_TOKENS,
                    max_answer_tokens: MAX_ANSWER_TOKENS,
                    threads: self.threads,
                },
            )
            .map_err(model_error)?;
        self.session = Some((session, tail.to_string()));
        Ok(())
    }

    fn choose(
        &mut self,
        transaction: &str,
        keys: &[String],
    ) -> Result<(String, u16), ChooserError> {
        let Some((session, tail)) = self.session.as_mut() else {
            return Err(ChooserError("choose before begin".into()));
        };
        let choices = match &self.choices {
            Some((k, choices)) if k == keys => choices,
            _ => {
                let choices = Choices::new(keys)
                    .map_err(|e| ChooserError(format!("the category list: {e}")))?;
                &self.choices.insert((keys.to_vec(), choices)).1
            }
        };
        let suffix = format!(
            "Categorise this transaction.\n{}{tail}{}",
            clean(transaction),
            self.answer_lead
        );
        let options = GenerateOptions {
            max_tokens: MAX_ANSWER_TOKENS,
            threads: self.threads,
            ..GenerateOptions::default()
        };
        let choice = session
            .choose(&suffix, choices, &options)
            .map_err(model_error)?;
        Ok((choice.key, choice.confidence_bp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_removes_what_llama_cpp_and_the_split_cant_take() {
        assert_eq!(clean("a\0b\u{1}c"), "a b c");
    }

    /// With the CI test model (`FM_LLM_TEST_MODEL`, as in fm-llm's tests): answers come
    /// only from the list, and the same transaction gets the same answer every time.
    #[test]
    fn the_model_answers_from_the_list_and_repeats_itself() {
        let Some(path) = std::env::var_os("FM_LLM_TEST_MODEL") else {
            eprintln!("skipping: set FM_LLM_TEST_MODEL to the CI test model's path");
            return;
        };
        let gpu = std::env::var_os("FM_LLM_TEST_CPU").is_none();
        let model = Model::load_with(
            std::path::Path::new(&path),
            &fm_llm::CI_TEST_MODEL,
            &fm_llm::LoadOptions { gpu },
        )
        .unwrap();
        let mut chooser = SessionChooser::new(&model, "", None);
        let keys: Vec<String> = ["food.groceries", "transport.fuel", "income"]
            .map(String::from)
            .to_vec();
        assert!(chooser.choose("Description: x\n", &keys).is_err());
        chooser
            .begin("Answer with one key: food.groceries, transport.fuel, income.\0")
            .unwrap();
        let t = "Description: SHELL OIL 57444\nAmount: -40.00 USD (money out)\n";
        let (key, confidence) = chooser.choose(t, &keys).unwrap();
        assert!(keys.contains(&key) || key == fm_llm::UNCATEGORISED, "{key}");
        assert!(confidence <= 10_000);
        assert_eq!(chooser.choose(t, &keys).unwrap(), (key, confidence));
        assert_eq!(chooser.model_id(), fm_llm::CI_TEST_MODEL.id);
    }
}
