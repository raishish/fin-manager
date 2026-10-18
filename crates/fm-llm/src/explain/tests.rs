//! The number check on fixed texts (no model): formats, matching rules, the template,
//! the explain flow, and TD.3's 100 answers with and without an injected wrong number.

use std::path::PathBuf;

use serde_json::Value as Json;

use super::extract::{Kind, Mark, numbers};
use super::facts::Dec;
use super::*;

fn usd(minor: i64) -> Value {
    Value::Money(Money {
        minor,
        currency: "USD".into(),
        minor_digits: 2,
    })
}

fn fact(name: &str, value: Value) -> Fact {
    Fact {
        name: name.into(),
        value,
        evidence: None,
    }
}

fn kinds(text: &str) -> Vec<Kind> {
    numbers(text, &[]).into_iter().map(|n| n.kind).collect()
}

fn passes(text: &str, facts: &[Fact]) -> bool {
    check(text, facts, &[]).passed()
}

#[test]
fn money_formats_are_read() {
    let d = Dec::new;
    assert_eq!(
        kinds("You spent $1,234.56 and got back -$12.50."),
        [
            Kind::Money {
                mark: Mark::Dollar,
                amount: d(123_456, 2),
                power: 0
            },
            Kind::Money {
                mark: Mark::Dollar,
                amount: d(1250, 2),
                power: 0
            },
        ]
    );
    assert_eq!(
        kinds("about $1.2k, then $3M, €12.30, £5, ¥500, USD 1,234, 7.50 EUR, 20 dollars"),
        [
            Kind::Money {
                mark: Mark::Dollar,
                amount: d(12, 1),
                power: 3
            },
            Kind::Money {
                mark: Mark::Dollar,
                amount: d(3, 0),
                power: 6
            },
            Kind::Money {
                mark: Mark::Euro,
                amount: d(1230, 2),
                power: 0
            },
            Kind::Money {
                mark: Mark::Pound,
                amount: d(5, 0),
                power: 0
            },
            Kind::Money {
                mark: Mark::Yen,
                amount: d(500, 0),
                power: 0
            },
            Kind::Money {
                mark: Mark::Code("USD".into()),
                amount: d(1234, 0),
                power: 0
            },
            Kind::Money {
                mark: Mark::Code("EUR".into()),
                amount: d(750, 2),
                power: 0
            },
            Kind::Money {
                mark: Mark::Dollar,
                amount: d(20, 0),
                power: 0
            },
        ]
    );
    assert_eq!(
        kinds("$2.5 million"),
        [Kind::Money {
            mark: Mark::Dollar,
            amount: d(25, 1),
            power: 6
        }]
    );
}

#[test]
fn percents_plain_and_written_numbers_are_read() {
    let d = Dec::new;
    assert_eq!(
        kinds("up 12%, then 12.3 percent, 4 percentage points, 7 per cent"),
        [
            Kind::Percent { amount: d(12, 0) },
            Kind::Percent { amount: d(123, 1) },
            Kind::Percent { amount: d(4, 0) },
            Kind::Percent { amount: d(7, 0) },
        ]
    );
    let plain = |v: i128| Kind::Plain {
        amount: d(v, 0),
        power: 0,
        year_like: false,
    };
    assert_eq!(
        kinds("1,234 rows, twelve visits, twenty-five rides, a dozen, two hundred, zero"),
        [
            plain(1234),
            plain(12),
            plain(25),
            plain(12),
            plain(200),
            plain(0)
        ]
    );
    assert_eq!(
        kinds("in 2025"),
        [Kind::Plain {
            amount: d(2025, 0),
            power: 0,
            year_like: true
        }]
    );
    // Not numbers: ordinals, words with digits, one-offs, "someone".
    assert!(kinds("the 3rd time, Q4, W2 forms, one-off costs, someone, the first week").is_empty());
}

#[test]
fn dates_and_months_are_read() {
    let date = |y: Option<i32>, m, d| Kind::Date {
        year: y,
        month: m,
        day: d,
    };
    assert_eq!(
        kinds("2026-03-03, 03/04/2026, March 5, 2026, 6 March 2026, Mar 7th 2026, March 8"),
        [
            date(Some(2026), 3, 3),
            date(Some(2026), 3, 4),
            date(Some(2026), 3, 5),
            date(Some(2026), 3, 6),
            date(Some(2026), 3, 7),
            date(None, 3, 8),
        ]
    );
    assert_eq!(
        kinds("in September 2025 and in 2025-10, Sept. 2025"),
        [
            Kind::Month {
                year: 2025,
                month: 9
            },
            Kind::Month {
                year: 2025,
                month: 10
            },
            Kind::Month {
                year: 2025,
                month: 9
            },
        ]
    );
    // "May" is a month only with a day or a year.
    assert!(kinds("you may want to check").is_empty());
}

#[test]
fn money_matches_money_at_the_precision_shown() {
    let facts = [fact("total", usd(123_456))];
    for ok in [
        "$1,234.56",
        "$1,235",
        "$1.2k",
        "1,234.56 dollars",
        "USD 1,234.56",
        "-$1,234.56",
    ] {
        assert!(passes(ok, &facts), "{ok}");
    }
    for wrong in [
        "$1,234.57",
        "$1,234",
        "$1.3k",
        "$1,200",
        "€1,234.56",
        "1,234.56",
        "1234.56%",
    ] {
        assert!(!passes(wrong, &facts), "{wrong}");
    }
    // The deliberate rule for hedges (README.md): "about" allows rounding to the digits
    // shown, at least two significant ones, within 5%.
    assert!(passes("about $1,200", &facts));
    assert!(passes("roughly $1,230", &facts));
    assert!(!passes("about $1,000", &facts), "one significant digit");
    assert!(!passes("about $1,300", &facts), "rounds to 1,200");
    assert!(!passes("over $1,200", &facts), "a bound is not a hedge");
}

#[test]
fn percents_match_percents_and_never_money() {
    let facts = [fact("change", Value::Percent { basis_points: 8319 })];
    for ok in ["83.19%", "83.2%", "83%", "83.19 percent", "about 83%"] {
        assert!(passes(ok, &facts), "{ok}");
    }
    for wrong in ["83.18%", "84%", "$83.19", "83.19", "8319"] {
        assert!(!passes(wrong, &facts), "{wrong}");
    }
}

#[test]
fn plain_numbers_match_counts_evidence_and_years() {
    let mut total = fact("total", usd(5000));
    total.evidence = Some(12);
    let facts = [
        total,
        fact("visits", Value::Count(9)),
        fact("year", Value::Year(2025)),
    ];
    for ok in [
        "12 transactions",
        "twelve transactions",
        "9 visits",
        "nine",
        "in 2025",
    ] {
        assert!(passes(ok, &facts), "{ok}");
    }
    // A bare year that is neither a Year fact nor a count is caught.
    for wrong in ["13 transactions", "in 2024", "50", "10 visits"] {
        assert!(!passes(wrong, &facts), "{wrong}");
    }
    // A four-digit count is fine as a count.
    let facts = [fact("rows", Value::Count(2024))];
    assert!(passes("2024 rows", &facts));
}

#[test]
fn dates_match_dates_months_and_years() {
    let facts = [fact(
        "date",
        Value::Date {
            year: 2025,
            month: 9,
            day: 3,
        },
    )];
    for ok in [
        "September 3, 2025",
        "2025-09-03",
        "09/03/2025",
        "September 3",
        "in September 2025",
        "in 2025",
    ] {
        assert!(passes(ok, &facts), "{ok}");
    }
    for wrong in ["September 4, 2025", "October 2025", "in 2026", "2025-09-04"] {
        assert!(!passes(wrong, &facts), "{wrong}");
    }
}

#[test]
fn names_and_text_values_are_not_numbers() {
    let facts = [
        fact("q4_2025", usd(100)),
        fact("merchant", Value::Text("IN-N-OUT 592".into())),
        fact(
            "top",
            Value::Texts(vec!["7-Eleven".into(), "Route 66 Diner".into()]),
        ),
    ];
    assert!(passes(
        "Q4 2025: $1.00 at IN-N-OUT 592, then 7-Eleven and Route 66 Diner.",
        &facts
    ));
    assert!(!passes("Q4 2025: $1.00 at IN-N-OUT 593.", &facts));
}

#[test]
fn derivations_are_allowed_only_when_listed() {
    let facts = [
        fact("this_month", usd(150_000)),
        fact("last_month", usd(120_000)),
    ];
    let text = "You spent $1,500.00, $300.00 more than last month's $1,200.00, up 25%.";
    assert!(!check(text, &facts, &[]).passed());
    let both = [
        Derivation::Difference("this_month".into(), "last_month".into()),
        Derivation::PercentChange("this_month".into(), "last_month".into()),
    ];
    assert!(check(text, &facts, &both).passed());
    // Only the difference: the percent is still unchecked, and only it.
    let report = check(text, &facts, &both[..1]);
    assert_eq!(report.unchecked.len(), 1);
    assert_eq!(report.unchecked[0].kind, "percent");
    let counts = [fact("a", Value::Count(7)), fact("b", Value::Count(5))];
    assert!(
        check(
            "12 in all",
            &counts,
            &[Derivation::Sum(vec!["a".into(), "b".into()])]
        )
        .passed()
    );
    assert!(
        check(
            "a is 140% of b",
            &counts,
            &[Derivation::Ratio("a".into(), "b".into())]
        )
        .passed()
    );
}

#[test]
fn the_template_writes_every_kind_and_passes_its_own_check() {
    let mut total = fact("total_spent", usd(-123_456));
    total.evidence = Some(42);
    let facts = vec![
        total,
        fact(
            "yen",
            Value::Money(Money {
                minor: 5000,
                currency: "JPY".into(),
                minor_digits: 0,
            }),
        ),
        fact(
            "swiss",
            Value::Money(Money {
                minor: 1230,
                currency: "CHF".into(),
                minor_digits: 2,
            }),
        ),
        fact("visits", Value::Count(3)),
        fact("share", Value::Percent { basis_points: 2800 }),
        fact(
            "change",
            Value::Percent {
                basis_points: -1234,
            },
        ),
        fact(
            "date",
            Value::Date {
                year: 2026,
                month: 1,
                day: 20,
            },
        ),
        fact(
            "month",
            Value::Month {
                year: 2025,
                month: 9,
            },
        ),
        fact("year", Value::Year(2026)),
        fact("merchant", Value::Text("Trader Joe's".into())),
        fact(
            "merchants",
            Value::Texts(vec!["Shell".into(), "Chevron".into()]),
        ),
    ];
    let text = template(&facts);
    assert_eq!(
        text,
        "Total spent: -$1,234.56, from 42 transactions. Yen: ¥5,000. Swiss: CHF 12.30. \
         Visits: 3. Share: 28%. Change: -12.34%. Date: January 20, 2026. \
         Month: September 2025. Year: 2026. Merchant: \"Trader Joe's\". \
         Merchants: \"Shell\", \"Chevron\"."
    );
    let report = check(&text, &facts, &[]);
    assert!(report.passed(), "{report:?}");
    // Nine values and one evidence count.
    assert_eq!(report.found, 10);
}

#[test]
fn explain_retries_then_falls_back() {
    let facts = [fact("total", usd(4250))];
    // Wrong, then right: the second is kept.
    let mut replies = vec!["You spent $42.50.", "You spent $43.50."]
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let e = explain("How much?", &facts, &[], 3, |_| {
        replies.pop().map(str::to_owned)
    });
    assert_eq!(e.text, "You spent $42.50.");
    assert_eq!((e.source, e.rejected.len()), (Source::Model, 0));
    let mut replies = vec!["You spent $43.50.", "You spent $42.50."]
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let e = explain("How much?", &facts, &[], 3, |_| {
        replies.pop().map(str::to_owned)
    });
    assert_eq!((e.source, e.rejected.len()), (Source::Model, 1));
    // Always wrong: the template, after every attempt.
    let e = explain("How much?", &facts, &[], 2, |_| Some("About $50.".into()));
    assert_eq!((e.source, e.rejected.len()), (Source::Template, 2));
    assert_eq!(e.text, "Total: $42.50.");
    // No model: the template at once.
    let e = explain("How much?", &facts, &[], 3, |_| None);
    assert_eq!((e.source, e.rejected.len()), (Source::Template, 0));
    // An empty text is no explanation.
    let e = explain("How much?", &facts, &[], 1, |_| Some("  ".into()));
    assert_eq!(e.source, Source::Template);
}

#[test]
fn the_prompt_holds_only_the_question_and_the_facts() {
    let facts = [
        fact("total", usd(4250)),
        fact("merchant", Value::Text("Blue Bottle".into())),
    ];
    let p = prompt("How much on coffee?", &facts, &[]);
    assert!(p.contains("Question: How much on coffee?"));
    assert!(p.contains("- Total: $42.50."));
    assert!(p.contains("- Merchant: \"Blue Bottle\"."));
    // Everything after the instructions is the question and one line per fact.
    let answer = p.split("Answer:\n").nth(1).unwrap();
    assert_eq!(
        answer.lines().filter(|l| l.starts_with("- ")).count(),
        facts.len()
    );
}

// ---- TD.3: the 100 questions' answers as facts.

fn questions() -> Vec<Json> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/questions/data/questions.json");
    let all: Json = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    all["questions"].as_array().unwrap().clone()
}

fn digits_of(currency: &str) -> u8 {
    match currency {
        "JPY" => 0,
        _ => 2,
    }
}

/// `YYYY-MM` as a month, `YYYY-MM-DD` as a date.
fn ymd(text: &str) -> Option<(i32, u8, Option<u8>)> {
    let parts: Vec<&str> = text.split('-').collect();
    match parts.as_slice() {
        [y, m] if y.len() == 4 && m.len() == 2 => Some((y.parse().ok()?, m.parse().ok()?, None)),
        [y, m, d] if y.len() == 4 => {
            Some((y.parse().ok()?, m.parse().ok()?, Some(d.parse().ok()?)))
        }
        _ => None,
    }
}

/// A TD.3 answer as the Ask flow would pass it.
fn answer_facts(answers: &[Json]) -> Vec<Fact> {
    answers
        .iter()
        .flat_map(|a| {
            let name = a["name"].as_str().unwrap().to_owned();
            let evidence = a["evidence"]["transaction_ids"]
                .as_array()
                .filter(|ids| !ids.is_empty())
                .map(|ids| ids.len() as u64);
            let one = |value| {
                vec![Fact {
                    name: name.clone(),
                    value,
                    evidence,
                }]
            };
            match a["kind"].as_str().unwrap() {
                "money" => {
                    let currency = a["amount"]["currency"].as_str().unwrap();
                    one(Value::Money(Money {
                        minor: a["amount"]["minor"].as_i64().unwrap(),
                        currency: currency.into(),
                        minor_digits: digits_of(currency),
                    }))
                }
                "count" => one(Value::Count(a["value"].as_i64().unwrap())),
                "percent" => one(Value::Percent {
                    basis_points: a["basis_points"].as_i64().unwrap(),
                }),
                "date" => {
                    let (year, month, day) = ymd(a["value"].as_str().unwrap()).unwrap();
                    one(Value::Date {
                        year,
                        month,
                        day: day.unwrap(),
                    })
                }
                "text" => {
                    let t = a["value"].as_str().unwrap();
                    match ymd(t) {
                        Some((year, month, None)) => one(Value::Month { year, month }),
                        _ => one(Value::Text(t.into())),
                    }
                }
                "texts" => {
                    let values: Vec<String> = a["values"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().to_owned())
                        .collect();
                    if values.iter().all(|v| matches!(ymd(v), Some((_, _, None)))) {
                        values
                            .iter()
                            .enumerate()
                            .map(|(i, v)| {
                                let (year, month, _) = ymd(v).unwrap();
                                Fact {
                                    name: format!("{name} {}", i + 1),
                                    value: Value::Month { year, month },
                                    evidence: None,
                                }
                            })
                            .collect()
                    } else {
                        one(Value::Texts(values))
                    }
                }
                other => panic!("unknown answer kind {other}"),
            }
        })
        .collect()
}

/// The facts with one number made wrong, and what was changed.
fn one_wrong(facts: &[Fact]) -> Option<(Vec<Fact>, &'static str)> {
    let mut wrong = facts.to_vec();
    for f in &mut wrong {
        let what = match &mut f.value {
            Value::Money(m) => {
                m.minor += 1;
                "money off by one minor unit"
            }
            Value::Count(n) => {
                *n += 1;
                "count off by one"
            }
            Value::Percent { basis_points } => {
                *basis_points += 10;
                "percent off by 0.1"
            }
            Value::Date { day, .. } => {
                *day = if *day == 1 { 2 } else { *day - 1 };
                "date off by a day"
            }
            Value::Month { year, month } => {
                // A month none of the facts holds: the check is by value, so a list's
                // other month would pass (README.md, "Limits").
                let taken: Vec<(i32, u8)> = facts
                    .iter()
                    .filter_map(|f| match f.value {
                        Value::Month { year, month } => Some((year, month)),
                        _ => None,
                    })
                    .collect();
                let start = (*year, *month);
                while taken.contains(&(*year, *month)) {
                    *month = if *month == 12 { 1 } else { *month + 1 };
                    if (*year, *month) == start {
                        break;
                    }
                }
                "month off by one or more"
            }
            _ => continue,
        };
        return Some((wrong, what));
    }
    None
}

/// The facts' first number written in another unit: a percent as dollars, money as a
/// percent, a count as money.
fn wrong_unit(facts: &[Fact]) -> Option<String> {
    facts.iter().find_map(|f| match &f.value {
        Value::Percent { basis_points } => Some(format!(
            "{}: {}.",
            template::label(&f.name),
            template::money(&Money {
                minor: *basis_points,
                currency: "USD".into(),
                minor_digits: 2,
            })
        )),
        Value::Money(m) if m.currency == "USD" => {
            let pct = template::percent(m.minor);
            Some(format!("{}: {pct}.", template::label(&f.name)))
        }
        Value::Count(n) => Some(format!("{}: ${n}.", template::label(&f.name))),
        _ => None,
    })
}

/// Done-when: 0 unchecked numbers across TD.3's answers, and a wrong number injected into
/// each answer (or the right number in the wrong unit) is caught.
#[test]
fn td3_answers_check_and_injected_wrong_numbers_are_caught() {
    let (mut checked, mut numbers_seen, mut caught, mut units_caught) = (0, 0, 0, 0);
    let mut kinds = std::collections::BTreeMap::new();
    for q in questions() {
        let id = q["id"].as_str().unwrap().to_owned();
        let facts = answer_facts(q["answers"].as_array().unwrap());
        if facts.is_empty() {
            continue;
        }
        checked += 1;
        let text = template(&facts);
        let report = check(&text, &facts, &[]);
        assert!(
            report.passed(),
            "{id}: {} unchecked",
            report.unchecked.len()
        );
        numbers_seen += report.found;

        // One number wrong: the template of the wrong facts, checked against the right ones.
        if let Some((wrong, what)) = one_wrong(&facts) {
            let report = check(&template(&wrong), &facts, &[]);
            assert!(!report.passed(), "{id}: {what} not caught");
            caught += 1;
            *kinds.entry(what).or_insert(0) += 1;
        } else {
            // Text answers only: an invented number added to the text is caught.
            let report = check(&format!("{text} That is 3 more than usual."), &facts, &[]);
            assert!(!report.passed(), "{id}: an invented count not caught");
            caught += 1;
            *kinds.entry("invented count").or_insert(0) += 1;
        }
        if let Some(text) = wrong_unit(&facts) {
            assert!(
                !check(&text, &facts, &[]).passed(),
                "{id}: wrong unit not caught: {text}"
            );
            units_caught += 1;
        }
    }
    println!(
        "TD.3: {checked} answers, {numbers_seen} numbers, 0 unchecked; \
         {caught} injected wrong numbers caught {kinds:?}; {units_caught} wrong units caught"
    );
    assert_eq!(checked, 100);
    assert_eq!(caught, 100);
}
