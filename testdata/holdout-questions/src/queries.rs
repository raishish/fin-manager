//! The expected structured query (T4.1) for each held-out question: what a perfect
//! interpreter would write. `tests/answers.rs` runs each one through `fm_core::query` on
//! the labelled ledger and checks it gives the question's answers.
//!
//! A **proxy** entry, as in TD.3, is one whose answers came from a ground-truth series
//! the question names only indirectly, and the query reaches the same transactions by a
//! name the question doesn't say. T4.2 reports accuracy with and without proxies.

use fm_core::model::{AccountKind, YearMonth};
use fm_core::query::*;

pub enum Expected {
    Query(Query),
    Proxy(Query, &'static str),
}

use AccountKind::{Checking, CreditCard, Savings};
use Measure::*;

fn ym(y: i32, m: u8) -> YearMonth {
    YearMonth::new(y, m).expect("valid month")
}

fn year(y: i32) -> PeriodSpec {
    PeriodSpec::Year(y)
}

fn months(y: i32, from: u8, to: u8) -> PeriodSpec {
    PeriodSpec::Months {
        from: ym(y, from),
        to: ym(y, to),
    }
}

fn quarter(y: i32, q: u8) -> PeriodSpec {
    PeriodSpec::Quarter {
        year: y,
        quarter: q,
    }
}

fn s(period: PeriodSpec) -> Scope {
    Scope {
        period,
        ..Scope::default()
    }
}

fn cat(mut scope: Scope, key: &str) -> Scope {
    scope.categories = Some(vec![key.into()]);
    scope
}

fn at(mut scope: Scope, merchant: &str) -> Scope {
    scope.merchant = Some(merchant.into());
    scope
}

fn on(mut scope: Scope, kind: AccountKind) -> Scope {
    scope.accounts = Some(AccountSelect {
        kinds: vec![kind],
        ids: vec![],
    });
    scope
}

fn to(mut scope: Scope, kind: AccountKind) -> Scope {
    scope.counterpart_accounts = Some(AccountSelect {
        kinds: vec![kind],
        ids: vec![],
    });
    scope
}

fn item(name: &str, ask: Ask) -> Item {
    Item {
        name: name.into(),
        ask,
    }
}

fn sum(name: &str, measure: Measure, scope: Scope) -> Item {
    item(name, Ask::Sum { measure, scope })
}

fn derive(name: &str, formula: DeriveOp, of: &[&str]) -> Item {
    item(
        name,
        Ask::Derive {
            formula,
            of: of.iter().map(|s| s.to_string()).collect(),
        },
    )
}

fn series(name: &str, select: SeriesSelect, output: SeriesOutput) -> Item {
    item(name, Ask::Recurring { select, output })
}

fn named(n: &str) -> SeriesSelect {
    SeriesSelect {
        name: Some(n.into()),
        ..SeriesSelect::default()
    }
}

fn q(items: Vec<Item>) -> Expected {
    Expected::Query(Query { items })
}

/// The expected query for question `id`.
pub fn expected(id: &str) -> Expected {
    use PeriodSpec::{AllTime, LastDays, LastQuarter, YearToDate};
    let total = |scope: Scope| q(vec![sum("total", Spending, scope)]);
    match id {
        // ---- totals
        "h01" => total(cat(s(year(2025)), "food.fast_food")),
        "h02" => total(cat(s(year(2025)), "utilities.internet")),
        "h03" => total(cat(s(YearToDate), "services.shipping")),
        "h04" => total(cat(s(year(2025)), "personal_care.hair_beauty")),
        "h05" => q(vec![sum("total", Income, s(quarter(2026, 2)))]),

        // ---- comparisons
        "h06" => q(vec![
            derive("more", DeriveOp::Larger, &["fast_food", "coffee"]),
            sum("fast_food", Spending, cat(s(YearToDate), "food.fast_food")),
            sum("coffee", Spending, cat(s(YearToDate), "food.coffee")),
        ]),
        "h07" => q(vec![
            sum(
                "2026_jan_sep",
                Spending,
                cat(s(months(2026, 1, 9)), "transport.rideshare"),
            ),
            sum(
                "2025_jan_sep",
                Spending,
                cat(s(months(2025, 1, 9)), "transport.rideshare"),
            ),
            derive(
                "difference",
                DeriveOp::Difference,
                &["2026_jan_sep", "2025_jan_sep"],
            ),
        ]),
        "h08" => {
            let r = |name: &str, output| {
                item(
                    name,
                    Ask::Rank {
                        measure: Spending,
                        scope: s(YearToDate),
                        by: By::Month,
                        order: Order::Lowest,
                        metric: Metric::Amount,
                        n: 1,
                        output,
                        position: 1,
                    },
                )
            };
            q(vec![
                r("month", RankOutput::Label),
                r("total", RankOutput::Amount),
            ])
        }
        "h09" => q(vec![
            derive("share", DeriveOp::Share, &["software", "total_spending"]),
            sum(
                "software",
                Spending,
                cat(s(year(2025)), "services.software"),
            ),
            sum("total_spending", Spending, s(year(2025))),
        ]),

        // ---- merchants
        "h10" => total(at(s(AllTime), "Starbucks")),
        "h11" => q(vec![item(
            "visits",
            Ask::Count {
                measure: Purchases,
                scope: at(s(year(2025)), "Black Swan Yoga"),
            },
        )]),
        "h12" => {
            let p = |name: &str, output| {
                item(
                    name,
                    Ask::Pick {
                        measure: Purchases,
                        scope: at(s(AllTime), "REI"),
                        order: PickOrder::Largest,
                        output,
                    },
                )
            };
            q(vec![
                p("amount", PickOutput::Amount),
                p("date", PickOutput::Date),
            ])
        }
        "h13" => {
            let r = |name: &str, output, n: usize, position: usize| {
                item(
                    name,
                    Ask::Rank {
                        measure: Spending,
                        scope: s(PeriodSpec::LastMonths(1)),
                        by: By::Merchant,
                        order: Order::Highest,
                        metric: Metric::Amount,
                        n,
                        output,
                        position,
                    },
                )
            };
            q(vec![
                r("merchants", RankOutput::Labels, 2, 1),
                r("total_1", RankOutput::Amount, 1, 1),
                r("total_2", RankOutput::Amount, 1, 2),
            ])
        }

        // ---- recurring
        "h14" => q(vec![series(
            "monthly",
            named("T-Mobile"),
            SeriesOutput::Latest,
        )]),
        "h15" => {
            let health = || SeriesSelect {
                categories: Some(vec!["insurance.health".into()]),
                ..SeriesSelect::default()
            };
            q(vec![
                series("from", health(), SeriesOutput::PriceFrom),
                series("old", health(), SeriesOutput::PriceBefore),
                series("new", health(), SeriesOutput::PriceAfter),
            ])
        }
        "h16" => q(vec![
            series("old", named("Google Workspace"), SeriesOutput::PriceBefore),
            series("new", named("Google Workspace"), SeriesOutput::PriceAfter),
            derive("increase", DeriveOp::Difference, &["new", "old"]),
        ]),
        "h17" => Expected::Proxy(
            Query {
                items: vec![series(
                    "monthly",
                    named("Capital Factory"),
                    SeriesOutput::Latest,
                )],
            },
            "the question says \"coworking membership\", not the space's name; its category (professional services) also holds the tax preparer",
        ),

        // ---- time ranges
        "h18" => total(s(LastDays(30))),
        "h19" => total(s(months(2025, 6, 8))),
        "h20" => total(cat(s(LastQuarter), "food.bars")),
        "h21" => total(cat(s(LastDays(90)), "food.groceries")),

        // ---- hard
        "h22" => total(cat(s(year(2025)), "financial.bank_fees")),
        "h23" => q(vec![
            sum("refunds", Refunds, at(s(AllTime), "Best Buy")),
            sum("net", Spending, at(s(AllTime), "Best Buy")),
        ]),
        "h24" => q(vec![sum(
            "total",
            TransfersIn,
            to(on(s(year(2025)), Checking), Savings),
        )]),
        "h25" => q(vec![sum(
            "total",
            TransfersOut,
            to(on(s(YearToDate), Checking), CreditCard),
        )]),
        other => panic!("no expected query for {other}"),
    }
}
