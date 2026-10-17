//! The questions. Each computes its answers from the persona's facts; nothing here is
//! a hand-written number.

use std::collections::{BTreeMap, BTreeSet};

use fm_core::model::{Currency, Money};

use crate::facts::{Fact, Kind, Period, div_round, today};
use crate::*;

fn q(text: &str, kind: &str, tags: &[&str], answers: Vec<Answer>, note: Option<&str>) -> Question {
    Question {
        id: String::new(),
        question: text.to_string(),
        kind: kind.to_string(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        answers,
        note: note.map(str::to_string),
        expected_query: None,
        proxy: None,
        known_gap: None,
        null_reason: None,
    }
}

fn all_time() -> Period {
    Period::new(fm_synth::start(), fm_synth::end())
}

fn y(year: i32) -> Period {
    Period::year(year)
}

fn key(k: &'static str) -> impl Fn(&Fact) -> bool {
    move |x| x.key.as_deref() == Some(k)
}

fn group(g: &'static str) -> impl Fn(&Fact) -> bool {
    move |x| x.group() == Some(g)
}

fn at(m: &'static str) -> impl Fn(&Fact) -> bool {
    move |x| x.merchant.as_deref() == Some(m)
}

fn series(id: &'static str) -> impl Fn(&Fact) -> bool {
    move |x| x.recurring.as_deref() == Some(id)
}

fn any(_: &Fact) -> bool {
    true
}

const CHECKING: &str = "Share Draft Checking";
const SAVINGS: &str = "Regular Share Savings";

fn eur_money(name: &str, minor: i64, ids: Vec<i64>) -> Answer {
    Answer::Money {
        name: name.into(),
        amount: Money::new(minor, Currency::new("EUR").expect("EUR")),
        evidence: Evidence {
            rule: Rule::NegatedOriginalSum,
            transaction_ids: ids,
        },
    }
}

/// The latest amount of a fixed-price series, as a positive cost or income.
fn latest(book: &Book, id: &str) -> i64 {
    match &book.series(id).amount {
        fm_synth::truth::AmountTruth::Fixed { steps } => {
            steps.last().expect("a step").amount.minor().abs()
        }
        fm_synth::truth::AmountTruth::Variable { max, .. } => max.minor().abs(),
    }
}

fn comparison(name_a: &str, a: &Sum, name_b: &str, b: &Sum) -> Vec<Answer> {
    vec![
        spending(name_a, a),
        spending(name_b, b),
        derived("difference", a.minor - b.minor),
    ]
}

fn month_text(p: &str) -> String {
    p.to_string()
}

pub fn all(b: &Book) -> Vec<Question> {
    let mut v = Vec::new();
    totals(b, &mut v);
    comparisons(b, &mut v);
    merchants(b, &mut v);
    recurring(b, &mut v);
    time_ranges(b, &mut v);
    hard(b, &mut v);
    v
}

fn totals(b: &Book, v: &mut Vec<Question>) {
    let t = "totals";
    v.push(q(
        "How much did I spend in total in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(y(2025), any))],
        None,
    ));
    v.push(q(
        "How much did I spend on groceries in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(y(2025), key("food.groceries")))],
        None,
    ));
    v.push(q(
        "How much did I spend at restaurants in the first quarter of 2026?",
        t,
        &[],
        vec![spending(
            "total",
            &b.spent(Period::quarter(2026, 1), key("food.restaurants")),
        )],
        None,
    ));
    v.push(q(
        "What did I spend on coffee in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(y(2025), key("food.coffee")))],
        None,
    ));
    v.push(q(
        "How much rent did I pay in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(y(2025), key("housing.rent")))],
        None,
    ));
    v.push(q(
        "How much did utilities cost me from June to August 2025?",
        t,
        &["group"],
        vec![spending(
            "total",
            &b.spent(Period::months(2025, 6, 8), group("utilities")),
        )],
        Some("All utility categories: electricity, internet and phone."),
    ));
    v.push(q(
        "What was my electricity bill in July 2025?",
        t,
        &[],
        vec![spending(
            "total",
            &b.spent(Period::month(2025, 7), key("utilities.electricity")),
        )],
        None,
    ));
    v.push(q(
        "How much did I spend on shopping in December 2025?",
        t,
        &["group"],
        vec![spending(
            "total",
            &b.spent(Period::month(2025, 12), group("shopping")),
        )],
        None,
    ));
    v.push(q(
        "How much did I spend on gas for the car in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(y(2025), key("transport.fuel")))],
        None,
    ));
    v.push(q(
        "How much have I paid for health insurance in 2026 so far?",
        t,
        &[],
        vec![spending(
            "total",
            &b.spent(y(2026), key("insurance.health")),
        )],
        None,
    ));
    v.push(q(
        "How much did software subscriptions cost me in 2025?",
        t,
        &[],
        vec![spending(
            "total",
            &b.spent(y(2025), key("services.software")),
        )],
        None,
    ));
    v.push(q(
        "What did I spend on fitness in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(y(2025), key("health.fitness")))],
        None,
    ));
    v.push(q(
        "How much did transportation cost me in 2025?",
        t,
        &["group"],
        vec![spending("total", &b.spent(y(2025), group("transport")))],
        Some("Fuel, rideshare, transit, parking and car maintenance."),
    ));
    v.push(q(
        "How much did I spend on food and drink in 2025?",
        t,
        &["group"],
        vec![spending("total", &b.spent(y(2025), group("food")))],
        None,
    ));
    v.push(q(
        "How much did I earn in 2025?",
        t,
        &["income"],
        vec![money("total", &b.earned(y(2025), any), Rule::Sum)],
        None,
    ));
    v.push(q(
        "How much income came in during January 2025?",
        t,
        &["income"],
        vec![money(
            "total",
            &b.earned(Period::month(2025, 1), any),
            Rule::Sum,
        )],
        None,
    ));
    v.push(q(
        "How much did Tailspin Toys pay me in 2025?",
        t,
        &["income", "merchant"],
        vec![money(
            "total",
            &b.earned(y(2025), at("Tailspin Toys")),
            Rule::Sum,
        )],
        None,
    ));
    v.push(q(
        "How much cash did I take out of ATMs in 2025?",
        t,
        &[],
        vec![spending(
            "total",
            &b.spent(y(2025), key("financial.cash_withdrawal")),
        )],
        None,
    ));
    v.push(q(
        "How much did I pay in income tax in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(y(2025), key("taxes.income")))],
        None,
    ));
    v.push(q(
        "How much did I spend on dining out at bars in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(y(2025), key("food.bars")))],
        None,
    ));
}

fn comparisons(b: &Book, v: &mut Vec<Question>) {
    let t = "comparisons";
    let g24 = b.spent(Period::quarter(2024, 4), key("food.groceries"));
    let g25 = b.spent(Period::quarter(2025, 4), key("food.groceries"));
    v.push(q(
        "Did I spend more on groceries in the last quarter of 2025 than in the last quarter of 2024?",
        t,
        &[],
        comparison("q4_2025", &g25, "q4_2024", &g24),
        Some("difference = Q4 2025 minus Q4 2024."),
    ));
    let aug = b.spent(Period::month(2026, 8), any);
    let sep = b.spent(Period::month(2026, 9), any);
    let mut a = comparison("september_2026", &sep, "august_2026", &aug);
    a.push(percent("change", sep.minor - aug.minor, aug.minor));
    v.push(q(
        "How did my spending in September 2026 compare with August 2026?",
        t,
        &["month_over_month"],
        a,
        Some("difference = September minus August; change is relative to August."),
    ));
    let h1_25 = b.spent(Period::months(2025, 1, 6), key("food.restaurants"));
    let h1_26 = b.spent(Period::months(2026, 1, 6), key("food.restaurants"));
    v.push(q(
        "Am I eating out at restaurants more in the first half of 2026 than in the first half of 2025?",
        t,
        &[],
        comparison("h1_2026", &h1_26, "h1_2025", &h1_25),
        None,
    ));
    let inc = b.earned(y(2025), any);
    let out = b.spent(y(2025), any);
    v.push(q(
        "How much did I earn and spend in 2025, and what was left over?",
        t,
        &["income", "transfers"],
        vec![
            money("income", &inc, Rule::Sum),
            spending("spending", &out),
            derived("left_over", inc.minor - out.minor),
        ],
        Some("Transfers (savings, tax set-asides, retirement, card payments) are neither."),
    ));
    let months = b.spent_by(y(2025), any, month_key);
    v.push(q(
        "Which month in 2025 did I spend the most?",
        t,
        &["ranking"],
        vec![
            text("month", month_text(&months[0].0)),
            spending("total", &months[0].1),
        ],
        Some("Months are YYYY-MM."),
    ));
    let mut by_month_income: Vec<(String, Sum)> = (1..=12)
        .map(|m| {
            (
                format!("2025-{m:02}"),
                b.earned(Period::month(2025, m), any),
            )
        })
        .collect();
    by_month_income.sort_by(|a, b| a.1.minor.cmp(&b.1.minor).then(a.0.cmp(&b.0)));
    v.push(q(
        "Which month in 2025 had the lowest income?",
        t,
        &["ranking", "income"],
        vec![
            text("month", by_month_income[0].0.clone()),
            money("total", &by_month_income[0].1, Rule::Sum),
        ],
        None,
    ));
    let coffee = b.spent(y(2025), key("food.coffee"));
    let bars = b.spent(y(2025), key("food.bars"));
    v.push(q(
        "Did I spend more on coffee or at bars in 2025?",
        t,
        &[],
        vec![
            text(
                "more",
                if coffee.minor >= bars.minor {
                    "coffee"
                } else {
                    "bars"
                },
            ),
            spending("coffee", &coffee),
            spending("bars", &bars),
        ],
        None,
    ));
    let elec = b.spent_by(y(2025), key("utilities.electricity"), month_key);
    let (hi, lo) = (&elec[0], &elec[elec.len() - 1]);
    v.push(q(
        "What were my most and least expensive electricity bills in 2025?",
        t,
        &["ranking", "recurring"],
        vec![
            text("highest_month", hi.0.clone()),
            spending("highest", &hi.1),
            text("lowest_month", lo.0.clone()),
            spending("lowest", &lo.1),
        ],
        None,
    ));
    let groc = b.spent(y(2025), key("food.groceries"));
    v.push(q(
        "What's my average monthly grocery bill in 2025?",
        t,
        &["average"],
        vec![
            derived("average", div_round(groc.minor, 12)),
            spending("total", &groc),
        ],
        Some("Total over the 12 months of 2025, divided by 12."),
    ));
    let rent = b.spent(y(2025), key("housing.rent"));
    v.push(q(
        "What share of my 2025 spending went to rent?",
        t,
        &["percent"],
        vec![
            percent("share", rent.minor, out.minor),
            spending("rent", &rent),
            spending("total_spending", &out),
        ],
        None,
    ));
    let shell = b.spent(y(2025), at("Shell"));
    let chevron = b.spent(y(2025), at("Chevron"));
    v.push(q(
        "Did I spend more on gas at Shell or Chevron in 2025?",
        t,
        &["merchant"],
        comparison("shell", &shell, "chevron", &chevron),
        None,
    ));
    let sw25 = b.spent(Period::months(2025, 1, 9), key("services.software"));
    let sw26 = b.spent(Period::months(2026, 1, 9), key("services.software"));
    v.push(q(
        "Have my software costs gone up this year compared with the same months last year?",
        t,
        &["relative_date", "year_over_year"],
        comparison("2026_jan_sep", &sw26, "2025_jan_sep", &sw25),
        Some(
            "This year so far is January to September 2026; the same months of 2025 are compared.",
        ),
    ));
    let q2_25 = b.spent(Period::quarter(2025, 2), any);
    let q2_26 = b.spent(Period::quarter(2026, 2), any);
    v.push(q(
        "How does my total spending in Q2 2026 compare with Q2 2025?",
        t,
        &["year_over_year"],
        comparison("q2_2026", &q2_26, "q2_2025", &q2_25),
        None,
    ));
    let ga = b.spent(Period::month(2026, 8), key("food.groceries"));
    let gs = b.spent(Period::month(2026, 9), key("food.groceries"));
    let mut a = comparison("september_2026", &gs, "august_2026", &ga);
    a.push(percent("change", gs.minor - ga.minor, ga.minor));
    v.push(q(
        "How did my grocery spending change from August to September 2026?",
        t,
        &["month_over_month"],
        a,
        None,
    ));
    let mut six: Vec<i64> = (4..=9)
        .map(|m| b.spent(Period::month(2026, m), any).minor)
        .collect();
    six.sort_unstable();
    v.push(q(
        "What's my typical monthly spending over the last six months?",
        t,
        &["median", "relative_date", "baseline"],
        vec![derived("median", div_round(six[2] + six[3], 2))],
        Some("The median of the monthly totals for April to September 2026 (SPEC decision 32's baseline): the mean of the middle two."),
    ));
}

fn merchants(b: &Book, v: &mut Vec<Question>) {
    let t = "merchants";
    let yr = y(2025);
    v.push(q(
        "How much did I spend at H-E-B in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(yr, at("H-E-B")))],
        None,
    ));
    let top = b.spent_by(yr, any, |x| x.merchant.clone());
    let mut a = vec![Answer::Texts {
        name: "merchants".into(),
        values: top.iter().take(3).map(|m| m.0.clone()).collect(),
    }];
    for (i, m) in top.iter().take(3).enumerate() {
        a.push(spending(&format!("total_{}", i + 1), &m.1));
    }
    v.push(q(
        "Who were my top three merchants by spending in 2025?",
        t,
        &["ranking"],
        a,
        Some("Ranked by spending net of refunds."),
    ));
    v.push(q(
        "How many times did I buy coffee at Cosmic Cup in 2025?",
        t,
        &["count"],
        vec![count(
            "visits",
            &b.count(yr, |x| {
                x.kind == Kind::Spending && at("Cosmic Cup Coffee")(x)
            }),
        )],
        None,
    ));
    let uber = b.spent(yr, at("Uber"));
    let rides = b.count(yr, |x| x.kind == Kind::Spending && at("Uber")(x));
    v.push(q(
        "What did an Uber ride cost me on average in 2025?",
        t,
        &["average"],
        vec![
            derived("average", div_round(uber.minor, rides.minor)),
            count("rides", &rides),
            spending("total", &uber),
        ],
        None,
    ));
    let biggest = b
        .facts
        .iter()
        .filter(|x| x.counted() && yr.contains(x.date()) && x.on_card && x.kind == Kind::Spending)
        .min_by_key(|x| (x.amount, x.id))
        .expect("card purchases in 2025");
    v.push(q(
        "What was my biggest single card purchase in 2025?",
        t,
        &["ranking"],
        vec![
            text("merchant", biggest.merchant.clone().unwrap_or_default()),
            spending(
                "amount",
                &Sum {
                    minor: -biggest.amount,
                    ids: vec![biggest.id],
                },
            ),
            date("date", biggest.date()),
        ],
        None,
    ));
    let places: BTreeSet<String> = b
        .facts
        .iter()
        .filter(|x| {
            x.counted()
                && yr.contains(x.date())
                && x.kind == Kind::Spending
                && key("food.restaurants")(x)
        })
        .filter_map(|x| x.merchant.clone())
        .collect();
    v.push(q(
        "How many different restaurants did I go to in 2025?",
        t,
        &["count", "distinct"],
        vec![Answer::Count {
            name: "restaurants".into(),
            value: places.len() as i64,
            evidence: Evidence {
                rule: Rule::Derived,
                transaction_ids: Vec::new(),
            },
        }],
        Some("Distinct merchants in the restaurants category."),
    ));
    v.push(q(
        "How much did I spend at Whole Foods compared with H-E-B in 2025?",
        t,
        &["comparison"],
        comparison(
            "whole_foods",
            &b.spent(yr, at("Whole Foods Market")),
            "heb",
            &b.spent(yr, at("H-E-B")),
        ),
        None,
    ));
    let cf = b
        .facts
        .iter()
        .filter(|x| x.counted() && at("Capital Factory")(x))
        .max_by_key(|x| (x.date(), x.id))
        .expect("coworking");
    v.push(q(
        "When did I last pay Capital Factory, and how much?",
        t,
        &["date"],
        vec![
            date("date", cf.date()),
            spending(
                "amount",
                &Sum {
                    minor: -cf.amount,
                    ids: vec![cf.id],
                },
            ),
        ],
        None,
    ));
    let tires = b
        .facts
        .iter()
        .filter(|x| x.counted() && at("Discount Tire")(x))
        .min_by_key(|x| (x.date(), x.id))
        .expect("tires");
    v.push(q(
        "When did I buy new tires, and what did they cost?",
        t,
        &["date", "one_off"],
        vec![
            date("date", tires.date()),
            spending(
                "amount",
                &Sum {
                    minor: -tires.amount,
                    ids: vec![tires.id],
                },
            ),
        ],
        Some("The tire shop is Discount Tire; the question doesn't name it."),
    ));
    v.push(q(
        "How much did I spend at Lone Star Smokehouse in 2026?",
        t,
        &[],
        vec![spending(
            "total",
            &b.spent(y(2026), at("Lone Star Smokehouse")),
        )],
        None,
    ));
    let mut counts: BTreeMap<String, Sum> = BTreeMap::new();
    for x in b
        .facts
        .iter()
        .filter(|x| x.counted() && yr.contains(x.date()) && x.kind == Kind::Spending)
    {
        if let Some(m) = &x.merchant {
            let s = counts.entry(m.clone()).or_default();
            s.minor += 1;
            s.ids.push(x.id);
        }
    }
    let mut counts: Vec<(String, Sum)> = counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.minor.cmp(&a.1.minor).then(a.0.cmp(&b.0)));
    v.push(q(
        "Where did I make the most purchases in 2025?",
        t,
        &["ranking", "count"],
        vec![
            text("merchant", counts[0].0.clone()),
            count("purchases", &counts[0].1),
        ],
        None,
    ));
    v.push(q(
        "How much did I spend at Target in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(yr, at("Target")))],
        None,
    ));
    v.push(q(
        "How much did I pay T-Mobile in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(yr, at("T-Mobile")))],
        None,
    ));
    v.push(q(
        "How much did I spend on Amazon over the 2025 holidays (October to December)?",
        t,
        &["refunds"],
        vec![spending(
            "total",
            &b.spent(Period::quarter(2025, 4), at("Amazon")),
        )],
        Some("Net of any Amazon refunds in the period."),
    ));
    v.push(q(
        "How much did I pay Austin Energy in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(yr, at("Austin Energy")))],
        None,
    ));
}

fn recurring(b: &Book, v: &mut Vec<Question>) {
    let t = "recurring";
    v.push(q(
        "How much is my Adobe subscription now?",
        t,
        &[],
        vec![derived("monthly", latest(b, "freelancer.adobe"))],
        None,
    ));
    let fm_synth::truth::AmountTruth::Fixed { steps } = &b.series("freelancer.adobe").amount else {
        unreachable!()
    };
    v.push(q(
        "When did Adobe raise its price, and by how much?",
        t,
        &["price_change"],
        vec![
            date("from", steps[1].from),
            derived("old", -steps[0].amount.minor()),
            derived("new", -steps[1].amount.minor()),
            derived(
                "increase",
                steps[0].amount.minor() - steps[1].amount.minor(),
            ),
        ],
        Some("The date is the first charge at the new price."),
    ));
    let active: Vec<&fm_synth::truth::SeriesTruth> = b
        .truth
        .recurring
        .iter()
        .filter(|s| s.kind == "subscription" && s.cadence == "monthly" && !s.ended)
        .collect();
    let monthly: i64 = active.iter().map(|s| latest(b, &s.id)).sum();
    let mut names: Vec<String> = active.iter().map(|s| s.merchant.clone()).collect();
    names.sort();
    v.push(q(
        "What do my monthly subscriptions cost me right now?",
        t,
        &["subscriptions"],
        vec![derived("monthly_total", monthly), Answer::Texts { name: "subscriptions".into(), values: names }],
        Some("Monthly subscriptions still running at today, at their latest price; annual plans not included."),
    ));
    let active_merchants: BTreeSet<&str> = b
        .truth
        .recurring
        .iter()
        .filter(|s| !s.ended)
        .map(|s| s.merchant.as_str())
        .collect();
    let mut cancelled: Vec<String> = b
        .truth
        .recurring
        .iter()
        .filter(|s| {
            s.kind == "subscription" && s.ended && !active_merchants.contains(s.merchant.as_str())
        })
        .map(|s| s.merchant.clone())
        .collect();
    cancelled.sort();
    v.push(q(
        "Which subscriptions have I cancelled for good?",
        t,
        &["subscriptions", "cancelled"],
        vec![Answer::Texts {
            name: "cancelled".into(),
            values: cancelled,
        }],
        Some("Zoom was cancelled and later restarted, so it isn't on the list."),
    ));
    v.push(q(
        "How much have I paid Zoom altogether?",
        t,
        &["subscriptions", "restart"],
        vec![spending("total", &b.spent(all_time(), at("Zoom")))],
        None,
    ));
    let mut annual: Vec<(String, i64)> = b
        .truth
        .recurring
        .iter()
        .filter(|s| s.kind == "subscription" && s.cadence == "annual")
        .map(|s| (s.merchant.clone(), latest(b, &s.id)))
        .collect();
    annual.sort();
    let mut a = vec![Answer::Texts {
        name: "subscriptions".into(),
        values: annual.iter().map(|x| x.0.clone()).collect(),
    }];
    for (m, c) in &annual {
        a.push(derived(&m.to_lowercase(), *c));
    }
    v.push(q(
        "Which subscriptions do I pay for yearly, and how much each?",
        t,
        &["subscriptions", "annual"],
        a,
        None,
    ));
    let fm_synth::truth::AmountTruth::Fixed { steps: rent } = &b.series("freelancer.rent").amount
    else {
        unreachable!()
    };
    v.push(q(
        "When did my rent go up, and what is it now?",
        t,
        &["price_change"],
        vec![
            date("from", rent[1].from),
            derived("old", -rent[0].amount.minor()),
            derived("new", -rent[1].amount.minor()),
        ],
        None,
    ));
    v.push(q(
        "How much have I paid for health insurance in total?",
        t,
        &["all_time"],
        vec![spending(
            "total",
            &b.spent(all_time(), series("freelancer.health_insurance")),
        )],
        Some("Over the whole history, October 2024 to September 2026."),
    ));
    v.push(q(
        "What was my electricity bill in August 2026?",
        t,
        &[],
        vec![spending(
            "total",
            &b.spent(Period::month(2026, 8), series("freelancer.electricity")),
        )],
        None,
    ));
    let e25 = b.spent(y(2025), series("freelancer.electricity"));
    let bills = b.count(y(2025), series("freelancer.electricity"));
    v.push(q(
        "What's my average electricity bill in 2025?",
        t,
        &["average"],
        vec![
            derived("average", div_round(e25.minor, bills.minor)),
            count("bills", &bills),
            spending("total", &e25),
        ],
        None,
    ));
    v.push(q(
        "How much did my Coursera subscription cost in total?",
        t,
        &["subscriptions", "cancelled"],
        vec![spending("total", &b.spent(all_time(), at("Coursera")))],
        None,
    ));
    let running = b
        .truth
        .recurring
        .iter()
        .filter(|s| matches!(s.kind.as_str(), "subscription" | "bill") && !s.ended)
        .count();
    v.push(q(
        "How many recurring bills and subscriptions do I have right now?",
        t,
        &["count"],
        vec![Answer::Count {
            name: "recurring".into(),
            value: running as i64,
            evidence: Evidence {
                rule: Rule::Derived,
                transaction_ids: Vec::new(),
            },
        }],
        Some("Bills and subscriptions still running at today, monthly, quarterly and annual."),
    ));
    v.push(q(
        "What does my retainer client pay me each month now?",
        t,
        &["income"],
        vec![derived("monthly", latest(b, "freelancer.retainer"))],
        None,
    ));
    v.push(q(
        "How much interest did my savings earn in 2025?",
        t,
        &["income"],
        vec![money(
            "total",
            &b.earned(y(2025), series("freelancer.savings.interest")),
            Rule::Sum,
        )],
        None,
    ));
    let est = b.spent(y(2025), series("freelancer.estimated_tax"));
    v.push(q(
        "How many estimated tax payments did I make in 2025, and how much did they add up to?",
        t,
        &["taxes"],
        vec![
            count(
                "payments",
                &b.count(y(2025), series("freelancer.estimated_tax")),
            ),
            spending("total", &est),
        ],
        Some("Quarterly estimates only; the April balance-due payment is separate."),
    ));
}

fn time_ranges(b: &Book, v: &mut Vec<Question>) {
    let t = "time_ranges";
    let rel = &["relative_date"][..];
    v.push(q(
        "How much did I spend last month?",
        t,
        rel,
        vec![spending("total", &b.spent(Period::last_months(1), any))],
        None,
    ));
    v.push(q(
        "How much have I spent so far this year?",
        t,
        rel,
        vec![spending(
            "total",
            &b.spent(Period::new(fm_synth::cal::ymd(2026, 1, 1), today()), any),
        )],
        None,
    ));
    v.push(q(
        "How much did I spend on groceries over the last three months?",
        t,
        rel,
        vec![spending(
            "total",
            &b.spent(Period::last_months(3), key("food.groceries")),
        )],
        None,
    ));
    v.push(q(
        "How much did I spend in the last two weeks?",
        t,
        rel,
        vec![spending("total", &b.spent(Period::last_days(14), any))],
        Some("The 14 days before today: 2026-09-17 to 2026-09-30."),
    ));
    let monday = fm_synth::cal::add_days(today(), -(fm_synth::cal::weekday(today()) as i32));
    let last_week = Period::new(
        fm_synth::cal::add_days(monday, -7),
        fm_synth::cal::add_days(monday, -1),
    );
    v.push(q(
        "How much did I spend last week?",
        t,
        rel,
        vec![spending("total", &b.spent(last_week, any))],
        Some("Monday to Sunday: 2026-09-21 to 2026-09-27."),
    ));
    v.push(q(
        "How much did I earn last quarter?",
        t,
        &["relative_date", "income"],
        vec![money(
            "total",
            &b.earned(Period::quarter(2026, 3), any),
            Rule::Sum,
        )],
        None,
    ));
    v.push(q(
        "How much have I spent at restaurants since January?",
        t,
        rel,
        vec![spending(
            "total",
            &b.spent(
                Period::new(fm_synth::cal::ymd(2026, 1, 1), today()),
                key("food.restaurants"),
            ),
        )],
        None,
    ));
    v.push(q(
        "What did I spend in the first half of 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(Period::months(2025, 1, 6), any))],
        None,
    ));
    v.push(q(
        "How much have I spent over the past 12 months?",
        t,
        rel,
        vec![spending("total", &b.spent(Period::last_months(12), any))],
        None,
    ));
    v.push(q(
        "How much did I spend between Black Friday and Christmas 2025?",
        t,
        &["holiday"],
        vec![spending(
            "total",
            &b.spent(
                Period::new(
                    fm_synth::cal::ymd(2025, 11, 28),
                    fm_synth::cal::ymd(2025, 12, 25),
                ),
                any,
            ),
        )],
        Some("2025-11-28 to 2025-12-25, both included."),
    ));
    v.push(q(
        "How much did I spend in April 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(Period::month(2025, 4), any))],
        None,
    ));
    v.push(q(
        "What have software subscriptions cost me this year?",
        t,
        rel,
        vec![spending(
            "total",
            &b.spent(y(2026), key("services.software")),
        )],
        None,
    ));
    v.push(q(
        "How much did I spend on coffee over the last two months?",
        t,
        rel,
        vec![spending(
            "total",
            &b.spent(Period::last_months(2), key("food.coffee")),
        )],
        None,
    ));
    v.push(q(
        "How much did I spend in the last three months of 2024?",
        t,
        &["first_quarter_of_data"],
        vec![spending("total", &b.spent(Period::quarter(2024, 4), any))],
        Some("The first three months of history."),
    ));
    v.push(q(
        "What did I spend on gas over the last six months?",
        t,
        rel,
        vec![spending(
            "total",
            &b.spent(Period::last_months(6), key("transport.fuel")),
        )],
        None,
    ));
}

fn hard(b: &Book, v: &mut Vec<Question>) {
    let t = "hard";
    let yr = y(2025);
    let pending: Vec<&Fact> = b.facts.iter().filter(|x| x.pending).collect();
    v.push(q(
        "Which purchases are still pending, and how much do they add up to?",
        t,
        &["pending"],
        vec![
            Answer::Count {
                name: "pending".into(),
                value: pending.len() as i64,
                evidence: Evidence {
                    rule: Rule::Count,
                    transaction_ids: pending.iter().map(|x| x.id).collect(),
                },
            },
            Answer::Money {
                name: "total".into(),
                amount: usd(-pending.iter().map(|x| x.amount).sum::<i64>()),
                evidence: Evidence {
                    rule: Rule::NegatedSum,
                    transaction_ids: pending.iter().map(|x| x.id).collect(),
                },
            },
        ],
        Some("Pending items are left out of every other total; this question asks for them."),
    ));
    v.push(q(
        "How much did I move into my savings account in 2025?",
        t,
        &["transfers"],
        vec![money(
            "total",
            &b.total(yr, |x| {
                x.account == SAVINGS && x.kind == Kind::Transfer && x.amount > 0
            }),
            Rule::Sum,
        )],
        Some("Transfers in, not income or spending."),
    ));
    v.push(q(
        "How much did I spend on my credit cards in 2025?",
        t,
        &["transfers", "cards"],
        vec![spending("total", &b.spent(yr, |x| x.on_card))],
        Some("Card purchases net of refunds, plus card interest and fees. Card payments are transfers and don't count."),
    ));
    v.push(q(
        "How much did I pay toward my credit cards in 2025?",
        t,
        &["transfers", "cards"],
        vec![money(
            "total",
            &b.total(yr, |x| {
                x.on_card && x.kind == Kind::Transfer && x.amount > 0
            }),
            Rule::Sum,
        )],
        Some("Payments received on the cards. They are transfers, not spending."),
    ));
    let outflow = b.total(yr, |x| x.account == CHECKING && x.amount < 0);
    let spent = b.spent(yr, |x| x.account == CHECKING);
    v.push(q(
        "How much money left my checking account in 2025, and how much of that was actual spending?",
        t,
        &["transfers"],
        vec![money_out("money_out", &outflow), spending("spending", &spent), derived("not_spending", -outflow.minor - spent.minor)],
        Some("The rest is transfers: card payments, savings and tax set-asides, the retirement contribution."),
    ));
    let gross = b.total(yr, |x| x.kind == Kind::Spending && at("Amazon")(x));
    let back = b.total(yr, |x| x.kind == Kind::Refund && at("Amazon")(x));
    v.push(q(
        "How much did I really spend on Amazon in 2025, after returns?",
        t,
        &["refunds"],
        vec![
            money_out("purchases", &gross),
            money("refunds", &back, Rule::Sum),
            spending("net", &b.spent(yr, at("Amazon"))),
        ],
        None,
    ));
    v.push(q(
        "How much did I get back in refunds in 2025?",
        t,
        &["refunds"],
        vec![money(
            "total",
            &b.total(yr, |x| x.kind == Kind::Refund),
            Rule::Sum,
        )],
        None,
    ));
    let eur: Vec<&Fact> = b
        .facts
        .iter()
        .filter(|x| x.counted() && x.original.as_ref().is_some_and(|o| o.1 == "EUR"))
        .collect();
    v.push(q(
        "How much did I spend in euros?",
        t,
        &["multi_currency"],
        vec![
            eur_money("euros", -eur.iter().map(|x| x.original.as_ref().expect("EUR").0).sum::<i64>(), eur.iter().map(|x| x.id).collect()),
            Answer::Money {
                name: "dollars".into(),
                amount: usd(-eur.iter().map(|x| x.amount).sum::<i64>()),
                evidence: Evidence { rule: Rule::NegatedSum, transaction_ids: eur.iter().map(|x| x.id).collect() },
            },
        ],
        Some("Purchases made in euros, in euros as the bank recorded them and in the dollars charged. Fees are separate."),
    ));
    let trip = b.spent(all_time(), |x| {
        x.has_tag("trip") || x.has_tag("foreign_fee") || at("TAP Air Portugal")(x)
    });
    v.push(q(
        "What did the Lisbon trip cost me altogether, in dollars?",
        t,
        &["multi_currency", "trip", "one_off"],
        vec![spending("total", &trip)],
        Some("Flights, the apartment, everything bought there and the foreign transaction fees."),
    ));
    v.push(q(
        "How much have I paid in foreign transaction fees?",
        t,
        &["multi_currency", "fees"],
        vec![spending(
            "total",
            &b.spent(all_time(), |x| x.has_tag("foreign_fee")),
        )],
        None,
    ));
    v.push(q(
        "How much credit card interest have I paid?",
        t,
        &["interest", "cards"],
        vec![spending(
            "total",
            &b.spent(all_time(), key("financial.interest")),
        )],
        None,
    ));
    let mut months: Vec<String> = b
        .spent_by(all_time(), key("financial.interest"), month_key)
        .into_iter()
        .map(|m| m.0)
        .collect();
    months.sort();
    v.push(q(
        "In which months was I charged card interest?",
        t,
        &["interest", "cards"],
        vec![Answer::Texts {
            name: "months".into(),
            values: months,
        }],
        Some("Months are YYYY-MM, in order."),
    ));
    let usual = b.spent(yr, |x| !x.one_off);
    let one_offs = b.spent(yr, |x| x.one_off);
    v.push(q(
        "What did I spend in 2025 if I leave out the one-off big expenses?",
        t,
        &["one_off", "baseline"],
        vec![spending("without_one_offs", &usual), spending("one_offs", &one_offs)],
        Some("One-offs: large irregular costs a baseline should leave out (SPEC decision 32), as the ground truth marks them."),
    ));
    v.push(q(
        "How much did I put toward retirement in 2025?",
        t,
        &["transfers"],
        vec![money_out("total", &b.total(yr, at("Vanguard")))],
        Some("SEP IRA contributions: transfers to my own account, not spending."),
    ));
    let irs = b.spent(yr, at("IRS"));
    v.push(q(
        "How much did I pay the IRS in 2025, everything included?",
        t,
        &["taxes"],
        vec![
            spending("total", &irs),
            count(
                "payments",
                &b.count(yr, |x| x.kind == Kind::Spending && at("IRS")(x)),
            ),
        ],
        Some("Estimated payments plus the April balance due."),
    ));
    let q1 = Period::quarter(2026, 1);
    v.push(q(
        "How much did I earn in the first quarter of 2026, and how much money came into checking?",
        t,
        &["income", "transfers"],
        vec![
            money(
                "income",
                &b.earned(q1, |x| x.account == CHECKING),
                Rule::Sum,
            ),
            money(
                "money_in",
                &b.total(q1, |x| x.account == CHECKING && x.amount > 0),
                Rule::Sum,
            ),
        ],
        Some("Money in also includes transfers from savings, which aren't income."),
    ));
    let rest: Vec<&Fact> = b
        .facts
        .iter()
        .filter(|x| x.counted() && x.has_tag("trip") && key("food.restaurants")(x))
        .collect();
    v.push(q(
        "What did I spend at restaurants in Lisbon, in euros and in dollars?",
        t,
        &["multi_currency", "trip"],
        vec![
            eur_money(
                "euros",
                -rest
                    .iter()
                    .map(|x| x.original.as_ref().expect("EUR").0)
                    .sum::<i64>(),
                rest.iter().map(|x| x.id).collect(),
            ),
            Answer::Money {
                name: "dollars".into(),
                amount: usd(-rest.iter().map(|x| x.amount).sum::<i64>()),
                evidence: Evidence {
                    rule: Rule::NegatedSum,
                    transaction_ids: rest.iter().map(|x| x.id).collect(),
                },
            },
        ],
        Some("Dollars as charged, without the separate fee lines."),
    ));
    let refund = b
        .facts
        .iter()
        .filter(|x| x.counted() && yr.contains(x.date()) && x.kind == Kind::Refund)
        .max_by_key(|x| (x.amount, -x.id))
        .expect("refunds in 2025");
    v.push(q(
        "What was the biggest refund I got in 2025, and from whom?",
        t,
        &["refunds", "ranking"],
        vec![
            text("merchant", refund.merchant.clone().unwrap_or_default()),
            money(
                "amount",
                &Sum {
                    minor: refund.amount,
                    ids: vec![refund.id],
                },
                Rule::Sum,
            ),
        ],
        None,
    ));
    v.push(q(
        "How much did cash cost me in 2025, withdrawals plus ATM fees?",
        t,
        &["fees"],
        vec![
            spending(
                "withdrawals",
                &b.spent(yr, key("financial.cash_withdrawal")),
            ),
            spending(
                "atm_fees",
                &b.spent(yr, |x| {
                    key("financial.bank_fees")(x) && x.account == CHECKING
                }),
            ),
        ],
        None,
    ));
    let aside = b.total(yr, |x| x.has_tag("tax_set_aside") && x.account == CHECKING);
    v.push(q(
        "How much did I set aside for taxes in 2025? Does that count as spending?",
        t,
        &["transfers", "taxes"],
        vec![money_out("set_aside", &aside), spending("spending_effect", &Sum::default())],
        Some("Set-asides move money to my own savings: transfers, so they add nothing to spending (spending_effect is 0)."),
    ));
}
