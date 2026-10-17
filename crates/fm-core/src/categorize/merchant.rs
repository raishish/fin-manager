//! The built-in merchant clean-up ([`BuiltinLogic::MerchantCleaner`]).
//!
//! Turns a raw bank description into a readable merchant name, from the text alone:
//! `SQ *SAKURA RAMEN     SAN FRANCISCO       CA` → `Sakura Ramen`,
//! `AMZN Mktp US*B12OG345P` → `Amazon`. The raw description is never changed; the name is
//! recorded as an `Assignment::Merchant` decision.
//!
//! Steps:
//! 1. **Fields.** A trailing state field (`…       CA`) is dropped, and the text is cut at
//!    the first run of two or more spaces: card networks pad the merchant name field.
//!    When the name fills its field, the city follows after one space and can't be told
//!    apart without guessing the field's width, so it stays (`Blue Bottle Coff San
//!    Francisco`). A merchant rule from one correction fixes the name for all of them.
//! 2. **Words.** The rest is normalised (see [`super::normalize`]) and processed word by
//!    word, so punctuation never matters.
//! 3. **Prefixes.** Payment processors and bank scaffolding in front of the merchant are
//!    removed: `SQ`, `TST`, `DD`, `IC`, `PAYPAL`, `CHECKCARD 0411`, `POS PURCHASE`,
//!    `ACH DEBIT`, `ZELLE PAYMENT TO`…
//! 4. **Tail.** Everything from the first store number (`#243`, `0004524`, `F10081`), date
//!    (`01/17`), ACH field (`DES:`, `PPD ID:`, `WEB ID:`, `CONF#`) or reference onward is
//!    removed, then a trailing two-letter state.
//! 5. **Name.** The words left are the **core** (`SAKURA RAMEN`). A well-known
//!    abbreviation in `assets/merchants/aliases.json` gives the name (`AMZN MKTP` →
//!    `Amazon`); otherwise the core is title-cased.
//!
//! Some descriptions name no merchant: paper checks (`Check 1085`), P2P payments that hide
//! the person (`VENMO PAYMENT 6479724873`). They give `None`, and no decision is written.
//!
//! [`BuiltinLogic::MerchantCleaner`]: crate::model::BuiltinLogic::MerchantCleaner

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use super::normalize::normalize;

/// Version of the clean-up, recorded on every decision it makes. Bump it, and the
/// version in `assets/merchants/aliases.json`, whenever the output changes.
pub const VERSION: u32 = 1;

/// What the clean-up found in a description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleaned {
    /// The merchant's words, normalised: `"SAKURA RAMEN"`. Learned rules match on it.
    pub core: String,
    /// The name to show: `"Sakura Ramen"`, or the alias, `"Amazon"`.
    pub name: String,
    /// Whether the name came from the alias table.
    pub aliased: bool,
}

/// Cleans a raw description. `None` when it names no merchant.
pub fn clean(description: &str) -> Option<Cleaned> {
    let core = core(description)?;
    match alias(&core) {
        Some(name) => Some(Cleaned {
            core,
            name: name.to_string(),
            aliased: true,
        }),
        None => Some(Cleaned {
            name: title_case(&core),
            core,
            aliased: false,
        }),
    }
}

/// The same clean-up without the alias table, for measuring what the table adds.
pub fn clean_without_aliases(description: &str) -> Option<Cleaned> {
    let core = core(description)?;
    Some(Cleaned {
        name: title_case(&core),
        core,
        aliased: false,
    })
}

/// Words in front of the merchant that payment processors and banks add. Matched as
/// whole normalised words at the start; the longest wins. `#` stands for one word of
/// digits (a card's last four, a date).
const PREFIXES: &[&str] = &[
    "SQ",
    "TST",
    "DD",
    "IC",
    "PAYPAL",
    "PP",
    "SP",
    "CHECKCARD #",
    "POS PURCHASE",
    "POS DEBIT",
    "POS",
    "DEBIT CARD PURCHASE",
    "PURCHASE AUTHORIZED ON # #",
    "RECURRING PAYMENT AUTHORIZED ON # #",
    "ACH DEBIT",
    "ACH CREDIT",
    "ACH",
    "ZELLE PAYMENT TO",
    "ZELLE PAYMENT FROM",
    "ZELLE TO",
    "ZELLE FROM",
    "ZELLE DEBIT",
    "ZELLE CREDIT",
    "VENMO PAYMENT",
    "VENMO CASHOUT",
];

/// Words that start the reference part of an ACH or P2P description. The merchant's
/// name ends before them.
const STOP_WORDS: &[&str] = &["DES", "INDN", "PPD", "CCD", "WEB", "CONF", "REF", "ON"];

/// Descriptions that name no merchant, as normalised whole-word prefixes.
const NO_MERCHANT: &[&str] = &["CHECK", "CHECK #", "DEPOSIT", "ATM DEPOSIT"];

/// US state and territory codes, for the trailing state.
const STATES: &[&str] = &[
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "DC", "FL", "GA", "HI", "ID", "IL", "IN", "IA",
    "KS", "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ", "NM",
    "NY", "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA",
    "WV", "WI", "WY", "PR",
];

fn core(description: &str) -> Option<String> {
    let name_field = name_field(description);
    let words: Vec<String> = normalize(name_field)
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    if words.is_empty() || (words.len() <= 2 && starts_with_pattern(&words, NO_MERCHANT).is_some())
    {
        return None;
    }

    let mut start = 0;
    while let Some(n) = starts_with_pattern(&words[start..], PREFIXES) {
        start += n;
    }
    let mut end = words.len();
    for (i, word) in words.iter().enumerate().skip(start) {
        // The first word is the merchant's even when it has digits (7-ELEVEN, 1-800-…).
        if i > start && (is_reference(word) || STOP_WORDS.contains(&word.as_str())) {
            end = i;
            break;
        }
    }
    let mut kept = &words[start..end];
    if kept.len() > 1 && STATES.contains(&kept[kept.len() - 1].as_str()) {
        kept = &kept[..kept.len() - 1];
    }
    // Only numbers and references left: no name (`VENMO PAYMENT 6479724873`).
    if kept.iter().all(|w| is_reference(w)) {
        return None;
    }
    Some(kept.join(" "))
}

/// The merchant name field: without a trailing padded state, and up to the first run of
/// two or more spaces.
fn name_field(description: &str) -> &str {
    let trimmed = description.trim();
    let without_state = match trimmed.rsplit_once("  ") {
        Some((before, last)) if STATES.contains(&last.trim()) => before.trim_end(),
        _ => trimmed,
    };
    match without_state.find("  ") {
        Some(i) if i > 0 => &without_state[..i],
        _ => without_state,
    }
}

/// A store number, id, date or reference: any word with a digit.
fn is_reference(word: &str) -> bool {
    word.chars().any(|c| c.is_ascii_digit())
}

/// How many words a pattern from `patterns` covers at the start of `words`: the
/// longest match, or `None`.
fn starts_with_pattern(words: &[String], patterns: &[&str]) -> Option<usize> {
    patterns
        .iter()
        .filter_map(|p| {
            let len = p.split(' ').count();
            let matches = len <= words.len()
                && p.split(' ').zip(words).all(|(part, word)| {
                    if part == "#" {
                        word.chars().all(|c| c.is_ascii_digit())
                    } else {
                        part == word
                    }
                });
            matches.then_some(len)
        })
        .max()
}

#[derive(Deserialize)]
struct AliasFile {
    version: u32,
    aliases: Vec<Alias>,
}

#[derive(Deserialize)]
struct Alias {
    prefix: String,
    merchant: String,
}

/// The alias table, parsed once (each prefix normalised), and positions by first word.
type AliasTable = (Vec<(Vec<String>, String)>, HashMap<String, Vec<usize>>);

fn alias_table() -> &'static AliasTable {
    static ALIASES: OnceLock<AliasTable> = OnceLock::new();
    ALIASES.get_or_init(|| {
        let file: AliasFile =
            serde_json::from_str(include_str!("../../assets/merchants/aliases.json"))
                .expect("assets/merchants/aliases.json is valid");
        assert_eq!(
            file.version, VERSION,
            "aliases.json and merchant::VERSION must match"
        );
        let list: Vec<(Vec<String>, String)> = file
            .aliases
            .into_iter()
            .map(|a| {
                let words = normalize(&a.prefix)
                    .split(' ')
                    .map(str::to_string)
                    .collect();
                (words, a.merchant)
            })
            .collect();
        let mut by_first: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, (prefix, _)) in list.iter().enumerate() {
            by_first.entry(prefix[0].clone()).or_default().push(i);
        }
        (list, by_first)
    })
}

/// The alias whose prefix covers the most words at the start of `core`.
fn alias(core: &str) -> Option<&'static str> {
    let words: Vec<&str> = core.split(' ').collect();
    let (list, by_first) = alias_table();
    by_first
        .get(words[0])?
        .iter()
        .map(|&i| &list[i])
        .filter(|(prefix, _)| {
            prefix.len() <= words.len() && prefix.iter().zip(&words).all(|(p, w)| p == w)
        })
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, merchant)| merchant.as_str())
}

/// `"SAKURA RAMEN"` → `"Sakura Ramen"`.
fn title_case(core: &str) -> String {
    core.split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(d: &str) -> Option<String> {
        clean(d).map(|c| c.name)
    }

    fn core_of(d: &str) -> Option<String> {
        clean(d).map(|c| c.core)
    }

    #[test]
    fn processors_store_numbers_and_locations_are_removed() {
        assert_eq!(
            core_of("SQ *SAKURA RAMEN     SAN FRANCISCO       CA").as_deref(),
            Some("SAKURA RAMEN")
        );
        // A full-width name field runs into the city with one space; the city stays.
        assert_eq!(
            core_of("SQ *BLUE BOTTLE COFF SAN FRANCISCO       CA").as_deref(),
            Some("BLUE BOTTLE COFF SAN FRANCISCO")
        );
        assert_eq!(
            core_of("BLUE BOTTLE COFFEE 6 BERKELEY            CA").as_deref(),
            Some("BLUE BOTTLE COFFEE")
        );
        assert_eq!(
            core_of("TST* HOPWORKS TAPROOM SAN FRANCISCO").as_deref(),
            Some("HOPWORKS TAPROOM SAN FRANCISCO")
        );
        assert_eq!(
            core_of("CHECKCARD 0411 KROGER 426 WORTHINGTON OH 24891211229345664883077").as_deref(),
            Some("KROGER")
        );
        assert_eq!(
            core_of("POS PURCHASE SQ *COSMIC CUP AUSTIN TX").as_deref(),
            Some("COSMIC CUP AUSTIN")
        );
        assert_eq!(
            core_of("SAFEWAY #4623 SAN FRANCISCO CA 03/11").as_deref(),
            Some("SAFEWAY")
        );
        assert_eq!(
            core_of("7-ELEVEN 123456 KYOTOJP").as_deref(),
            Some("7 ELEVEN")
        );
    }

    #[test]
    fn ach_and_p2p_references_are_removed() {
        assert_eq!(
            core_of("ROCKET MORTGAGE DES:MTG PYMT ID:ERI9GRHMXE INDN:TESTWORTH JORDAN CO ID:8022934197 PPD")
                .as_deref(),
            Some("ROCKET MORTGAGE")
        );
        assert_eq!(
            core_of("NELNET STUDENT LN WEB ID: 1272658658").as_deref(),
            Some("NELNET STUDENT LN")
        );
        assert_eq!(
            name("Zelle payment to Harper Pretendley Conf# jwndcq6w7").as_deref(),
            Some("Harper Pretendley")
        );
        assert_eq!(
            name("Zelle payment from Logan Ipsumworth JPM99j9ze99").as_deref(),
            Some("Logan Ipsumworth")
        );
        assert_eq!(
            core_of("ZELLE TO MADEUPSON DEVON ON 09/18 REF # PP0ZL1PKY").as_deref(),
            Some("MADEUPSON DEVON")
        );
    }

    #[test]
    fn descriptions_without_a_merchant_give_none() {
        assert_eq!(clean("Check 1085"), None);
        assert_eq!(clean("CHECK # 1100"), None);
        assert_eq!(clean("VENMO PAYMENT 6479724873 WEB ID: 3264681992"), None);
        assert_eq!(clean("   "), None);
        assert_eq!(clean("#1234"), None);
    }

    #[test]
    fn aliases_name_well_known_merchants() {
        assert_eq!(name("AMZN Mktp US*B12OG345P").as_deref(), Some("Amazon"));
        assert_eq!(name("AMAZON MKTPL*I7BX79OSG").as_deref(), Some("Amazon"));
        assert_eq!(
            name("WHOLEFDS MKT #70358 SAN FRANCISCO CA 09/09").as_deref(),
            Some("Whole Foods Market")
        );
        assert_eq!(name("TARGET.COM  *").as_deref(), Some("Target"));
        assert_eq!(name("UBER   *TRIP M1RX").as_deref(), Some("Uber"));
        assert_eq!(name("UBER *EATS").as_deref(), Some("Uber Eats"));
        assert_eq!(name("DD *DOORDASH THAI BASIL").as_deref(), Some("DoorDash"));
        assert_eq!(
            name("MCDONALD S F10081 DUBLINOH").as_deref(),
            Some("McDonald's")
        );
        // Not an alias: title case.
        assert_eq!(
            name("GOLDEN LOTUS RESTAUR SAN FRANCISCO CA").as_deref(),
            Some("Golden Lotus Restaur San Francisco")
        );
        assert!(
            !clean("SQ *SAKURA RAMEN     SAN FRANCISCO       CA")
                .unwrap()
                .aliased
        );
        assert_eq!(
            clean_without_aliases("AMZN Mktp US*B12OG345P")
                .unwrap()
                .name,
            "Amzn Mktp Us"
        );
    }

    #[test]
    fn alias_table_loads_and_matches_its_version() {
        let (aliases, _) = alias_table();
        assert!(aliases.len() > 50);
        for (prefix, merchant) in aliases {
            assert!(!prefix.is_empty() && prefix.iter().all(|w| !w.is_empty()));
            assert!(!merchant.trim().is_empty());
        }
    }
}
