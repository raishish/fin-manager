//! Keeps `schema.json` honest: its enums and ops are exactly the serde names of the Rust
//! types, a query of every shape validates, and every TD.3 expected query validates.
//!
//! The validator covers only the keywords the schema uses, and fails on any other, so
//! a keyword it doesn't know can't be skipped silently.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value as Json, json};

use super::*;
use crate::model::{AccountKind, Date, YearMonth};

fn schema() -> Json {
    serde_json::from_str(SCHEMA).expect("schema.json parses")
}

const KEYWORDS: &[&str] = &[
    "$schema",
    "$id",
    "title",
    "description",
    "$ref",
    "$defs",
    "oneOf",
    "const",
    "enum",
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "minItems",
    "minimum",
    "maximum",
    "pattern",
];

type CharTest = Box<dyn Fn(char) -> bool>;

/// `^...$` patterns made of `[0-9]`, `[A-Z]`, literal characters and `{n}` repeats.
fn pattern_matches(pattern: &str, text: &str) -> bool {
    let body = pattern
        .strip_prefix('^')
        .and_then(|p| p.strip_suffix('$'))
        .expect("anchored pattern");
    let mut tokens: Vec<(CharTest, usize)> = Vec::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        let test: CharTest = match c {
            '[' => {
                let class: String = chars.by_ref().take_while(|c| *c != ']').collect();
                match class.as_str() {
                    "0-9" => Box::new(|c: char| c.is_ascii_digit()),
                    "A-Z" => Box::new(|c: char| c.is_ascii_uppercase()),
                    other => panic!("unsupported class {other}"),
                }
            }
            '{' | '}' | '\\' | '(' | ')' | '*' | '+' | '?' | '|' | '.' => {
                panic!("unsupported pattern syntax {c}")
            }
            lit => Box::new(move |c: char| c == lit),
        };
        let mut repeat = 1;
        if chars.peek() == Some(&'{') {
            chars.next();
            let n: String = chars.by_ref().take_while(|c| *c != '}').collect();
            repeat = n.parse().expect("a repeat count");
        }
        tokens.push((test, repeat));
    }
    let mut text = text.chars();
    for (test, repeat) in &tokens {
        for _ in 0..*repeat {
            if !text.next().is_some_and(test) {
                return false;
            }
        }
    }
    text.next().is_none()
}

fn type_ok(t: &str, v: &Json) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "null" => v.is_null(),
        "boolean" => v.is_boolean(),
        other => panic!("unsupported type {other}"),
    }
}

/// Checks `v` against `s`; the error says where it failed.
fn validate(root: &Json, s: &Json, v: &Json, at: &str) -> Result<(), String> {
    let s = s.as_object().expect("a schema is an object");
    for k in s.keys() {
        assert!(KEYWORDS.contains(&k.as_str()), "unsupported keyword {k}");
    }
    let fail = |why: &str| Err(format!("{at}: {why}"));
    if let Some(r) = s.get("$ref") {
        let name = r
            .as_str()
            .and_then(|r| r.strip_prefix("#/$defs/"))
            .expect("local ref");
        validate(root, &root["$defs"][name], v, at)?;
    }
    if let Some(options) = s.get("oneOf") {
        let ok = options
            .as_array()
            .expect("oneOf is a list")
            .iter()
            .filter(|o| validate(root, o, v, at).is_ok())
            .count();
        if ok != 1 {
            return fail(&format!("matches {ok} of oneOf"));
        }
    }
    if let Some(c) = s.get("const")
        && c != v
    {
        return fail("not the const");
    }
    if let Some(e) = s.get("enum")
        && !e.as_array().expect("enum is a list").contains(v)
    {
        return fail("not in enum");
    }
    if let Some(t) = s.get("type") {
        let ok = match t {
            Json::String(t) => type_ok(t, v),
            Json::Array(ts) => ts
                .iter()
                .any(|t| type_ok(t.as_str().expect("type name"), v)),
            _ => panic!("bad type"),
        };
        if !ok {
            return fail("wrong type");
        }
    }
    if let Some(obj) = v.as_object() {
        let props = s.get("properties").and_then(Json::as_object);
        if let Some(props) = props {
            for (k, child) in obj {
                match props.get(k) {
                    Some(ps) => validate(root, ps, child, &format!("{at}.{k}"))?,
                    None if s.get("additionalProperties") == Some(&json!(false)) => {
                        return fail(&format!("unknown field {k}"));
                    }
                    None => {}
                }
            }
        }
        for r in s
            .get("required")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let r = r.as_str().expect("field name");
            if !obj.contains_key(r) {
                return fail(&format!("missing {r}"));
            }
        }
    }
    if let Some(items) = v.as_array() {
        if let Some(each) = s.get("items") {
            for (i, x) in items.iter().enumerate() {
                validate(root, each, x, &format!("{at}[{i}]"))?;
            }
        }
        if s.get("minItems")
            .and_then(Json::as_u64)
            .is_some_and(|m| (items.len() as u64) < m)
        {
            return fail("too few items");
        }
    }
    if let Some(n) = v.as_i64() {
        if s.get("minimum")
            .and_then(Json::as_i64)
            .is_some_and(|m| n < m)
        {
            return fail("below minimum");
        }
        if s.get("maximum")
            .and_then(Json::as_i64)
            .is_some_and(|m| n > m)
        {
            return fail("above maximum");
        }
    }
    if let (Some(p), Some(text)) = (s.get("pattern"), v.as_str())
        && !pattern_matches(p.as_str().expect("pattern"), text)
    {
        return fail("doesn't match pattern");
    }
    Ok(())
}

fn valid(v: &Json) -> Result<(), String> {
    let root = schema();
    validate(&root, &root, v, "$")
}

fn names<T: Serialize>(variants: &[T]) -> BTreeSet<String> {
    variants
        .iter()
        .map(|v| match serde_json::to_value(v).unwrap() {
            Json::String(s) => s,
            other => panic!("not a unit variant: {other}"),
        })
        .collect()
}

fn schema_enum(def: &str) -> BTreeSet<String> {
    schema()["$defs"][def]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("{def} has an enum"))
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

/// Lists every variant; the exhaustive `match` stops compiling when one is added, so
/// the list (and then the schema) has to be updated.
macro_rules! all {
    ($ty:ident: $($v:ident),+ $(,)?) => {{
        fn _exhaustive(x: $ty) {
            match x {
                $($ty::$v)|+ => {}
            }
        }
        [$($ty::$v),+]
    }};
}

#[test]
fn enums_are_the_serde_names() {
    let checks: Vec<(&str, BTreeSet<String>)> = vec![
        (
            "Measure",
            names(
                &all!(Measure: Spending, Purchases, Refunds, Income, TransfersIn, TransfersOut, MoneyIn, MoneyOut, Pending),
            ),
        ),
        ("Per", names(&all!(Per: Transaction, Month))),
        (
            "By",
            names(&all!(By: Category, Group, Merchant, Month, Account)),
        ),
        ("Order", names(&all!(Order: Highest, Lowest))),
        ("Metric", names(&all!(Metric: Amount, Count))),
        (
            "RankOutput",
            names(&all!(RankOutput: Labels, Label, Amount, Count)),
        ),
        (
            "PickOrder",
            names(&all!(PickOrder: Largest, Smallest, Latest, Earliest)),
        ),
        (
            "PickOutput",
            names(&all!(PickOutput: Merchant, Amount, Date)),
        ),
        ("Amounts", names(&all!(Amounts: Account, Original))),
        (
            "SeriesKindSel",
            names(&all!(SeriesKindSel: Subscription, Bill, Income, Interest, Transfer)),
        ),
        (
            "CadenceSel",
            names(
                &all!(CadenceSel: Weekly, Biweekly, Semimonthly, Monthly, Quarterly, Semiannual, Annual),
            ),
        ),
        (
            "SeriesStatus",
            names(&all!(SeriesStatus: Active, Ended, Cancelled, Any)),
        ),
        (
            "SeriesOutput",
            names(
                &all!(SeriesOutput: Names, Count, Latest, LatestTotal, PriceFrom, PriceBefore, PriceAfter, MemberCount, MemberTotal),
            ),
        ),
        (
            "DeriveOp",
            names(&all!(DeriveOp: Difference, Sum, PercentChange, Share, Larger)),
        ),
        (
            "AccountKind",
            names(
                &all!(AccountKind: Checking, Savings, CreditCard, Cash, Loan, Mortgage, Brokerage, Retirement, Other),
            ),
        ),
    ];
    for (def, rust) in checks {
        assert_eq!(schema_enum(def), rust, "{def}");
    }
}

fn scope() -> Scope {
    Scope {
        period: PeriodSpec::Year(2025),
        categories: Some(vec!["food.groceries".into()]),
        groups: Some(vec!["food".into()]),
        merchant: Some("H-E-B".into()),
        accounts: Some(AccountSelect {
            kinds: vec![AccountKind::Checking],
            ids: vec![crate::model::AccountId(1)],
        }),
        counterpart_accounts: Some(AccountSelect::default()),
        description: Some("foreign transaction fee".into()),
        original_currency: Some("EUR".into()),
        amounts: Amounts::Original,
    }
}

/// One of each `Ask`, with every optional field set.
fn every_ask() -> Vec<Ask> {
    let m = Measure::Spending;
    vec![
        Ask::Sum {
            measure: m,
            scope: scope(),
        },
        Ask::Count {
            measure: m,
            scope: Scope::default(),
        },
        Ask::Average {
            measure: m,
            scope: scope(),
            per: Per::Month,
        },
        Ask::Median {
            measure: m,
            scope: scope(),
            per: Per::Transaction,
        },
        Ask::Rank {
            measure: m,
            scope: scope(),
            by: By::Merchant,
            order: Order::Highest,
            metric: Metric::Count,
            n: 3,
            output: RankOutput::Labels,
            position: 2,
        },
        Ask::Keys {
            measure: m,
            scope: scope(),
            by: By::Month,
        },
        Ask::Distinct {
            measure: m,
            scope: scope(),
            by: By::Category,
        },
        Ask::Pick {
            measure: m,
            scope: scope(),
            order: PickOrder::Latest,
            output: PickOutput::Date,
        },
        Ask::Recurring {
            select: SeriesSelect {
                name: Some("Adobe".into()),
                categories: Some(vec!["housing.rent".into()]),
                kinds: Some(vec![SeriesKindSel::Bill]),
                cadences: Some(vec![CadenceSel::Annual]),
                status: SeriesStatus::Cancelled,
                period: Some(PeriodSpec::LastWeek),
            },
            output: SeriesOutput::MemberTotal,
        },
        Ask::Derive {
            formula: DeriveOp::PercentChange,
            of: vec!["a".into(), "b".into()],
        },
    ]
}

fn op(a: &Ask) -> &'static str {
    match a {
        Ask::Sum { .. } => "sum",
        Ask::Count { .. } => "count",
        Ask::Average { .. } => "average",
        Ask::Median { .. } => "median",
        Ask::Rank { .. } => "rank",
        Ask::Keys { .. } => "keys",
        Ask::Distinct { .. } => "distinct",
        Ask::Pick { .. } => "pick",
        Ask::Recurring { .. } => "recurring",
        Ask::Derive { .. } => "derive",
    }
}

fn ym(s: &str) -> YearMonth {
    s.parse().unwrap()
}

fn every_period() -> Vec<PeriodSpec> {
    let d = |s: &str| s.parse::<Date>().unwrap();
    let all = vec![
        PeriodSpec::Range {
            from: d("2025-11-28"),
            to: d("2025-12-25"),
        },
        PeriodSpec::Month(ym("2025-07")),
        PeriodSpec::Months {
            from: ym("2025-06"),
            to: ym("2025-08"),
        },
        PeriodSpec::Quarter {
            year: 2025,
            quarter: 4,
        },
        PeriodSpec::Year(2025),
        PeriodSpec::LastMonths(3),
        PeriodSpec::LastDays(14),
        PeriodSpec::LastWeek,
        PeriodSpec::LastQuarter,
        PeriodSpec::YearToDate,
        PeriodSpec::AllTime,
    ];
    // Exhaustive: a new variant must be added above.
    for p in &all {
        match p {
            PeriodSpec::Range { .. }
            | PeriodSpec::Month(_)
            | PeriodSpec::Months { .. }
            | PeriodSpec::Quarter { .. }
            | PeriodSpec::Year(_)
            | PeriodSpec::LastMonths(_)
            | PeriodSpec::LastDays(_)
            | PeriodSpec::LastWeek
            | PeriodSpec::LastQuarter
            | PeriodSpec::YearToDate
            | PeriodSpec::AllTime => {}
        }
    }
    all
}

#[test]
fn every_shape_validates_and_ops_match() {
    let asks = every_ask();
    let rust_ops: BTreeSet<&str> = asks.iter().map(op).collect();
    let schema_ops: BTreeSet<String> = schema()["$defs"]["Ask"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["properties"]["op"]["const"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        rust_ops
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        schema_ops
    );
    let mut items: Vec<Item> = asks
        .into_iter()
        .enumerate()
        .map(|(i, ask)| Item {
            name: format!("item{i}"),
            ask,
        })
        .collect();
    for (i, period) in every_period().into_iter().enumerate() {
        items.push(Item {
            name: format!("period{i}"),
            ask: Ask::Sum {
                measure: Measure::Income,
                scope: Scope {
                    period,
                    ..Scope::default()
                },
            },
        });
    }
    let query = Query { items };
    let json = serde_json::to_value(&query).unwrap();
    valid(&json).unwrap();
    assert_eq!(serde_json::from_value::<Query>(json).unwrap(), query);
}

#[test]
fn schema_and_serde_reject_the_same_mistakes() {
    let good = json!({"items": [{"name": "total", "ask": {
        "op": "sum", "measure": "spending", "scope": {"period": {"year": 2025}}}}]});
    valid(&good).unwrap();
    serde_json::from_value::<Query>(good.clone()).unwrap();

    let mut bad = Vec::new();
    let mut with = |path: &[&str], value: Json| {
        let mut v = good.clone();
        let mut at = &mut v["items"][0];
        for p in &path[..path.len() - 1] {
            at = &mut at[*p];
        }
        at[path[path.len() - 1]] = value;
        bad.push((path.join("."), v));
    };
    with(&["ask", "measure"], json!("spend"));
    with(&["ask", "op"], json!("total"));
    with(&["ask", "extra"], json!(1));
    with(
        &["ask", "scope", "period"],
        json!({"year": 2025, "month": "2025-01"}),
    );
    with(
        &["ask", "scope", "period"],
        json!({"quarter": {"year": 2025, "quarter": 5}}),
    );
    with(&["ask", "scope", "period"], json!({"month": "2025-1"}));
    with(&["ask", "scope", "period"], json!("yesterday"));
    with(&["ask", "scope", "category"], json!(["food.groceries"]));
    with(&["ask", "scope", "original_currency"], json!("euro"));
    for (what, v) in bad {
        assert!(valid(&v).is_err(), "schema accepts a bad {what}");
        // Serde rejects the same, except checks only the schema makes (currency shape
        // and quarter range are checked when the query runs).
        if !what.ends_with("original_currency") && !v.to_string().contains("\"quarter\":5") {
            assert!(
                serde_json::from_value::<Query>(v).is_err(),
                "serde accepts a bad {what}"
            );
        }
    }
    let no_items = json!({"items": []});
    assert!(valid(&no_items).is_err());
}

#[test]
fn every_td3_expected_query_validates() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/questions/data/questions.json");
    let set: Json = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut n = 0;
    for q in set["questions"].as_array().unwrap() {
        let query = &q["expected_query"];
        if query.is_null() {
            continue;
        }
        let id = q["id"].as_str().unwrap();
        valid(query).unwrap_or_else(|e| panic!("{id}: {e}"));
        serde_json::from_value::<Query>(query.clone()).unwrap_or_else(|e| panic!("{id}: {e}"));
        n += 1;
    }
    assert_eq!(n, 98);
}

#[test]
fn the_pattern_matcher_works() {
    assert!(pattern_matches("^[0-9]{4}-[0-9]{2}$", "2025-07"));
    assert!(!pattern_matches("^[0-9]{4}-[0-9]{2}$", "2025-7"));
    assert!(!pattern_matches("^[0-9]{4}-[0-9]{2}$", "2025-07-01"));
    assert!(pattern_matches("^[A-Z]{3}$", "EUR"));
    assert!(!pattern_matches("^[A-Z]{3}$", "eur"));
}
