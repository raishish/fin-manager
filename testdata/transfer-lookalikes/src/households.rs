//! The three households: each a year (2025) of scripted scenarios over everyday spending.

use fm_core::model::AccountKind::{Checking, CreditCard, Savings};
use fm_synth::cal::{day_of_month, ymd};
use fm_synth::describe::Style;
use fm_synth::rng::Rng;

use crate::book::Book;
use crate::patterns::*;

pub struct Household {
    pub key: &'static str,
    pub description: &'static str,
    pub build: fn(u64) -> Book,
}

pub static ALL: &[Household] = &[
    Household {
        key: "alex",
        description: "Renter with Chase checking and card, an Amex, Ally savings and a second checking at Bank of \
                      America. Equal scheduled savings transfers that cross, rent split with a roommate, Zelle both \
                      ways, card refunds next to equal bills, weakly worded and long-weekend transfers.",
        build: alex,
    },
    Household {
        key: "jordan",
        description: "Family with Bank of America checking and savings, a Capital One and a Citi card, and a credit \
                      union checking. Two balance transfers on days both cards are paid the same amount, bills equal \
                      to savings moves, deposits equal to brokerage outflows, transfers with no wording.",
        build: jordan,
    },
    Household {
        key: "pat",
        description: "Retiree with Wells Fargo checking and savings, Ally savings, an Amex and a Discover card. Pension \
                      days with an equal savings withdrawal, card charges equal to deposits, crossing equal transfers \
                      to Ally, one-sided wording and long-weekend landings.",
        build: pat,
    },
];

fn rng(seed: u64, key: &str) -> Rng {
    Rng::derive(seed, &format!("transfer-lookalikes/{key}"))
}

fn dollars(r: &mut Rng, lo: i64, hi: i64) -> i64 {
    r.range(lo, hi) * 100
}

const MONTHS: std::ops::RangeInclusive<u8> = 1..=12;

fn alex(seed: u64) -> Book {
    let mut b = Book::new("alex", "Alex Example");
    let mut r = rng(seed, "alex");
    let chk = b.account(
        "checking",
        "Chase Total Checking",
        Checking,
        "Chase",
        "4821",
        Style::ChaseChecking,
    );
    let sav = b.account(
        "savings",
        "Ally Online Savings",
        Savings,
        "Ally Bank",
        "7730",
        Style::AllySavings,
    );
    let chk2 = b.account(
        "checking2",
        "BofA Advantage Checking",
        Checking,
        "Bank of America",
        "2291",
        Style::BofaChecking,
    );
    let card = b.account(
        "freedom",
        "Chase Freedom Unlimited",
        CreditCard,
        "Chase",
        "9014",
        Style::ChaseCard,
    );
    let amex = b.account(
        "amex",
        "American Express Gold Card",
        CreditCard,
        "American Express",
        "31007",
        Style::AmexCard,
    );
    let people = [
        ("Riley", "Notareal"),
        ("Quinn", "Placeholder"),
        ("Morgan", "Sampleby"),
        ("Casey", "Mockridge"),
    ];
    for m in MONTHS {
        let day = |d: u8| day_of_month(2025, m, d);
        let pay = r.range(40_000, 180_000);
        card_payment(&mut b, &mut r, chk, card, day(6), pay, 0);
        let pay = r.range(20_000, 90_000);
        card_payment(&mut b, &mut r, chk, amex, day(27), pay, 1 + (m % 2) as i32);
        equal_recurring(&mut b, &mut r, chk, sav, day(1), 25_000);
        rent_split(
            &mut b,
            &mut r,
            chk,
            chk2,
            sav,
            day(3),
            142_500,
            people[0],
            m % 2 == 0,
        );
        let amt = r.range(5_000, 20_000);
        refund_vs_bill(
            &mut b,
            &mut r,
            chk,
            card,
            day(12),
            amt,
            1 + (m % 3) as i32,
            ("COMCAST", "BILL PAY", "utilities.internet", "Xfinity"),
            ("AMZN MKTP US RETURN", "shopping.general", "Amazon"),
        );
        let amt = dollars(&mut r, 30, 120);
        zelle_out_in(
            &mut b,
            &mut r,
            chk,
            chk2,
            day(18),
            amt,
            people[1 + (m as usize % 3)],
            people[1 + ((m as usize + 1) % 3)],
        );
        let amt = dollars(&mut r, 20, 90);
        card_charge_vs_deposit(
            &mut b,
            &mut r,
            card,
            chk,
            day(15),
            amt,
            ("SFMTA PAYBYPHONE", "transport.parking_tolls", "SFMTA"),
        );
        if m % 3 == 0 {
            let amt = dollars(&mut r, 200, 900);
            weak_one_sided(&mut b, &mut r, chk, chk2, day(20), amt, 1);
        }
        if m % 4 == 2 {
            let amt = dollars(&mut r, 100, 600);
            weak_none(&mut b, &mut r, chk2, sav, day(9), amt, 2);
        }
        if m % 3 == 1 {
            payroll_vs_equal_transfer(
                &mut b,
                &mut r,
                sav,
                chk,
                day(24),
                341_218,
                ("NORTHWIND ANALYT", "PAYROLL", "Northwind Analytics"),
            );
        }
    }
    for (d, lag) in [
        (ymd(2025, 1, 17), 4),
        (ymd(2025, 5, 23), 4),
        (ymd(2025, 8, 29), 4),
    ] {
        let amt = dollars(&mut r, 300, 1200);
        long_weekend(&mut b, &mut r, chk, chk2, d, amt, lag);
    }
    background(
        &mut b,
        &mut r,
        &[chk, card, amex, chk2],
        ymd(2025, 1, 1),
        ymd(2025, 12, 31),
    );
    b
}

fn jordan(seed: u64) -> Book {
    let mut b = Book::new("jordan", "Jordan Testworth");
    let mut r = rng(seed, "jordan");
    let chk = b.account(
        "checking",
        "Joint Checking",
        Checking,
        "Bank of America",
        "2290",
        Style::BofaChecking,
    );
    let sav = b.account(
        "savings",
        "Advantage Savings",
        Savings,
        "Bank of America",
        "2307",
        Style::BofaChecking,
    );
    let capone = b.account(
        "savor",
        "Capital One Savor",
        CreditCard,
        "Capital One",
        "6612",
        Style::CapOneCard,
    );
    let citi = b.account(
        "citi",
        "Citi Double Cash",
        CreditCard,
        "Citi",
        "4409",
        Style::CapOneCard,
    );
    let cu = b.account(
        "cu_checking",
        "Share Draft Checking",
        Checking,
        "Lakeline Federal Credit Union",
        "0193",
        Style::CreditUnion,
    );
    let people = [
        ("Harper", "Pretendley"),
        ("Jamie", "Exampleton"),
        ("Reese", "Dummyfield"),
        ("Sage", "Lorem"),
    ];
    for m in MONTHS {
        let day = |d: u8| day_of_month(2025, m, d);
        if m == 3 || m == 9 {
            // Balance transfer days: both cards paid the same amount as the transfer.
            let amt = dollars(&mut r, 800, 2500);
            balance_transfer(&mut b, &mut r, chk, capone, citi, day(14), amt);
        }
        let pay = r.range(60_000, 240_000);
        card_payment(&mut b, &mut r, chk, capone, day(22), pay, (m % 2) as i32);
        let pay = r.range(30_000, 120_000);
        card_payment(&mut b, &mut r, chk, citi, day(10), pay, 1);
        own_transfer(&mut b, &mut r, chk, sav, day(2), 40_000, 0, "savings_move");
        let amt = 7_500 + i64::from(m) * 311;
        pmt_bill_vs_savings_move(
            &mut b,
            &mut r,
            chk,
            sav,
            day(26),
            amt,
            ("SPECTRUM", "utilities.internet", "Spectrum"),
        );
        let ins = 24_118 + i64::from(m) * 100;
        bill_vs_savings_move(
            &mut b,
            &mut r,
            chk,
            sav,
            day(12),
            ins,
            ("STATE FARM RO 27", "SFPP", "insurance.auto", "State Farm"),
        );
        let amt = r.range(4_000, 16_000);
        refund_vs_bill(
            &mut b,
            &mut r,
            chk,
            capone,
            day(7),
            amt,
            2,
            ("AEP OHIO", "BILL PAY", "utilities.electricity", "AEP Ohio"),
            ("TARGET 00012345 RETURN", "shopping.general", "Target"),
        );
        if m % 2 == 0 {
            let amt = dollars(&mut r, 300, 1500);
            deposit_vs_savings_outflow(&mut b, &mut r, sav, chk, day(16), amt);
        }
        let amt = dollars(&mut r, 40, 140);
        zelle_out_in(
            &mut b,
            &mut r,
            cu,
            chk,
            day(19),
            amt,
            people[m as usize % 4],
            people[(m as usize + 2) % 4],
        );
        if m % 2 == 1 {
            let amt = dollars(&mut r, 150, 700);
            weak_none(&mut b, &mut r, chk, cu, day(25), amt, 1);
        }
        equal_recurring(&mut b, &mut r, cu, sav, day(4), 15_000);
        if m % 3 == 2 {
            payroll_vs_equal_transfer(
                &mut b,
                &mut r,
                sav,
                chk,
                day(15),
                216_849,
                ("FABRIKAM ISD", "PAYROLL", "Fabrikam Schools"),
            );
        }
    }
    for (d, lag) in [
        (ymd(2025, 7, 3), 4),
        (ymd(2025, 10, 10), 4),
        (ymd(2025, 11, 26), 5),
    ] {
        let amt = dollars(&mut r, 200, 900);
        long_weekend(&mut b, &mut r, chk, cu, d, amt, lag);
    }
    background(
        &mut b,
        &mut r,
        &[chk, capone, citi, cu],
        ymd(2025, 1, 1),
        ymd(2025, 12, 31),
    );
    b
}

fn pat(seed: u64) -> Book {
    let mut b = Book::new("pat", "Pat Exampleton");
    let mut r = rng(seed, "pat");
    let chk = b.account(
        "checking",
        "Everyday Checking",
        Checking,
        "Wells Fargo",
        "6170",
        Style::WellsChecking,
    );
    let sav = b.account(
        "savings",
        "Way2Save Savings",
        Savings,
        "Wells Fargo",
        "6189",
        Style::WellsChecking,
    );
    let ally = b.account(
        "ally",
        "Ally Online Savings",
        Savings,
        "Ally Bank",
        "5510",
        Style::AllySavings,
    );
    let amex = b.account(
        "blue_cash",
        "Amex Blue Cash Preferred",
        CreditCard,
        "American Express",
        "41008",
        Style::AmexCard,
    );
    let disc = b.account(
        "discover",
        "Discover it Cash Back",
        CreditCard,
        "Discover",
        "1188",
        Style::CapOneCard,
    );
    for m in MONTHS {
        let day = |d: u8| day_of_month(2025, m, d);
        let pay = r.range(30_000, 150_000);
        card_payment(&mut b, &mut r, chk, amex, day(9), pay, 1 + (m % 2) as i32);
        let pay = r.range(10_000, 60_000);
        card_payment(&mut b, &mut r, chk, disc, day(25), pay, 0);
        own_transfer(&mut b, &mut r, chk, sav, day(20), 30_000, 0, "savings_move");
        equal_recurring(&mut b, &mut r, chk, ally, day(5), 50_000);
        let amt = r.range(3_000, 12_000);
        refund_vs_bill(
            &mut b,
            &mut r,
            chk,
            amex,
            day(14),
            amt,
            1,
            (
                "TUCSON ELECTRIC",
                "TEP PAYMT",
                "utilities.electricity",
                "Tucson Electric Power",
            ),
            ("WALGREENS #1234 REFUND", "health.pharmacy", "Walgreens"),
        );
        let tax = 14_123 + i64::from(m) * 37;
        bill_vs_savings_move(
            &mut b,
            &mut r,
            chk,
            sav,
            day(26),
            tax,
            (
                "CITY OF TUCSON",
                "WATER BILL",
                "utilities.water_trash",
                "Tucson Water",
            ),
        );
        let amt = dollars(&mut r, 25, 80);
        card_charge_vs_deposit(
            &mut b,
            &mut r,
            disc,
            chk,
            day(11),
            amt,
            (
                "TUCSON BOTANICAL GARD",
                "entertainment.events",
                "Tucson Botanical Gardens",
            ),
        );
        if m % 2 == 1 {
            let amt = dollars(&mut r, 500, 2000);
            deposit_vs_savings_outflow(&mut b, &mut r, ally, chk, day(17), amt);
        }
        if m % 3 == 0 {
            let amt = dollars(&mut r, 250, 1000);
            weak_one_sided(&mut b, &mut r, sav, chk, day(3), amt, 0);
        }
        let amt = 9_000 + i64::from(m) * 213;
        pmt_bill_vs_savings_move(
            &mut b,
            &mut r,
            chk,
            sav,
            day(8),
            amt,
            (
                "COX COMMUNICATIONS",
                "utilities.internet",
                "Cox Communications",
            ),
        );
        if m % 3 == 1 {
            payroll_vs_equal_transfer(
                &mut b,
                &mut r,
                sav,
                chk,
                day(2),
                192_044,
                (
                    "AZ STATE RETIREMEN",
                    "PENSION",
                    "Arizona State Retirement System",
                ),
            );
        }
    }
    for (d, lag) in [
        (ymd(2025, 2, 14), 4),
        (ymd(2025, 5, 23), 4),
        (ymd(2025, 12, 24), 5),
    ] {
        let amt = dollars(&mut r, 400, 1500);
        long_weekend(&mut b, &mut r, ally, chk, d, amt, lag);
    }
    background(
        &mut b,
        &mut r,
        &[chk, amex, disc],
        ymd(2025, 1, 1),
        ymd(2025, 12, 31),
    );
    b
}
