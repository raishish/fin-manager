//! Removes financial and personal content from free text before it goes into a crash
//! report.
//!
//! Panic messages are free text, so no scrubber can promise to catch everything. This one
//! is deliberately heavy-handed and works in layers:
//! 1. Known sensitive terms (merchant names, payees, account names) supplied by the app
//!    while the ledger is unlocked, matched case-insensitively.
//! 2. Quoted text (`"..."`, `'...'`): `Debug` output of strings, where data usually shows up.
//! 3. URLs, email addresses and file paths.
//! 4. Every number, with any currency sign or sign in front of it, so no amount, date,
//!    account number or index survives.
//! 5. Long mixed letter-and-digit tokens (ids, tokens) and ALL-CAPS words of two or more
//!    letters, the usual shape of bank merchant strings ("SQ *BLUE BOTTLE"), except a short
//!    list of code and format names.
//!
//! The user still sees the result before anything leaves the machine.

/// Replaces a known term, quoted text or an ALL-CAPS word.
pub const REDACTED: &str = "<redacted>";
/// Replaces a number or amount.
pub const NUMBER: &str = "<n>";
/// Replaces a URL.
pub const URL: &str = "<url>";
/// Replaces an email address.
pub const EMAIL: &str = "<email>";
/// Replaces a file path.
pub const PATH: &str = "<path>";
/// Replaces a long token that mixes letters and digits.
pub const ID: &str = "<id>";

/// Known terms shorter than this are ignored: they would match inside ordinary words.
pub const MIN_TERM_CHARS: usize = 3;

/// Mixed letter-and-digit tokens at least this long are treated as ids.
const MIN_ID_CHARS: usize = 12;

/// ALL-CAPS words that are code or format names, not merchant text.
const CODE_WORDS: &[&str] = &[
    "API", "ASCII", "CSV", "EOF", "FFI", "GET", "GGUF", "HEAD", "HTTP", "HTTPS", "ID", "IO", "IPC",
    "JSON", "OFX", "OK", "OS", "POST", "QFX", "SGML", "SQL", "TLS", "URL", "UTF", "UUID", "XML",
];

const CURRENCY_SIGNS: &[char] = &['$', '€', '£', '¥', '₹', '₩', '₽', '¢'];

/// Scrubs text for crash reports. See the module docs for what it removes.
#[derive(Debug, Clone, Default)]
pub struct Scrubber {
    /// Lower-cased (ASCII) known terms, longest first so longer names win.
    terms: Vec<String>,
}

impl Scrubber {
    /// A scrubber with no known terms; the pattern layers still apply.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the known sensitive terms.
    pub fn set_terms<I, S>(&mut self, terms: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut terms: Vec<String> = terms
            .into_iter()
            .map(|t| t.as_ref().trim().to_ascii_lowercase())
            .filter(|t| t.chars().count() >= MIN_TERM_CHARS)
            .collect();
        terms.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        terms.dedup();
        self.terms = terms;
    }

    /// Forgets the known terms (call on lock).
    pub fn clear_terms(&mut self) {
        self.terms.clear();
    }

    pub fn scrub(&self, text: &str) -> String {
        let text = self.redact_terms(text);
        let text = redact_quotes(&text);
        scrub_words(&text)
    }

    /// Only the known-terms layer, for text that is code (function names).
    pub(crate) fn scrub_terms_only(&self, text: &str) -> String {
        self.redact_terms(text)
    }

    fn redact_terms(&self, text: &str) -> String {
        let mut out = text.to_string();
        // Lower-cased once, then edited in step with `out`: ASCII lower-casing keeps byte
        // offsets, so a match in `lower` is the same range in `out`.
        let mut lower = out.to_ascii_lowercase();
        let redacted_lower = REDACTED.to_ascii_lowercase();
        for term in &self.terms {
            let mut start = 0;
            while let Some(found) = lower[start..].find(term.as_str()) {
                let at = start + found;
                out.replace_range(at..at + term.len(), REDACTED);
                lower.replace_range(at..at + term.len(), &redacted_lower);
                start = at + REDACTED.len();
            }
        }
        out
    }
}

/// Replaces `"..."` and `'...'` spans with a quoted [`REDACTED`]. An unclosed `"` redacts
/// to the end. A `'` only opens a quote when it doesn't follow a letter or digit, so
/// apostrophes ("doesn't") are left alone.
fn redact_quotes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let opens_single = c == '\'' && (i == 0 || !chars[i - 1].is_alphanumeric());
        if c == '"' || opens_single {
            let close = find_close(&chars, i + 1, c);
            match close {
                Some(end) => {
                    out.push(c);
                    out.push_str(REDACTED);
                    out.push(c);
                    i = end + 1;
                }
                None if c == '"' => {
                    out.push(c);
                    out.push_str(REDACTED);
                    i = chars.len();
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Index of the quote closing one opened just before `from`, skipping `\`-escaped quotes.
/// A closing `'` must not be followed by a letter or digit.
fn find_close(chars: &[char], from: usize, quote: char) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            c if c == quote => {
                let next_ok = quote == '"' || chars.get(i + 1).is_none_or(|n| !n.is_alphanumeric());
                if next_ok {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Applies the URL, email, path, number, id and ALL-CAPS rules to each
/// whitespace-separated word, keeping the whitespace as it was.
fn scrub_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    for c in text.chars() {
        if c.is_whitespace() {
            out.push_str(&scrub_word(&word));
            word.clear();
            out.push(c);
        } else {
            word.push(c);
        }
    }
    out.push_str(&scrub_word(&word));
    out
}

fn scrub_word(word: &str) -> String {
    if word.is_empty() {
        return String::new();
    }
    // Keep punctuation around the word (`(`, `,`, `.`, `:`...) so the sentence still reads.
    let core_start = word
        .find(|c: char| !is_wrapping_punct(c))
        .unwrap_or(word.len());
    let core_end = word
        .rfind(|c: char| !is_wrapping_punct(c))
        .map_or(core_start, |i| {
            i + word[i..].chars().next().map_or(1, char::len_utf8)
        });
    let (lead, core, trail) = (
        &word[..core_start],
        &word[core_start..core_end],
        &word[core_end..],
    );
    let scrubbed = if core.contains("://") {
        URL.to_string()
    } else if is_email(core) {
        EMAIL.to_string()
    } else if is_path(core) {
        PATH.to_string()
    } else {
        scrub_tokens(core)
    };
    format!("{lead}{scrubbed}{trail}")
}

fn is_wrapping_punct(c: char) -> bool {
    matches!(
        c,
        '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':' | '.' | '!' | '?' | '`'
    )
}

fn is_email(s: &str) -> bool {
    s.split_once('@')
        .is_some_and(|(user, domain)| !user.is_empty() && domain.contains('.'))
}

fn is_path(s: &str) -> bool {
    let bytes = s.as_bytes();
    let windows_drive = bytes.len() > 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    s.starts_with('/')
        || s.starts_with("~/")
        || s.starts_with("./")
        || s.starts_with("../")
        || s.starts_with("\\\\")
        || windows_drive
        || s.contains("/Users/")
        || s.contains("/home/")
        || (s.contains('/') && s.contains('.') && !s.contains("::"))
}

/// Splits a word into runs of letters/digits/underscores and everything else, then
/// scrubs ids, numbers and ALL-CAPS runs.
fn scrub_tokens(word: &str) -> String {
    let chars: Vec<char> = word.chars().collect();
    let mut out = String::with_capacity(word.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_number_start(&chars, i) {
            // A currency sign or +/- directly before a digit, then digits with , . separators.
            let mut j = i + 1;
            while j < chars.len()
                && (chars[j].is_ascii_digit()
                    || (matches!(chars[j], ',' | '.')
                        && chars.get(j + 1).is_some_and(char::is_ascii_digit)))
            {
                j += 1;
            }
            out.push_str(NUMBER);
            i = j;
        } else if c.is_alphanumeric() || c == '_' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let token: String = chars[i..j].iter().collect();
            out.push_str(&scrub_token(&token));
            i = j;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn is_number_start(chars: &[char], i: usize) -> bool {
    let c = chars[i];
    let next_is_digit = chars.get(i + 1).is_some_and(char::is_ascii_digit);
    c.is_ascii_digit() || (next_is_digit && (CURRENCY_SIGNS.contains(&c) || c == '-' || c == '+'))
}

/// One run of letters, digits and underscores that doesn't start with a digit.
fn scrub_token(token: &str) -> String {
    let has_digit = token.chars().any(|c| c.is_ascii_digit());
    let has_alpha = token.chars().any(char::is_alphabetic);
    if has_digit && has_alpha && token.chars().count() >= MIN_ID_CHARS {
        return ID.to_string();
    }
    if is_all_caps_word(token) {
        return REDACTED.to_string();
    }
    if has_digit {
        // `abc123`: keep the letters, drop the digits.
        let mut out = String::new();
        let mut in_number = false;
        for c in token.chars() {
            if c.is_ascii_digit() {
                if !in_number {
                    out.push_str(NUMBER);
                }
                in_number = true;
            } else {
                out.push(c);
                in_number = false;
            }
        }
        return out;
    }
    token.to_string()
}

fn is_all_caps_word(token: &str) -> bool {
    let letters = token.chars().filter(|c| c.is_alphabetic()).count();
    letters >= 2
        && token
            .chars()
            .all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_')
        && !token.contains('_')
        && !CODE_WORDS.contains(&token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrub(text: &str) -> String {
        Scrubber::new().scrub(text)
    }

    #[test]
    fn amounts_and_numbers_are_removed() {
        assert_eq!(scrub("paid $1,234.56 today"), "paid <n> today");
        // Currency codes go too: the ALL-CAPS rule can't tell them from a merchant.
        assert_eq!(scrub("balance -42.10 USD"), "balance <n> <redacted>");
        assert_eq!(scrub("€9,99 and £3"), "<n> and <n>");
        assert_eq!(scrub("account 000123456789"), "account <n>");
        assert_eq!(
            scrub("index out of bounds: the len is 3 but the index is 5"),
            "index out of bounds: the len is <n> but the index is <n>"
        );
        assert_eq!(scrub("on 2026-10-16"), "on <n><n><n>");
        let out = scrub("12.50 -3 +7 1e5 0x1F");
        assert!(!out.chars().any(|c| c.is_ascii_digit()), "{out}");
    }

    #[test]
    fn quoted_text_is_removed() {
        assert_eq!(
            scrub(r#"bad merchant "Joe's Coffee" in row"#),
            r#"bad merchant "<redacted>" in row"#
        );
        assert_eq!(scrub(r#"got "a \"b\" c" here"#), r#"got "<redacted>" here"#);
        assert_eq!(scrub(r#"unclosed "Blue Bottle"#), r#"unclosed "<redacted>"#);
        assert_eq!(scrub("char 'x' found"), "char '<redacted>' found");
        assert_eq!(scrub("it doesn't parse"), "it doesn't parse");
    }

    #[test]
    fn urls_emails_and_paths_are_removed() {
        assert_eq!(
            scrub("GET https://bank.example/acct?id=9 failed"),
            "GET <url> failed"
        );
        assert_eq!(scrub("from jane.doe@example.com."), "from <email>.");
        assert_eq!(
            scrub("open /Users/jane/Documents/chase.csv: denied"),
            "open <path>: denied"
        );
        assert_eq!(scrub(r"C:\Users\jane\x.ofx"), "<path>");
        assert_eq!(scrub("read ~/bank/2026.qfx"), "read <path>");
    }

    #[test]
    fn caps_words_and_ids_are_removed() {
        assert_eq!(
            scrub("unknown payee SQ *BLUE BOTTLE COFFEE"),
            "unknown payee <redacted> *<redacted> <redacted> <redacted>"
        );
        assert_eq!(scrub("token abcd1234efgh5678"), "token <id>");
        assert_eq!(scrub("invalid JSON in CSV row"), "invalid JSON in CSV row");
    }

    #[test]
    fn rust_panic_messages_stay_readable() {
        assert_eq!(
            scrub("called `Option::unwrap()` on a `None` value"),
            "called `Option::unwrap()` on a `None` value"
        );
        assert_eq!(
            scrub("called `Result::unwrap()` on an `Err` value: ParseError { kind: Empty }"),
            "called `Result::unwrap()` on an `Err` value: ParseError { kind: Empty }"
        );
    }

    #[test]
    fn known_terms_are_removed_case_insensitively() {
        let mut s = Scrubber::new();
        s.set_terms(["Starbucks", "blue bottle", "ab", "  Trader Joe  "]);
        assert_eq!(
            s.scrub("no rule for starbucks or Blue Bottle at trader joe"),
            "no rule for <redacted> or <redacted> at <redacted>"
        );
        // Terms under MIN_TERM_CHARS are ignored.
        assert_eq!(s.scrub("tab"), "tab");
        s.clear_terms();
        assert_eq!(s.scrub("starbucks"), "starbucks");
    }

    #[test]
    fn longer_terms_win() {
        let mut s = Scrubber::new();
        s.set_terms(["Joe", "Trader Joe's"]);
        assert_eq!(s.scrub("at trader joe's"), "at <redacted>");
    }

    #[test]
    fn non_ascii_text_is_handled() {
        let mut s = Scrubber::new();
        s.set_terms(["Café Olé"]);
        assert_eq!(s.scrub("paid café olé ¥500"), "paid <redacted> <n>");
        assert_eq!(scrub("“curly” — ok"), "“curly” — ok");
    }
}
