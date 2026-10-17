//! Text normalisation for rule matching.
//!
//! Bank descriptions differ in case, punctuation and spacing for the same merchant
//! (`TRADER JOE'S`, `TRADER JOE S #243`, `Trader Joe's`). Both a rule's pattern and the
//! description are reduced to the same form before comparing: uppercase, every run of
//! characters that aren't letters or digits replaced by one space, trimmed. Matches
//! then line up on word boundaries, so a pattern never matches inside a longer word.

use crate::model::TextMatch;

/// Uppercase words separated by single spaces: `"Trader Joe's #243"` →
/// `"TRADER JOE S 243"`.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut gap = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.extend(c.to_uppercase());
        } else {
            gap = true;
        }
    }
    out
}

/// Whether `text` matches, comparing normalised forms on word boundaries.
///
/// - `Equals`: the same words.
/// - `StartsWith`: the text's first words are the pattern's words.
/// - `Contains`: the pattern's words appear together somewhere in the text.
///
/// A pattern that normalises to nothing (only punctuation) matches nothing.
pub fn text_matches(matcher: &TextMatch, text: &str) -> bool {
    normalized_matches(matcher, &normalize(matcher.pattern()), &normalize(text))
}

/// [`text_matches`] on text and pattern already normalised: `pattern` is
/// `normalize(matcher.pattern())`. For matching many rules against many descriptions.
pub fn normalized_matches(matcher: &TextMatch, pattern: &str, text: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    match matcher {
        TextMatch::Equals(_) => text == pattern,
        TextMatch::StartsWith(_) => text
            .strip_prefix(pattern)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(' ')),
        TextMatch::Contains(_) => text.match_indices(pattern).any(|(i, _)| {
            let before = i == 0 || text.as_bytes()[i - 1] == b' ';
            let end = i + pattern.len();
            let after = end == text.len() || text.as_bytes()[end] == b' ';
            before && after
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_case_punctuation_and_spacing() {
        assert_eq!(normalize("Trader Joe's #243"), "TRADER JOE S 243");
        assert_eq!(
            normalize("  SQ *BLUE  BOTTLE--COFF  "),
            "SQ BLUE BOTTLE COFF"
        );
        assert_eq!(normalize("APPLE.COM/BILL"), "APPLE COM BILL");
        assert_eq!(normalize("Café Ñandú"), "CAFÉ ÑANDÚ");
        assert_eq!(normalize("straße"), "STRASSE");
        assert_eq!(normalize("*** ---"), "");
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn matches_on_word_boundaries() {
        let contains = |p: &str| TextMatch::Contains(p.into());
        let starts = |p: &str| TextMatch::StartsWith(p.into());
        let equals = |p: &str| TextMatch::Equals(p.into());

        assert!(text_matches(
            &contains("trader joe's"),
            "TRADER JOE S #243 SAN FRANCISCO CA"
        ));
        assert!(text_matches(
            &contains("BLUE BOTTLE"),
            "SQ *BLUE BOTTLE COFF BERKELEY CA"
        ));
        assert!(!text_matches(&contains("TARGET"), "STARGET SUPPLY"));
        assert!(!text_matches(&contains("UBER"), "UBEREATS"));
        assert!(text_matches(&contains("UBER"), "UBER *EATS"));
        assert!(!text_matches(
            &contains("BLUE BOTTLE COFFEE"),
            "SQ *BLUE BOTTLE COFF"
        ));

        assert!(text_matches(&starts("netflix"), "NETFLIX.COM 901"));
        assert!(!text_matches(&starts("NET"), "NETFLIX.COM"));
        assert!(!text_matches(&starts("COM"), "NETFLIX.COM"));

        assert!(text_matches(&equals("apple.com/bill"), "APPLE.COM/BILL"));
        assert!(!text_matches(&equals("APPLE"), "APPLE.COM/BILL"));

        // Punctuation-only patterns can't match everything.
        assert!(!text_matches(&contains("*"), "ANYTHING"));
        assert!(!text_matches(&equals("--"), ""));
    }
}
