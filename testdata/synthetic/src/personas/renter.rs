//! A salaried single renter in a big city: card-heavy, lots of small purchases,
//! subscriptions that change, money split with friends.

use fm_core::model::AccountKind::{Checking, CreditCard, Savings};

use crate::cal::{add_days, weekday, ymd};
use crate::describe::Style;
use crate::sim::{
    Amount, Cadence, CardTerms, DraftBucket, EVENINGS_OUT, HOLIDAY_SEASON, Habit, Recurring,
    SavingsTerms, Sim, Via, WEEKDAYS, WEEKENDS,
};

pub const KEY: &str = "renter";
pub const DESCRIPTION: &str = "Alex Example, 29, a salaried product analyst renting a studio in \
San Francisco. Chase checking and Freedom card, Ally online savings with an automatic monthly \
transfer, an Amex for dining. Pays both cards in full. Splits dinners over Venmo and Zelle, \
changes subscriptions, buys a laptop and furniture, and visits New York.";

/// Utility bills: higher in winter (heating).
const WINTER: [i64; 12] = [100, 90, 70, 50, 30, 20, 15, 15, 20, 40, 70, 95];

pub fn build(seed: u64) -> Sim {
    let mut s = Sim::new(
        KEY,
        seed,
        crate::start(),
        crate::end(),
        "SAN FRANCISCO CA",
        "Alex Example",
    );
    s.nearby = vec!["OAKLAND CA", "DALY CITY CA", "BERKELEY CA"];
    let start = s.start;

    let chk = s.account(
        "checking",
        "Chase Total Checking",
        Checking,
        "Chase",
        "4821",
        Style::ChaseChecking,
    );
    let sav = s.account(
        "savings",
        "Ally Online Savings",
        Savings,
        "Ally Bank",
        "7730",
        Style::AllySavings,
    );
    let freedom = s.account(
        "freedom",
        "Chase Freedom Unlimited",
        CreditCard,
        "Chase",
        "9014",
        Style::ChaseCard,
    );
    let amex = s.account(
        "amex_gold",
        "American Express Gold Card",
        CreditCard,
        "American Express",
        "31007",
        Style::AmexCard,
    );
    s.card_terms(
        freedom,
        CardTerms {
            close_day: 12,
            pay_from: chk,
            due_days: 25,
            lag: (0, 0),
            partial_months: vec![],
            partial_pct: 100,
            apr_bp: 2149,
            foreign_fee_bp: 300,
        },
    );
    s.card_terms(
        amex,
        CardTerms {
            close_day: 3,
            pay_from: chk,
            due_days: 24,
            lag: (1, 2),
            partial_months: vec![],
            partial_pct: 100,
            apr_bp: 2399,
            foreign_fee_bp: 0,
        },
    );
    s.savings_terms(
        sav,
        SavingsTerms {
            opening: 800_000,
            apy: vec![
                (start, 425),
                (ymd(2025, 1, 15), 390),
                (ymd(2025, 10, 1), 360),
                (ymd(2026, 4, 1), 340),
            ],
        },
    );

    // Income, rent and bills.
    s.recurring(Recurring::new(
        "renter.payroll",
        "income",
        chk,
        "northwind",
        Cadence::Biweekly(ymd(2024, 10, 4)),
        Amount::Steps(vec![
            (start, 341_218),
            (ymd(2025, 4, 4), 365_540),
            (ymd(2026, 4, 3), 384_012),
        ]),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "renter.rent",
        "bill",
        chk,
        "bayview_rent",
        Cadence::Monthly(1),
        Amount::Steps(vec![(start, 285_000), (ymd(2025, 10, 1), 293_500)]),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "renter.electricity",
        "bill",
        chk,
        "pge",
        Cadence::Monthly(18),
        Amount::Seasonal(4_500, 11_000, WINTER),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "renter.internet",
        "bill",
        freedom,
        "comcast",
        Cadence::Monthly(9),
        Amount::Steps(vec![(start, 7_000), (ymd(2025, 10, 1), 8_500)]),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "renter.phone",
        "bill",
        freedom,
        "mint_mobile",
        Cadence::Months(&[11], 27),
        Amount::fixed(18_000),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "renter.renters_insurance",
        "bill",
        freedom,
        "lemonade",
        Cadence::Monthly(6),
        Amount::fixed(1_400),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "renter.student_loan",
        "bill",
        chk,
        "nelnet",
        Cadence::Monthly(15),
        Amount::fixed(31_250),
        Via::Ach,
    ));

    // Subscriptions: a price rise, a cancellation, a switch and a new one.
    s.recurring(Recurring::new(
        "renter.netflix",
        "subscription",
        freedom,
        "netflix",
        Cadence::Monthly(14),
        Amount::Steps(vec![(start, 1_549), (ymd(2025, 2, 1), 1_799)]),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "renter.spotify",
        "subscription",
        freedom,
        "spotify",
        Cadence::Monthly(22),
        Amount::Steps(vec![(start, 1_199), (ymd(2025, 7, 1), 1_299)]),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "renter.icloud",
        "subscription",
        freedom,
        "apple_icloud",
        Cadence::Monthly(20),
        Amount::fixed(299),
        Via::Card,
    ));
    s.recurring(
        Recurring::new(
            "renter.classpass",
            "subscription",
            amex,
            "classpass",
            Cadence::Monthly(2),
            Amount::fixed(4_900),
            Via::Card,
        )
        .to(ymd(2025, 8, 1)),
    );
    s.recurring(
        Recurring::new(
            "renter.equinox",
            "subscription",
            freedom,
            "equinox",
            Cadence::Monthly(5),
            Amount::fixed(26_000),
            Via::Card,
        )
        .from(ymd(2025, 9, 1)),
    );
    s.recurring(
        Recurring::new(
            "renter.max",
            "subscription",
            freedom,
            "max",
            Cadence::Monthly(17),
            Amount::Steps(vec![(start, 999), (ymd(2026, 6, 1), 1_099)]),
            Via::Card,
        )
        .from(ymd(2025, 6, 10)),
    );

    // Saving and investing.
    let auto = s.scheduled_transfer(
        "renter.savings_auto",
        chk,
        sav,
        Cadence::Monthly(5),
        Amount::Steps(vec![(start, 50_000), (ymd(2025, 5, 1), 75_000)]),
        (1, 3),
        None,
    );
    s.recurring(Recurring::new(
        "renter.robinhood",
        "transfer",
        chk,
        "robinhood",
        Cadence::Monthly(20),
        Amount::fixed(20_000),
        Via::Ach,
    ));

    // Everyday spending.
    s.habit(
        Habit::new(
            "coffee",
            &[(amex, 1)],
            &[("blue_bottle", 5), ("daily_grind", 4), ("starbucks", 2)],
            4.0,
        )
        .days(WEEKDAYS),
    );
    s.habit(Habit::new(
        "groceries",
        &[(freedom, 3), (chk, 1)],
        &[
            ("trader_joes", 6),
            ("safeway", 3),
            ("whole_foods", 2),
            ("instacart", 1),
        ],
        1.6,
    ));
    s.habit(
        Habit::new(
            "restaurants",
            &[(amex, 1)],
            &[
                ("blue_heron", 2),
                ("casa_verde", 4),
                ("sakura_ramen", 3),
                ("golden_lotus", 2),
                ("olive_thyme", 1),
            ],
            1.8,
        )
        .days(EVENINGS_OUT),
    );
    s.habit(
        Habit::new(
            "bars",
            &[(amex, 1)],
            &[("rusty_anchor", 2), ("hopworks", 3)],
            0.8,
        )
        .days([0, 0, 1, 2, 4, 5, 1]),
    );
    s.habit(Habit::new(
        "fast_food",
        &[(freedom, 1)],
        &[("in_n_out", 2), ("chipotle", 3), ("subway", 1)],
        0.7,
    ));
    s.habit(
        Habit::new(
            "delivery",
            &[(freedom, 1)],
            &[("doordash", 3), ("uber_eats", 2)],
            1.0,
        )
        .days(EVENINGS_OUT),
    );
    s.habit(
        Habit::new(
            "rideshare",
            &[(freedom, 1)],
            &[("uber", 3), ("lyft", 2)],
            1.3,
        )
        .days(EVENINGS_OUT),
    );
    s.habit(Habit::new("transit", &[(freedom, 1)], &[("clipper", 1)], 0.35).days(WEEKDAYS));
    s.habit(Habit::new("parking", &[(freedom, 1)], &[("sfmta", 1)], 0.3).days(WEEKENDS));
    s.habit(
        Habit::new(
            "shopping",
            &[(freedom, 1)],
            &[("amazon", 6), ("target", 2)],
            1.1,
        )
        .season(HOLIDAY_SEASON),
    );
    s.habit(
        Habit::new(
            "clothing",
            &[(freedom, 1)],
            &[("uniqlo", 2), ("nordstrom_rack", 2)],
            0.15,
        )
        .season(HOLIDAY_SEASON),
    );
    s.habit(Habit::new(
        "pharmacy",
        &[(freedom, 1)],
        &[("walgreens", 3), ("cvs", 1)],
        0.3,
    ));
    s.habit(Habit::new("hair", &[(freedom, 1)], &[("luxe_hair", 1)], 0.22).days(WEEKENDS));
    s.habit(Habit::new("laundry", &[(chk, 1)], &[("fluff_fold", 1)], 0.5).days(WEEKENDS));
    s.habit(
        Habit::new(
            "events",
            &[(freedom, 1)],
            &[("ticketmaster", 1), ("amc", 2)],
            0.15,
        )
        .days(EVENINGS_OUT),
    );
    s.habit(Habit::new("games", &[(freedom, 1)], &[("steam", 1)], 0.08));
    s.habit(Habit::new(
        "books",
        &[(freedom, 1)],
        &[("barnes_noble", 1)],
        0.06,
    ));
    s.habit(Habit::new("doctor", &[(freedom, 1)], &[("bayside_medical", 1)], 0.06).days(WEEKDAYS));
    s.habit(Habit::new("massage", &[(freedom, 1)], &[("massage_envy", 1)], 0.06).days(WEEKENDS));

    // Cash, and money with friends.
    let friends = s.people("friends", 6);
    let mut r = s.rng("p2p");
    for d in s.days() {
        let weekend_night = weekday(d) >= 4;
        if r.chance(if weekend_night { 0.12 } else { 0.02 }) {
            let f = *r.pick(&friends);
            s.venmo(chk, d, r.around(1_500, 6_000), f, "food.restaurants");
        }
        if r.chance(if weekend_night { 0.07 } else { 0.02 }) {
            // A friend pays back their share of a dinner Alex covered.
            let f = *r.pick(&friends);
            let i = s.zelle(
                chk,
                d,
                r.around(1_500, 5_500),
                f,
                false,
                DraftBucket::Refund("food.restaurants", None),
            );
            s.draft(i).tags.push("reimbursement");
        }
        if r.chance(0.025) {
            let fee = r.chance(0.3).then_some(300);
            s.atm(chk, d, *r.pick(&[4_000, 6_000, 10_000]), fee);
        }
    }
    for (d, f) in [
        (ymd(2025, 3, 2), 0),
        (ymd(2025, 9, 14), 1),
        (ymd(2026, 5, 3), 2),
    ] {
        let i = s.zelle(
            chk,
            d,
            r.around(8_000, 16_000),
            friends[f],
            true,
            DraftBucket::Category("entertainment.events"),
        );
        s.draft(i).tags.push("concert_tickets");
    }
    for (d, f) in [
        (ymd(2025, 1, 18), 3),
        (ymd(2025, 7, 9), 4),
        (ymd(2026, 2, 21), 5),
    ] {
        s.paypal(
            freedom,
            d,
            r.around(4_000, 9_000),
            friends[f],
            "giving.gifts",
        );
    }
    // Near misses: a friend is paid the same amount, the same day, as the savings
    // transfer. Not a transfer.
    for &o in auto.iter().skip(4).step_by(7).take(3) {
        let (date, cents) = (s.drafts()[o].date, -s.drafts()[o].amount);
        let i = s.zelle(
            chk,
            date,
            cents,
            friends[r.below(6) as usize],
            true,
            DraftBucket::Category("travel.lodging"),
        );
        s.draft(i).tags.push("near_miss_transfer");
    }

    // One-offs.
    let laptop = s.buy(freedom, "apple_store", ymd(2025, 2, 8), 219_900);
    s.draft(laptop).one_off = true;
    s.transfer(
        sav,
        chk,
        ymd(2025, 2, 10),
        200_000,
        2,
        crate::truth::PairKind::OwnTransfer,
    );
    let sofa = s.buy(freedom, "ikea", ymd(2025, 6, 14), 124_037);
    s.draft(sofa).one_off = true;
    let crown = s.buy(freedom, "bright_smile", ymd(2026, 3, 11), 115_000);
    s.draft(crown).one_off = true;
    for (d, cents) in [(ymd(2025, 3, 14), 650_000), (ymd(2026, 3, 13), 720_000)] {
        let i = s.ach(chk, "northwind", d, -cents);
        s.draft(i).one_off = true;
        s.draft(i).tags.push("bonus");
    }
    for (d, cents) in [(ymd(2025, 4, 2), 84_217), (ymd(2026, 3, 27), 61_540)] {
        s.ach(chk, "irs_refund", d, -cents);
    }
    s.buy(freedom, "ca_dmv", ymd(2025, 8, 19), 18_700);
    for d in [
        ymd(2025, 2, 10),
        ymd(2025, 5, 8),
        ymd(2026, 2, 9),
        ymd(2026, 5, 7),
    ] {
        let c = r.around(6_000, 11_000);
        s.buy(freedom, "flowers", d, c);
    }
    for d in [ymd(2024, 12, 3), ymd(2025, 12, 2)] {
        s.buy(freedom, "wikimedia", d, 5_000);
    }

    // A long weekend in New York.
    let flight = s.buy(freedom, "united", ymd(2025, 9, 30), 48_760);
    s.draft(flight).one_off = true;
    let trip_start = ymd(2025, 11, 20);
    for day in 0..5 {
        let d = add_days(trip_start, day);
        for _ in 0..2 {
            let m = *r.pick(&["nyc_deli", "nyc_pizza"]);
            let c = r.around(1_200, 6_500);
            let i = s.buy(amex, m, d, c);
            s.draft(i).tags.push("trip");
        }
        for _ in 0..r.range(1, 4) {
            let i = s.buy(freedom, "mta_nyc", d, 290);
            s.draft(i).tags.push("trip");
        }
    }
    let hotel = s.buy_at(
        freedom,
        "hilton",
        add_days(trip_start, 4),
        104_532,
        "NEW YORK NY",
    );
    s.draft(hotel).one_off = true;
    s.draft(hotel).tags.push("trip");

    s
}
