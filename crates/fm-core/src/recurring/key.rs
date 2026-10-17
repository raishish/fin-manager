//! A merchant key: the part of a bank description that stays the same from one charge
//! to the next, so a series can be grouped without a merchant clean-up.

/// Words banks put around the merchant that say nothing about who it is.
const GENERIC: &[&str] = &[
    "ACH",
    "DEBIT",
    "CREDIT",
    "POS",
    "PURCHASE",
    "WEB",
    "ID",
    "PPD",
    "CCD",
    "DES",
    "INDN",
    "CO",
    "SQ",
    "TST",
    "ONLINE",
    "RECURRING",
    "CHECKCARD",
    "DEBITCARD",
    "VISA",
    "DDA",
    "ELECTRONIC",
    "PREAUTHORIZED",
    "ORIG",
    "IAT",
    "XX",
    "XXSOC",
];

/// Web domain suffixes, with their dot.
const DOMAINS: &[&str] = &[".COM", ".NET", ".ORG", ".IO", ".CO", ".US", ".TV"];

/// First words that name a kind of payee rather than one payee ("CITY OF ...").
const WEAK: &[&str] = &[
    "CITY", "STATE", "COUNTY", "TOWN", "VILLAGE", "THE", "US", "USA", "AMERICAN", "FIRST",
    "NATIONAL",
];

/// The description's first meaningful word (two when the first is [`WEAK`]), upper-cased,
/// and a finer key with one more word ("AMAZON PRIME" against "AMAZON MKTPLACE").
///
/// `.`, `&`, `'` and `-` are dropped rather than split on, so `AT&T` and `ATT`, or
/// `T-MOBILE` and `TMOBILE`, give one key. Words with digits (reference numbers, store
/// numbers, dates) and generic bank words are skipped. Empty when nothing is left.
pub(crate) fn merchant_keys(description: &str) -> (String, String) {
    let upper = description.to_uppercase();
    // A web domain names the merchant without its suffix: NETFLIX.COM is NETFLIX.
    let mut bare = String::with_capacity(upper.len());
    let mut rest = upper.as_str();
    while let Some(dot) = rest.find('.') {
        bare.push_str(&rest[..dot]);
        rest = &rest[dot..];
        let suffix = DOMAINS.iter().find(|d| {
            rest.strip_prefix(**d)
                .is_some_and(|after| !after.starts_with(|c: char| c.is_ascii_alphanumeric()))
        });
        match suffix {
            Some(d) => rest = &rest[d.len()..],
            None => {
                bare.push('.');
                rest = &rest[1..];
            }
        }
    }
    bare.push_str(rest);
    let joined: String = bare
        .chars()
        .filter(|c| !matches!(c, '.' | '&' | '\'' | '-'))
        .flat_map(char::to_uppercase)
        .collect();
    let words: Vec<&str> = joined
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 2 && !w.chars().any(|c| c.is_ascii_digit()))
        .filter(|w| !GENERIC.contains(w))
        .collect();
    let take = match words.first() {
        Some(first) if WEAK.contains(first) => 2,
        _ => 1,
    };
    let coarse = words
        .iter()
        .take(take)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let fine = words
        .iter()
        .take(take + 1)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    (coarse, fine)
}

#[cfg(test)]
mod tests {
    use super::merchant_keys;

    #[test]
    fn fine_keys() {
        assert_eq!(merchant_keys("Amazon Prime*J4WLGLFZ1").1, "AMAZON PRIME");
        assert_eq!(merchant_keys("AMAZON MKTPLACE PMTS").1, "AMAZON MKTPLACE");
        assert_eq!(merchant_keys("CITY OF AUSTIN UTILITY").1, "CITY OF AUSTIN");
    }

    #[test]
    fn keys() {
        for (description, key) in [
            ("NETFLIX.COM 365", "NETFLIX"),
            ("NETFLIX.COM 479      LOS GATOS", "NETFLIX"),
            ("FIGMA.COM", "FIGMA"),
            ("APPLE.COM/BILL", "APPLE"),
            ("Amazon.com*GRDSYVCO9 Amzn.com/bill", "AMAZON"),
            ("B&N.COM", "BN"),
            ("COMCAST.COMPANY", "COMCASTCOMPANY"),
            ("CLASSPASS*JUEGPT     CLASSPASS.COM", "CLASSPASS"),
            ("AT&T*BILL PAYMENT    800-331-0500", "ATT"),
            ("ATT*BILL PAYMENT     800-331-0500", "ATT"),
            ("TMOBILE*AUTO PAY", "TMOBILE"),
            ("T-MOBILE AUTO PAY", "TMOBILE"),
            ("ACH DEBIT CITY OF AUSTIN UTILITY", "CITY OF"),
            ("SQ *SUNRISE SWIM SCHOOL", "SUNRISE"),
            ("Check 1100", "CHECK"),
            ("CHECK # 1395", "CHECK"),
            ("12345 678", ""),
        ] {
            assert_eq!(merchant_keys(description).0, key, "{description}");
        }
    }
}
