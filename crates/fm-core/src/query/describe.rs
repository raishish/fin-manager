//! A query in plain English, one line per item, for the user to check what was
//! understood (T4.2, shown by the Ask UI in T4.4). Periods are shown resolved
//! against `as_of`, so "last month" reads as the month it means.
//!
//! The text names categories, groups and accounts by their display names, and repeats
//! a merchant name only when the query itself contains one. It never includes
//! amounts.

use super::period::resolve;
use super::{
    AccountSelect, Amounts, Ask, By, DeriveOp, Measure, Metric, Order, Per, PeriodSpec, PickOrder,
    PickOutput, Query, RankOutput, Scope, SeriesKindSel, SeriesOutput, SeriesSelect, SeriesStatus,
};
use crate::model::{AccountKind, Date, Ledger, YearMonth};
use crate::views::Period;

/// The dates `spec` covers when today is `as_of`, as [`run`](super::run) resolves them.
/// `None` for an invalid period.
pub fn resolve_period(ledger: &Ledger, spec: &PeriodSpec, as_of: Date) -> Option<Period> {
    let earliest = ledger
        .transactions
        .iter()
        .map(|t| t.transaction_date.unwrap_or(t.posted_date))
        .min()
        .unwrap_or(as_of);
    resolve(spec, as_of, earliest).ok()
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn month_name(m: YearMonth) -> String {
    format!("{} {}", MONTHS[usize::from(m.month() - 1)], m.year())
}

fn dates(p: Period) -> String {
    if p.from == p.to {
        return p.from.to_string();
    }
    let (fm, tm) = (p.from.year_month(), p.to.year_month());
    let whole = p.from == fm.first_day() && p.to == tm.last_day();
    match (whole, fm == tm) {
        (true, true) => month_name(fm),
        (true, false) if fm.month() == 1 && tm.month() == 12 && fm.year() == tm.year() => {
            fm.year().to_string()
        }
        (true, false) => format!("{} to {}", month_name(fm), month_name(tm)),
        (false, _) => format!("{} to {}", p.from, p.to),
    }
}

fn period(ledger: &Ledger, spec: &PeriodSpec, as_of: Date) -> String {
    let label = match spec {
        PeriodSpec::LastMonths(1) => Some("last month".to_owned()),
        PeriodSpec::LastMonths(n) => Some(format!("the last {n} months")),
        PeriodSpec::LastDays(n) => Some(format!("the last {n} days")),
        PeriodSpec::LastWeek => Some("last week".to_owned()),
        PeriodSpec::LastQuarter => Some("last quarter".to_owned()),
        PeriodSpec::YearToDate => Some("this year so far".to_owned()),
        PeriodSpec::AllTime => Some("all time".to_owned()),
        PeriodSpec::Quarter { year, quarter } => Some(format!("Q{quarter} {year}")),
        _ => None,
    };
    match (label, resolve_period(ledger, spec, as_of)) {
        (Some(l), Some(p)) => format!("{l} ({})", dates(p)),
        (None, Some(p)) => dates(p),
        (Some(l), None) => l,
        (None, None) => "an invalid period".to_owned(),
    }
}

fn measure(m: Measure) -> &'static str {
    match m {
        Measure::Spending => "spending (net of refunds)",
        Measure::Purchases => "purchases (before refunds)",
        Measure::Refunds => "refunds",
        Measure::Income => "income",
        Measure::TransfersIn => "transfers in",
        Measure::TransfersOut => "transfers out",
        Measure::MoneyIn => "all money in",
        Measure::MoneyOut => "all money out",
        Measure::Pending => "pending charges",
    }
}

fn kind(k: AccountKind) -> &'static str {
    match k {
        AccountKind::Checking => "checking",
        AccountKind::Savings => "savings",
        AccountKind::CreditCard => "credit card",
        AccountKind::Cash => "cash",
        AccountKind::Loan => "loan",
        AccountKind::Mortgage => "mortgage",
        AccountKind::Brokerage => "brokerage",
        AccountKind::Retirement => "retirement",
        AccountKind::Other => "other",
    }
}

fn accounts(ledger: &Ledger, s: &AccountSelect) -> String {
    let mut names: Vec<String> = s
        .kinds
        .iter()
        .map(|k| format!("{} accounts", kind(*k)))
        .collect();
    for id in &s.ids {
        names.push(
            ledger
                .accounts
                .iter()
                .find(|a| a.id == *id)
                .map_or_else(|| format!("account {}", id.0), |a| a.name.clone()),
        );
    }
    if names.is_empty() {
        "no accounts".to_owned()
    } else {
        names.join(" or ")
    }
}

fn category_names(ledger: &Ledger, keys: &[String]) -> String {
    keys.iter()
        .map(|k| {
            ledger
                .taxonomy
                .categories
                .iter()
                .find(|c| &c.key == k)
                .map_or_else(|| k.clone(), |c| c.name.clone())
        })
        .collect::<Vec<_>>()
        .join(" or ")
}

fn scope(ledger: &Ledger, s: &Scope, as_of: Date) -> String {
    let mut parts = Vec::new();
    if let Some(keys) = &s.categories {
        parts.push(category_names(ledger, keys));
    }
    if let Some(keys) = &s.groups {
        let names: Vec<String> = keys
            .iter()
            .map(|k| {
                ledger
                    .taxonomy
                    .groups
                    .iter()
                    .find(|g| &g.key == k)
                    .map_or_else(|| k.clone(), |g| g.name.clone())
            })
            .collect();
        parts.push(format!("all of {}", names.join(" or ")));
    }
    if let Some(m) = &s.merchant {
        parts.push(format!("at {m}"));
    }
    if let Some(a) = &s.accounts {
        parts.push(format!("on {}", accounts(ledger, a)));
    }
    if let Some(a) = &s.counterpart_accounts {
        parts.push(format!("to or from {}", accounts(ledger, a)));
    }
    if let Some(d) = &s.description {
        parts.push(format!("described as \"{d}\""));
    }
    if let Some(c) = &s.original_currency {
        parts.push(match s.amounts {
            Amounts::Original => format!("paid in {c}, in {c}"),
            Amounts::Account => format!("paid in {c}"),
        });
    }
    parts.push(period(ledger, &s.period, as_of));
    parts.join(" · ")
}

fn by(b: By) -> &'static str {
    match b {
        By::Category => "category",
        By::Group => "group",
        By::Merchant => "merchant",
        By::Month => "month",
        By::Account => "account",
    }
}

fn series(ledger: &Ledger, s: &SeriesSelect, as_of: Date) -> String {
    let mut parts = Vec::new();
    match s.status {
        SeriesStatus::Active => parts.push("still running".to_owned()),
        SeriesStatus::Ended => parts.push("ended".to_owned()),
        SeriesStatus::Cancelled => parts.push("cancelled for good".to_owned()),
        SeriesStatus::Any => {}
    }
    if let Some(c) = &s.cadences {
        let names: Vec<String> = c.iter().map(|c| format!("{c:?}").to_lowercase()).collect();
        parts.push(names.join(" or "));
    }
    let kinds = s.kinds.as_ref().map_or_else(
        || "recurring payments".to_owned(),
        |k| {
            k.iter()
                .map(|k| match k {
                    SeriesKindSel::Subscription => "subscriptions",
                    SeriesKindSel::Bill => "bills",
                    SeriesKindSel::Income => "regular income",
                    SeriesKindSel::Interest => "interest",
                    SeriesKindSel::Transfer => "regular transfers",
                })
                .collect::<Vec<_>>()
                .join(" and ")
        },
    );
    parts.push(kinds);
    if let Some(n) = &s.name {
        parts.push(format!("named {n}"));
    }
    if let Some(keys) = &s.categories {
        parts.push(format!("in {}", category_names(ledger, keys)));
    }
    if let Some(p) = &s.period {
        parts.push(format!("charges in {}", period(ledger, p, as_of)));
    }
    parts.join(" ")
}

fn ask(ledger: &Ledger, a: &Ask, as_of: Date) -> String {
    match a {
        Ask::Sum {
            measure: m,
            scope: s,
        } => {
            format!("total {} · {}", measure(*m), scope(ledger, s, as_of))
        }
        Ask::Count {
            measure: m,
            scope: s,
        } => {
            format!("number of {} · {}", measure(*m), scope(ledger, s, as_of))
        }
        Ask::Average {
            measure: m,
            scope: s,
            per,
        }
        | Ask::Median {
            measure: m,
            scope: s,
            per,
        } => {
            let what = if matches!(a, Ask::Average { .. }) {
                "average"
            } else {
                "median"
            };
            let per = match per {
                Per::Month => "per month",
                Per::Transaction => "per transaction",
            };
            format!("{what} {} {per} · {}", measure(*m), scope(ledger, s, as_of))
        }
        Ask::Rank {
            measure: m,
            scope: s,
            by: b,
            order,
            metric,
            n,
            output,
            position,
        } => {
            let most = match (order, metric) {
                (Order::Highest, Metric::Amount) => "highest",
                (Order::Lowest, Metric::Amount) => "lowest",
                (Order::Highest, Metric::Count) => "most frequent",
                (Order::Lowest, Metric::Count) => "least frequent",
            };
            let (b, m, nth) = (by(*b), measure(*m), nth(*position));
            let which = match output {
                RankOutput::Labels => format!("the {n} {b}s with the {most} {m}"),
                RankOutput::Label => format!("the {b} with the {most} {m}{nth}"),
                RankOutput::Amount => format!("{m} of the {b} with the {most}{nth}"),
                RankOutput::Count => format!("number of {m} of the {b} with the {most}{nth}"),
            };
            format!("{which} · {}", scope(ledger, s, as_of))
        }
        Ask::Keys {
            measure: m,
            scope: s,
            by: b,
        } => {
            format!(
                "every {} with {} · {}",
                by(*b),
                measure(*m),
                scope(ledger, s, as_of)
            )
        }
        Ask::Distinct {
            measure: m,
            scope: s,
            by: b,
        } => format!(
            "how many different {}s with {} · {}",
            by(*b),
            measure(*m),
            scope(ledger, s, as_of)
        ),
        Ask::Pick {
            measure: m,
            scope: s,
            order,
            output,
        } => {
            let which = match order {
                PickOrder::Largest => "the largest",
                PickOrder::Smallest => "the smallest",
                PickOrder::Latest => "the latest",
                PickOrder::Earliest => "the earliest",
            };
            let what = match output {
                PickOutput::Merchant => "merchant",
                PickOutput::Amount => "amount",
                PickOutput::Date => "date",
            };
            format!(
                "{what} of {which} of {} · {}",
                measure(*m),
                scope(ledger, s, as_of)
            )
        }
        Ask::Recurring { select, output } => {
            let what = match output {
                SeriesOutput::Names => "names of",
                SeriesOutput::Count => "how many",
                SeriesOutput::Latest => "current price of",
                SeriesOutput::LatestTotal => "current prices added up for",
                SeriesOutput::PriceFrom => "date of the last price change of",
                SeriesOutput::PriceBefore => "price before the last change of",
                SeriesOutput::PriceAfter => "price after the last change of",
                SeriesOutput::MemberCount => "number of charges of",
                SeriesOutput::MemberTotal => "charges added up for",
            };
            format!("{what} {}", series(ledger, select, as_of))
        }
        Ask::Derive { formula, of } => {
            let a = of.first().map_or("?", String::as_str);
            let b = of.get(1).map_or("?", String::as_str);
            match formula {
                DeriveOp::Difference => format!("{a} minus {b}"),
                DeriveOp::Sum => of.join(" plus "),
                DeriveOp::PercentChange => format!("change from {b} to {a}, in percent"),
                DeriveOp::Share => format!("{a} as a share of {b}, in percent"),
                DeriveOp::Larger => format!("the larger of {a} and {b}"),
            }
        }
    }
}

fn nth(position: usize) -> String {
    match position {
        1 => String::new(),
        n => format!(" (number {n})"),
    }
}

/// `query` in plain English, one line per item: `"<name>: <what it asks>"`.
pub fn describe(ledger: &Ledger, query: &Query, as_of: Date) -> Vec<String> {
    query
        .items
        .iter()
        .map(|i| format!("{}: {}", i.name, ask(ledger, &i.ask, as_of).trim_end()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn ledger() -> Ledger {
        let mut l: Ledger = serde_json::from_slice(
            &std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../testdata/synthetic/data/freelancer/ledger.json"
            ))
            .unwrap(),
        )
        .unwrap();
        l.decisions.clear();
        l
    }

    fn lines(json: &str) -> Vec<String> {
        let q: Query = serde_json::from_str(json).unwrap();
        describe(&ledger(), &q, "2026-10-01".parse().unwrap())
    }

    #[test]
    fn sums_name_the_measure_filters_and_resolved_period() {
        assert_eq!(
            lines(
                r#"{"items":[{"name":"total","ask":{"op":"sum","measure":"spending",
                "scope":{"period":{"last_months":3},"categories":["food.groceries"]}}}]}"#
            ),
            [
                "total: total spending (net of refunds) · Groceries · the last 3 months (July 2026 to September 2026)"
            ]
        );
        assert_eq!(
            lines(
                r#"{"items":[{"name":"set_aside","ask":{"op":"sum","measure":"transfers_out",
                "scope":{"period":{"year":2025},"accounts":{"kinds":["checking"]},
                "counterpart_accounts":{"kinds":["savings"]}}}}]}"#
            ),
            [
                "set_aside: total transfers out · on checking accounts · to or from savings accounts · 2025"
            ]
        );
        assert_eq!(
            lines(
                r#"{"items":[{"name":"euros","ask":{"op":"sum","measure":"spending",
                "scope":{"period":"all_time","original_currency":"EUR","amounts":"original"}}}]}"#
            ),
            [
                "euros: total spending (net of refunds) · paid in EUR, in EUR · all time (2024-10-01 to 2026-10-01)"
            ]
        );
    }

    #[test]
    fn other_asks_read_naturally() {
        let got = lines(
            r#"{"items":[
            {"name":"month","ask":{"op":"rank","measure":"spending","scope":{"period":{"year":2025}},
              "by":"month","order":"highest","output":"label"}},
            {"name":"top","ask":{"op":"rank","measure":"spending","scope":{"period":"last_week"},
              "by":"merchant","order":"highest","n":3,"output":"labels"}},
            {"name":"when","ask":{"op":"pick","measure":"purchases","scope":{"period":{"quarter":{"year":2026,"quarter":1}},
              "merchant":"Discount Tire"},"order":"latest","output":"date"}},
            {"name":"subs","ask":{"op":"recurring","select":{"kinds":["subscription"],"cadences":["monthly"],
              "status":"active"},"output":"latest_total"}},
            {"name":"change","ask":{"op":"derive","formula":"percent_change","of":["sep","aug"]}}]}"#,
        );
        assert_eq!(
            got,
            [
                "month: the month with the highest spending (net of refunds) · 2025",
                "top: the 3 merchants with the highest spending (net of refunds) · last week (2026-09-21 to 2026-09-27)",
                "when: date of the latest of purchases (before refunds) · at Discount Tire · Q1 2026 (January 2026 to March 2026)",
                "subs: current prices added up for still running monthly subscriptions",
                "change: change from aug to sep, in percent",
            ]
        );
    }

    #[test]
    fn every_td3_expected_query_reads_without_raw_keys() {
        let set: serde_json::Value = serde_json::from_slice(
            &std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../testdata/questions/data/questions.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let l = ledger();
        for q in set["questions"].as_array().unwrap() {
            if q["expected_query"].is_null() {
                continue;
            }
            let query: Query = serde_json::from_value(q["expected_query"].clone()).unwrap();
            let text = describe(&l, &query, "2026-10-01".parse().unwrap());
            assert_eq!(text.len(), query.items.len());
            for line in text {
                // Category keys are shown by name.
                assert!(
                    !line.contains("food.") && !line.contains("utilities."),
                    "{line}"
                );
                assert!(!line.contains("invalid period"), "{line}");
            }
        }
    }
}
