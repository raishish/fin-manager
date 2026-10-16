//! A two-income family with kids and a mortgage in a suburb: big recurring bills,
//! seasonal costs, paper checks, a paid-off car loan, a family holiday.

use fm_core::model::AccountKind::{Checking, CreditCard, Savings};

use crate::cal::{add_days, weekday, ymd};
use crate::describe::Style;
use crate::sim::{
    Amount, Cadence, CardTerms, DraftBucket, EVENINGS_OUT, GARDEN_SEASON, HOLIDAY_SEASON, Habit,
    Recurring, SavingsTerms, Sim, Via, WEEKDAYS, WEEKENDS,
};
use crate::truth::PairKind;

pub const KEY: &str = "family";
pub const DESCRIPTION: &str = "Jordan and Casey Testworth, a nurse and a teacher with two kids, \
owning a house in a Columbus, Ohio suburb. Joint Bank of America checking and savings, a Capital \
One and a Chase card, both paid in full. Mortgage, HOA, seasonal utilities, after-school care in \
term time and summer camp, a babysitter paid by Zelle, checks for piano lessons, lawn care and \
church. The car loan is paid off in November 2025. A furnace replacement and a Disney World trip.";

/// Electricity: air conditioning in summer.
const SUMMER: [i64; 12] = [35, 30, 25, 20, 40, 80, 100, 95, 60, 25, 25, 35];
/// Gas: heating in winter.
const WINTER: [i64; 12] = [100, 90, 65, 35, 15, 5, 5, 5, 10, 30, 65, 95];

pub fn build(seed: u64) -> Sim {
    let mut s = Sim::new(
        KEY,
        seed,
        crate::start(),
        crate::end(),
        "DUBLIN OH",
        "Jordan Testworth",
    );
    s.nearby = vec!["COLUMBUS OH", "WORTHINGTON OH", "HILLIARD OH"];
    let start = s.start;

    let chk = s.account(
        "checking",
        "Joint Checking",
        Checking,
        "Bank of America",
        "2290",
        Style::BofaChecking,
    );
    let sav = s.account(
        "savings",
        "Advantage Savings",
        Savings,
        "Bank of America",
        "2307",
        Style::BofaChecking,
    );
    let savor = s.account(
        "savor",
        "Capital One Savor",
        CreditCard,
        "Capital One",
        "6612",
        Style::CapOneCard,
    );
    let sapphire = s.account(
        "sapphire",
        "Chase Sapphire Preferred",
        CreditCard,
        "Chase",
        "0457",
        Style::ChaseCard,
    );
    s.card_terms(
        savor,
        CardTerms {
            close_day: 22,
            pay_from: chk,
            due_days: 25,
            lag: (0, 1),
            partial_months: vec![],
            partial_pct: 100,
            apr_bp: 2624,
            foreign_fee_bp: 0,
        },
    );
    s.card_terms(
        sapphire,
        CardTerms {
            close_day: 8,
            pay_from: chk,
            due_days: 25,
            lag: (1, 2),
            partial_months: vec![],
            partial_pct: 100,
            apr_bp: 2199,
            foreign_fee_bp: 0,
        },
    );
    s.savings_terms(
        sav,
        SavingsTerms {
            opening: 1_500_000,
            apy: vec![(start, 1)],
        },
    );

    // Two paychecks.
    s.recurring(Recurring::new(
        "family.payroll_jordan",
        "income",
        chk,
        "contoso_health",
        Cadence::Biweekly(ymd(2024, 10, 11)),
        Amount::Steps(vec![(start, 278_055), (ymd(2025, 7, 1), 289_010)]),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "family.payroll_casey",
        "income",
        chk,
        "fabrikam_schools",
        Cadence::SemiMonthly(15, 31),
        Amount::Steps(vec![(start, 210_533), (ymd(2025, 8, 1), 216_849)]),
        Via::Ach,
    ));

    // House and bills.
    s.recurring(Recurring::new(
        "family.mortgage",
        "bill",
        chk,
        "rocket_mortgage",
        Cadence::Monthly(1),
        Amount::Steps(vec![(start, 241_266), (ymd(2026, 1, 1), 248_821)]),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "family.hoa",
        "bill",
        chk,
        "maple_ridge_hoa",
        Cadence::Months(&[1, 4, 7, 10], 10),
        Amount::fixed(22_500),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "family.electricity",
        "bill",
        chk,
        "aep_ohio",
        Cadence::Monthly(12),
        Amount::Seasonal(9_000, 26_000, SUMMER),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "family.gas",
        "bill",
        chk,
        "columbia_gas",
        Cadence::Monthly(20),
        Amount::Seasonal(3_800, 21_000, WINTER),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "family.water",
        "bill",
        chk,
        "columbus_water",
        Cadence::Monthly(25),
        Amount::Range(7_000, 11_000),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "family.internet",
        "bill",
        savor,
        "spectrum",
        Cadence::Monthly(3),
        Amount::fixed(7_999),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "family.phone",
        "bill",
        savor,
        "verizon",
        Cadence::Monthly(19),
        Amount::Steps(vec![(start, 18_640), (ymd(2025, 9, 1), 19_212)]),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "family.car_insurance",
        "bill",
        chk,
        "state_farm_auto",
        Cadence::Monthly(7),
        Amount::Steps(vec![(start, 15_840), (ymd(2025, 6, 1), 17_122)]),
        Via::Ach,
    ));
    s.recurring(Recurring::new(
        "family.life_insurance",
        "bill",
        chk,
        "haven_life",
        Cadence::Monthly(28),
        Amount::fixed(3_850),
        Via::Ach,
    ));
    s.recurring(
        Recurring::new(
            "family.car_loan",
            "bill",
            chk,
            "toyota_financial",
            Cadence::Monthly(17),
            Amount::fixed(48_917),
            Via::Ach,
        )
        .to(ymd(2025, 11, 30)),
    );
    s.recurring(Recurring::new(
        "family.ymca",
        "subscription",
        chk,
        "ymca",
        Cadence::Monthly(1),
        Amount::fixed(8_900),
        Via::Ach,
    ));

    // Kids: after-school care in term time, camp in summer, activities.
    s.recurring(
        Recurring::new(
            "family.after_school",
            "bill",
            chk,
            "little_acorns",
            Cadence::Monthly(1),
            Amount::fixed(64_000),
            Via::Ach,
        )
        .only(&[1, 2, 3, 4, 5, 8, 9, 10, 11, 12]),
    );
    for (y, wk) in [(2025, ymd(2025, 6, 9)), (2026, ymd(2026, 6, 8))] {
        for w in 0..7 {
            let i = s.buy(
                savor,
                "ymca_camp",
                add_days(wk, w * 7),
                if y == 2025 { 31_000 } else { 32_500 },
            );
            s.draft(i).tags.push("summer_camp");
        }
    }
    s.recurring(Recurring::new(
        "family.swim",
        "subscription",
        sapphire,
        "sunrise_swim",
        Cadence::Monthly(10),
        Amount::fixed(9_600),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "family.piano",
        "bill",
        chk,
        "melody_lane",
        Cadence::Monthly(3),
        Amount::fixed(16_000),
        Via::Check,
    ));
    for d in [
        ymd(2025, 2, 20),
        ymd(2025, 7, 22),
        ymd(2026, 2, 19),
        ymd(2026, 7, 21),
    ] {
        s.buy(sapphire, "youth_soccer", d, 14_500);
    }

    // Subscriptions.
    s.recurring(Recurring::new(
        "family.netflix",
        "subscription",
        sapphire,
        "netflix",
        Cadence::Monthly(21),
        Amount::Steps(vec![(start, 2_299), (ymd(2025, 2, 1), 2_499)]),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "family.disney_plus",
        "subscription",
        sapphire,
        "disney_plus",
        Cadence::Monthly(11),
        Amount::Steps(vec![(start, 1_399), (ymd(2025, 10, 1), 1_599)]),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "family.spotify",
        "subscription",
        sapphire,
        "spotify",
        Cadence::Monthly(26),
        Amount::Steps(vec![(start, 1_699), (ymd(2025, 7, 1), 1_799)]),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "family.icloud",
        "subscription",
        sapphire,
        "apple_icloud",
        Cadence::Monthly(4),
        Amount::fixed(999),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "family.prime",
        "subscription",
        savor,
        "amazon_prime",
        Cadence::Months(&[2], 11),
        Amount::fixed(13_900),
        Via::Card,
    ));
    s.recurring(Recurring::new(
        "family.chewy",
        "subscription",
        savor,
        "chewy",
        Cadence::Monthly(24),
        Amount::Steps(vec![(start, 6_840), (ymd(2025, 11, 1), 7_210)]),
        Via::Card,
    ));

    // Saving: monthly to savings; college savings outside the ledger.
    s.scheduled_transfer(
        "family.savings_auto",
        chk,
        sav,
        Cadence::Monthly(2),
        Amount::fixed(40_000),
        (0, 0),
        None,
    );
    s.recurring(Recurring::new(
        "family.college_529",
        "transfer",
        chk,
        "ohio_529",
        Cadence::Monthly(16),
        Amount::fixed(25_000),
        Via::Ach,
    ));

    // Paper checks: lawn care every other Friday in season, church on the first Sunday.
    s.recurring(
        Recurring::new(
            "family.lawn",
            "bill",
            chk,
            "green_thumb",
            Cadence::Biweekly(ymd(2024, 10, 4)),
            Amount::fixed(5_500),
            Via::Check,
        )
        .only(&[4, 5, 6, 7, 8, 9, 10]),
    );
    s.recurring(Recurring::new(
        "family.church",
        "bill",
        chk,
        "st_marks",
        Cadence::NthWeekday(6, 1),
        Amount::fixed(15_000),
        Via::Check,
    ));

    // Everyday spending.
    s.habit(
        Habit::new(
            "groceries",
            &[(savor, 3), (chk, 1)],
            &[("kroger", 6), ("aldi", 2)],
            1.8,
        )
        .scale(160),
    );
    s.habit(Habit::new("costco", &[(sapphire, 1)], &[("costco", 1)], 0.5).days(WEEKENDS));
    s.habit(Habit::new(
        "fuel",
        &[(savor, 2), (chk, 1)],
        &[("speedway", 3), ("costco_gas", 2), ("shell", 1)],
        1.3,
    ));
    s.habit(
        Habit::new(
            "restaurants",
            &[(savor, 1)],
            &[("olive_garden", 2), ("copper_pot", 1), ("golden_lotus", 2)],
            0.7,
        )
        .days(EVENINGS_OUT),
    );
    s.habit(Habit::new(
        "fast_food",
        &[(savor, 2), (chk, 1)],
        &[
            ("mcdonalds", 3),
            ("chick_fil_a", 3),
            ("panera", 2),
            ("chipotle", 2),
        ],
        1.6,
    ));
    s.habit(
        Habit::new(
            "coffee",
            &[(savor, 1)],
            &[("dunkin", 2), ("starbucks", 1)],
            1.3,
        )
        .days(WEEKDAYS),
    );
    s.habit(Habit::new("delivery", &[(savor, 1)], &[("doordash", 1)], 0.35).days(EVENINGS_OUT));
    s.habit(
        Habit::new(
            "shopping",
            &[(sapphire, 2), (savor, 1)],
            &[("target", 3), ("walmart", 1), ("amazon", 5)],
            2.2,
        )
        .scale(130)
        .season(HOLIDAY_SEASON),
    );
    s.habit(
        Habit::new(
            "kids_clothes",
            &[(sapphire, 1)],
            &[("carters", 1), ("childrens_place", 1), ("old_navy", 1)],
            0.25,
        )
        .season(HOLIDAY_SEASON),
    );
    s.habit(Habit::new(
        "clothing",
        &[(sapphire, 1)],
        &[("tj_maxx", 2), ("old_navy", 1)],
        0.15,
    ));
    s.habit(Habit::new("pharmacy", &[(savor, 1)], &[("cvs", 1)], 0.5));
    s.habit(
        Habit::new(
            "pediatrician",
            &[(savor, 1)],
            &[("riverside_peds", 1)],
            0.15,
        )
        .days(WEEKDAYS),
    );
    s.habit(
        Habit::new(
            "home",
            &[(sapphire, 1)],
            &[("home_depot", 3), ("lowes", 2)],
            0.45,
        )
        .season(GARDEN_SEASON)
        .days(WEEKENDS),
    );
    s.habit(Habit::new("pets", &[(savor, 1)], &[("petco", 1)], 0.12));
    s.habit(Habit::new("haircuts", &[(savor, 1)], &[("great_clips", 1)], 0.3).days(WEEKENDS));
    s.habit(Habit::new("movies", &[(sapphire, 1)], &[("amc", 1)], 0.15).days(WEEKENDS));
    s.habit(Habit::new(
        "sports",
        &[(sapphire, 1)],
        &[("dicks", 1)],
        0.08,
    ));
    s.habit(Habit::new(
        "crafts",
        &[(sapphire, 1)],
        &[("michaels", 1)],
        0.1,
    ));

    // Twice-yearly visits.
    let mut r = s.rng("visits");
    for d in [
        ymd(2025, 1, 14),
        ymd(2025, 7, 15),
        ymd(2026, 1, 13),
        ymd(2026, 7, 14),
    ] {
        let c = r.around(9_000, 18_000);
        s.buy(sapphire, "bright_smile", d, c);
    }
    for d in [
        ymd(2025, 3, 6),
        ymd(2025, 9, 18),
        ymd(2026, 3, 5),
        ymd(2026, 9, 17),
    ] {
        let c = r.around(7_000, 26_000);
        s.buy(savor, "banfield", d, c);
    }

    // People: a regular babysitter, occasionally another; a neighbour shares the school run.
    let sitters = s.people("sitters", 3);
    let neighbour = s.people("neighbour", 1)[0];
    let mut p = s.rng("p2p");
    for d in s.days() {
        if weekday(d) >= 4 && p.chance(0.16) {
            let who = if p.chance(0.85) {
                sitters[0]
            } else {
                sitters[1 + p.below(2) as usize]
            };
            let i = s.zelle(
                chk,
                d,
                p.around(6_000, 12_000),
                who,
                true,
                DraftBucket::Category("family.childcare"),
            );
            s.draft(i).tags.push("babysitter");
        }
        if weekday(d) < 5 && p.chance(0.012) {
            let i = s.zelle(
                chk,
                d,
                p.around(2_000, 4_500),
                neighbour,
                false,
                DraftBucket::Refund("transport.fuel", None),
            );
            s.draft(i).tags.push("reimbursement");
        }
        if p.chance(0.02) {
            let fee = p.chance(0.25).then_some(250);
            s.atm(chk, d, *p.pick(&[6_000, 10_000, 20_000]), fee);
        }
    }

    // One-offs.
    s.transfer(
        sav,
        chk,
        ymd(2025, 1, 15),
        500_000,
        0,
        PairKind::OwnTransfer,
    );
    let furnace = s.buy(sapphire, "buckeye_hvac", ymd(2025, 1, 17), 685_000);
    s.draft(furnace).one_off = true;
    let laptop = s.buy(savor, "best_buy", ymd(2025, 8, 9), 64_999);
    s.draft(laptop).one_off = true;
    let braces = s.buy(sapphire, "smile_ortho", ymd(2025, 9, 4), 180_000);
    s.draft(braces).one_off = true;
    s.recurring(
        Recurring::new(
            "family.orthodontist",
            "bill",
            sapphire,
            "smile_ortho",
            Cadence::Monthly(4),
            Amount::fixed(15_000),
            Via::Card,
        )
        .from(ymd(2025, 10, 1)),
    );
    let repair = s.buy(savor, "firestone", ymd(2026, 2, 12), 124_018);
    s.draft(repair).one_off = true;
    for (d, fed, state) in [
        (ymd(2025, 3, 5), 341_200, 28_100),
        (ymd(2026, 3, 10), 298_000, 24_000),
    ] {
        s.ach(chk, "irs_refund", d, -fed);
        s.ach(chk, "ohio_refund", add_days(d, 9), -state);
    }
    for d in [ymd(2025, 6, 20), ymd(2026, 6, 19)] {
        s.buy(sapphire, "ohio_bmv", d, 7_925);
    }

    // Disney World, June 2025.
    let flights = s.buy(sapphire, "delta", ymd(2025, 2, 3), 178_040);
    s.draft(flights).one_off = true;
    let tickets = s.buy(sapphire, "wdw_tickets", ymd(2025, 3, 15), 234_000);
    s.draft(tickets).one_off = true;
    let trip = ymd(2025, 6, 22);
    for day in 0..6 {
        for _ in 0..3 {
            let c = r.around(4_500, 19_000);
            let i = s.buy(sapphire, "disney_dining", add_days(trip, day), c);
            s.draft(i).tags.push("trip");
        }
    }
    let hotel = s.buy(sapphire, "disney_resort", add_days(trip, 6), 210_066);
    s.draft(hotel).one_off = true;
    s.draft(hotel).tags.push("trip");

    s
}
