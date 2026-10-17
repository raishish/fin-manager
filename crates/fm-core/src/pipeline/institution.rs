//! Naming the institution behind an OFX file the way CSV profiles name it, so files from
//! one bank in different formats land on the same account (`Target::Auto` matches the
//! last four digits only at the same institution).
//!
//! OFX identifies a bank by `FI/ORG` and `FI/FID`, and Quicken files add `INTU.BID`; the
//! values are terse (`B1` is Chase, `C1` is Capital One). The table holds the identities
//! of the institutions in TD.2's corpus (`testdata/bank-exports/data/layouts.json`).
//! Unknown banks are named by `ORG` and `FID` together.

use fm_import_types::Institution;

/// `(INTU.BID, name)`: Quicken's id is the most specific.
const BY_INTU_BID: &[(&str, &str)] = &[
    ("10898", "Chase"),
    ("7492", "Capital One"),
    ("17410", "Capital One"),
    ("1236", "Capital One"),
    ("3000", "Wells Fargo"),
    ("2102", "Citi"),
    ("4894", "Citi"),
    ("1402", "U.S. Bank"),
    ("7138", "PNC"),
    ("3101", "American Express"),
    ("18940", "Apple Card"),
    ("6157", "Ally Bank"),
    ("14545", "Navy Federal Credit Union"),
    ("4351", "Barclays"),
    ("4537", "Synchrony"),
    ("53292", "Fifth Third"),
    ("118", "Citizens"),
    ("12913", "KeyBank"),
    ("11025", "Huntington"),
    ("5702", "Regions"),
];

/// `(ORG, FID, name)`, for plain OFX without `INTU.BID`.
const BY_ORG_FID: &[(&str, &str, &str)] = &[
    ("B1", "10898", "Chase"),
    ("C1", "1001", "Capital One"),
    ("Capital One Bank", "1001", "Capital One"),
    ("WFB", "3000", "Wells Fargo"),
    ("Citibank", "2102", "Citi"),
    ("U.S. Bank", "1402", "U.S. Bank"),
    ("PNC", "7138", "PNC"),
    ("AMEX", "3101", "American Express"),
    ("Apple Card", "18940", "Apple Card"),
    ("Ally", "6157", "Ally Bank"),
    ("NFCU", "1001", "Navy Federal Credit Union"),
    ("Barclays Bank Delaware", "4351", "Barclays"),
    ("SYNCB", "4537", "Synchrony"),
    ("Fifth Third Bank", "5829", "Fifth Third"),
    ("Citizens", "118", "Citizens"),
    ("KeyBank", "5901", "KeyBank"),
    ("Huntington", "11025", "Huntington"),
    ("regions.com", "243", "Regions"),
];

/// The institution's name: from the table when the file's identity is known,
/// otherwise its `ORG` as written.
pub(crate) fn name(institution: &Institution) -> Option<String> {
    if let Some(bid) = institution.intu_bid.as_deref()
        && let Some((_, name)) = BY_INTU_BID.iter().find(|(b, _)| *b == bid)
    {
        return Some((*name).to_owned());
    }
    let org = institution.org.as_deref()?;
    let fid = institution.fid.as_deref().unwrap_or("");
    Some(
        BY_ORG_FID
            .iter()
            .find(|(o, f, _)| o.eq_ignore_ascii_case(org) && *f == fid)
            .map_or_else(
                // An unknown bank is ORG and FID together: ORG alone ("Credit Union") can
                // be shared by unrelated institutions.
                || {
                    if fid.is_empty() {
                        org.to_owned()
                    } else {
                        format!("{org} (FID {fid})")
                    }
                },
                |(_, _, name)| (*name).to_owned(),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fi(org: &str, fid: &str, bid: Option<&str>) -> Institution {
        Institution {
            org: Some(org.into()),
            fid: Some(fid.into()),
            intu_bid: bid.map(Into::into),
            layout: None,
        }
    }

    #[test]
    fn known_banks_get_the_csv_profiles_names() {
        assert_eq!(
            name(&fi("C1", "1001", Some("7492"))).as_deref(),
            Some("Capital One")
        );
        assert_eq!(
            name(&fi("B1", "10898", Some("10898"))).as_deref(),
            Some("Chase")
        );
        // FID 1001 is both Capital One and Navy Federal: ORG decides.
        assert_eq!(
            name(&fi("NFCU", "1001", None)).as_deref(),
            Some("Navy Federal Credit Union")
        );
        assert_eq!(
            name(&fi("Capital One Bank", "1001", None)).as_deref(),
            Some("Capital One")
        );
        // Unknown banks are named by ORG and FID together.
        assert_eq!(
            name(&fi("Lakeline FCU", "9", None)).as_deref(),
            Some("Lakeline FCU (FID 9)")
        );
        assert_ne!(
            name(&fi("Credit Union", "1", None)),
            name(&fi("Credit Union", "2", None))
        );
        assert_eq!(name(&Institution::default()), None);
    }

    /// Every CSV profile's institution that also offers OFX is spelled the same here.
    #[test]
    fn names_match_the_csv_profiles() {
        let csv: Vec<String> = fm_import_csv::ProfileSet::with_builtin()
            .profiles()
            .iter()
            .map(|p| p.institution.clone())
            .collect();
        for (_, name) in BY_INTU_BID {
            let offers_csv = [
                "Chase",
                "Capital One",
                "Wells Fargo",
                "Citi",
                "U.S. Bank",
                "PNC",
                "American Express",
                "Apple Card",
                "Ally Bank",
                "Navy Federal Credit Union",
                "Barclays",
            ];
            if offers_csv.contains(name) {
                assert!(
                    csv.iter().any(|c| c == name),
                    "{name} not a CSV profile institution"
                );
            }
        }
    }
}
