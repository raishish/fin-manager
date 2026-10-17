//! Every layout in the corpus. CSV layouts are in `csv.rs`, OFX/QFX in `ofx.rs`;
//! this file has the shared pieces: builders, per-row extras, transaction types and
//! the banks' own category names.

mod csv;
mod ofx;

use std::sync::OnceLock;

use fm_core::model::Taxonomy;

use crate::render::FileCtx;
use crate::source::{Kind, Row};
use crate::spec::{
    CsvSpec, Format, Layout, OfxSpec, PLAIN, Provenance, no_category, no_extra, no_lead_rows,
    no_preamble, no_type,
};

/// Every layout, CSV first.
pub fn all() -> Vec<Layout> {
    let mut v = csv::layouts();
    v.extend(ofx::layouts());
    v
}

/// Institution names, in a fixed order (used for fake routing numbers).
pub fn institutions() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = all().iter().map(|l| l.institution).collect();
    names.sort_unstable();
    names.dedup();
    names
}

// ---- Builders ----

fn base(
    id: &'static str,
    institution: &'static str,
    kinds: &[&'static str],
    extension: &'static str,
    provenance: Provenance,
    format: Format,
) -> Layout {
    Layout {
        id,
        institution,
        account_kinds: kinds.to_vec(),
        extension,
        provenance,
        newest_first: true,
        includes_pending: false,
        format,
        type_of: no_type,
        category_of: no_category,
        extra_of: no_extra,
        preamble: no_preamble,
        lead_rows: no_lead_rows,
    }
}

fn prov(confidence: &'static str, sources: &[&'static str], notes: &'static str) -> Provenance {
    Provenance {
        sources: sources.to_vec(),
        confidence,
        notes,
    }
}

fn csv_spec(columns: Vec<crate::spec::Column>, date_format: &'static str) -> CsvSpec {
    CsvSpec {
        delimiter: ',',
        quote_all: false,
        quote_header: false,
        quote_empty: false,
        line_ending: "lf",
        utf8_bom: false,
        skip_lines: 0,
        has_header: true,
        columns,
        date_format,
        amount_format: PLAIN,
        skip_rows_without_amount: false,
        trailing_delimiter: "none",
        trim: false,
    }
}

fn ofx_spec(
    version: &'static str,
    org: &'static str,
    fid: &'static str,
    intu_bid: Option<&'static str>,
) -> OfxSpec {
    OfxSpec {
        version,
        qfx: intu_bid.is_some(),
        close_leaf_tags: false,
        line_per_element: true,
        org,
        fid,
        intu_bid,
        tz: "",
        name_max: 32,
        memo: "none",
        dtuser: false,
        text: "name",
        date_only_postings: false,
        leading_blank_line: false,
        header_fileuids: true,
        card_as_creditline: false,
        language: "ENG",
        severity: "INFO",
        dtstart: None,
    }
}

// ---- Per-row extras ----

/// The persona's name as banks print it, and their city.
fn holder(persona: &str) -> (&'static str, &'static str) {
    match persona {
        "renter" => ("Alex Example", "SAN FRANCISCO CA"),
        "family" => ("Jordan Testworth", "DUBLIN OH"),
        "freelancer" => ("Sam Fakerson", "AUSTIN TX"),
        _ => ("Pat Exampleton", "TUCSON AZ"),
    }
}

/// A stable number per transaction, for values that just need to look varied.
fn spread(row: &Row, modulo: i64) -> i64 {
    (row.truth_id.0 * 7919).rem_euclid(modulo)
}

fn extras(what: &str, row: &Row, ctx: &FileCtx) -> String {
    let (name, city) = holder(&ctx.persona);
    let (town, state) = city.rsplit_once(' ').unwrap_or((city, ""));
    match what {
        "holder_upper" => name.to_uppercase(),
        "holder" => name.to_string(),
        "account_dash" => format!("-{}", ctx.account.masked_number.as_deref().unwrap_or("")),
        "details" => match (row.kind, row.amount < 0) {
            (Kind::Check, _) => "CHECK".into(),
            (_, true) => "DEBIT".into(),
            (_, false) => "CREDIT".into(),
        },
        "address" => city.to_string(),
        "extended_details" => format!(
            "{:06} {}\n{}",
            spread(row, 1_000_000),
            row.payee().to_uppercase(),
            town
        ),
        "street" => format!("{} MAIN ST\n{town}", 100 + spread(row, 9_000)),
        "city_state" => format!("{town}\n{state}"),
        "zip" => format!("{:05}", 10_000 + spread(row, 89_999)),
        "country" => "UNITED STATES".into(),
        "time" => format!(
            "{:02}:{:02}:{:02}",
            spread(row, 24),
            spread(row, 60),
            spread(row, 59)
        ),
        "routing" => ctx.routing.clone(),
        "number" => ctx.number.clone(),
        "card_ending" => {
            if matches!(row.kind, Kind::Purchase | Kind::Refund) {
                ctx.account.masked_number.clone().unwrap_or_default()
            } else {
                String::new()
            }
        }
        "subcategory" => row
            .category
            .as_deref()
            .and_then(category_name)
            .unwrap_or("")
            .to_string(),
        "nfcu_group" => match row.kind {
            Kind::Purchase => "Purchases",
            Kind::Refund | Kind::AchCredit | Kind::P2pIn | Kind::InterestEarned => "Deposits",
            Kind::TransferIn | Kind::TransferOut | Kind::CardPaymentOut => "Transfers",
            Kind::Check | Kind::Atm | Kind::AchDebit | Kind::P2pOut => "Withdrawals",
            _ => "Fees",
        }
        .into(),
        other => panic!("no extra {other}"),
    }
}

fn taxonomy() -> &'static Taxonomy {
    static T: OnceLock<Taxonomy> = OnceLock::new();
    T.get_or_init(fm_synth::taxonomy)
}

/// The shipped taxonomy's display name for a category key.
fn category_name(key: &str) -> Option<&'static str> {
    taxonomy()
        .categories
        .iter()
        .find(|c| c.key == key)
        .map(|c| c.name.as_str())
}

/// The shipped taxonomy's group name for a category key.
fn group_name(key: &str) -> Option<&'static str> {
    let c = taxonomy().categories.iter().find(|c| c.key == key)?;
    taxonomy()
        .groups
        .iter()
        .find(|g| g.id == c.group_id)
        .map(|g| g.name.as_str())
}

/// First table entry whose key prefix matches.
fn lookup(table: &[(&str, &'static str)], key: &str) -> &'static str {
    table
        .iter()
        .find(|(prefix, _)| key.starts_with(prefix))
        .map(|(_, name)| *name)
        .unwrap_or("")
}

// ---- Transaction types ----

/// OFX `TRNTYPE` as most banks use it.
fn trntype(k: Kind, _: &Row) -> &'static str {
    match k {
        Kind::Purchase | Kind::P2pOut => "DEBIT",
        Kind::Refund | Kind::P2pIn => "CREDIT",
        Kind::CardPaymentIn | Kind::CardPaymentOut => "PAYMENT",
        Kind::TransferIn | Kind::TransferOut => "XFER",
        Kind::AchDebit => "DIRECTDEBIT",
        Kind::AchCredit => "DIRECTDEP",
        Kind::Check => "CHECK",
        Kind::Atm => "ATM",
        Kind::Fee | Kind::InterestCharged => "FEE",
        Kind::InterestEarned => "INT",
    }
}

/// OFX `TRNTYPE` by sign only (Chase).
fn trntype_by_sign(_: Kind, row: &Row) -> &'static str {
    if row.amount < 0 { "DEBIT" } else { "CREDIT" }
}

/// Wells Fargo: `POS` for card purchases.
fn trntype_wells(k: Kind, row: &Row) -> &'static str {
    match k {
        Kind::Purchase => "POS",
        Kind::Check => "CHECK",
        Kind::AchDebit => "DIRECTDEBIT",
        _ => trntype_by_sign(k, row),
    }
}

/// Apple Card: `DEBIT`, `CREDIT` and `INT`.
fn trntype_apple(k: Kind, row: &Row) -> &'static str {
    if k == Kind::InterestCharged {
        "INT"
    } else {
        trntype_by_sign(k, row)
    }
}

// ---- Bank categories ----

fn chase_category(key: Option<&str>, _: Kind) -> &'static str {
    key.map_or("", |k| {
        lookup(
            &[
                ("food.groceries", "Groceries"),
                ("food.", "Food & Drink"),
                ("transport.fuel", "Gas"),
                ("transport.auto", "Automotive"),
                ("transport.", "Travel"),
                ("travel.", "Travel"),
                ("shopping.", "Shopping"),
                ("family.education", "Education"),
                ("family.", "Shopping"),
                ("pets.", "Shopping"),
                ("entertainment.", "Entertainment"),
                ("utilities.", "Bills & Utilities"),
                ("insurance.", "Bills & Utilities"),
                ("services.professional", "Professional Services"),
                ("services.", "Bills & Utilities"),
                ("health.", "Health & Wellness"),
                ("personal_care.", "Personal"),
                ("housing.", "Home"),
                ("financial.", "Fees & Adjustments"),
                ("giving.", "Gifts & Donations"),
                ("taxes.", "Miscellaneous"),
            ],
            k,
        )
    })
}

fn capone_category(key: Option<&str>, kind: Kind) -> &'static str {
    match (key, kind) {
        (None, Kind::CardPaymentIn) => "Payment/Credit",
        (None, _) => "Other",
        (Some(_), Kind::Refund) => "Payment/Credit",
        (Some(k), _) => lookup(
            &[
                ("food.", "Dining"),
                ("transport.fuel", "Gas/Automotive"),
                ("transport.auto", "Gas/Automotive"),
                ("transport.", "Other Travel"),
                ("travel.flights", "Airfare"),
                ("travel.lodging", "Lodging"),
                ("travel.car_rental", "Car Rental"),
                ("travel.", "Other Travel"),
                ("entertainment.", "Entertainment"),
                ("health.", "Health Care"),
                ("utilities.phone", "Phone/Cable"),
                ("utilities.internet", "Phone/Cable"),
                ("utilities.", "Utilities"),
                ("insurance.", "Insurance"),
                ("financial.", "Fee/Interest Charge"),
                ("services.", "Other Services"),
                ("", "Merchandise"),
            ],
            k,
        ),
    }
}

fn discover_category(key: Option<&str>, kind: Kind) -> &'static str {
    match (key, kind) {
        (_, Kind::CardPaymentIn | Kind::Refund) => "Payments and Credits",
        (None, Kind::InterestCharged) => "Interest",
        (None, _) => "Fees",
        (Some(k), _) => lookup(
            &[
                ("food.groceries", "Supermarkets"),
                ("food.", "Restaurants"),
                ("transport.fuel", "Gasoline"),
                ("transport.auto", "Automotive"),
                ("transport.", "Travel/ Entertainment"),
                ("travel.", "Travel/ Entertainment"),
                ("entertainment.", "Travel/ Entertainment"),
                ("health.", "Medical Services"),
                ("housing.", "Home Improvement"),
                ("family.education", "Education"),
                ("taxes.", "Government Services"),
                ("financial.interest", "Interest"),
                ("financial.", "Fees"),
                ("utilities.", "Services"),
                ("insurance.", "Services"),
                ("services.", "Services"),
                ("", "Merchandise"),
            ],
            k,
        ),
    }
}

fn amex_category(key: Option<&str>, kind: Kind) -> &'static str {
    match (key, kind) {
        (None, Kind::CardPaymentIn) => "",
        (None, _) => "Fees & Adjustments-Fees & Adjustments",
        (Some(k), _) => lookup(
            &[
                ("food.groceries", "Merchandise & Supplies-Groceries"),
                ("food.", "Restaurant-Restaurant"),
                ("transport.fuel", "Transportation-Fuel"),
                ("transport.", "Transportation-Taxis & Coach"),
                ("travel.flights", "Travel-Airline"),
                ("travel.lodging", "Travel-Lodging"),
                ("travel.", "Travel-Travel Agencies"),
                ("entertainment.", "Entertainment-Associations"),
                ("utilities.", "Communications-Cable & Internet Comm"),
                ("health.", "Business Services-Health Care Services"),
                ("financial.", "Fees & Adjustments-Fees & Adjustments"),
                ("services.", "Business Services-Other Services"),
                ("", "Merchandise & Supplies-General Retail"),
            ],
            k,
        ),
    }
}

fn apple_category(key: Option<&str>, kind: Kind) -> &'static str {
    match (key, kind) {
        (None, Kind::CardPaymentIn) => "Payment",
        (None, Kind::InterestCharged) => "Interest",
        (None, _) => "Other",
        (Some(k), _) => lookup(
            &[
                ("food.groceries", "Grocery"),
                ("food.", "Restaurants"),
                ("transport.fuel", "Gas"),
                ("transport.", "Transportation"),
                ("travel.", "Travel"),
                ("entertainment.", "Entertainment"),
                ("health.", "Medical"),
                ("utilities.", "Utilities"),
                ("insurance.", "Insurance"),
                ("services.", "Other"),
                ("financial.", "Other"),
                ("", "Shopping"),
            ],
            k,
        ),
    }
}

fn usaa_category(key: Option<&str>, kind: Kind) -> &'static str {
    match (key, kind) {
        (None, Kind::CardPaymentOut) => "Credit Card Payment",
        (None, Kind::TransferIn | Kind::TransferOut) => "Transfer",
        (None, Kind::InterestEarned) => "Interest Income",
        (None, Kind::AchCredit) => "Paychecks/Salary",
        (None, _) => "Uncategorized",
        (Some(k), _) => lookup(
            &[
                ("food.groceries", "Groceries"),
                ("food.", "Restaurants/Dining"),
                ("transport.fuel", "Gasoline/Fuel"),
                ("transport.", "Automotive Expenses"),
                ("housing.rent", "Rent"),
                ("housing.mortgage", "Mortgages"),
                ("housing.", "Home Improvement"),
                ("utilities.", "Utilities"),
                ("insurance.", "Insurance"),
                ("health.", "Healthcare/Medical"),
                ("entertainment.", "Entertainment"),
                ("travel.", "Travel"),
                ("financial.cash", "ATM/Cash Withdrawals"),
                ("financial.", "Service Charges/Fees"),
                ("giving.", "Gifts"),
                ("taxes.", "Taxes"),
                ("", "General Merchandise"),
            ],
            k,
        ),
    }
}

fn nfcu_category(key: Option<&str>, kind: Kind) -> &'static str {
    match (key, kind) {
        (None, Kind::CardPaymentOut) => "Credit Card Payments",
        (None, Kind::TransferIn | Kind::TransferOut) => "Transfers",
        (None, Kind::InterestEarned) => "Interest Income",
        (None, Kind::AchCredit) => "Income",
        (None, _) => "Other",
        (Some(k), _) => lookup(
            &[
                ("food.groceries", "Groceries"),
                ("food.", "Restaurants"),
                ("transport.fuel", "Gas & Fuel"),
                ("transport.", "Auto & Transport"),
                ("housing.", "Home"),
                ("utilities.", "Utilities"),
                ("insurance.", "Insurance"),
                ("health.", "Health & Medical"),
                ("entertainment.", "Entertainment"),
                ("travel.", "Travel"),
                ("financial.", "Fees & Charges"),
                ("giving.", "Gifts & Donations"),
                ("taxes.", "Taxes"),
                ("", "Shopping"),
            ],
            k,
        ),
    }
}

/// Truist's two levels, reconstructed: our own group and category names.
fn truist_category(key: Option<&str>, kind: Kind) -> &'static str {
    match key {
        Some(k) => group_name(k).unwrap_or(""),
        None => match kind {
            Kind::AchCredit | Kind::InterestEarned => "Income",
            Kind::TransferIn | Kind::TransferOut | Kind::CardPaymentOut => "Transfers",
            _ => "Other",
        },
    }
}
