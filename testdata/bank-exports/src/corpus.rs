//! Which files the corpus contains: for each, a layout, the TD.1 account its rows come
//! from, the date range asked for and the day it was downloaded.
//!
//! Accounts are reused across institutions: the corpus tests file layouts, so a TD.1
//! account whose descriptions look like one bank's can appear in another bank's layout.
//! Several sets overlap on purpose (T1.3 must import them without duplicates), and some
//! downloads are made mid-cycle, so a pending purchase in one posts in the next.

use fm_core::model::Date;
use fm_synth::cal::ymd;

pub struct FileSpec {
    /// File name without extension (the folder is the institution).
    pub name: &'static str,
    pub layout: &'static str,
    pub persona: &'static str,
    /// TD.1 account key (`checking`, `savings`, or a card key such as `freedom`).
    pub account: &'static str,
    pub start: Date,
    pub end: Date,
    pub as_of: Date,
}

#[allow(clippy::too_many_arguments)]
fn f(
    name: &'static str,
    layout: &'static str,
    persona: &'static str,
    account: &'static str,
    start: Date,
    end: Date,
    as_of: Date,
) -> FileSpec {
    FileSpec {
        name,
        layout,
        persona,
        account,
        start,
        end,
        as_of,
    }
}

/// Calendar year, downloaded a few days after it ended.
fn year(
    name: &'static str,
    layout: &'static str,
    persona: &'static str,
    account: &'static str,
    y: i32,
) -> FileSpec {
    let as_of = if y == 2026 {
        ymd(2026, 9, 30)
    } else {
        ymd(y + 1, 1, 4)
    };
    let end = if y == 2026 {
        ymd(2026, 9, 30)
    } else {
        ymd(y, 12, 31)
    };
    f(name, layout, persona, account, ymd(y, 1, 1), end, as_of)
}

pub fn files() -> Vec<FileSpec> {
    vec![
        // Chase: two overlapping card CSVs and a QFX covering both; checking CSV and QFX.
        f(
            "freedom_2025_h1",
            "chase_card_csv",
            "renter",
            "freedom",
            ymd(2025, 1, 1),
            ymd(2025, 6, 30),
            ymd(2025, 7, 2),
        ),
        f(
            "freedom_2025_q2_q3",
            "chase_card_csv",
            "renter",
            "freedom",
            ymd(2025, 4, 1),
            ymd(2025, 9, 30),
            ymd(2025, 10, 1),
        ),
        year("freedom_2025", "chase_card_qfx", "renter", "freedom", 2025),
        year(
            "sapphire_2026",
            "chase_card_csv",
            "family",
            "sapphire",
            2026,
        ),
        year(
            "checking_2025",
            "chase_checking_csv",
            "renter",
            "checking",
            2025,
        ),
        f(
            "checking_2026_q3",
            "chase_checking_qfx",
            "renter",
            "checking",
            ymd(2026, 7, 1),
            ymd(2026, 9, 30),
            ymd(2026, 9, 30),
        ),
        // Bank of America: CSV only (QFX ended 2025-09-30).
        f(
            "checking_2025_h2",
            "bofa_checking_csv",
            "family",
            "checking",
            ymd(2025, 7, 1),
            ymd(2025, 12, 31),
            ymd(2026, 1, 5),
        ),
        year(
            "savings_2025",
            "bofa_checking_csv",
            "family",
            "savings",
            2025,
        ),
        year("card_2025", "bofa_card_csv", "family", "sapphire", 2025),
        // Wells Fargo: the legacy headerless CSV, the 2026 CSV, and QFX overlapping the legacy file.
        f(
            "checking_2025_h1_legacy",
            "wells_fargo_legacy_csv",
            "retiree",
            "checking",
            ymd(2025, 1, 1),
            ymd(2025, 6, 30),
            ymd(2025, 7, 1),
        ),
        year(
            "checking_2026",
            "wells_fargo_csv",
            "retiree",
            "checking",
            2026,
        ),
        f(
            "checking_2025_q2_q4",
            "wells_fargo_qfx",
            "retiree",
            "checking",
            ymd(2025, 4, 1),
            ymd(2025, 12, 31),
            ymd(2026, 1, 4),
        ),
        year(
            "savings_2025",
            "wells_fargo_qfx",
            "retiree",
            "savings",
            2025,
        ),
        // Citi: two pairs of mid-cycle downloads, each made on a Monday so weekend
        // purchases are still pending, then the next download after they post; and QFX.
        // The rows come from the busiest card (the renter's dining card).
        f(
            "card_2025_06_16",
            "citi_card_csv",
            "renter",
            "amex_gold",
            ymd(2025, 5, 17),
            ymd(2025, 6, 16),
            ymd(2025, 6, 16),
        ),
        f(
            "card_2025_07_14",
            "citi_card_csv",
            "renter",
            "amex_gold",
            ymd(2025, 6, 1),
            ymd(2025, 7, 14),
            ymd(2025, 7, 14),
        ),
        f(
            "card_2025_12_15",
            "citi_card_csv",
            "renter",
            "amex_gold",
            ymd(2025, 11, 16),
            ymd(2025, 12, 15),
            ymd(2025, 12, 15),
        ),
        f(
            "card_2026_01_12",
            "citi_card_csv",
            "renter",
            "amex_gold",
            ymd(2025, 12, 1),
            ymd(2026, 1, 12),
            ymd(2026, 1, 12),
        ),
        year("card_2026", "citi_qfx", "retiree", "quicksilver", 2026),
        // Capital One: card and 360 accounts, CSV and QFX.
        year(
            "savor_2025",
            "capital_one_card_csv",
            "family",
            "savor",
            2025,
        ),
        year(
            "venture_2025",
            "capital_one_card_qfx",
            "freelancer",
            "venture",
            2025,
        ),
        year(
            "checking_2025",
            "capital_one_360_csv",
            "freelancer",
            "checking",
            2025,
        ),
        year(
            "savings_2026",
            "capital_one_360_qfx",
            "freelancer",
            "savings",
            2026,
        ),
        // U.S. Bank, PNC, Truist.
        year("checking_2026", "us_bank_csv", "family", "checking", 2026),
        f(
            "checking_2026_q1",
            "us_bank_qfx",
            "family",
            "checking",
            ymd(2026, 1, 1),
            ymd(2026, 3, 31),
            ymd(2026, 4, 2),
        ),
        f(
            "checking_2025_h2",
            "pnc_csv",
            "freelancer",
            "checking",
            ymd(2025, 7, 1),
            ymd(2025, 12, 31),
            ymd(2026, 1, 4),
        ),
        year("card_2026", "pnc_qfx", "family", "savor", 2026),
        year("checking_2026", "truist_csv", "family", "checking", 2026),
        // American Express: basic and extended CSV, QFX.
        year("gold_2025", "amex_csv", "renter", "amex_gold", 2025),
        year(
            "blue_cash_2025",
            "amex_extended_csv",
            "retiree",
            "blue_cash",
            2025,
        ),
        year("blue_cash_2026", "amex_qfx", "retiree", "blue_cash", 2026),
        // Discover, Apple Card.
        year("card_2025", "discover_csv", "freelancer", "venture", 2025),
        f(
            "card_2025_10",
            "apple_card_csv",
            "freelancer",
            "freedom_flex",
            ymd(2025, 10, 1),
            ymd(2025, 10, 31),
            ymd(2025, 11, 1),
        ),
        year(
            "card_2025",
            "apple_card_qfx",
            "freelancer",
            "freedom_flex",
            2025,
        ),
        // Ally (the renter's real savings bank).
        year("savings_2025", "ally_csv", "renter", "savings", 2025),
        year("savings_2026", "ally_qfx", "renter", "savings", 2026),
        // Schwab and USAA: mid-cycle downloads made while a debit-card purchase is still
        // pending, then the next download.
        f(
            "checking_2025_09_15",
            "schwab_checking_csv",
            "freelancer",
            "checking",
            ymd(2025, 8, 16),
            ymd(2025, 9, 15),
            ymd(2025, 9, 15),
        ),
        f(
            "checking_2025_10_15",
            "schwab_checking_csv",
            "freelancer",
            "checking",
            ymd(2025, 9, 1),
            ymd(2025, 10, 15),
            ymd(2025, 10, 15),
        ),
        f(
            "checking_2026_03_13",
            "usaa_csv",
            "renter",
            "checking",
            ymd(2026, 2, 13),
            ymd(2026, 3, 13),
            ymd(2026, 3, 13),
        ),
        f(
            "checking_2026_04_13",
            "usaa_csv",
            "renter",
            "checking",
            ymd(2026, 3, 1),
            ymd(2026, 4, 13),
            ymd(2026, 4, 13),
        ),
        // Navy Federal, TD Bank, Barclays, Synchrony.
        year(
            "checking_2025",
            "navy_federal_csv",
            "retiree",
            "checking",
            2025,
        ),
        year(
            "checking_2026",
            "navy_federal_ofx",
            "retiree",
            "checking",
            2026,
        ),
        year("checking_2025", "td_bank_csv", "family", "checking", 2025),
        year("card_2026", "barclays_csv", "family", "sapphire", 2026),
        year("card_2026", "barclays_qfx", "renter", "freedom", 2026),
        f(
            "card_2026_h1",
            "synchrony_qfx",
            "family",
            "savor",
            ymd(2026, 1, 1),
            ymd(2026, 6, 30),
            ymd(2026, 7, 3),
        ),
        // Web Connect banks without a public sample (reconstructed).
        year(
            "checking_2025",
            "fifth_third_qfx",
            "freelancer",
            "checking",
            2025,
        ),
        year("checking_2025", "citizens_qfx", "renter", "checking", 2025),
        year("savings_2025", "keybank_qfx", "family", "savings", 2025),
        year(
            "checking_2026",
            "huntington_qfx",
            "retiree",
            "checking",
            2026,
        ),
        f(
            "checking_2025_q4",
            "regions_qfx",
            "family",
            "checking",
            ymd(2025, 10, 1),
            ymd(2025, 12, 31),
            ymd(2026, 1, 4),
        ),
    ]
}
