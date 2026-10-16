//! A self-employed designer: irregular income, tax set-asides, quarterly estimated
//! taxes, software subscriptions, a card balance carried through lean months, a trip
//! abroad paid in euros.

use fm_core::model::AccountKind::{Checking, CreditCard, Savings};

use crate::cal::{add_days, months, ymd};
use crate::describe::Style;
use crate::sim::{
    Amount, Cadence, CardTerms, DraftBucket, EVENINGS_OUT, HOLIDAY_SEASON, Habit, Recurring,
    SavingsTerms, Sim, Via, WEEKDAYS,
};
use crate::truth::PairKind;

pub const KEY: &str = "freelancer";
pub const DESCRIPTION: &str = "Sam Fakerson, 36, a freelance UX designer renting in Austin, Texas. \
Credit union checking and savings, a Capital One Venture and a Chase Freedom Flex. One retainer \
client plus irregular project payments; sets aside a share of each large payment for tax and pays \
quarterly estimates. Carries a card balance through lean months and pays interest. Software \
subscriptions come and go. Ten days in Lisbon paid partly in euros.";

const SUMMER: [i64; 12] = [25, 20, 25, 40, 65, 90, 100, 100, 80, 45, 25, 25];

/// Months with little project work.
const LEAN: [(i32, u8); 4] = [(2025, 1), (2025, 2), (2025, 8), (2026, 1)];

pub fn build(seed: u64) -> Sim {
    let mut s = Sim::new(
        KEY,
        seed,
        crate::start(),
        crate::end(),
        "AUSTIN TX",
        "Sam Fakerson",
    );
    s.nearby = vec!["ROUND ROCK TX", "PFLUGERVILLE TX", "CEDAR PARK TX"];
    let start = s.start;

    let chk = s.account(
        "checking",
        "Share Draft Checking",
        Checking,
        "Lakeline Federal Credit Union",
        "0193",
        Style::CreditUnion,
    );
    let sav = s.account(
        "savings",
        "Regular Share Savings",
        Savings,
        "Lakeline Federal Credit Union",
        "0101",
        Style::CreditUnion,
    );
    let venture = s.account(
        "venture",
        "Capital One Venture",
        CreditCard,
        "Capital One",
        "8834",
        Style::CapOneCard,
    );
    let flex = s.account(
        "freedom_flex",
        "Chase Freedom Flex",
        CreditCard,
        "Chase",
        "5521",
        Style::ChaseCard,
    );
    s.card_terms(
        venture,
        CardTerms {
            close_day: 17,
            pay_from: chk,
            due_days: 25,
            lag: (1, 2),
            partial_months: LEAN.to_vec(),
            partial_pct: 35,
            apr_bp: 2399,
            foreign_fee_bp: 0,
        },
    );
    s.card_terms(
        flex,
        CardTerms {
            close_day: 26,
            pay_from: chk,
            due_days: 22,
            lag: (1, 3),
            partial_months: vec![],
            partial_pct: 100,
            apr_bp: 2049,
            foreign_fee_bp: 300,
        },
    );
    s.savings_terms(
        sav,
        SavingsTerms {
            opening: 1_200_000,
            apy: vec![(start, 75), (ymd(2025, 9, 1), 60)],
        },
    );

    // Income: a retainer, then project payments of all sizes, fewer in lean months.
    let retainer = s.recurring(Recurring::new(
        "freelancer.retainer",
        "income",
        chk,
        "tailspin",
        Cadence::Monthly(5),
        Amount::Steps(vec![(start, 420_000), (ymd(2026, 1, 1), 460_000)]),
        Via::Ach,
    ));
    let mut r = s.rng("projects");
    let mut payments = retainer;
    for (y, m) in months(s.start, s.end) {
        let n = if LEAN.contains(&(y, m)) {
            r.range(0, 1)
        } else {
            r.range(1, 2)
        };
        for _ in 0..n {
            let d = crate::cal::business_on_or_after(crate::cal::day_of_month(
                y,
                m,
                r.range(2, 27) as u8,
            ));
            if d > s.end {
                continue;
            }
            let client = *r.pick_weighted(&[("wide_world", 3), ("stripe", 4), ("upwork", 2)]);
            let cents = r.around(60_000, 600_000) / 100 * 100;
            payments.push(s.ach(chk, client, d, -cents));
        }
    }
    // A quarter of every large payment goes to savings for tax.
    for i in payments {
        let (date, cents) = (s.drafts()[i].posted, s.drafts()[i].amount);
        if cents >= 300_000 && date <= add_days(s.end, -5) {
            let lag = r.range(0, 2) as i32;
            let (o, n) = s.transfer(
                chk,
                sav,
                add_days(date, lag),
                cents / 4 / 100 * 100,
                0,
                PairKind::OwnTransfer,
            );
            s.draft(o).tags.push("tax_set_aside");
            s.draft(n).tags.push("tax_set_aside");
        }
    }

    // Quarterly estimated tax, funded from savings the day before. The savings-to-checking
    // transfer and the tax payment have the same amount on the same account: not a pair.
    let mut t = s.rng("taxes");
    for date in Cadence::Months(&[1, 4, 6, 9], 15).dates(s.start, s.end) {
        let d = crate::cal::business_on_or_after(date);
        let cents = t.around(280_000, 450_000) / 100 * 100;
        s.transfer(sav, chk, add_days(d, -1), cents, 0, PairKind::OwnTransfer);
        let i = s.ach(chk, "irs_payment", d, cents);
        s.draft(i).recurring = Some("freelancer.estimated_tax".into());
        s.draft(i).tags.push("near_miss_transfer");
    }
    s.declare(
        "freelancer.estimated_tax",
        "bill",
        "IRS",
        chk,
        "quarterly",
        false,
        false,
    );
    let balance_due = s.ach(chk, "irs_payment", ymd(2025, 4, 15), 124_000);
    s.draft(balance_due).one_off = true;
    for (d, c) in [(ymd(2025, 4, 10), 600_000), (ymd(2026, 4, 10), 650_000)] {
        let i = s.ach(chk, "vanguard", d, c);
        s.draft(i).tags.push("sep_ira");
    }

    // Rent and bills.
    s.recurring(Recurring::new(
        "freelancer.rent",
        "bill",
        chk,
        "appfolio_rent",
        Cadence::Monthly(1),
        Amount::Steps(vec![(start, 187_500), (ymd(2025, 9, 1), 194_000)]),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "freelancer.electricity",
        "bill",
        chk,
        "austin_energy",
        Cadence::Monthly(8),
        Amount::Seasonal(9_000, 29_000, SUMMER),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "freelancer.internet",
        "bill",
        venture,
        "google_fiber",
        Cadence::Monthly(12),
        Amount::fixed(7_000),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "freelancer.phone",
        "bill",
        venture,
        "tmobile",
        Cadence::Monthly(21),
        Amount::Steps(vec![(start, 6_500), (ymd(2025, 4, 1), 7_000)]),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "freelancer.health_insurance",
        "bill",
        chk,
        "bcbs_tx",
        Cadence::Monthly(1),
        Amount::Steps(vec![(start, 51_240), (ymd(2026, 1, 1), 54_890)]),
        Via::Ach,
    ));
    s.recurring(
        Recurring::new(
            "freelancer.coworking",
            "subscription",
            venture,
            "capital_factory",
            Cadence::Monthly(1),
            Amount::fixed(25_000),
            Via::Card,
        )
        .from(ymd(2025, 3, 1)),
    );

    // Software: price rises, annual plans, a cancellation, a cancel-and-restart.
    s.recurring(Recurring::new(
        "freelancer.adobe",
        "subscription",
        flex,
        "adobe",
        Cadence::Monthly(6),
        Amount::Steps(vec![(start, 5_999), (ymd(2025, 6, 1), 6_999)]),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "freelancer.figma",
        "subscription",
        flex,
        "figma",
        Cadence::Months(&[3], 3),
        Amount::fixed(14_400),
        Via::Card,
    ));
    s.recurring(
        Recurring::new(
            "freelancer.github",
            "subscription",
            flex,
            "github",
            Cadence::Monthly(9),
            Amount::fixed(400),
            Via::Card,
        )
        .to(ymd(2025, 9, 30)),
    );
    s.recurring(Recurring::new(
        "freelancer.notion",
        "subscription",
        flex,
        "notion",
        Cadence::Monthly(13),
        Amount::fixed(1_000),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "freelancer.dropbox",
        "subscription",
        venture,
        "dropbox",
        Cadence::Months(&[7], 18),
        Amount::fixed(11_988),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "freelancer.google_workspace",
        "subscription",
        flex,
        "google_workspace",
        Cadence::Monthly(1),
        Amount::Steps(vec![(start, 720), (ymd(2025, 3, 1), 840)]),
        Via::Card,
    ));
    s.recurring(
        Recurring::new(
            "freelancer.zoom",
            "subscription",
            flex,
            "zoom",
            Cadence::Monthly(22),
            Amount::fixed(1_599),
            Via::Card,
        )
        .to(ymd(2025, 4, 30)),
    );
    s.recurring(
        Recurring::new(
            "freelancer.zoom_restart",
            "subscription",
            flex,
            "zoom",
            Cadence::Monthly(22),
            Amount::fixed(1_599),
            Via::Card,
        )
        .from(ymd(2026, 1, 1)),
    );
    s.recurring(
        Recurring::new(
            "freelancer.coursera",
            "subscription",
            venture,
            "coursera",
            Cadence::Monthly(28),
            Amount::fixed(4_900),
            Via::Card,
        )
        .from(ymd(2025, 10, 1))
        .to(ymd(2026, 3, 31)),
    );

    // Everyday spending.
    s.habit(
        Habit::new(
            "coffee",
            &[(venture, 3), (chk, 1)],
            &[("cosmic_cup", 4), ("starbucks", 1)],
            2.5,
        )
        .days(WEEKDAYS),
    );
    s.habit(Habit::new(
        "groceries",
        &[(venture, 2), (chk, 1)],
        &[("heb", 5), ("whole_foods", 2)],
        1.5,
    ));
    s.habit(
        Habit::new(
            "restaurants",
            &[(flex, 1)],
            &[("lone_star_bbq", 3), ("casa_verde", 2), ("golden_lotus", 1)],
            1.2,
        )
        .days(EVENINGS_OUT),
    );
    s.habit(Habit::new(
        "fast_food",
        &[(venture, 1)],
        &[("taco_bell", 2), ("chick_fil_a", 2), ("subway", 1)],
        0.8,
    ));
    s.habit(
        Habit::new("bars", &[(flex, 1)], &[("hopworks", 1), ("luckys", 2)], 0.6)
            .days([0, 0, 1, 2, 4, 5, 1]),
    );
    s.habit(Habit::new("rideshare", &[(venture, 1)], &[("uber", 1)], 0.4).days(EVENINGS_OUT));
    s.habit(Habit::new(
        "fuel",
        &[(venture, 1)],
        &[("shell", 1), ("chevron", 1)],
        0.7,
    ));
    s.habit(
        Habit::new(
            "shopping",
            &[(venture, 1)],
            &[("amazon", 5), ("target", 1)],
            0.9,
        )
        .season(HOLIDAY_SEASON),
    );
    s.habit(Habit::new(
        "electronics",
        &[(venture, 1)],
        &[("best_buy", 1)],
        0.04,
    ));
    s.habit(Habit::new("outdoors", &[(venture, 1)], &[("rei", 1)], 0.08));
    s.habit(Habit::new("parking", &[(venture, 1)], &[("parkmobile", 1)], 0.3).days(WEEKDAYS));
    s.habit(
        Habit::new(
            "shipping",
            &[(chk, 1)],
            &[("usps", 2), ("ups_store", 1)],
            0.25,
        )
        .days(WEEKDAYS),
    );
    s.habit(Habit::new(
        "yoga",
        &[(venture, 1)],
        &[("black_swan_yoga", 1)],
        0.6,
    ));
    s.habit(Habit::new(
        "haircuts",
        &[(venture, 1)],
        &[("great_clips", 1)],
        0.2,
    ));

    let friends = s.people("friends", 4);
    let mut p = s.rng("p2p");
    for d in s.days() {
        if p.chance(0.03) {
            let fee = p.chance(0.5).then_some(350);
            s.atm(chk, d, *p.pick(&[4_000, 6_000, 8_000]), fee);
        }
        if p.chance(0.015) {
            let f = *p.pick(&friends);
            s.zelle(
                chk,
                d,
                p.around(2_000, 7_000),
                f,
                true,
                DraftBucket::Category("food.restaurants"),
            );
        }
    }
    for (d, f) in [
        (ymd(2025, 6, 7), 0),
        (ymd(2025, 12, 13), 1),
        (ymd(2026, 6, 20), 2),
    ] {
        s.paypal(
            venture,
            d,
            p.around(3_000, 8_000),
            friends[f],
            "giving.gifts",
        );
    }
    for d in [ymd(2025, 3, 28), ymd(2026, 3, 30)] {
        let c = p.around(6_000, 11_000);
        s.buy(flex, "hr_block", d, c);
    }

    // One-offs.
    let laptop = s.buy(venture, "apple_store", ymd(2025, 9, 3), 284_900);
    s.draft(laptop).one_off = true;
    let tires = s.buy(venture, "discount_tire", ymd(2026, 1, 20), 89_012);
    s.draft(tires).one_off = true;
    let dentist = s.buy(venture, "bright_smile", ymd(2025, 11, 12), 68_000);
    s.draft(dentist).one_off = true;

    // Lisbon, May 2025: flights in dollars, the flat in euros, daily spending in euros on
    // the Chase card (which adds a 3% foreign transaction fee line to each purchase).
    const EUR_MARCH: i64 = 1_084_000;
    const EUR_MAY: i64 = 1_124_000;
    let flights = s.buy(flex, "tap_air", ymd(2025, 3, 2), 114_060);
    s.draft(flights).one_off = true;
    let flat = s.buy_foreign(venture, "airbnb", ymd(2025, 3, 5), 98_000, "EUR", EUR_MARCH);
    s.draft(flat).one_off = true;
    s.draft(flat).tags.push("trip");
    let mut e = s.rng("lisbon");
    let arrive = ymd(2025, 5, 14);
    for day in 0..10 {
        let d = add_days(arrive, day);
        for m in [
            "pastelaria",
            "taberna_flores",
            "time_out_market",
            "metro_lisboa",
        ] {
            if e.chance(0.7) {
                let (lo, hi) = crate::catalog::merchant(m).price;
                let i = s.buy_foreign(flex, m, d, e.around(lo, hi), "EUR", EUR_MAY);
                s.draft(i).tags.push("trip");
            }
        }
        if e.chance(0.3) {
            let i = s.buy_foreign(flex, "pingo_doce", d, e.around(800, 4_500), "EUR", EUR_MAY);
            s.draft(i).tags.push("trip");
        }
    }

    s
}
