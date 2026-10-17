//! Matcher unit tests.

use super::*;
use TransactionStatus::{Pending, Posted};

fn d(s: &str) -> Date {
    s.parse().unwrap()
}

/// A row whose source gives only a posted date (an OFX row without DTUSER).
fn cand(status: TransactionStatus, date: &str, amount: i64, desc: &str) -> Candidate {
    Candidate {
        status,
        dates_given: DatesGiven::Posted,
        posted_date: d(date),
        transaction_date: None,
        amount_minor: amount,
        external_id: None,
        check_number: None,
        description: normalise(desc),
        memo: None,
        combined: None,
        same_import: false,
    }
}

/// Gives the row a purchase date too (both dates).
fn with_txn(mut c: Candidate, date: &str) -> Candidate {
    c.dates_given = DatesGiven::Both;
    c.transaction_date = Some(d(date));
    c
}

/// A row from a purchase-date-only layout: its single date is the purchase date.
fn bought(status: TransactionStatus, date: &str, amount: i64, desc: &str) -> Candidate {
    let mut c = cand(status, date, amount, desc);
    c.dates_given = DatesGiven::Purchase;
    c.transaction_date = Some(d(date));
    c
}

fn with_memo(mut c: Candidate, memo: &str) -> Candidate {
    c.memo = Some(normalise(memo));
    c.combined = Some(normalise(&format!("{} {memo}", c.description)));
    c
}

fn with_id(mut c: Candidate, id: &str) -> Candidate {
    c.external_id = Some(id.into());
    c
}

fn classify(existing: &[Candidate], records: &[Candidate]) -> Vec<Outcome> {
    let m = Matcher {
        existing: existing
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, c)| (TransactionId(i as i64 + 1), c))
            .collect(),
    };
    m.classify_all(records)
}

const T1: TransactionId = TransactionId(1);
const T2: TransactionId = TransactionId(2);

#[test]
fn normalising_and_comparing_texts() {
    assert_eq!(normalise("  sq *Daily   Grind, SF  "), "SQ DAILY GRIND SF");
    assert_eq!(
        text_match("SQ DAILY GRIND", "SQ DAILY GRIND"),
        Some(TextMatch::Exact)
    );
    // OFX NAME cut at 32 characters against the CSV's full text.
    assert_eq!(
        text_match(
            "AZ STATE RETIREMEN PENSION 2601",
            "AZ STATE RETIREMEN PENSION 26010 ACH"
        ),
        Some(TextMatch::Prefix)
    );
    assert_eq!(
        text_match("SQ", "SQ DAILY GRIND"),
        None,
        "too short a prefix"
    );
    assert_eq!(text_match("TRADER JOE S", "WHOLE FOODS"), None);
    assert_eq!(text_match("", ""), None);
}

#[test]
fn duplicates_across_exports_are_claimed_once_each() {
    let coffee = cand(Posted, "2026-01-02", -500, "COFFEE SHOP");
    let out = classify(
        &[coffee.clone(), coffee.clone()],
        &[coffee.clone(), coffee.clone(), coffee],
    );
    assert_eq!(
        out,
        [Outcome::Duplicate(T1), Outcome::Duplicate(T2), Outcome::New]
    );
}

#[test]
fn same_day_same_amount_different_merchant_is_not_a_duplicate() {
    let existing = [cand(Posted, "2026-01-02", -500, "COFFEE SHOP")];
    for rec in [
        cand(Posted, "2026-01-02", -500, "BOOK STORE"),
        cand(Posted, "2026-01-03", -500, "COFFEE SHOP"),
        cand(Posted, "2026-01-02", -501, "COFFEE SHOP"),
    ] {
        assert_eq!(
            classify(&existing, std::slice::from_ref(&rec)),
            [Outcome::New],
            "{rec:?}"
        );
    }
}

/// Review 1, item 1: a daily fare. Y was bought 01-30 and posted 01-31; X was bought
/// 01-31 and posted 02-01. They share the date 01-31, but as Y's posted date and X's
/// purchase date: two purchases, not one.
#[test]
fn daily_purchases_a_day_apart_are_not_merged() {
    let y = with_txn(
        cand(Posted, "2026-01-31", -290, "MTA NYCT PAYGO"),
        "2026-01-30",
    );
    let x = with_txn(
        cand(Posted, "2026-02-01", -290, "MTA NYCT PAYGO"),
        "2026-01-31",
    );
    assert_eq!(
        classify(std::slice::from_ref(&y), std::slice::from_ref(&x)),
        [Outcome::New]
    );
    assert_eq!(classify(&[x], &[y]), [Outcome::New]);
}

/// TD.4 case 01: two fares posted the same day but bought on different days are two
/// purchases.
#[test]
fn rows_with_both_dates_need_the_same_purchase_date() {
    let jan30 = with_txn(
        cand(Posted, "2026-02-01", -290, "MTA NYCT PAYGO"),
        "2026-01-30",
    );
    let jan31 = with_txn(
        cand(Posted, "2026-02-01", -290, "MTA NYCT PAYGO"),
        "2026-01-31",
    );
    assert_eq!(
        classify(std::slice::from_ref(&jan30), std::slice::from_ref(&jan31)),
        [Outcome::New]
    );
    // Re-downloaded, each finds its own twin whatever the order.
    assert_eq!(
        classify(&[jan30.clone(), jan31.clone()], &[jan31, jan30]),
        [Outcome::Duplicate(T2), Outcome::Duplicate(T1)]
    );
}

/// TD.4 case 07b: a posted-date-only export against a purchase-date-only one, two
/// days apart. No date is shared; the posting lag is.
#[test]
fn a_posted_only_row_matches_its_purchase_date_twin_within_the_lag() {
    let qfx = cand(Posted, "2026-07-04", -4410, "SHELL OIL 57444");
    let csv = bought(Posted, "2026-07-02", -4410, "SHELL OIL 57444");
    assert_eq!(
        classify(std::slice::from_ref(&qfx), std::slice::from_ref(&csv)),
        [Outcome::Duplicate(T1)]
    );
    assert_eq!(
        classify(std::slice::from_ref(&csv), std::slice::from_ref(&qfx)),
        [Outcome::Duplicate(T1)]
    );
    // Beyond the lag, or posted before the purchase: no.
    let late = cand(Posted, "2026-07-06", -4410, "SHELL OIL 57444");
    let early = cand(Posted, "2026-07-01", -4410, "SHELL OIL 57444");
    assert_eq!(
        classify(std::slice::from_ref(&csv), &[late]),
        [Outcome::New]
    );
    assert_eq!(classify(&[csv], &[early]), [Outcome::New]);
    // Two rows with only a posted date can't be lagged against each other.
    let a = cand(Posted, "2026-07-02", -4410, "SHELL OIL 57444");
    assert_eq!(classify(&[a], &[qfx]), [Outcome::New]);
}

/// TD.4 case 02: generic descriptions, merchant in the memo: the memos must agree.
#[test]
fn generic_descriptions_need_agreeing_memos() {
    let tj = with_memo(
        cand(Posted, "2026-03-03", -2345, "POS PURCHASE"),
        "TRADER JOE S 552",
    );
    let chevron = with_memo(
        cand(Posted, "2026-03-03", -2345, "POS PURCHASE"),
        "CHEVRON 0091",
    );
    assert_eq!(
        classify(std::slice::from_ref(&tj), std::slice::from_ref(&chevron)),
        [Outcome::New]
    );
    assert_eq!(
        classify(std::slice::from_ref(&tj), std::slice::from_ref(&tj)),
        [Outcome::Duplicate(T1)]
    );
    // A memo on one side only: the descriptions decide.
    let plain = cand(Posted, "2026-03-03", -2345, "TRADER JOE S 552");
    let with = with_memo(
        cand(Posted, "2026-03-03", -2345, "TRADER JOE S 552"),
        "ANYTHING",
    );
    assert_eq!(classify(&[plain], &[with]), [Outcome::Duplicate(T1)]);
}

/// TD.4 case 05: another statement of the same file matches only by bank id, as an
/// exact repeat; two identical ATM withdrawals without one stay two.
#[test]
fn rows_of_the_same_file_match_only_by_bank_id() {
    let mut energy = with_id(cand(Posted, "2026-04-30", -18230, "ENERGY CO"), "P0430A");
    energy.same_import = true;
    let mut atm = cand(Posted, "2026-05-02", -2000, "ATM WITHDRAWAL");
    atm.same_import = true;
    let out = classify(
        &[energy.clone(), atm.clone()],
        &[
            with_id(cand(Posted, "2026-04-30", -18230, "ENERGY CO"), "P0430A"),
            cand(Posted, "2026-05-02", -2000, "ATM WITHDRAWAL"),
        ],
    );
    assert_eq!(out, [Outcome::Duplicate(T1), Outcome::New]);
}

#[test]
fn a_transaction_date_shown_as_the_posted_date_still_matches() {
    // Chase: posted 01-01, bought 12-31. A layout with only a purchase-date column
    // reports 12-31 as the posted date, with no separate transaction date.
    let chase = with_txn(
        cand(Posted, "2026-01-01", -10604, "AMZN Mktp US*SRGMCJYOH"),
        "2025-12-31",
    );
    let other = bought(Posted, "2025-12-31", -10604, "AMZN Mktp US*SRGMCJYOH");
    assert_eq!(
        classify(std::slice::from_ref(&chase), std::slice::from_ref(&other)),
        [Outcome::Duplicate(T1)]
    );
    assert_eq!(
        classify(&[other], std::slice::from_ref(&chase)),
        [Outcome::Duplicate(T1)]
    );
    // Barclays gives both dates, both 12-31: the shared transaction date matches.
    let barclays = with_txn(
        cand(Posted, "2025-12-31", -10604, "AMZN Mktp US*SRGMCJYOH"),
        "2025-12-31",
    );
    assert_eq!(classify(&[chase], &[barclays]), [Outcome::Duplicate(T1)]);
}

#[test]
fn the_exact_posted_date_wins_over_a_cross_date_match() {
    let a = with_txn(
        cand(Posted, "2026-01-02", -290, "MTA NYCT PAYGO"),
        "2026-01-01",
    );
    let b = with_txn(
        cand(Posted, "2026-01-03", -290, "MTA NYCT PAYGO"),
        "2026-01-02",
    );
    // A row with only a posted date of 01-02 matches a by posted date and b by cross
    // date; a must win.
    let rec = cand(Posted, "2026-01-02", -290, "MTA NYCT PAYGO");
    assert_eq!(classify(&[b, a], &[rec]), [Outcome::Duplicate(T2)]);
}

/// Review 1, item 2: generic memos don't make different merchants agree, and a memo
/// is never compared with a description.
#[test]
fn generic_memos_do_not_merge_different_merchants() {
    let peets = with_memo(
        cand(Posted, "2026-01-02", -475, "PEETS COFFEE 0123"),
        "POS PURCHASE",
    );
    let starbucks = with_memo(
        cand(Posted, "2026-01-02", -475, "STARBUCKS STORE 4521"),
        "POS PURCHASE",
    );
    assert_eq!(
        classify(&[peets], std::slice::from_ref(&starbucks)),
        [Outcome::New]
    );
    // One row's memo equal to the other's description: not compared.
    let memo_says = with_memo(
        cand(Posted, "2026-01-02", -475, "ACH DEBIT"),
        "STARBUCKS STORE 4521",
    );
    assert_eq!(classify(&[memo_says], &[starbucks]), [Outcome::New]);
    // Memos decide only when a description is missing.
    let a = with_memo(cand(Posted, "2026-01-02", -475, ""), "STARBUCKS STORE 4521");
    let b = with_memo(cand(Posted, "2026-01-02", -475, ""), "STARBUCKS STORE 4521");
    assert_eq!(classify(&[a], &[b]), [Outcome::Duplicate(T1)]);
    // Exact text wins over a prefix match.
    let full = cand(Posted, "2026-01-02", -475, "STARBUCKS STORE 4521 SEATTLE");
    let exact = cand(Posted, "2026-01-02", -475, "STARBUCKS STORE 4521");
    assert_eq!(
        classify(&[full, exact.clone()], &[exact]),
        [Outcome::Duplicate(T2)]
    );
}

/// Review 1, item 3: a bank id alone doesn't make a match.
#[test]
fn a_reused_bank_id_needs_the_rest_to_agree() {
    let existing = [with_id(cand(Posted, "2026-01-02", -500, "COFFEE"), "FIT1")];
    // Same id and amount, two days apart: one transaction.
    let near = with_id(cand(Posted, "2026-01-04", -500, "COFFEE"), "FIT1");
    assert_eq!(classify(&existing, &[near]), [Outcome::Duplicate(T1)]);
    // Same id, another amount, or a month later: a different transaction.
    for rec in [
        with_id(cand(Posted, "2026-01-02", -999, "SOMETHING ELSE"), "FIT1"),
        with_id(cand(Posted, "2026-02-02", -500, "COFFEE"), "FIT1"),
    ] {
        assert_eq!(
            classify(&existing, std::slice::from_ref(&rec)),
            [Outcome::New],
            "{rec:?}"
        );
    }
    // Same id on a pending row: settles only if the settlement rules agree.
    let pending = [with_id(
        cand(Pending, "2025-06-15", -1000, "RAMEN HOUSE"),
        "ID",
    )];
    let settled = with_id(cand(Posted, "2025-06-17", -1200, "RAMEN HOUSE"), "ID");
    assert_eq!(classify(&pending, &[settled]), [Outcome::Posts(T1)]);
    let far = with_id(cand(Posted, "2025-07-17", -1200, "RAMEN HOUSE"), "ID");
    assert_eq!(classify(&pending, &[far]), [Outcome::New]);
}

#[test]
fn pending_settles_with_a_tip_a_hold_or_a_later_date() {
    for (pending_date, pending, posted_date, posted) in [
        ("2025-06-15", -2895, "2025-06-15", -3475),  // tip, +20%
        ("2025-06-15", -17500, "2025-06-16", -4210), // fuel hold, settled lower
        ("2026-03-13", -3206, "2026-03-15", -3206),  // posts two days later
        ("2026-03-13", -3206, "2026-03-20", -3206),  // the last day of the window
    ] {
        let p = cand(Pending, pending_date, pending, "SQ *SAKURA RAMEN");
        let rec = cand(Posted, posted_date, posted, "SQ *SAKURA RAMEN");
        assert_eq!(
            classify(&[p], &[rec]),
            [Outcome::Posts(T1)],
            "{pending} → {posted}"
        );
    }
}

#[test]
fn pending_does_not_settle_outside_the_window() {
    let p = cand(Pending, "2025-06-15", -1000, "SQ *SAKURA RAMEN");
    for rec in [
        cand(Posted, "2025-06-14", -1000, "SQ *SAKURA RAMEN"), // before it
        cand(Posted, "2025-06-23", -1000, "SQ *SAKURA RAMEN"), // 8 days later
        cand(Posted, "2025-06-16", -1400, "SQ *SAKURA RAMEN"), // +40%
        cand(Posted, "2025-06-16", -99, "SQ *SAKURA RAMEN"),   // under 10%
        cand(Posted, "2025-06-16", 1000, "SQ *SAKURA RAMEN"),  // a refund
        cand(Posted, "2025-06-16", -1000, "SOMEWHERE ELSE"),
    ] {
        assert_eq!(
            classify(std::slice::from_ref(&p), std::slice::from_ref(&rec)),
            [Outcome::New],
            "{rec:?}"
        );
    }
}

/// Review 1, item 8: the nearest settlement claims the pending transaction, whatever
/// the row order, rather than a later visit at the pending price.
#[test]
fn the_nearest_settlement_claims_the_pending_transaction() {
    let p = cand(Pending, "2025-06-15", -2895, "SQ *SAKURA RAMEN");
    let tipped = cand(Posted, "2025-06-15", -3475, "SQ *SAKURA RAMEN");
    let later_visit = cand(Posted, "2025-06-18", -2895, "SQ *SAKURA RAMEN");
    assert_eq!(
        classify(
            std::slice::from_ref(&p),
            &[tipped.clone(), later_visit.clone()]
        ),
        [Outcome::Posts(T1), Outcome::New]
    );
    assert_eq!(
        classify(&[p], &[later_visit, tipped]),
        [Outcome::New, Outcome::Posts(T1)]
    );
}

#[test]
fn a_pending_row_for_something_already_posted_is_dropped() {
    let posted = [cand(
        Posted,
        "2025-09-17",
        -985,
        "POS PURCHASE SQ COSMIC CUP",
    )];
    let rec = cand(Pending, "2025-09-15", -985, "POS PURCHASE SQ COSMIC CUP");
    assert_eq!(classify(&posted, &[rec]), [Outcome::AlreadyPosted(T1)]);
    // The same pending row seen in two mid-cycle downloads is a duplicate.
    let pending = [cand(Pending, "2025-06-16", -639, "SQ DAILY GRIND")];
    let again = cand(Pending, "2025-06-16", -639, "SQ DAILY GRIND");
    assert_eq!(classify(&pending, &[again]), [Outcome::Duplicate(T1)]);
}

/// Review 2, item 1: a Chase QFX (posted dates only) for February, then a Chase CSV
/// (purchase and posted dates) for March. The CSV's 02-28 purchase posted 03-01 is not
/// the QFX's ride posted 02-28.
#[test]
fn posted_dates_are_compared_with_posted_dates() {
    let qfx_feb28 = cand(Posted, "2026-02-28", -290, "MTA NYCT PAYGO");
    let csv_bought_feb28 = with_txn(
        cand(Posted, "2026-03-01", -290, "MTA NYCT PAYGO"),
        "2026-02-28",
    );
    assert_eq!(
        classify(
            std::slice::from_ref(&qfx_feb28),
            std::slice::from_ref(&csv_bought_feb28)
        ),
        [Outcome::New]
    );
    assert_eq!(classify(&[csv_bought_feb28], &[qfx_feb28]), [Outcome::New]);
    // The same posting in both formats does match: posted date against posted date.
    let csv = with_txn(
        cand(Posted, "2026-03-01", -290, "MTA NYCT PAYGO"),
        "2026-02-27",
    );
    let qfx = cand(Posted, "2026-03-01", -290, "MTA NYCT PAYGO");
    assert_eq!(classify(&[csv], &[qfx]), [Outcome::Duplicate(T1)]);
}

/// Review 2, item 1: a daily fare in a purchase-date-only CSV against a posted-only QFX
/// two days behind. Every row has several candidates within ±3 days, so none is paired.
#[test]
fn ambiguous_lagged_pairings_stay_new() {
    let qfx: Vec<Candidate> = ["2026-03-03", "2026-03-04", "2026-03-05"]
        .into_iter()
        .map(|day| cand(Posted, day, -290, "MTA NYCT PAYGO"))
        .collect();
    let csv: Vec<Candidate> = ["2026-03-01", "2026-03-02", "2026-03-03"]
        .into_iter()
        .map(|day| bought(Posted, day, -290, "MTA NYCT PAYGO"))
        .collect();
    assert!(classify(&qfx, &csv).iter().all(|o| *o == Outcome::New));
    assert!(classify(&csv, &qfx).iter().all(|o| *o == Outcome::New));
    // A lone pairing still matches (TD.4 07b).
    let one = [cand(Posted, "2026-03-05", -6215, "DINNER PLACE")];
    assert_eq!(
        classify(&one, &[bought(Posted, "2026-03-03", -6215, "DINNER PLACE")]),
        [Outcome::Duplicate(T1)]
    );
}

/// Review 2, item 2: a recycled short bank id on two different merchants.
#[test]
fn a_recycled_bank_id_needs_text_or_an_exact_date() {
    let atm = [with_id(
        cand(Posted, "2026-03-02", -2000, "ATM WITHDRAWAL 0042"),
        "77",
    )];
    let zelle = with_id(cand(Posted, "2026-03-04", -2000, "ZELLE TO PAT"), "77");
    assert_eq!(classify(&atm, &[zelle]), [Outcome::New]);
    // Same id and same day: one transaction even if the texts differ (a renamed merchant).
    let renamed = with_id(cand(Posted, "2026-03-02", -2000, "CASH WITHDRAWAL"), "77");
    assert_eq!(classify(&atm, &[renamed]), [Outcome::Duplicate(T1)]);
}

/// Review 2, item 3: a short generic text doesn't prefix-match a longer one.
#[test]
fn short_texts_must_match_exactly() {
    let generic = [with_memo(
        cand(Posted, "2026-03-03", -2345, "POS PURCHASE"),
        "TRADER JOE S 552",
    )];
    let chevron = cand(Posted, "2026-03-03", -2345, "POS PURCHASE CHEVRON 0091");
    assert_eq!(classify(&generic, &[chevron]), [Outcome::New]);
    assert_eq!(
        text_match("POS PURCHASE", "POS PURCHASE CHEVRON 0091"),
        None
    );
}

/// Review 2, item 5: check numbers decide when both rows have one.
#[test]
fn different_check_numbers_are_different_transactions() {
    let mut c1 = cand(Posted, "2026-03-03", -12000, "CHECK");
    c1.check_number = Some("1041".into());
    let mut c2 = c1.clone();
    c2.check_number = Some("1042".into());
    assert_eq!(classify(std::slice::from_ref(&c1), &[c2]), [Outcome::New]);
    assert_eq!(
        classify(std::slice::from_ref(&c1), &[c1.clone()]),
        [Outcome::Duplicate(T1)]
    );
}

/// Review 2, item 5: a pending row and its posted version in one statement.
#[test]
fn a_pending_row_settles_into_its_posted_twin_in_the_same_file() {
    let out = classify(
        &[],
        &[
            cand(Pending, "2026-03-06", -4000, "TST BLUE HERON BISTRO"),
            cand(Posted, "2026-03-06", -4800, "TST BLUE HERON BISTRO"),
            cand(Posted, "2026-03-09", -4000, "TST BLUE HERON BISTRO"),
        ],
    );
    assert_eq!(out, [Outcome::New, Outcome::SettlesRow(0), Outcome::New]);
    // Posted listed first: it is the transaction, and the pending row points at it.
    let out = classify(
        &[],
        &[
            cand(Posted, "2026-03-06", -4800, "TST BLUE HERON BISTRO"),
            cand(Pending, "2026-03-06", -4000, "TST BLUE HERON BISTRO"),
        ],
    );
    assert_eq!(out, [Outcome::New, Outcome::PendingOf(0)]);
}

/// TD.5 case 13: a QFX with a generic NAME and the merchant in MEMO against the same
/// bank's CSV, which prints them as one description.
#[test]
fn name_and_memo_together_match_a_csv_description() {
    let qfx = [with_memo(
        cand(Posted, "2026-03-12", -6120, "POS PURCHASE"),
        "H-E-B #475 AUSTIN TX",
    )];
    let csv = cand(
        Posted,
        "2026-03-12",
        -6120,
        "POS PURCHASE H-E-B #475 AUSTIN TX",
    );
    assert_eq!(
        classify(&qfx, std::slice::from_ref(&csv)),
        [Outcome::Duplicate(T1)]
    );
    // Another merchant behind the same generic NAME is not it.
    let chevron = cand(Posted, "2026-03-12", -6120, "POS PURCHASE CHEVRON 0091");
    assert_eq!(classify(&qfx, &[chevron]), [Outcome::New]);
}
