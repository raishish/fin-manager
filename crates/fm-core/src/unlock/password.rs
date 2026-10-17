//! Master password rules (SPEC §7: "the app enforces a minimum password strength").
//!
//! A new password needs at least [`MIN_CHARS`] characters and zxcvbn's top score,
//! [`MIN_SCORE`] = 4 (about 10^10 guesses or more). The ledger can be attacked offline
//! (a stolen disk), where zxcvbn rates score 3 as only "moderate protection"; a short
//! passphrase such as "purple monkey dishwasher" already reaches 4. Every password is NFC-normalised before it reaches
//! storage, so the same text typed with composed or decomposed accents (`é` or
//! `e` + `◌́`, which differ between keyboards and systems) unlocks the same ledger.

use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

pub const MIN_CHARS: usize = 12;
pub const MIN_SCORE: u8 = 4;

/// Words that make a password guessable for this app in particular.
const APP_WORDS: &[&str] = &[
    "fin-manager",
    "finmanager",
    "finance",
    "finances",
    "money",
    "budget",
    "ledger",
    "bank",
    "password",
];

/// How strong a candidate password is, for the strength meter and the create /
/// change checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Strength {
    /// zxcvbn's score, 0 (very guessable) to 4.
    pub score: u8,
    /// Characters after normalisation.
    pub chars: usize,
    /// True if the password may be used.
    pub acceptable: bool,
    /// What is wrong, if anything; shown under the field.
    pub warning: Option<String>,
    /// How to make it stronger.
    pub suggestions: Vec<String>,
}

/// NFC form of `password`, wiped when dropped.
pub fn normalize(password: &str) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::with_capacity(password.len()));
    out.extend(password.nfc());
    out
}

/// Rates a candidate master password.
pub fn strength(password: &str) -> Strength {
    let normalized = normalize(password);
    let chars = normalized.chars().count();
    let entropy = zxcvbn::zxcvbn(&normalized, APP_WORDS);
    let score = u8::from(entropy.score());
    let (mut warning, suggestions) = match entropy.feedback() {
        Some(feedback) => (
            feedback.warning().map(|w| w.to_string()),
            feedback
                .suggestions()
                .iter()
                .map(ToString::to_string)
                .collect(),
        ),
        None => (None, Vec::new()),
    };
    if chars < MIN_CHARS {
        warning = Some(format!("Use at least {MIN_CHARS} characters."));
    } else if score < MIN_SCORE && warning.is_none() {
        warning = Some("This password is too easy to guess.".to_string());
    }
    Strength {
        score,
        chars,
        acceptable: chars >= MIN_CHARS && score >= MIN_SCORE,
        warning,
        suggestions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_passwords_are_refused_whatever_their_score() {
        let s = strength("Zq8#vT!2pL");
        assert_eq!(s.chars, 10);
        assert!(!s.acceptable);
        assert_eq!(s.warning.as_deref(), Some("Use at least 12 characters."));
    }

    #[test]
    fn long_but_guessable_passwords_are_refused() {
        for weak in [
            "passwordpassword",
            "123456789012",
            "qwertyuiopasdf",
            "aaaaaaaaaaaaaaaa",
            "myfinancepassword",
            "budgetbudget2024",
            "Summer2024!!!",
            "ledgerbank2024",
        ] {
            let s = strength(weak);
            assert!(!s.acceptable, "{weak} scored {}", s.score);
            assert!(s.warning.is_some(), "{weak}");
        }
    }

    #[test]
    fn strong_passwords_are_accepted() {
        for strong in [
            "correct horse battery staple",
            "Tr0ub4dor&3-plus-more-words",
            "velvet anchor mosaic tundra",
            "purple monkey dishwasher",
            "blue-castle-river-42",
        ] {
            let s = strength(strong);
            assert!(s.acceptable, "{strong} scored {}", s.score);
            assert!(s.score >= MIN_SCORE);
        }
    }

    #[test]
    fn composed_and_decomposed_text_normalise_alike() {
        let composed = "caf\u{e9} cr\u{e8}me br\u{fb}l\u{e9}e";
        let decomposed = "cafe\u{301} cre\u{300}me bru\u{302}le\u{301}e";
        assert_ne!(composed, decomposed);
        assert_eq!(*normalize(composed), *normalize(decomposed));
        assert_eq!(strength(composed), strength(decomposed));
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        // 12 characters, 24 bytes.
        assert_eq!(strength("ééééééééééé1").chars, 12);
    }
}
