//! OFX and QFX (Quicken Web Connect) layouts. FI identifiers come from Intuit's
//! Quicken FI directory and from sample files; account and routing numbers are always
//! fake.

use super::*;

const QUICKEN_DIRECTORY: &str = "https://ofx-prod-filist.intuit.com/qm2400/data/fidir.txt";
const OFXTOOLS_FI: &str = "https://github.com/csingley/ofxtools/blob/master/ofxtools/config/fi.cfg";

pub fn layouts() -> Vec<Layout> {
    let mut v = vec![
        chase_card(),
        chase_checking(),
        wells(),
        citi(),
        capone_card(),
        capone_360(),
        us_bank(),
        pnc(),
        amex(),
        apple(),
        ally(),
        navy_federal(),
        barclays(),
        synchrony(),
    ];
    v.extend(reconstructed());
    v
}

fn qfx(
    id: &'static str,
    institution: &'static str,
    kinds: &[&'static str],
    provenance: Provenance,
    spec: OfxSpec,
) -> Layout {
    let mut l = base(
        id,
        institution,
        kinds,
        if spec.qfx { "qfx" } else { "ofx" },
        provenance,
        Format::Ofx(spec),
    );
    l.type_of = trntype;
    l.newest_first = false;
    l
}

fn chase_card() -> Layout {
    let mut l = qfx(
        "chase_card_qfx",
        "Chase",
        &["credit_card"],
        prov(
            "high",
            &[
                "https://github.com/amkchari/ExpenseManager/tree/HEAD/statements",
                QUICKEN_DIRECTORY,
            ],
            "OFX 1.0.2 SGML without closing tags. A blank line before OFXHEADER, `[0:GMT]` times, no MEMO, \
             TRNTYPE DEBIT or CREDIT only. NAME isn't held to 32 characters at Chase. Real card FITIDs can be \
             synthetic strings (not rendered).",
        ),
        OfxSpec {
            tz: "[0:GMT]",
            leading_blank_line: true,
            name_max: 64,
            ..ofx_spec("102", "B1", "10898", Some("10898"))
        },
    );
    l.type_of = trntype_by_sign;
    l
}

fn chase_checking() -> Layout {
    let mut l = qfx(
        "chase_checking_qfx",
        "Chase",
        &["checking", "savings"],
        prov(
            "high",
            &[
                "https://github.com/amkchari/ExpenseManager/tree/HEAD/statements",
                QUICKEN_DIRECTORY,
            ],
            "Same identifiers as the card download. Both NAME and MEMO carry the description; NAME seen up to \
             35 characters. TRNTYPE DEBIT or CREDIT only.",
        ),
        OfxSpec {
            tz: "[0:GMT]",
            leading_blank_line: true,
            name_max: 64,
            text: "name_and_memo",
            ..ofx_spec("102", "B1", "10898", Some("10898"))
        },
    );
    l.type_of = trntype_by_sign;
    l
}

fn wells() -> Layout {
    let mut l = qfx(
        "wells_fargo_qfx",
        "Wells Fargo",
        &["checking", "savings", "credit_card"],
        prov(
            "high",
            &[
                "https://github.com/mattgibb/account_layer/blob/HEAD/spec/fixtures/wells_fargo_statements/Checking2.qfx",
                "https://github.com/aclindsa/ofxgo/blob/HEAD/samples/busted_responses/wellsfargo.qfx",
                QUICKEN_DIRECTORY,
            ],
            "OFX 1.0.2 SGML. NAME truncated to 32 characters with the full text in MEMO. TRNTYPE POS for card \
             purchases and DIRECTDEBIT for ACH. Real MEMOs can carry `?MCC=` codes (not rendered).",
        ),
        OfxSpec {
            tz: "[-8:PST]",
            memo: "overflow",
            ..ofx_spec("102", "WFB", "3000", Some("3000"))
        },
    );
    l.type_of = trntype_wells;
    l
}

fn citi() -> Layout {
    qfx(
        "citi_qfx",
        "Citi",
        &["credit_card"],
        prov(
            "medium",
            &[
                "https://github.com/amkchari/ExpenseManager/tree/HEAD/statements",
                "https://wiki.gnucash.org/wiki/OFX_Direct_Connect_Bank_Settings",
                QUICKEN_DIRECTORY,
            ],
            "Downloaded through the Citibank banking login: the card comes as a BANK statement with ACCTTYPE \
             CREDITLINE, not a credit-card statement. No time-zone suffix. The Citi Cards login (INTU.BID \
             4894) may differ.",
        ),
        OfxSpec {
            card_as_creditline: true,
            ..ofx_spec("102", "Citibank", "2102", Some("2102"))
        },
    )
}

fn capone_card() -> Layout {
    qfx(
        "capital_one_card_qfx",
        "Capital One",
        &["credit_card"],
        prov(
            "medium",
            &[
                "https://github.com/ddoherty03/treetop-ofx-test/blob/HEAD/cc_ofx2_resp.qfx",
                QUICKEN_DIRECTORY,
            ],
            "Closed leaf tags, DTUSER present, NAME and MEMO identical. Real files also put a nonstandard \
             CCACCTTO inside each STMTTRN (not rendered).",
        ),
        OfxSpec {
            close_leaf_tags: true,
            dtuser: true,
            text: "name_and_memo",
            ..ofx_spec("102", "C1", "1001", Some("7492"))
        },
    )
}

fn capone_360() -> Layout {
    qfx(
        "capital_one_360_qfx",
        "Capital One",
        &["checking", "savings"],
        prov(
            "high",
            &[
                "https://github.com/redstreet/beancount_reds_importers/blob/main/beancount_reds_importers/importers/capitalonebank/tests/360Checking.qfx",
                "https://github.com/howeyc/ledger/blob/HEAD/ledger/cmd/internal/import/qfx/sample.qfx",
            ],
            "OFX 2.0.2 XML. No NAME at all: the description is only in MEMO. INTU.BID 17410 ('Capital One \
             Bank - New', 2026); older files have 1236. Real DTSERVER values are malformed (not rendered).",
        ),
        OfxSpec {
            text: "memo_only",
            ..ofx_spec("202", "Capital One Bank", "1001", Some("17410"))
        },
    )
}

fn us_bank() -> Layout {
    qfx(
        "us_bank_qfx",
        "U.S. Bank",
        &["checking", "savings", "credit_card"],
        prov(
            "high",
            &[
                "https://github.com/monetr/monetr/blob/HEAD/server/datasources/ofx/fixtures/sample-usbank.qfx",
                QUICKEN_DIRECTORY,
            ],
            "OFX 1.0.2 SGML. NAME cut to 32 characters (real files pad it with trailing spaces, not rendered); \
             real MEMOs hold `reference; MCC;` (not rendered).",
        ),
        ofx_spec("102", "U.S. Bank", "1402", Some("1402")),
    )
}

fn pnc() -> Layout {
    qfx(
        "pnc_qfx",
        "PNC",
        &["checking", "savings", "credit_card"],
        prov(
            "high",
            &[
                "https://github.com/amkchari/ExpenseManager/tree/HEAD/statements",
                OFXTOOLS_FI,
                QUICKEN_DIRECTORY,
            ],
            "OFX 1.0.2 SGML with `[-4:EDT]` times, DTUSER present, and DTSTART set to the Unix epoch whatever \
             range was asked for. Real amounts can have one decimal (`88.0`; not rendered).",
        ),
        OfxSpec {
            tz: "[-4:EDT]",
            dtuser: true,
            dtstart: Some("19700101120000"),
            ..ofx_spec("102", "PNC", "7138", Some("7138"))
        },
    )
}

fn amex() -> Layout {
    qfx(
        "amex_qfx",
        "American Express",
        &["credit_card"],
        prov(
            "high",
            &[
                "https://github.com/beancount/beangulp/blob/HEAD/examples/Downloads/ofxdownload.ofx",
                QUICKEN_DIRECTORY,
            ],
            "OFX 1.0.2 SGML on a single line, `[-7:MST]` times. Real files add proprietary tags (ORIGIN.ID, \
             AMEX.BASICACCT and others) and a REFNUM equal to FITID (not rendered).",
        ),
        OfxSpec {
            line_per_element: false,
            tz: "[-7:MST]",
            ..ofx_spec("102", "AMEX", "3101", Some("3101"))
        },
    )
}

fn apple() -> Layout {
    let mut l = qfx(
        "apple_card_qfx",
        "Apple Card",
        &["credit_card"],
        prov(
            "high",
            &[
                "https://github.com/amkchari/ExpenseManager/tree/HEAD/statements",
                "https://support.apple.com/en-lb/102284",
            ],
            "OFX 1.0.2 SGML on a single line, `[0:GMT]` times, TRNTYPE DEBIT, CREDIT or INT, NAME up to 32 \
             characters, no MEMO. Real FITIDs are UUIDs.",
        ),
        OfxSpec {
            line_per_element: false,
            tz: "[0:GMT]",
            ..ofx_spec("102", "Apple Card", "18940", Some("18940"))
        },
    );
    l.type_of = trntype_apple;
    l
}

fn ally() -> Layout {
    qfx(
        "ally_qfx",
        "Ally Bank",
        &["checking", "savings"],
        prov(
            "high",
            &[
                "https://github.com/chilts/node-ofx/blob/HEAD/test/data/example-xml.qfx",
                QUICKEN_DIRECTORY,
            ],
            "An SGML (version 102) header without the OLDFILEUID/NEWFILEUID lines, but an XML-style body: \
             closed tags, on one line. SEVERITY written `Info`. No time zone.",
        ),
        OfxSpec {
            close_leaf_tags: true,
            line_per_element: false,
            header_fileuids: false,
            severity: "Info",
            ..ofx_spec("102", "Ally", "6157", Some("6157"))
        },
    )
}

fn navy_federal() -> Layout {
    qfx(
        "navy_federal_ofx",
        "Navy Federal Credit Union",
        &["checking", "savings", "credit_card"],
        prov(
            "high",
            &[
                "https://github.com/monetr/monetr/blob/HEAD/server/datasources/ofx/fixtures/sample-nfcu.qfx",
                "https://github.com/monetr/monetr/blob/HEAD/server/datasources/ofx/fixtures/sample-nfcu-2.qfx",
            ],
            "The OFX 2.2 XML variant (an SGML variant also exists). NAME and MEMO both carry the description. \
             Rendered without INTU.BID (Quicken lists 14545), so as plain .ofx.",
        ),
        OfxSpec {
            text: "name_and_memo",
            ..ofx_spec("220", "NFCU", "1001", None)
        },
    )
}

fn barclays() -> Layout {
    qfx(
        "barclays_qfx",
        "Barclays",
        &["credit_card"],
        prov(
            "high",
            &[
                "https://github.com/dwhixon59/forecastengine/blob/HEAD/src/test/resources/qdl20251215.qfx",
            ],
            "OFX 1.0.2 SGML on a single line, LANGUAGE written `eng`, DTUSER present, no time zone. Real files \
             mask ACCTID.",
        ),
        OfxSpec {
            line_per_element: false,
            language: "eng",
            dtuser: true,
            ..ofx_spec("102", "Barclays Bank Delaware", "4351", Some("4351"))
        },
    )
}

fn synchrony() -> Layout {
    qfx(
        "synchrony_qfx",
        "Synchrony",
        &["credit_card"],
        prov(
            "medium",
            &[
                "https://github.com/alexislopez2010/financehub/blob/HEAD/apps/web/lib/import/ofx.test.ts",
            ],
            "PayPal Credit (issued by Synchrony), late-2025 shape: `[-8:PST]` on DTSERVER but date-only \
             DTPOSTED and DTUSER. Real files can have empty NAME elements (not rendered). No CSV sample found \
             for any Synchrony card.",
        ),
        OfxSpec {
            tz: "[-8:PST]",
            date_only_postings: true,
            dtuser: true,
            ..ofx_spec("102", "SYNCB", "4537", Some("4537"))
        },
    )
}

/// Banks whose Web Connect download is listed in the Quicken FI directory but for which
/// no sample file or CSV layout was found. The files use the common Web Connect shape.
fn reconstructed() -> Vec<Layout> {
    let note = "Reconstructed. The Quicken FI directory lists a Web Connect (QFX) download, which gives the \
                INTU.BID; ORG and FID are the institution's Direct Connect values where public, otherwise \
                assumed. No public sample file or CSV layout was found.";
    [
        (
            "fifth_third_qfx",
            "Fifth Third",
            "Fifth Third Bank",
            "5829",
            "53292",
        ),
        ("citizens_qfx", "Citizens", "Citizens", "118", "118"),
        ("keybank_qfx", "KeyBank", "KeyBank", "5901", "12913"),
        (
            "huntington_qfx",
            "Huntington",
            "Huntington",
            "11025",
            "11025",
        ),
        ("regions_qfx", "Regions", "regions.com", "243", "5702"),
    ]
    .into_iter()
    .map(|(id, institution, org, fid, bid)| {
        qfx(
            id,
            institution,
            &["checking", "savings"],
            prov("reconstructed", &[QUICKEN_DIRECTORY, OFXTOOLS_FI], note),
            OfxSpec {
                tz: "[-5:EST]",
                memo: "overflow",
                ..ofx_spec("102", org, fid, Some(bid))
            },
        )
    })
    .collect()
}
