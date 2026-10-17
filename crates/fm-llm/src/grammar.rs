//! The closed list a model picks from, and the grammar that enforces it.
//!
//! A choice is written as one JSON object, `{"category":"<key>"}`, where
//! `<key>` is one of the caller's keys or [`UNCATEGORISED`] (SPEC constraint 3).
//! The opening `{"category":"` is put into the prompt as the start of the
//! model's answer ([`CHOICE_PREFIX`]); the grammar covers the rest: one of the
//! keys, JSON-escaped, then `"}`. Nothing else can be generated.

use std::collections::HashSet;
use std::fmt;

/// The key a model picks when none of the categories fits.
pub const UNCATEGORISED: &str = "uncategorised";

/// Start of every choice. It is appended to the prompt, so the model only
/// generates the key and the closing `"}`.
pub const CHOICE_PREFIX: &str = r#"{"category":""#;

const CHOICE_SUFFIX: &str = r#""}"#;

/// Why a list of keys can't be offered to a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoicesError {
    /// An empty key.
    EmptyKey,
    /// The same key twice.
    Duplicate(String),
    /// A key equal to [`UNCATEGORISED`], which is always offered.
    Reserved,
}

impl fmt::Display for ChoicesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyKey => f.write_str("a category key is empty"),
            Self::Duplicate(key) => write!(f, "category key {key:?} is listed twice"),
            Self::Reserved => write!(f, "{UNCATEGORISED:?} is reserved and always offered"),
        }
    }
}

impl std::error::Error for ChoicesError {}

/// The keys a model may choose from, plus [`UNCATEGORISED`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choices {
    /// The caller's keys in order, then [`UNCATEGORISED`].
    keys: Vec<String>,
}

impl Choices {
    /// Keys are matched exactly, as given: any non-empty string, distinct, and
    /// not [`UNCATEGORISED`].
    pub fn new<I, S>(keys: I) -> Result<Self, ChoicesError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut seen = HashSet::new();
        let mut list = Vec::new();
        for key in keys {
            let key = key.into();
            if key.is_empty() {
                return Err(ChoicesError::EmptyKey);
            }
            if key == UNCATEGORISED {
                return Err(ChoicesError::Reserved);
            }
            if !seen.insert(key.clone()) {
                return Err(ChoicesError::Duplicate(key));
            }
            list.push(key);
        }
        list.push(UNCATEGORISED.to_string());
        Ok(Self { keys: list })
    }

    /// Every key the model may produce, [`UNCATEGORISED`] last.
    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    /// GBNF grammar (llama.cpp's grammar format) for what follows
    /// [`CHOICE_PREFIX`]: one key, JSON-escaped, then `"}`.
    pub fn grammar(&self) -> String {
        let mut out = String::from("root ::= choice ");
        out.push_str(&gbnf_literal(CHOICE_SUFFIX));
        out.push_str("\nchoice ::= ");
        for (i, key) in self.keys.iter().enumerate() {
            if i > 0 {
                out.push_str(" | ");
            }
            out.push_str(&gbnf_literal(&json_string_body(key)));
        }
        out.push('\n');
        out
    }

    /// The key a complete answer chose, or `None` if `answer` is not exactly
    /// a choice from this list. `answer` is what followed [`CHOICE_PREFIX`].
    pub fn parse(&self, answer: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|key| Self::answer(key) == answer)
            .map(String::as_str)
    }

    /// The text that follows [`CHOICE_PREFIX`] when the model picks `key`.
    pub fn answer(key: &str) -> String {
        format!("{}{CHOICE_SUFFIX}", json_string_body(key))
    }
}

/// `s` as it appears between the quotes of a JSON string.
fn json_string_body(s: &str) -> String {
    let quoted = serde_json::Value::String(s.to_string()).to_string();
    quoted[1..quoted.len() - 1].to_string()
}

/// `s` as a GBNF string literal. JSON-escaped text has no raw control
/// characters, but they are escaped anyway so the grammar is always valid.
fn gbnf_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncategorised_is_always_offered_last() {
        let choices = Choices::new(["food.groceries", "food.dining"]).unwrap();
        assert_eq!(
            choices.keys(),
            ["food.groceries", "food.dining", UNCATEGORISED]
        );
        assert_eq!(
            Choices::new(Vec::<String>::new()).unwrap().keys(),
            [UNCATEGORISED]
        );
    }

    #[test]
    fn bad_lists_are_refused() {
        assert_eq!(Choices::new([""]), Err(ChoicesError::EmptyKey));
        assert_eq!(
            Choices::new(["a", "b", "a"]),
            Err(ChoicesError::Duplicate("a".into()))
        );
        assert_eq!(Choices::new([UNCATEGORISED]), Err(ChoicesError::Reserved));
    }

    #[test]
    fn grammar_lists_every_key_escaped() {
        let choices = Choices::new(["a.b", r#"q"uote"#, r"back\slash", "new\nline"]).unwrap();
        assert_eq!(
            choices.grammar(),
            concat!(
                r#"root ::= choice "\"}""#,
                "\n",
                r#"choice ::= "a.b" | "q\\\"uote" | "back\\\\slash" | "new\\nline" | "uncategorised""#,
                "\n",
            )
        );
    }

    #[test]
    fn parse_accepts_exactly_the_listed_answers() {
        let choices = Choices::new(["food", "food.dining", r#"q"t"#]).unwrap();
        assert_eq!(choices.parse(r#"food"}"#), Some("food"));
        assert_eq!(choices.parse(r#"food.dining"}"#), Some("food.dining"));
        assert_eq!(choices.parse(r#"q\"t"}"#), Some(r#"q"t"#));
        assert_eq!(choices.parse(r#"uncategorised"}"#), Some(UNCATEGORISED));
        for bad in ["food", r#"food""#, r#"foo"}"#, r#"q"t"}"#, r#"food"} "#, ""] {
            assert_eq!(choices.parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_full_answer_is_the_json_object() {
        let choices = Choices::new([r"we\ird ✓ key"]).unwrap();
        let key = &choices.keys()[0];
        let answer = format!("{}{}", json_string_body(key), CHOICE_SUFFIX);
        let full: serde_json::Value =
            serde_json::from_str(&format!("{CHOICE_PREFIX}{answer}")).unwrap();
        assert_eq!(full["category"], serde_json::Value::String(key.clone()));
        assert_eq!(choices.parse(&answer), Some(key.as_str()));
    }

    #[test]
    fn control_characters_are_escaped_in_literals() {
        assert_eq!(gbnf_literal("a\u{7}b"), r#""a\u0007b""#);
        assert_eq!(json_string_body("a\u{7}b"), r"a\u0007b");
    }
}
