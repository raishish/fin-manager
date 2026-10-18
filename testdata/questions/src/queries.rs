//! The expected structured query (T4.1) for each question: what a perfect interpreter
//! would write. `tests/answers.rs` runs each one through `fm_core::query` on the
//! labelled ledger and checks it gives the question's answers.
//!
//! Two kinds of entry besides a plain query:
//! - **proxy**: the answers were computed from a ground-truth tag or series the ledger
//!   doesn't have, and the query reaches the same transactions another way. A model
//!   can't be expected to guess these, so T4.2 reports accuracy with and without them.
//! - **null**: the question can't be expressed with what the ledger records.

use fm_core::model::{AccountKind, Date, YearMonth};
use fm_core::query::*;

pub enum Expected {
    Query(Query),
    Proxy(Query, &'static str),
    Null(&'static str),
}

use AccountKind::{Checking, CreditCard, Savings};
use Measure::*;

fn ym(y: i32, m: u8) -> YearMonth {
    YearMonth::new(y, m).expect("valid month")
}

fn year(y: i32) -> PeriodSpec {
    PeriodSpec::Year(y)
}

fn month(y: i32, m: u8) -> PeriodSpec {
    PeriodSpec::Month(ym(y, m))
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

fn range(from: (i32, u8, u8), to: (i32, u8, u8)) -> PeriodSpec {
    let d = |(y, m, day): (i32, u8, u8)| Date::from_ymd(y, m, day).expect("valid date");
    PeriodSpec::Range {
        from: d(from),
        to: d(to),
    }
}

fn s(period: PeriodSpec) -> Scope {
    Scope {
        period,
        ..Scope::default()
    }
}

/// Narrowing a scope, one field at a time.
trait Narrow {
    fn cat(self, key: &str) -> Scope;
    fn group(self, key: &str) -> Scope;
    fn at(self, merchant: &str) -> Scope;
    fn on(self, kind: AccountKind) -> Scope;
    fn to(self, kind: AccountKind) -> Scope;
    fn says(self, words: &str) -> Scope;
    fn paid_in(self, currency: &str) -> Scope;
    fn in_original(self) -> Scope;
}

impl Narrow for Scope {
    fn cat(mut self, key: &str) -> Scope {
        self.categories = Some(vec![key.into()]);
        self
    }
    fn group(mut self, key: &str) -> Scope {
        self.groups = Some(vec![key.into()]);
        self
    }
    fn at(mut self, merchant: &str) -> Scope {
        self.merchant = Some(merchant.into());
        self
    }
    fn on(mut self, kind: AccountKind) -> Scope {
        self.accounts = Some(AccountSelect {
            kinds: vec![kind],
            ids: vec![],
        });
        self
    }
    fn to(mut self, kind: AccountKind) -> Scope {
        self.counterpart_accounts = Some(AccountSelect {
            kinds: vec![kind],
            ids: vec![],
        });
        self
    }
    fn says(mut self, words: &str) -> Scope {
        self.description = Some(words.into());
        self
    }
    fn paid_in(mut self, currency: &str) -> Scope {
        self.original_currency = Some(currency.into());
        self
    }
    fn in_original(mut self) -> Scope {
        self.amounts = Amounts::Original;
        self
    }
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

fn count(name: &str, measure: Measure, scope: Scope) -> Item {
    item(name, Ask::Count { measure, scope })
}

fn average(name: &str, measure: Measure, scope: Scope, per: Per) -> Item {
    item(
        name,
        Ask::Average {
            measure,
            scope,
            per,
        },
    )
}

/// The group at `position` (or the top `n` labels) of a ranking.
#[allow(
    clippy::too_many_arguments,
    reason = "one argument per field of Ask::Rank, so each query reads like its JSON"
)]
fn rank(
    name: &str,
    measure: Measure,
    scope: Scope,
    by: By,
    order: Order,
    metric: Metric,
    output: RankOutput,
    position: usize,
) -> Item {
    item(
        name,
        Ask::Rank {
            measure,
            scope,
            by,
            order,
            metric,
            n: if output == RankOutput::Labels {
                position
            } else {
                1
            },
            output,
            position: if output == RankOutput::Labels {
                1
            } else {
                position
            },
        },
    )
}

fn pick(name: &str, measure: Measure, scope: Scope, order: PickOrder, output: PickOutput) -> Item {
    item(
        name,
        Ask::Pick {
            measure,
            scope,
            order,
            output,
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

fn derive(name: &str, formula: DeriveOp, of: &[&str]) -> Item {
    item(
        name,
        Ask::Derive {
            formula,
            of: of.iter().map(|s| s.to_string()).collect(),
        },
    )
}

fn difference(name: &str, a: &str, b: &str) -> Item {
    derive(name, DeriveOp::Difference, &[a, b])
}

fn q(items: Vec<Item>) -> Expected {
    Expected::Query(Query { items })
}

fn proxy(why: &'static str, items: Vec<Item>) -> Expected {
    Expected::Proxy(Query { items }, why)
}

fn spent(p: PeriodSpec) -> Scope {
    s(p)
}

/// The expected query for question `id`.
pub fn expected(id: &str) -> Expected {
    use By::{Merchant as ByMerchant, Month as ByMonth};
    use Metric::{Amount as ByAmount, Count as ByCount};
    use Order::{Highest, Lowest};
    use PeriodSpec::{AllTime, LastMonths};
    use RankOutput::{Amount as AmountAt, Count as CountAt, Label, Labels};
    let total = |scope: Scope| q(vec![sum("total", Spending, scope)]);
    match id {
        // ---- totals
        "q001" => total(spent(year(2025))),
        "q002" => total(spent(year(2025)).cat("food.groceries")),
        "q003" => total(spent(quarter(2026, 1)).cat("food.restaurants")),
        "q004" => total(spent(year(2025)).cat("food.coffee")),
        "q005" => total(spent(year(2025)).cat("housing.rent")),
        "q006" => total(spent(months(2025, 6, 8)).group("utilities")),
        "q007" => total(spent(month(2025, 7)).cat("utilities.electricity")),
        "q008" => total(spent(month(2025, 12)).group("shopping")),
        "q009" => total(spent(year(2025)).cat("transport.fuel")),
        "q010" => total(spent(PeriodSpec::YearToDate).cat("insurance.health")),
        "q011" => total(spent(year(2025)).cat("services.software")),
        "q012" => total(spent(year(2025)).cat("health.fitness")),
        "q013" => total(spent(year(2025)).group("transport")),
        "q014" => total(spent(year(2025)).group("food")),
        "q015" => q(vec![sum("total", Income, s(year(2025)))]),
        "q016" => q(vec![sum("total", Income, s(month(2025, 1)))]),
        "q017" => q(vec![sum(
            "total",
            Income,
            s(year(2025)).at("Tailspin Toys"),
        )]),
        "q018" => total(spent(year(2025)).cat("financial.cash_withdrawal")),
        "q019" => total(spent(year(2025)).cat("taxes.income")),
        "q020" => total(spent(year(2025)).cat("food.bars")),

        // ---- comparisons
        "q021" => q(vec![
            sum(
                "q4_2025",
                Spending,
                s(quarter(2025, 4)).cat("food.groceries"),
            ),
            sum(
                "q4_2024",
                Spending,
                s(quarter(2024, 4)).cat("food.groceries"),
            ),
            difference("difference", "q4_2025", "q4_2024"),
        ]),
        "q022" => q(vec![
            sum("september_2026", Spending, s(month(2026, 9))),
            sum("august_2026", Spending, s(month(2026, 8))),
            difference("difference", "september_2026", "august_2026"),
            derive(
                "change",
                DeriveOp::PercentChange,
                &["september_2026", "august_2026"],
            ),
        ]),
        "q023" => q(vec![
            sum(
                "h1_2026",
                Spending,
                s(months(2026, 1, 6)).cat("food.restaurants"),
            ),
            sum(
                "h1_2025",
                Spending,
                s(months(2025, 1, 6)).cat("food.restaurants"),
            ),
            difference("difference", "h1_2026", "h1_2025"),
        ]),
        "q024" => q(vec![
            sum("income", Income, s(year(2025))),
            sum("spending", Spending, s(year(2025))),
            difference("left_over", "income", "spending"),
        ]),
        "q025" => {
            let r = |name, out| {
                rank(
                    name,
                    Spending,
                    s(year(2025)),
                    ByMonth,
                    Highest,
                    ByAmount,
                    out,
                    1,
                )
            };
            q(vec![r("month", Label), r("total", AmountAt)])
        }
        "q026" => {
            let r = |name, out| {
                rank(
                    name,
                    Income,
                    s(year(2025)),
                    ByMonth,
                    Lowest,
                    ByAmount,
                    out,
                    1,
                )
            };
            q(vec![r("month", Label), r("total", AmountAt)])
        }
        "q027" => q(vec![
            derive("more", DeriveOp::Larger, &["coffee", "bars"]),
            sum("coffee", Spending, s(year(2025)).cat("food.coffee")),
            sum("bars", Spending, s(year(2025)).cat("food.bars")),
        ]),
        "q028" => {
            let r = |name, order, out| {
                let scope = s(year(2025)).cat("utilities.electricity");
                rank(name, Spending, scope, ByMonth, order, ByAmount, out, 1)
            };
            q(vec![
                r("highest_month", Highest, Label),
                r("highest", Highest, AmountAt),
                r("lowest_month", Lowest, Label),
                r("lowest", Lowest, AmountAt),
            ])
        }
        "q029" => q(vec![
            average(
                "average",
                Spending,
                s(year(2025)).cat("food.groceries"),
                Per::Month,
            ),
            sum("total", Spending, s(year(2025)).cat("food.groceries")),
        ]),
        "q030" => q(vec![
            derive("share", DeriveOp::Share, &["rent", "total_spending"]),
            sum("rent", Spending, s(year(2025)).cat("housing.rent")),
            sum("total_spending", Spending, s(year(2025))),
        ]),
        "q031" => q(vec![
            sum("shell", Spending, s(year(2025)).at("Shell")),
            sum("chevron", Spending, s(year(2025)).at("Chevron")),
            difference("difference", "shell", "chevron"),
        ]),
        "q032" => q(vec![
            sum(
                "2026_jan_sep",
                Spending,
                s(months(2026, 1, 9)).cat("services.software"),
            ),
            sum(
                "2025_jan_sep",
                Spending,
                s(months(2025, 1, 9)).cat("services.software"),
            ),
            difference("difference", "2026_jan_sep", "2025_jan_sep"),
        ]),
        "q033" => q(vec![
            sum("q2_2026", Spending, s(quarter(2026, 2))),
            sum("q2_2025", Spending, s(quarter(2025, 2))),
            difference("difference", "q2_2026", "q2_2025"),
        ]),
        "q034" => q(vec![
            sum(
                "september_2026",
                Spending,
                s(month(2026, 9)).cat("food.groceries"),
            ),
            sum(
                "august_2026",
                Spending,
                s(month(2026, 8)).cat("food.groceries"),
            ),
            difference("difference", "september_2026", "august_2026"),
            derive(
                "change",
                DeriveOp::PercentChange,
                &["september_2026", "august_2026"],
            ),
        ]),
        "q035" => q(vec![item(
            "median",
            Ask::Median {
                measure: Spending,
                scope: s(LastMonths(6)),
                per: Per::Month,
            },
        )]),

        // ---- merchants
        "q036" => total(spent(year(2025)).at("H-E-B")),
        "q037" => {
            let r = |name, out, position| {
                rank(
                    name,
                    Spending,
                    s(year(2025)),
                    ByMerchant,
                    Highest,
                    ByAmount,
                    out,
                    position,
                )
            };
            q(vec![
                r("merchants", Labels, 3),
                r("total_1", AmountAt, 1),
                r("total_2", AmountAt, 2),
                r("total_3", AmountAt, 3),
            ])
        }
        "q038" => q(vec![count(
            "visits",
            Purchases,
            s(year(2025)).at("Cosmic Cup Coffee"),
        )]),
        "q039" => q(vec![
            average(
                "average",
                Spending,
                s(year(2025)).at("Uber"),
                Per::Transaction,
            ),
            count("rides", Purchases, s(year(2025)).at("Uber")),
            sum("total", Spending, s(year(2025)).at("Uber")),
        ]),
        "q040" => {
            let p = |name, out| {
                pick(
                    name,
                    Purchases,
                    s(year(2025)).on(CreditCard),
                    PickOrder::Largest,
                    out,
                )
            };
            q(vec![
                p("merchant", PickOutput::Merchant),
                p("amount", PickOutput::Amount),
                p("date", PickOutput::Date),
            ])
        }
        "q041" => q(vec![item(
            "restaurants",
            Ask::Distinct {
                measure: Purchases,
                scope: s(year(2025)).cat("food.restaurants"),
                by: ByMerchant,
            },
        )]),
        "q042" => q(vec![
            sum(
                "whole_foods",
                Spending,
                s(year(2025)).at("Whole Foods Market"),
            ),
            sum("heb", Spending, s(year(2025)).at("H-E-B")),
            difference("difference", "whole_foods", "heb"),
        ]),
        "q043" => {
            let p = |name, out| {
                pick(
                    name,
                    Spending,
                    s(AllTime).at("Capital Factory"),
                    PickOrder::Latest,
                    out,
                )
            };
            q(vec![
                p("date", PickOutput::Date),
                p("amount", PickOutput::Amount),
            ])
        }
        "q044" => {
            let p = |name, out| {
                let scope = s(AllTime).cat("transport.auto_maintenance");
                pick(name, Purchases, scope, PickOrder::Largest, out)
            };
            proxy(
                "the question doesn't name the shop: the tires are the largest auto-maintenance purchase",
                vec![p("date", PickOutput::Date), p("amount", PickOutput::Amount)],
            )
        }
        "q045" => total(spent(year(2026)).at("Lone Star Smokehouse")),
        "q046" => {
            let r = |name, out| {
                rank(
                    name,
                    Purchases,
                    s(year(2025)),
                    ByMerchant,
                    Highest,
                    ByCount,
                    out,
                    1,
                )
            };
            q(vec![r("merchant", Label), r("purchases", CountAt)])
        }
        "q047" => total(spent(year(2025)).at("Target")),
        "q048" => total(spent(year(2025)).at("T-Mobile")),
        "q049" => total(spent(quarter(2025, 4)).at("Amazon")),
        "q050" => total(spent(year(2025)).at("Austin Energy")),

        // ---- recurring
        "q051" => q(vec![series(
            "monthly",
            named("Adobe"),
            SeriesOutput::Latest,
        )]),
        "q052" => q(vec![
            series("from", named("Adobe"), SeriesOutput::PriceFrom),
            series("old", named("Adobe"), SeriesOutput::PriceBefore),
            series("new", named("Adobe"), SeriesOutput::PriceAfter),
            difference("increase", "new", "old"),
        ]),
        "q053" => {
            let select = SeriesSelect {
                kinds: Some(vec![SeriesKindSel::Subscription]),
                cadences: Some(vec![CadenceSel::Monthly]),
                status: SeriesStatus::Active,
                ..SeriesSelect::default()
            };
            q(vec![
                series("monthly_total", select.clone(), SeriesOutput::LatestTotal),
                series("subscriptions", select, SeriesOutput::Names),
            ])
        }
        "q054" => q(vec![series(
            "cancelled",
            SeriesSelect {
                kinds: Some(vec![SeriesKindSel::Subscription]),
                status: SeriesStatus::Cancelled,
                ..SeriesSelect::default()
            },
            SeriesOutput::Names,
        )]),
        "q055" => total(spent(AllTime).at("Zoom")),
        "q056" => q(vec![
            series(
                "subscriptions",
                SeriesSelect {
                    kinds: Some(vec![SeriesKindSel::Subscription]),
                    cadences: Some(vec![CadenceSel::Annual]),
                    ..SeriesSelect::default()
                },
                SeriesOutput::Names,
            ),
            series("dropbox", named("Dropbox"), SeriesOutput::Latest),
            series("figma", named("Figma"), SeriesOutput::Latest),
        ]),
        "q057" => {
            let rent = || SeriesSelect {
                categories: Some(vec!["housing.rent".into()]),
                ..SeriesSelect::default()
            };
            q(vec![
                series("from", rent(), SeriesOutput::PriceFrom),
                series("old", rent(), SeriesOutput::PriceBefore),
                series("new", rent(), SeriesOutput::PriceAfter),
            ])
        }
        "q058" => proxy(
            "truth counts the health-insurance series; the ledger reaches the same transactions through the category",
            vec![sum("total", Spending, s(AllTime).cat("insurance.health"))],
        ),
        "q059" => proxy(
            "truth counts the electricity series; the ledger reaches the same transactions through the category",
            vec![sum(
                "total",
                Spending,
                s(month(2026, 8)).cat("utilities.electricity"),
            )],
        ),
        "q060" => {
            let e = || s(year(2025)).cat("utilities.electricity");
            proxy(
                "truth counts the electricity series; the ledger reaches the same transactions through the category",
                vec![
                    average("average", Spending, e(), Per::Transaction),
                    count("bills", Purchases, e()),
                    sum("total", Spending, e()),
                ],
            )
        }
        "q061" => total(spent(AllTime).at("Coursera")),
        "q062" => q(vec![series(
            "recurring",
            SeriesSelect {
                kinds: Some(vec![SeriesKindSel::Subscription, SeriesKindSel::Bill]),
                status: SeriesStatus::Active,
                ..SeriesSelect::default()
            },
            SeriesOutput::Count,
        )]),
        "q063" => q(vec![series(
            "monthly",
            SeriesSelect {
                kinds: Some(vec![SeriesKindSel::Income]),
                cadences: Some(vec![CadenceSel::Monthly]),
                status: SeriesStatus::Active,
                ..SeriesSelect::default()
            },
            SeriesOutput::Latest,
        )]),
        "q064" => proxy(
            "truth counts the savings-interest series; the ledger reaches the same transactions as income on the savings account",
            vec![sum("total", Income, s(year(2025)).on(Savings))],
        ),
        "q065" => {
            let irs = || SeriesSelect {
                name: Some("IRS".into()),
                cadences: Some(vec![CadenceSel::Quarterly]),
                period: Some(year(2025)),
                ..SeriesSelect::default()
            };
            q(vec![
                series("payments", irs(), SeriesOutput::MemberCount),
                series("total", irs(), SeriesOutput::MemberTotal),
            ])
        }

        // ---- time ranges
        "q066" => total(spent(LastMonths(1))),
        "q067" => total(spent(PeriodSpec::YearToDate)),
        "q068" => total(spent(LastMonths(3)).cat("food.groceries")),
        "q069" => total(spent(PeriodSpec::LastDays(14))),
        "q070" => total(spent(PeriodSpec::LastWeek)),
        "q071" => q(vec![sum("total", Income, s(PeriodSpec::LastQuarter))]),
        "q072" => total(spent(PeriodSpec::YearToDate).cat("food.restaurants")),
        "q073" => total(spent(months(2025, 1, 6))),
        "q074" => total(spent(LastMonths(12))),
        "q075" => total(spent(range((2025, 11, 28), (2025, 12, 25)))),
        "q076" => total(spent(month(2025, 4))),
        "q077" => total(spent(PeriodSpec::YearToDate).cat("services.software")),
        "q078" => total(spent(LastMonths(2)).cat("food.coffee")),
        "q079" => total(spent(quarter(2024, 4))),
        "q080" => total(spent(LastMonths(6)).cat("transport.fuel")),

        // ---- hard
        "q081" => q(vec![
            count("pending", Pending, s(AllTime)),
            sum("total", Pending, s(AllTime)),
        ]),
        "q082" => q(vec![sum("total", TransfersIn, s(year(2025)).on(Savings))]),
        "q083" => total(spent(year(2025)).on(CreditCard)),
        "q084" => q(vec![sum(
            "total",
            TransfersIn,
            s(year(2025)).on(CreditCard),
        )]),
        "q085" => q(vec![
            sum("money_out", MoneyOut, s(year(2025)).on(Checking)),
            sum("spending", Spending, s(year(2025)).on(Checking)),
            difference("not_spending", "money_out", "spending"),
        ]),
        "q086" => q(vec![
            sum("purchases", Purchases, s(year(2025)).at("Amazon")),
            sum("refunds", Refunds, s(year(2025)).at("Amazon")),
            sum("net", Spending, s(year(2025)).at("Amazon")),
        ]),
        "q087" => q(vec![sum("total", Refunds, s(year(2025)))]),
        "q088" => q(vec![
            sum("euros", Spending, s(AllTime).paid_in("EUR").in_original()),
            sum("dollars", Spending, s(AllTime).paid_in("EUR")),
        ]),
        "q089" => Expected::Null(
            "which payments belong to a trip is a tag, and tags are a later increment (SPEC: no tags in the MVP)",
        ),
        "q090" => proxy(
            "truth tags the fee lines; the ledger finds them by category and the bank's description",
            vec![sum(
                "total",
                Spending,
                s(AllTime)
                    .cat("financial.bank_fees")
                    .says("foreign transaction fee"),
            )],
        ),
        "q091" => total(spent(AllTime).cat("financial.interest")),
        "q092" => q(vec![item(
            "months",
            Ask::Keys {
                measure: Spending,
                scope: s(AllTime).cat("financial.interest"),
                by: ByMonth,
            },
        )]),
        "q093" => Expected::Null(
            "one-off marks are a user input from decision 32's planning work and aren't in the data model yet",
        ),
        "q094" => proxy(
            "truth names the retirement account's payee; the ledger reaches it as transfers out to that merchant",
            vec![sum("total", TransfersOut, s(year(2025)).at("Vanguard"))],
        ),
        "q095" => q(vec![
            sum("total", Spending, s(year(2025)).at("IRS")),
            count("payments", Purchases, s(year(2025)).at("IRS")),
        ]),
        "q096" => q(vec![
            sum("income", Income, s(quarter(2026, 1)).on(Checking)),
            sum("money_in", MoneyIn, s(quarter(2026, 1)).on(Checking)),
        ]),
        "q097" => {
            let lisbon = || s(AllTime).cat("food.restaurants").paid_in("EUR");
            proxy(
                "truth tags the trip; the ledger finds the Lisbon restaurants as restaurants paid in euros",
                vec![
                    sum("euros", Spending, lisbon().in_original()),
                    sum("dollars", Spending, lisbon()),
                ],
            )
        }
        "q098" => {
            let p = |name, out| pick(name, Refunds, s(year(2025)), PickOrder::Largest, out);
            q(vec![
                p("merchant", PickOutput::Merchant),
                p("amount", PickOutput::Amount),
            ])
        }
        "q099" => q(vec![
            sum(
                "withdrawals",
                Spending,
                s(year(2025)).cat("financial.cash_withdrawal"),
            ),
            sum(
                "atm_fees",
                Spending,
                s(year(2025)).cat("financial.bank_fees").on(Checking),
            ),
        ]),
        "q100" => {
            let aside = || s(year(2025)).on(Checking).to(Savings);
            proxy(
                "truth tags the set-asides; the ledger finds them as transfers from checking to savings",
                vec![
                    sum("set_aside", TransfersOut, aside()),
                    sum("spending_effect", Spending, aside()),
                ],
            )
        }
        other => panic!("no expected query for {other}"),
    }
}
