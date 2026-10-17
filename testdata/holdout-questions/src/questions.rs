//! The held-out questions. Each computes its answers from the persona's facts with TD.3's
//! helpers; nothing here is a hand-written number.

use std::collections::HashMap;

use fm_questions::facts::{Fact, Kind, Period};
use fm_questions::*;

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

/// So far this year: from 1 January to today, as TD.3 counts it.
fn this_year() -> Period {
    Period::new(fm_synth::cal::ymd(2026, 1, 1), fm_questions::facts::today())
}

fn y(year: i32) -> Period {
    Period::year(year)
}

fn key(k: &'static str) -> impl Fn(&Fact) -> bool {
    move |x| x.key.as_deref() == Some(k)
}

fn at(m: &'static str) -> impl Fn(&Fact) -> bool {
    move |x| x.merchant.as_deref() == Some(m)
}

fn any(_: &Fact) -> bool {
    true
}

const CHECKING: &str = "Share Draft Checking";
const SAVINGS: &str = "Regular Share Savings";

/// The latest amount of a fixed-price series, as a positive cost or income.
fn latest(book: &Book, id: &str) -> i64 {
    match &book.series(id).amount {
        fm_synth::truth::AmountTruth::Fixed { steps } => {
            steps.last().expect("a step").amount.minor().abs()
        }
        fm_synth::truth::AmountTruth::Variable { max, .. } => max.minor().abs(),
    }
}

/// The steps of a fixed-price series.
fn steps<'a>(book: &'a Book, id: &str) -> &'a [fm_synth::truth::Step] {
    match &book.series(id).amount {
        fm_synth::truth::AmountTruth::Fixed { steps } => steps,
        fm_synth::truth::AmountTruth::Variable { .. } => panic!("{id} has a fixed price"),
    }
}

fn comparison(name_a: &str, a: &Sum, name_b: &str, b: &Sum) -> Vec<Answer> {
    vec![
        spending(name_a, a),
        spending(name_b, b),
        derived("difference", a.minor - b.minor),
    ]
}

/// One fact as a sum of its own.
fn one(x: &Fact) -> Sum {
    Sum {
        minor: -x.amount,
        ids: vec![x.id],
    }
}

/// The account each transfer's other side is in, from truth's transfer pairs.
fn counterpart_accounts(b: &Book) -> HashMap<i64, String> {
    let account: HashMap<i64, &str> = b.facts.iter().map(|x| (x.id, x.account.as_str())).collect();
    b.truth
        .transactions
        .iter()
        .filter_map(|t| match &t.bucket {
            fm_synth::truth::TruthBucket::Transfer {
                counterpart: Some(other),
            } => Some((t.id.0, account.get(&other.0)?.to_string())),
            _ => None,
        })
        .collect()
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
        "What did fast food set me back in 2025?",
        t,
        &[],
        vec![spending("total", &b.spent(y(2025), key("food.fast_food")))],
        None,
    ));
    v.push(q(
        "How much did I pay for internet in 2025?",
        t,
        &[],
        vec![spending(
            "total",
            &b.spent(y(2025), key("utilities.internet")),
        )],
        None,
    ));
    v.push(q(
        "What have I spent on shipping and postage this year?",
        t,
        &["relative_date"],
        vec![spending(
            "total",
            &b.spent(this_year(), key("services.shipping")),
        )],
        Some("This year so far: from 2026-01-01 to today."),
    ));
    v.push(q(
        "How much went on haircuts and beauty in 2025?",
        t,
        &[],
        vec![spending(
            "total",
            &b.spent(y(2025), key("personal_care.hair_beauty")),
        )],
        None,
    ));
    v.push(q(
        "What did I earn between April and June 2026?",
        t,
        &["income"],
        vec![money(
            "total",
            &b.earned(Period::quarter(2026, 2), any),
            Rule::Sum,
        )],
        None,
    ));
}

fn comparisons(b: &Book, v: &mut Vec<Question>) {
    let t = "comparisons";
    let ytd = this_year();
    let fast = b.spent(ytd, key("food.fast_food"));
    let coffee = b.spent(ytd, key("food.coffee"));
    v.push(q(
        "Have I spent more on fast food or on coffee so far this year?",
        t,
        &["relative_date"],
        vec![
            text(
                "more",
                if fast.minor >= coffee.minor {
                    "fast_food"
                } else {
                    "coffee"
                },
            ),
            spending("fast_food", &fast),
            spending("coffee", &coffee),
        ],
        None,
    ));
    let rides26 = b.spent(ytd, key("transport.rideshare"));
    let rides25 = b.spent(Period::months(2025, 1, 9), key("transport.rideshare"));
    v.push(q(
        "Am I spending more on rideshares this year than I had by this point last year?",
        t,
        &["relative_date", "year_over_year"],
        comparison("2026_jan_sep", &rides26, "2025_jan_sep", &rides25),
        Some("January to September of each year."),
    ));
    let by_month = b.spent_by(ytd, any, month_key);
    let low = by_month.last().expect("months in 2026");
    v.push(q(
        "Which month this year was my cheapest?",
        t,
        &["ranking", "relative_date"],
        vec![text("month", low.0.clone()), spending("total", &low.1)],
        Some("The month of 2026 so far with the least spending."),
    ));
    let software = b.spent(y(2025), key("services.software"));
    let out = b.spent(y(2025), any);
    v.push(q(
        "What percentage of what I spent in 2025 was software?",
        t,
        &["percent"],
        vec![
            percent("share", software.minor, out.minor),
            spending("software", &software),
            spending("total_spending", &out),
        ],
        None,
    ));
}

fn merchants(b: &Book, v: &mut Vec<Question>) {
    let t = "merchants";
    v.push(q(
        "How much have I spent at Starbucks, all told?",
        t,
        &["all_time"],
        vec![spending("total", &b.spent(all_time(), at("Starbucks")))],
        None,
    ));
    v.push(q(
        "How many yoga classes did I pay for at Black Swan in 2025?",
        t,
        &["count"],
        vec![count(
            "visits",
            &b.count(y(2025), |x| {
                x.kind == Kind::Spending && at("Black Swan Yoga")(x)
            }),
        )],
        None,
    ));
    let rei = b
        .facts
        .iter()
        .filter(|x| x.counted() && x.kind == Kind::Spending && at("REI")(x))
        .min_by_key(|x| (x.amount, x.id))
        .expect("REI purchases");
    v.push(q(
        "What's the most I've spent at REI in one go, and when was it?",
        t,
        &["ranking", "date"],
        vec![spending("amount", &one(rei)), date("date", rei.date())],
        None,
    ));
    let top = b.spent_by(Period::month(2026, 9), any, |x| x.merchant.clone());
    let mut a = vec![Answer::Texts {
        name: "merchants".into(),
        values: top.iter().take(2).map(|m| m.0.clone()).collect(),
    }];
    for (i, m) in top.iter().take(2).enumerate() {
        a.push(spending(&format!("total_{}", i + 1), &m.1));
    }
    v.push(q(
        "Where did most of my money go last month? Just the top two places.",
        t,
        &["ranking", "relative_date"],
        a,
        Some("Merchants ranked by spending in September 2026, net of refunds."),
    ));
}

fn recurring(b: &Book, v: &mut Vec<Question>) {
    let t = "recurring";
    v.push(q(
        "What's my phone bill each month at the moment?",
        t,
        &[],
        vec![derived("monthly", latest(b, "freelancer.phone"))],
        None,
    ));
    let health = steps(b, "freelancer.health_insurance");
    v.push(q(
        "My health insurance went up at some point. When, and what was it before and after?",
        t,
        &["price_change"],
        vec![
            date("from", health[1].from),
            derived("old", -health[0].amount.minor()),
            derived("new", -health[1].amount.minor()),
        ],
        Some("The date is the first premium at the new price."),
    ));
    let workspace = steps(b, "freelancer.google_workspace");
    v.push(q(
        "By how much did Google Workspace raise its price?",
        t,
        &["price_change", "subscriptions"],
        vec![
            derived("old", -workspace[0].amount.minor()),
            derived("new", -workspace[1].amount.minor()),
            derived(
                "increase",
                workspace[0].amount.minor() - workspace[1].amount.minor(),
            ),
        ],
        None,
    ));
    v.push(q(
        "What does my coworking membership cost per month?",
        t,
        &["subscriptions"],
        vec![derived("monthly", latest(b, "freelancer.coworking"))],
        Some("The coworking space is Capital Factory."),
    ));
}

fn time_ranges(b: &Book, v: &mut Vec<Question>) {
    let t = "time_ranges";
    let rel: &[&str] = &["relative_date"];
    v.push(q(
        "What did I spend in the past 30 days?",
        t,
        rel,
        vec![spending("total", &b.spent(Period::last_days(30), any))],
        Some("The 30 days before today: 2026-09-01 to 2026-09-30."),
    ));
    v.push(q(
        "How much did I spend over the summer of 2025, June through August?",
        t,
        &[],
        vec![spending("total", &b.spent(Period::months(2025, 6, 8), any))],
        None,
    ));
    v.push(q(
        "What did bars cost me last quarter?",
        t,
        rel,
        vec![spending(
            "total",
            &b.spent(Period::quarter(2026, 3), key("food.bars")),
        )],
        Some("Last quarter: July to September 2026."),
    ));
    v.push(q(
        "How much have I spent on groceries in the last 90 days?",
        t,
        rel,
        vec![spending(
            "total",
            &b.spent(Period::last_days(90), key("food.groceries")),
        )],
        Some("The 90 days before today: 2026-07-03 to 2026-09-30."),
    ));
}

fn hard(b: &Book, v: &mut Vec<Question>) {
    let t = "hard";
    v.push(q(
        "How much did bank fees cost me in 2025?",
        t,
        &["fees"],
        vec![spending(
            "total",
            &b.spent(y(2025), key("financial.bank_fees")),
        )],
        Some("ATM and foreign transaction fees; card interest is a separate category."),
    ));
    let back = b.total(all_time(), |x| x.kind == Kind::Refund && at("Best Buy")(x));
    v.push(q(
        "What did I get back from Best Buy, and what did I end up spending there?",
        t,
        &["refunds"],
        vec![
            money("refunds", &back, Rule::Sum),
            spending("net", &b.spent(all_time(), at("Best Buy"))),
        ],
        Some("All time, net of the refund."),
    ));
    let other = counterpart_accounts(b);
    let from_savings = b.total(y(2025), |x| {
        x.account == CHECKING
            && x.kind == Kind::Transfer
            && x.amount > 0
            && other.get(&x.id).map(String::as_str) == Some(SAVINGS)
    });
    v.push(q(
        "How much did I pull out of savings back into checking in 2025?",
        t,
        &["transfers"],
        vec![money("total", &from_savings, Rule::Sum)],
        Some("Transfers in to checking from savings: not income."),
    ));
    let cards: Vec<String> = {
        let mut c: Vec<String> = b
            .facts
            .iter()
            .filter(|x| x.on_card)
            .map(|x| x.account.clone())
            .collect();
        c.sort();
        c.dedup();
        c
    };
    let paid = b.total(this_year(), |x| {
        x.account == CHECKING
            && x.kind == Kind::Transfer
            && x.amount < 0
            && other.get(&x.id).is_some_and(|a| cards.contains(a))
    });
    v.push(q(
        "How much have I paid off on my credit cards from checking this year?",
        t,
        &["transfers", "cards", "relative_date"],
        vec![money_out("total", &paid)],
        Some("Card payments are transfers, not spending."),
    ));
}
