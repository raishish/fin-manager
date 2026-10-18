//! The worked examples in the prompt: hand-written questions and the queries they
//! mean. They cover every op and the measures that are easy to get wrong (refunds,
//! transfers, pending, money abroad, relative periods, recurring series).
//!
//! None comes from TD.3, the test set: a test fails if any question or query here is
//! also one of TD.3's (see `tests.rs`).

use serde_json::{Value as Json, json};

/// One worked example.
pub struct Example {
    pub question: &'static str,
    pub query: Json,
}

fn sum(name: &str, measure: &str, scope: Json) -> Json {
    json!({"name": name, "ask": {"op": "sum", "measure": measure, "scope": scope}})
}

fn derive(name: &str, formula: &str, of: &[&str]) -> Json {
    json!({"name": name, "ask": {"op": "derive", "formula": formula, "of": of}})
}

fn query(items: Vec<Json>) -> Json {
    json!({ "items": items })
}

/// The examples, in prompt order.
pub fn all() -> Vec<Example> {
    let ex = |question, items| Example {
        question,
        query: query(items),
    };
    let year = |y: i32| json!({"year": y});
    vec![
        ex(
            "How much have I paid Netflix altogether?",
            vec![sum(
                "total",
                "spending",
                json!({"period": "all_time", "merchant": "Netflix"}),
            )],
        ),
        ex(
            "How much did I pay off on my credit cards in 2024?",
            vec![sum(
                "paid",
                "transfers_in",
                json!({"period": year(2024), "accounts": {"kinds": ["credit_card"]}}),
            )],
        ),
        ex(
            "How much did I spend on streaming services in 2024?",
            vec![sum(
                "total",
                "spending",
                json!({"period": year(2024), "categories": ["entertainment.streaming"]}),
            )],
        ),
        ex(
            "What did eating out cost me last month?",
            vec![sum(
                "total",
                "spending",
                json!({"period": {"last_months": 1}, "categories": ["food.restaurants", "food.fast_food"]}),
            )],
        ),
        ex(
            "How much went on pharmacy bills between March and May 2024?",
            vec![sum(
                "total",
                "spending",
                json!({"period": {"months": {"from": "2024-03", "to": "2024-05"}}, "categories": ["health.pharmacy"]}),
            )],
        ),
        ex(
            "How much did I spend at Costco in the last 90 days?",
            vec![sum(
                "total",
                "spending",
                json!({"period": {"last_days": 90}, "merchant": "Costco"}),
            )],
        ),
        ex(
            "How much did Contoso pay me in 2023?",
            vec![sum(
                "total",
                "income",
                json!({"period": year(2023), "merchant": "Contoso"}),
            )],
        ),
        ex(
            "What did I spend on food last week, groceries and eating out together?",
            vec![sum(
                "total",
                "spending",
                json!({"period": "last_week", "groups": ["food"]}),
            )],
        ),
        ex(
            "How many taxi rides did I take in 2024?",
            vec![
                json!({"name": "rides", "ask": {"op": "count", "measure": "purchases",
                "scope": {"period": year(2024), "categories": ["transport.rideshare"]}}}),
            ],
        ),
        ex(
            "How much did I earn and spend in 2024, and what was left?",
            vec![
                sum("earned", "income", json!({"period": year(2024)})),
                sum("spent", "spending", json!({"period": year(2024)})),
                derive("left", "difference", &["earned", "spent"]),
            ],
        ),
        ex(
            "What's my typical monthly fuel spending lately?",
            vec![
                json!({"name": "median", "ask": {"op": "median", "measure": "spending",
                "scope": {"period": {"last_months": 6}, "categories": ["transport.fuel"]}, "per": "month"}}),
            ],
        ),
        ex(
            "How many different gyms or fitness studios did I pay in 2024?",
            vec![
                json!({"name": "places", "ask": {"op": "distinct", "measure": "purchases",
                "scope": {"period": year(2024), "categories": ["health.fitness"]}, "by": "merchant"}}),
            ],
        ),
        ex(
            "Which months was I charged a bank fee in?",
            vec![
                json!({"name": "months", "ask": {"op": "keys", "measure": "spending",
                "scope": {"period": "all_time", "categories": ["financial.bank_fees"]}, "by": "month"}}),
            ],
        ),
        ex(
            "How much did I get back in returns so far this year?",
            vec![sum("total", "refunds", json!({"period": "year_to_date"}))],
        ),
        ex(
            "How much have I paid in ATM surcharge fees this year?",
            vec![sum(
                "total",
                "spending",
                json!({"period": "year_to_date", "categories": ["financial.bank_fees"], "description": "atm surcharge"}),
            )],
        ),
        ex(
            "Which bills have I stopped paying?",
            vec![json!({"name": "stopped", "ask": {"op": "recurring",
                "select": {"kinds": ["bill"], "status": "cancelled"}, "output": "names"}})],
        ),
        ex(
            "How did my electricity bill in January 2024 compare with December 2023?",
            vec![
                sum(
                    "january_2024",
                    "spending",
                    json!({"period": {"month": "2024-01"}, "categories": ["utilities.electricity"]}),
                ),
                sum(
                    "december_2023",
                    "spending",
                    json!({"period": {"month": "2023-12"}, "categories": ["utilities.electricity"]}),
                ),
                derive(
                    "difference",
                    "difference",
                    &["january_2024", "december_2023"],
                ),
                derive(
                    "change",
                    "percent_change",
                    &["january_2024", "december_2023"],
                ),
            ],
        ),
        ex(
            "Did I spend more on clothes or on sporting goods in 2023?",
            vec![
                sum(
                    "clothes",
                    "spending",
                    json!({"period": year(2023), "categories": ["shopping.clothing"]}),
                ),
                sum(
                    "sporting_goods",
                    "spending",
                    json!({"period": year(2023), "categories": ["shopping.sporting_goods"]}),
                ),
                derive("difference", "difference", &["clothes", "sporting_goods"]),
                derive("more", "larger", &["clothes", "sporting_goods"]),
            ],
        ),
        ex(
            "Which category did I spend the most on in 2023, and how much?",
            vec![
                json!({"name": "category", "ask": {"op": "rank", "measure": "spending",
                    "scope": {"period": year(2023)}, "by": "category", "order": "highest", "output": "label"}}),
                json!({"name": "total", "ask": {"op": "rank", "measure": "spending",
                    "scope": {"period": year(2023)}, "by": "category", "order": "highest", "output": "amount"}}),
            ],
        ),
        ex(
            "In which month of 2024 was my grocery bill lowest?",
            vec![
                json!({"name": "month", "ask": {"op": "rank", "measure": "spending",
                    "scope": {"period": year(2024), "categories": ["food.groceries"]},
                    "by": "month", "order": "lowest", "output": "label"}}),
                json!({"name": "total", "ask": {"op": "rank", "measure": "spending",
                    "scope": {"period": year(2024), "categories": ["food.groceries"]},
                    "by": "month", "order": "lowest", "output": "amount"}}),
            ],
        ),
        ex(
            "Who were my two biggest shopping merchants in 2024?",
            vec![
                json!({"name": "merchants", "ask": {"op": "rank", "measure": "spending",
                    "scope": {"period": year(2024), "groups": ["shopping"]},
                    "by": "merchant", "order": "highest", "n": 2, "output": "labels"}}),
                json!({"name": "first", "ask": {"op": "rank", "measure": "spending",
                    "scope": {"period": year(2024), "groups": ["shopping"]},
                    "by": "merchant", "order": "highest", "output": "amount", "position": 1}}),
                json!({"name": "second", "ask": {"op": "rank", "measure": "spending",
                    "scope": {"period": year(2024), "groups": ["shopping"]},
                    "by": "merchant", "order": "highest", "output": "amount", "position": 2}}),
            ],
        ),
        ex(
            "What's my average monthly phone bill over the last 12 months?",
            vec![
                json!({"name": "average", "ask": {"op": "average", "measure": "spending",
                    "scope": {"period": {"last_months": 12}, "categories": ["utilities.phone"]}, "per": "month"}}),
                sum(
                    "total",
                    "spending",
                    json!({"period": {"last_months": 12}, "categories": ["utilities.phone"]}),
                ),
            ],
        ),
        ex(
            "When did I last fill up at Exxon, and how much was it?",
            vec![
                json!({"name": "date", "ask": {"op": "pick", "measure": "purchases",
                    "scope": {"period": "all_time", "merchant": "Exxon"}, "order": "latest", "output": "date"}}),
                json!({"name": "amount", "ask": {"op": "pick", "measure": "purchases",
                    "scope": {"period": "all_time", "merchant": "Exxon"}, "order": "latest", "output": "amount"}}),
            ],
        ),
        ex(
            "What was my most expensive hotel stay in 2024, and where?",
            vec![
                json!({"name": "merchant", "ask": {"op": "pick", "measure": "purchases",
                    "scope": {"period": year(2024), "categories": ["travel.lodging"]}, "order": "largest", "output": "merchant"}}),
                json!({"name": "amount", "ask": {"op": "pick", "measure": "purchases",
                    "scope": {"period": year(2024), "categories": ["travel.lodging"]}, "order": "largest", "output": "amount"}}),
                json!({"name": "date", "ask": {"op": "pick", "measure": "purchases",
                    "scope": {"period": year(2024), "categories": ["travel.lodging"]}, "order": "largest", "output": "date"}}),
            ],
        ),
        ex(
            "What percentage of my 2024 spending went to groceries?",
            vec![
                sum(
                    "groceries",
                    "spending",
                    json!({"period": year(2024), "categories": ["food.groceries"]}),
                ),
                sum("total_spending", "spending", json!({"period": year(2024)})),
                derive("share", "share", &["groceries", "total_spending"]),
            ],
        ),
        ex(
            "How much did I move from savings back into checking in 2024? Is that spending?",
            vec![
                sum(
                    "moved",
                    "transfers_out",
                    json!({"period": year(2024), "accounts": {"kinds": ["savings"]},
                        "counterpart_accounts": {"kinds": ["checking"]}}),
                ),
                sum(
                    "spending_effect",
                    "spending",
                    json!({"period": year(2024), "accounts": {"kinds": ["savings"]},
                        "counterpart_accounts": {"kinds": ["checking"]}}),
                ),
            ],
        ),
        ex(
            "How much money came into my savings account last quarter, and how much of it was interest?",
            vec![
                sum(
                    "money_in",
                    "money_in",
                    json!({"period": "last_quarter", "accounts": {"kinds": ["savings"]}}),
                ),
                sum(
                    "interest",
                    "income",
                    json!({"period": "last_quarter", "accounts": {"kinds": ["savings"]}}),
                ),
            ],
        ),
        ex(
            "Do I have any card charges still pending? How much?",
            vec![
                json!({"name": "pending", "ask": {"op": "count", "measure": "pending",
                    "scope": {"period": "all_time", "accounts": {"kinds": ["credit_card"]}}}}),
                sum(
                    "total",
                    "pending",
                    json!({"period": "all_time", "accounts": {"kinds": ["credit_card"]}}),
                ),
            ],
        ),
        ex(
            "How much did I spend in Canadian dollars, in Canadian dollars and in my own currency?",
            vec![
                sum(
                    "canadian_dollars",
                    "spending",
                    json!({"period": "all_time", "original_currency": "CAD", "amounts": "original"}),
                ),
                sum(
                    "charged",
                    "spending",
                    json!({"period": "all_time", "original_currency": "CAD"}),
                ),
            ],
        ),
        ex(
            "What do my streaming subscriptions cost me now?",
            vec![
                json!({"name": "subscriptions", "ask": {"op": "recurring",
                    "select": {"kinds": ["subscription"], "categories": ["entertainment.streaming"], "status": "active"},
                    "output": "names"}}),
                json!({"name": "total", "ask": {"op": "recurring",
                    "select": {"kinds": ["subscription"], "categories": ["entertainment.streaming"], "status": "active"},
                    "output": "latest_total"}}),
            ],
        ),
        ex(
            "Did my gym put its price up? When, and what do I pay now?",
            vec![
                json!({"name": "from", "ask": {"op": "recurring",
                    "select": {"categories": ["health.fitness"]}, "output": "price_from"}}),
                json!({"name": "now", "ask": {"op": "recurring",
                    "select": {"categories": ["health.fitness"]}, "output": "price_after"}}),
            ],
        ),
        ex(
            "How many Netflix charges were there in 2024, and what did they add up to?",
            vec![
                json!({"name": "charges", "ask": {"op": "recurring",
                    "select": {"name": "Netflix", "period": year(2024)}, "output": "member_count"}}),
                json!({"name": "total", "ask": {"op": "recurring",
                    "select": {"name": "Netflix", "period": year(2024)}, "output": "member_total"}}),
            ],
        ),
        ex(
            "Which monthly bills am I paying right now, and what do they come to?",
            vec![
                json!({"name": "bills", "ask": {"op": "recurring",
                    "select": {"kinds": ["bill"], "cadences": ["monthly"], "status": "active"}, "output": "names"}}),
                json!({"name": "total", "ask": {"op": "recurring",
                    "select": {"kinds": ["bill"], "cadences": ["monthly"], "status": "active"}, "output": "latest_total"}}),
            ],
        ),
        ex(
            "What do my yearly memberships cost me now, and which are they?",
            vec![
                json!({"name": "total", "ask": {"op": "recurring",
                    "select": {"kinds": ["subscription"], "cadences": ["annual"], "status": "active"}, "output": "latest_total"}}),
                json!({"name": "memberships", "ask": {"op": "recurring",
                    "select": {"kinds": ["subscription"], "cadences": ["annual"], "status": "active"}, "output": "names"}}),
            ],
        ),
    ]
}
