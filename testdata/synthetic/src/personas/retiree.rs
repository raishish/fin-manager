//! A retired homeowner: Social Security and a pension, few but large bills, paper
//! checks, medical costs, a trip to Japan paid in yen, a new roof.

use fm_core::model::AccountKind::{Checking, CreditCard, Savings};

use crate::cal::{add_days, ymd};
use crate::describe::Style;
use crate::sim::{
    Amount, Cadence, CardTerms, DraftBucket, EVENINGS_OUT, GARDEN_SEASON, HOLIDAY_SEASON, Habit,
    Recurring, SavingsTerms, Sim, Via, WEEKDAYS,
};
use crate::truth::PairKind;

pub const KEY: &str = "retiree";
pub const DESCRIPTION: &str = "Pat Exampleton, 71, a retired teacher who owns a paid-off house in \
Tucson, Arizona. Wells Fargo checking and savings, an Amex Blue Cash Preferred and a Capital One \
Quicksilver, both paid in full. Social Security (with yearly cost-of-living rises) and a pension; \
a yearly IRA withdrawal from an account outside the ledger. Property tax twice a year, insurance \
paid every six or twelve months, checks for church and a house cleaner. Two weeks in Japan paid in \
yen, partly on the card that charges a foreign transaction fee. A new roof and hearing aids.";

const SUMMER: [i64; 12] = [15, 15, 20, 35, 60, 90, 100, 100, 85, 45, 20, 15];
const WINTER: [i64; 12] = [100, 85, 55, 30, 15, 5, 5, 5, 5, 20, 55, 95];

pub fn build(seed: u64) -> Sim {
    let mut s = Sim::new(
        KEY,
        seed,
        crate::start(),
        crate::end(),
        "TUCSON AZ",
        "Pat Exampleton",
    );
    s.nearby = vec!["ORO VALLEY AZ", "MARANA AZ"];
    let start = s.start;

    let chk = s.account(
        "checking",
        "Everyday Checking",
        Checking,
        "Wells Fargo",
        "6170",
        Style::WellsChecking,
    );
    let sav = s.account(
        "savings",
        "Way2Save Savings",
        Savings,
        "Wells Fargo",
        "6189",
        Style::WellsChecking,
    );
    let amex = s.account(
        "blue_cash",
        "Amex Blue Cash Preferred",
        CreditCard,
        "American Express",
        "41008",
        Style::AmexCard,
    );
    let quick = s.account(
        "quicksilver",
        "Capital One Quicksilver",
        CreditCard,
        "Capital One",
        "3378",
        Style::CapOneCard,
    );
    s.card_terms(
        amex,
        CardTerms {
            close_day: 15,
            pay_from: chk,
            due_days: 25,
            lag: (1, 2),
            partial_months: vec![],
            partial_pct: 100,
            apr_bp: 2124,
            foreign_fee_bp: 270,
        },
    );
    s.card_terms(
        quick,
        CardTerms {
            close_day: 5,
            pay_from: chk,
            due_days: 25,
            lag: (0, 1),
            partial_months: vec![],
            partial_pct: 100,
            apr_bp: 2574,
            foreign_fee_bp: 0,
        },
    );
    s.savings_terms(
        sav,
        SavingsTerms {
            opening: 6_200_000,
            apy: vec![(start, 1)],
        },
    );

    // Income.
    s.recurring(Recurring::new(
        "retiree.social_security",
        "income",
        chk,
        "ssa",
        Cadence::NthWeekday(2, 2),
        Amount::Steps(vec![
            (start, 218_400),
            (ymd(2025, 1, 1), 223_860),
            (ymd(2026, 1, 1), 229_500),
        ]),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "retiree.pension",
        "income",
        chk,
        "asrs",
        Cadence::Monthly(1),
        Amount::fixed(192_044),
        Via::Ach,
    ));
    s.recurring(
        Recurring::new(
            "retiree.ira_withdrawal",
            "transfer",
            chk,
            "fidelity",
            Cadence::Months(&[12], 10),
            Amount::Steps(vec![(start, 840_000), (ymd(2025, 12, 1), 880_000)]),
            Via::Ach,
        )
        .inflow(),
    );

    // House, bills and insurance.
    s.recurring(Recurring::new(
        "retiree.property_tax",
        "bill",
        chk,
        "pima_county",
        Cadence::Months(&[3, 10], 25),
        Amount::Steps(vec![(start, 141_230), (ymd(2025, 10, 1), 146_870)]),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "retiree.electricity",
        "bill",
        chk,
        "tep",
        Cadence::Monthly(14),
        Amount::Seasonal(8_000, 32_000, SUMMER),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "retiree.gas",
        "bill",
        chk,
        "southwest_gas",
        Cadence::Monthly(22),
        Amount::Seasonal(2_500, 9_500, WINTER),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "retiree.water",
        "bill",
        chk,
        "tucson_water",
        Cadence::Monthly(26),
        Amount::Range(5_500, 9_500),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "retiree.internet",
        "bill",
        amex,
        "cox",
        Cadence::Monthly(9),
        Amount::Steps(vec![(start, 6_499), (ymd(2026, 2, 1), 6_999)]),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "retiree.phone",
        "bill",
        amex,
        "att_wireless",
        Cadence::Monthly(23),
        Amount::fixed(5_500),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "retiree.medigap",
        "bill",
        chk,
        "uhc_medigap",
        Cadence::Monthly(1),
        Amount::Steps(vec![(start, 19_845), (ymd(2026, 1, 1), 21_420)]),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "retiree.car_insurance",
        "bill",
        chk,
        "progressive",
        Cadence::Months(&[2, 8], 12),
        Amount::Steps(vec![(start, 61_200), (ymd(2025, 8, 1), 64_800)]),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "retiree.home_insurance",
        "bill",
        chk,
        "state_farm_home",
        Cadence::Months(&[9], 15),
        Amount::Steps(vec![(start, 148_000), (ymd(2026, 9, 1), 156_500)]),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "retiree.cleaning",
        "bill",
        chk,
        "sunny_days",
        Cadence::Biweekly(ymd(2024, 10, 3)),
        Amount::fixed(12_000),
        Via::Check,
    ));
    s.recurring(Recurring::new(
        "retiree.church",
        "bill",
        chk,
        "st_marks",
        Cadence::NthWeekday(6, 2),
        Amount::fixed(20_000),
        Via::Check,
    ));

    // Subscriptions and giving. Apple Music and iCloud bill under the same descriptor on
    // the same day; only the amount tells them apart.
    s.recurring(Recurring::new(
        "retiree.netflix",
        "subscription",
        amex,
        "netflix",
        Cadence::Monthly(18),
        Amount::Steps(vec![(start, 699), (ymd(2025, 2, 1), 799)]),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "retiree.apple_music",
        "subscription",
        amex,
        "apple_music",
        Cadence::Monthly(12),
        Amount::fixed(1_099),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "retiree.icloud",
        "subscription",
        amex,
        "apple_icloud",
        Cadence::Monthly(12),
        Amount::fixed(99),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "retiree.gardens",
        "subscription",
        amex,
        "botanical_gardens",
        Cadence::Months(&[2], 1),
        Amount::fixed(9_500),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "retiree.msf",
        "subscription",
        amex,
        "msf",
        Cadence::Monthly(5),
        Amount::fixed(2_500),
        Via::Card,
    ));
    for d in [ymd(2024, 12, 6), ymd(2025, 12, 5)] {
        s.buy(amex, "red_cross", d, 25_000);
    }

    // Savings: small monthly top-up.
    s.scheduled_transfer(
        "retiree.savings_auto",
        chk,
        sav,
        Cadence::Monthly(20),
        Amount::fixed(30_000),
        (0, 0),
        None,
    );

    // Everyday spending.
    s.habit(Habit::new(
        "groceries",
        &[(amex, 1)],
        &[("frys", 5), ("sprouts", 2)],
        1.6,
    ));
    s.habit(Habit::new("costco", &[(amex, 1)], &[("costco", 1)], 0.25).scale(70));
    s.habit(
        Habit::new(
            "restaurants",
            &[(quick, 2), (amex, 1)],
            &[("saguaro_grill", 2), ("mesquite_diner", 3)],
            1.0,
        )
        .days(EVENINGS_OUT),
    );
    s.habit(Habit::new(
        "coffee",
        &[(quick, 1)],
        &[("dutch_bros", 1)],
        0.6,
    ));
    s.habit(Habit::new(
        "pharmacy",
        &[(amex, 1)],
        &[("walgreens", 2), ("cvs", 1)],
        0.7,
    ));
    s.habit(
        Habit::new(
            "cardiology",
            &[(quick, 1)],
            &[("desert_sky_cardio", 1)],
            0.1,
        )
        .days(WEEKDAYS),
    );
    s.habit(Habit::new("labs", &[(quick, 1)], &[("quest", 1)], 0.05).days(WEEKDAYS));
    s.habit(Habit::new("fuel", &[(amex, 1)], &[("chevron", 1)], 0.7));
    s.habit(
        Habit::new(
            "home",
            &[(quick, 1)],
            &[("ace_hardware", 2), ("home_depot", 1)],
            0.25,
        )
        .season(GARDEN_SEASON),
    );
    s.habit(Habit::new("pets", &[(quick, 1)], &[("petco", 1)], 0.15));
    s.habit(Habit::new(
        "hair",
        &[(quick, 1)],
        &[("great_clips", 1)],
        0.2,
    ));
    s.habit(
        Habit::new(
            "shopping",
            &[(amex, 1)],
            &[("amazon", 3), ("walmart", 1), ("target", 1)],
            0.7,
        )
        .season(HOLIDAY_SEASON),
    );
    s.habit(Habit::new(
        "books",
        &[(quick, 1)],
        &[("barnes_noble", 1)],
        0.1,
    ));

    let mut r = s.rng("visits");
    for d in [
        ymd(2025, 2, 11),
        ymd(2025, 8, 12),
        ymd(2026, 2, 10),
        ymd(2026, 8, 11),
    ] {
        let c = r.around(9_000, 16_000);
        s.buy(quick, "bright_smile", d, c);
    }
    for d in [ymd(2025, 8, 20), ymd(2026, 8, 19)] {
        let c = r.around(15_000, 32_000);
        s.buy(quick, "clear_view", d, c);
    }
    for d in [ymd(2025, 3, 19), ymd(2026, 3, 18)] {
        let c = r.around(8_000, 18_000);
        s.buy(quick, "desert_paws", d, c);
    }
    for (d, c) in [(ymd(2025, 4, 15), 61_000), (ymd(2026, 4, 15), 58_500)] {
        s.ach(chk, "irs_payment", d, c);
    }

    // Gifts to the grandchildren; cash.
    let grandkids = s.people("grandkids", 2);
    for (y, dates) in [
        (2025, [(3, 14), (8, 2), (11, 19)]),
        (2026, [(3, 13), (8, 3), (11, 18)]),
    ] {
        for (k, (m, d)) in dates.into_iter().enumerate() {
            let date = ymd(y, m, d);
            if date > s.end {
                continue;
            }
            let i = s.zelle(
                chk,
                date,
                10_000,
                grandkids[k % 2],
                true,
                DraftBucket::Category("giving.gifts"),
            );
            s.draft(i).tags.push("gift");
        }
    }
    for d in s.days() {
        if r.chance(0.02) {
            s.atm(chk, d, *r.pick(&[6_000, 10_000, 20_000]), None);
        }
    }

    // One-offs: a new roof (by check, funded from savings), an emergency vet visit,
    // hearing aids.
    s.transfer(
        sav,
        chk,
        ymd(2025, 10, 6),
        1_000_000,
        0,
        PairKind::OwnTransfer,
    );
    let roof = s.check(
        chk,
        ymd(2025, 10, 8),
        980_000,
        "Sun Valley Roofing",
        "housing.maintenance",
    );
    s.draft(roof).one_off = true;
    let vet = s.buy(quick, "desert_paws", ymd(2025, 7, 30), 135_000);
    s.draft(vet).one_off = true;
    let hearing = s.buy(amex, "costco_hearing", ymd(2026, 2, 18), 149_900);
    s.draft(hearing).one_off = true;

    // Japan, April 2025, paid in yen: mostly on the Quicksilver (no foreign fee), but the
    // Amex comes out on two days and adds a 2.7% fee line to each purchase.
    const JPY: i64 = 670_000;
    let flights = s.buy(quick, "jal", ymd(2025, 1, 20), 186_040);
    s.draft(flights).one_off = true;
    let mut j = s.rng("japan");
    let arrive = ymd(2025, 4, 3);
    for day in 0..13 {
        let d = add_days(arrive, day);
        let card = if day == 4 || day == 9 { amex } else { quick };
        let plan: [(&'static str, i64, i64); 4] = [
            ("lawson", 400, 2_500),
            ("seven_eleven_jp", 300, 2_000),
            ("ichiran", 980, 3_200),
            ("kyoto_kaiseki", 8_000, 24_000),
        ];
        for (m, lo, hi) in plan {
            if j.chance(if m == "kyoto_kaiseki" { 0.25 } else { 0.6 }) {
                let i = s.buy_foreign(card, m, d, j.around(lo, hi), "JPY", JPY);
                s.draft(i).tags.push("trip");
            }
        }
        if day == 3 || day == 8 {
            let i = s.buy_foreign(card, "jr_central", d, j.around(13_000, 15_000), "JPY", JPY);
            s.draft(i).tags.push("trip");
        }
    }
    let hotel = s.buy_foreign(
        quick,
        "granvia_kyoto",
        add_days(arrive, 12),
        245_000,
        "JPY",
        JPY,
    );
    s.draft(hotel).one_off = true;
    s.draft(hotel).tags.push("trip");

    // San Diego to see family, July 2026.
    let sw = s.buy(amex, "southwest", ymd(2026, 5, 2), 41_200);
    s.draft(sw).one_off = true;
    for day in 0..4 {
        let c = j.around(1_800, 5_500);
        let i = s.buy(amex, "sd_tacos", add_days(ymd(2026, 7, 10), day), c);
        s.draft(i).tags.push("trip");
    }
    let car = s.buy_at(amex, "hertz", ymd(2026, 7, 14), 31_248, "SAN DIEGO CA");
    s.draft(car).tags.push("trip");
    let sd_hotel = s.buy_at(amex, "hilton", ymd(2026, 7, 14), 76_800, "SAN DIEGO CA");
    s.draft(sd_hotel).one_off = true;
    s.draft(sd_hotel).tags.push("trip");

    s
}
