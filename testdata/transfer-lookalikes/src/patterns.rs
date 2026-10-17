//! One function per scenario. Each writes the rows of one occurrence: true transfers are
//! declared as pairs, look-alike rows are grouped under their pattern name.

// Scenario builders take the accounts, date, amount and wording of one occurrence
// directly; bundling them into structs would only move the same arguments around.
#![allow(clippy::too_many_arguments)]

use fm_core::model::Date;
use fm_synth::cal::add_days;
use fm_synth::describe::{self, Style};
use fm_synth::rng::Rng;

use crate::book::{Book, Bucket};

/// The card issuer's ACH identity on a bank statement, and the card's own payment line.
fn issuer(b: &Book, card: usize) -> (&'static str, &'static str, &'static str) {
    match b.accounts[card].institution {
        "Chase" => ("CHASE CREDIT CRD", "EPAY", "AUTOMATIC PAYMENT - THANK"),
        "American Express" => ("AMERICAN EXPRESS", "ACH PMT", "AUTOPAY PAYMENT - THANK YOU"),
        "Capital One" => ("CAPITAL ONE", "MOBILE PMT", "CAPITAL ONE MOBILE PYMT"),
        "Citi" => ("CITI CARD ONLINE", "PAYMENT", "ONLINE PAYMENT, THANK YOU"),
        _ => (
            "DISCOVER E-PAYMENT",
            "PAYMENT",
            "INTERNET PAYMENT - THANK YOU",
        ),
    }
}

/// A bank-side ACH line, or a plain text for styles without an ACH format.
fn ach(
    b: &Book,
    acct: usize,
    company: &str,
    entry: &str,
    credit: bool,
    d: Date,
    r: &mut Rng,
) -> String {
    let v = b.view(acct);
    match v.style {
        Style::AllySavings => format!(
            "{} {company} {entry}",
            if credit {
                "ACH Deposit"
            } else {
                "ACH Withdrawal"
            }
        ),
        s if s.is_card() => format!("{company} {entry}"),
        _ => describe::ach(&v, company, entry, credit, d, r),
    }
}

/// A checking account pays a card in full: a true card payment.
pub fn card_payment(
    b: &mut Book,
    r: &mut Rng,
    chk: usize,
    card: usize,
    d: Date,
    cents: i64,
    lag: i32,
) -> (usize, usize) {
    let (company, entry, on_card) = issuer(b, card);
    let cv = b.view(chk);
    let out_desc = if cv.style == Style::ChaseChecking && b.accounts[card].institution == "Chase" {
        format!(
            "Payment to Chase card ending in {} {}",
            b.view(card).last4,
            fm_synth::cal::mm_dd(d)
        )
    } else {
        ach(b, chk, company, entry, false, d, r)
    };
    let o = b.add(
        chk,
        d,
        -cents,
        out_desc,
        Bucket::Transfer,
        "card_payment",
        None,
    );
    let i = b.add(
        card,
        add_days(d, lag),
        cents,
        on_card,
        Bucket::Transfer,
        "card_payment",
        None,
    );
    b.pair(o, i, "card_payment", "card_payment", "strong");
    (o, i)
}

/// Money between two own bank accounts, worded the way the banks word it.
pub fn own_transfer(
    b: &mut Book,
    r: &mut Rng,
    from: usize,
    to: usize,
    d: Date,
    cents: i64,
    lag: i32,
    pattern: &'static str,
) -> (usize, usize) {
    let (fv, tv) = (b.view(from), b.view(to));
    let same_bank = b.accounts[from].institution == b.accounts[to].institution;
    let (out_desc, in_desc) = if same_bank && fv.style != Style::AllySavings {
        (
            describe::internal_transfer(&fv, &tv, true, d, r),
            describe::internal_transfer(&tv, &fv, false, d, r),
        )
    } else {
        let side = |b: &Book, r: &mut Rng, me: usize, other: usize, out: bool| {
            if b.view(me).style == Style::AllySavings {
                describe::external_savings_side(&b.view(other), out)
            } else {
                let company = b.accounts[other].institution.to_uppercase();
                ach(b, me, &company, "TRANSFER", !out, d, r)
            }
        };
        (side(b, r, from, to, true), side(b, r, to, from, false))
    };
    let o = b.add(from, d, -cents, out_desc, Bucket::Transfer, pattern, None);
    let i = b.add(
        to,
        add_days(d, lag),
        cents,
        in_desc,
        Bucket::Transfer,
        pattern,
        None,
    );
    b.pair(o, i, "own_transfer", pattern, "strong");
    (o, i)
}

/// A true transfer with transfer wording on the money-out side only.
pub fn weak_one_sided(
    b: &mut Book,
    r: &mut Rng,
    from: usize,
    to: usize,
    d: Date,
    cents: i64,
    lag: i32,
) {
    let o = b.add(
        from,
        d,
        -cents,
        format!("ONLINE TRANSFER REF #IB0{}", r.alnum(7)),
        Bucket::Transfer,
        "weak_one_sided",
        None,
    );
    let i = b.add(
        to,
        add_days(d, lag),
        cents,
        format!("DEPOSIT {}", r.digits(6)),
        Bucket::Transfer,
        "weak_one_sided",
        None,
    );
    b.pair(o, i, "own_transfer", "weak_one_sided", "one_sided");
}

/// A true transfer with no transfer wording and no account names on either side.
pub fn weak_none(b: &mut Book, r: &mut Rng, from: usize, to: usize, d: Date, cents: i64, lag: i32) {
    let o = b.add(
        from,
        d,
        -cents,
        format!("WITHDRAWAL #{}", r.digits(7)),
        Bucket::Transfer,
        "weak_none",
        None,
    );
    let i = b.add(
        to,
        add_days(d, lag),
        cents,
        format!("DEPOSIT #{}", r.digits(7)),
        Bucket::Transfer,
        "weak_none",
        None,
    );
    b.pair(o, i, "own_transfer", "weak_none", "none");
}

/// A transfer started before a long weekend that lands 4–5 days later.
pub fn long_weekend(
    b: &mut Book,
    r: &mut Rng,
    from: usize,
    to: usize,
    d: Date,
    cents: i64,
    lag: i32,
) {
    own_transfer(b, r, from, to, d, cents, lag, "long_weekend");
}

/// Two equal scheduled transfers two days apart whose landings cross: the first lands
/// later than the second. Both are true pairs; the crossed pairings are look-alikes.
pub fn equal_recurring(b: &mut Book, r: &mut Rng, from: usize, to: usize, d: Date, cents: i64) {
    let (o1, i1) = own_transfer(b, r, from, to, d, cents, 4, "equal_recurring");
    let (o2, i2) = own_transfer(b, r, from, to, add_days(d, 2), cents, 0, "equal_recurring");
    b.group("equal_recurring_crossed", &[o1, i1, o2, i2]);
}

/// A card refund days after an unrelated bill of the same amount from checking.
pub fn refund_vs_bill(
    b: &mut Book,
    r: &mut Rng,
    chk: usize,
    card: usize,
    d: Date,
    cents: i64,
    lag: i32,
    biller: (&str, &str, &'static str, &str),
    store: (&str, &'static str, &str),
) {
    let (company, entry, key, name) = biller;
    let bill = ach(b, chk, company, entry, false, d, r);
    let o = b.add(
        chk,
        d,
        -cents,
        bill,
        Bucket::Category(key),
        "card_refund_vs_bill",
        Some(name),
    );
    let (desc, rkey, rname) = store;
    let i = b.add(
        card,
        add_days(d, lag),
        cents,
        desc,
        Bucket::Refund(rkey),
        "card_refund_vs_bill",
        Some(rname),
    );
    b.group("card_refund_vs_bill", &[o, i]);
}

/// A deposit into checking a day after the same amount left savings for an outside
/// brokerage account (a one-sided transfer to an account not in the ledger).
pub fn deposit_vs_savings_outflow(
    b: &mut Book,
    r: &mut Rng,
    sav: usize,
    chk: usize,
    d: Date,
    cents: i64,
) {
    let o_desc = ach(b, sav, "VANGUARD", "BUY INVESTMENT", false, d, r);
    let o = b.add(
        sav,
        d,
        -cents,
        o_desc,
        Bucket::Transfer,
        "deposit_vs_savings_outflow",
        Some("Vanguard"),
    );
    let i_desc = ach(b, chk, "IRS TREAS 310", "TAX REF", true, add_days(d, 1), r);
    let i = b.add(
        chk,
        add_days(d, 1),
        cents,
        i_desc,
        Bucket::Refund("taxes.income"),
        "deposit_vs_savings_outflow",
        Some("IRS"),
    );
    b.group("deposit_vs_savings_outflow", &[o, i]);
}

/// A savings move and a bill of the same amount, the same day, from the same checking.
pub fn bill_vs_savings_move(
    b: &mut Book,
    r: &mut Rng,
    chk: usize,
    sav: usize,
    d: Date,
    cents: i64,
    biller: (&str, &str, &'static str, &str),
) {
    let (o, i) = own_transfer(b, r, chk, sav, d, cents, 1, "savings_move");
    let (company, entry, key, name) = biller;
    let bill_desc = ach(b, chk, company, entry, false, d, r);
    let bill = b.add(
        chk,
        d,
        -cents,
        bill_desc,
        Bucket::Category(key),
        "bill_vs_savings_move",
        Some(name),
    );
    b.group("bill_vs_savings_move", &[o, i, bill]);
}

/// Zelle out of one own account to a friend, and Zelle into another own account from a
/// different friend, same amount, a day or two apart.
pub fn zelle_out_in(
    b: &mut Book,
    r: &mut Rng,
    from: usize,
    to: usize,
    d: Date,
    cents: i64,
    out_to: (&str, &str),
    in_from: (&str, &str),
) {
    let o_desc = describe::zelle(&b.view(from), out_to.0, out_to.1, true, d, r);
    let o = b.add(
        from,
        d,
        -cents,
        o_desc,
        Bucket::Category("food.restaurants"),
        "zelle_out_vs_zelle_in",
        Some(&format!("{} {}", out_to.0, out_to.1)),
    );
    let id = add_days(d, 1);
    let i_desc = describe::zelle(&b.view(to), in_from.0, in_from.1, false, id, r);
    let i = b.add(
        to,
        id,
        cents,
        i_desc,
        Bucket::Refund("food.restaurants"),
        "zelle_out_vs_zelle_in",
        Some(&format!("{} {}", in_from.0, in_from.1)),
    );
    b.group("zelle_out_vs_zelle_in", &[o, i]);
}

/// A roommate pays back half the rent into a second own account (by plain ACH or by
/// Zelle) on the day the same amount moves from checking to savings.
pub fn rent_split(
    b: &mut Book,
    r: &mut Rng,
    chk: usize,
    chk2: usize,
    sav: usize,
    d: Date,
    half: i64,
    roommate: (&str, &str),
    by_zelle: bool,
) {
    let rent = ach(b, chk, "BAYVIEW PROP MGMT", "RENT PMT", false, d, r);
    b.add(
        chk,
        d,
        -2 * half,
        rent,
        Bucket::Category("housing.rent"),
        "rent_split_repayment",
        Some("Bayview Property Management"),
    );
    let back = if by_zelle {
        describe::zelle(&b.view(chk2), roommate.0, roommate.1, false, d, r)
    } else {
        let company = format!(
            "{} {}",
            roommate.0.to_uppercase(),
            roommate.1.to_uppercase()
        );
        ach(b, chk2, &company, "RENT SHARE", true, d, r)
    };
    let i = b.add(
        chk2,
        d,
        half,
        back,
        Bucket::Refund("housing.rent"),
        "rent_split_repayment",
        Some(&format!("{} {}", roommate.0, roommate.1)),
    );
    let (o, si) = own_transfer(b, r, chk, sav, d, half, 1, "savings_move");
    b.group("rent_split_repayment", &[i, o, si]);
}

/// A bill whose bank line says PMT, paid the same day and from the same checking as an
/// equal savings move whose out side says only ONLINE TRANSFER. Both candidates carry
/// transfer wording and the savings side names the checking account, so the evidence
/// ties; the bill is written first.
pub fn pmt_bill_vs_savings_move(
    b: &mut Book,
    r: &mut Rng,
    chk: usize,
    sav: usize,
    d: Date,
    cents: i64,
    biller: (&str, &'static str, &str),
) {
    let (company, key, name) = biller;
    let bill_desc = ach(b, chk, company, "WEB PMT", false, d, r);
    let bill = b.add(
        chk,
        d,
        -cents,
        bill_desc,
        Bucket::Category(key),
        "pmt_bill_vs_savings_move",
        Some(name),
    );
    let o = b.add(
        chk,
        d,
        -cents,
        format!("ONLINE TRANSFER REF #IB0{}", r.alnum(7)),
        Bucket::Transfer,
        "savings_move",
        None,
    );
    let in_desc = describe::internal_transfer(&b.view(sav), &b.view(chk), false, d, r);
    let i = b.add(
        sav,
        d,
        cents,
        in_desc,
        Bucket::Transfer,
        "savings_move",
        None,
    );
    b.pair(o, i, "own_transfer", "savings_move", "strong");
    b.group("pmt_bill_vs_savings_move", &[bill, o, i]);
}

/// A paycheck lands the day the same amount moves from savings into the same checking.
pub fn payroll_vs_equal_transfer(
    b: &mut Book,
    r: &mut Rng,
    sav: usize,
    chk: usize,
    d: Date,
    cents: i64,
    employer: (&str, &str, &str),
) {
    let (company, entry, name) = employer;
    let pay_desc = ach(b, chk, company, entry, true, d, r);
    let pay = b.add(
        chk,
        d,
        cents,
        pay_desc,
        Bucket::Income,
        "payroll_vs_equal_transfer",
        Some(name),
    );
    let (o, i) = own_transfer(b, r, sav, chk, d, cents, 0, "savings_move");
    b.group("payroll_vs_equal_transfer", &[pay, o, i]);
}

/// A card purchase and a checking deposit of the same amount the same day.
pub fn card_charge_vs_deposit(
    b: &mut Book,
    r: &mut Rng,
    card: usize,
    chk: usize,
    d: Date,
    cents: i64,
    store: (&str, &'static str, &str),
) {
    let (desc, key, name) = store;
    let o = b.add(
        card,
        d,
        -cents,
        desc,
        Bucket::Category(key),
        "card_charge_vs_deposit",
        Some(name),
    );
    let i = b.add(
        chk,
        d,
        cents,
        format!("MOBILE DEPOSIT REF NUMBER {}", r.digits(9)),
        Bucket::Income,
        "card_charge_vs_deposit",
        None,
    );
    b.group("card_charge_vs_deposit", &[o, i]);
}

/// A balance transfer from card `from_card` to card `to_card`, on the day checking pays
/// both cards the same amount. Three true pairs; every crossed pairing is a look-alike.
pub fn balance_transfer(
    b: &mut Book,
    r: &mut Rng,
    chk: usize,
    from_card: usize,
    to_card: usize,
    d: Date,
    cents: i64,
) {
    let (p1o, p1i) = card_payment(b, r, chk, from_card, d, cents, 0);
    let (p2o, p2i) = card_payment(b, r, chk, to_card, d, cents, 0);
    let from_name = b.accounts[from_card].institution.to_uppercase();
    let o = b.add(
        to_card,
        d,
        -cents,
        format!("BALANCE TRANSFER - {from_name}"),
        Bucket::Transfer,
        "balance_transfer",
        None,
    );
    let i = b.add(
        from_card,
        d,
        cents,
        "BALANCE TRANSFER CREDIT".to_string(),
        Bucket::Transfer,
        "balance_transfer",
        None,
    );
    b.pair(o, i, "card_to_card", "balance_transfer", "strong");
    b.group("balance_transfer_vs_payments", &[p1o, p1i, p2o, p2i, o, i]);
}

/// Everyday rows that make the ledger look like a real one (cents amounts, so they
/// rarely collide with the scenarios).
pub fn background(b: &mut Book, r: &mut Rng, accounts: &[usize], start: Date, end: Date) {
    const SHOPS: &[(&str, &str, &str)] = &[
        ("TRADER JOE S #552", "food.groceries", "Trader Joe's"),
        ("SQ *COSMIC CUP COFFEE", "food.coffee", "Cosmic Cup Coffee"),
        ("SHELL OIL 57444", "transport.fuel", "Shell"),
        ("AMZN Mktp US*2K4L19XZ0", "shopping.general", "Amazon"),
        ("CHIPOTLE 1262", "food.fast_food", "Chipotle"),
        ("TARGET 00012345", "shopping.general", "Target"),
        ("WALGREENS #1234", "health.pharmacy", "Walgreens"),
    ];
    let mut d = start;
    while d <= end {
        if r.chance(0.6) {
            let acct = *r.pick(accounts);
            let (desc, key, name) = *r.pick(SHOPS);
            let cents = r.range(301, 14_999);
            b.add(
                acct,
                d,
                -cents,
                desc,
                Bucket::Category(key),
                "background",
                Some(name),
            );
        }
        d = add_days(d, 1);
    }
}
