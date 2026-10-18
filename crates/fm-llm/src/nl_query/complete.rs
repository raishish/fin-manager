//! Answer completion: fixed rules that add the companion values a full answer shows,
//! when the model wrote the main one and stopped. Off unless the app asks for it.
//!
//! Every rule only **adds** items, built from the model's own items (same measure,
//! same scope); nothing the model wrote is changed or removed. The added items are in
//! the query the user sees, and [`Completion::added`] says which they are and which
//! rule added each, so the UI can mark them ("Added by the app: <why>").
//!
//! The rules are listed in [`RULES`] (the eval report prints them).
//!
//! An added item can fail where the model's own would not (a price change on a series
//! that never changed price). The app runs the completed query and, if it fails, the
//! model's own query instead.

use serde_json::{Map, Value as Json, json};

/// One completion rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    /// Stable id, e.g. `rank-amount`.
    pub id: &'static str,
    /// What the model wrote.
    pub wrote: &'static str,
    /// What the rule adds; a one-line reason the UI can show next to an added item.
    pub adds: &'static str,
}

const fn rule(id: &'static str, wrote: &'static str, adds: &'static str) -> Rule {
    Rule { id, wrote, adds }
}

/// Every completion rule.
pub const RULES: &[Rule] = &[
    rule(
        "rank-amount",
        "a ranked label",
        "the same rank's amount (or count, if ranked by count)",
    ),
    rule(
        "rank-label",
        "a ranked amount or count",
        "the same rank's label",
    ),
    rule(
        "top-amounts",
        "the top n labels",
        "the amount (or count) at each position",
    ),
    rule(
        "pick-others",
        "one output of a picked transaction",
        "the other two of merchant, amount and date",
    ),
    rule(
        "pair-compare",
        "two money sums of one measure",
        "their difference and percent change if nothing is derived from them yet, and the larger",
    ),
    rule(
        "difference-change",
        "a difference",
        "the percent change of the same two items",
    ),
    rule(
        "part-share",
        "a sum within another sum's scope (same measure and period, fewer filters)",
        "its share of the larger (instead of a difference)",
    ),
    rule(
        "price-steps",
        "a recurring price before, after or date of change",
        "the other two",
    ),
    rule("names-total", "recurring names", "their latest total"),
    rule(
        "total-names",
        "a recurring latest total",
        "the names behind it",
    ),
    rule(
        "average-parts",
        "an average",
        "the sum and the count (of purchases, for spending)",
    ),
    rule(
        "counts-sum",
        "two counts, nothing derived from them",
        "their sum",
    ),
    rule("count-sum", "a count", "the sum"),
    rule(
        "original-charged",
        "a sum in the original currency",
        "the same in the account's currency",
    ),
    rule(
        "money-split",
        "all money in or out",
        "the part of it that is income or spending, for the same accounts and period",
    ),
];

/// One item [`complete`] added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub name: String,
    /// The rule's [`Rule::id`].
    pub rule: &'static str,
    /// The rule's [`Rule::adds`], for the UI.
    pub why: &'static str,
}

/// The completed query and what was added to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub query: Json,
    /// In the order they were added (also their order at the end of `query.items`).
    pub added: Vec<Added>,
}

type Items = Vec<Json>;

fn op(item: &Json) -> &str {
    item["ask"]["op"].as_str().unwrap_or("")
}

fn name(item: &Json) -> &str {
    item["name"].as_str().unwrap_or("")
}

/// An `ask` with one field changed.
fn with(ask: &Json, key: &str, value: Json) -> Json {
    let mut a = ask.clone();
    if let Some(o) = a.as_object_mut() {
        o.insert(key.to_owned(), value);
    }
    a
}

/// Whether an item with this ask is already there (by ask, whatever its name).
fn has(items: &Items, ask: &Json) -> bool {
    items.iter().any(|i| &i["ask"] == ask)
}

struct Adder {
    items: Items,
    added: Vec<Added>,
}

impl Adder {
    fn unique(&self, base: &str) -> String {
        let taken = |n: &str| self.items.iter().any(|i| name(i) == n);
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|k| format!("{base}_{k}"))
            .find(|n| !taken(n))
            .unwrap_or_default()
    }

    fn add(&mut self, rule: &'static str, base: &str, ask: Json) {
        if !has(&self.items, &ask) {
            let n = self.unique(base);
            let why = RULES.iter().find(|r| r.id == rule).map_or("", |r| r.adds);
            self.added.push(Added {
                name: n.clone(),
                rule,
                why,
            });
            self.items.push(json!({"name": n, "ask": ask}));
        }
    }
}

fn is_sum_of_money(item: &Json) -> bool {
    op(item) == "sum" && item["ask"]["measure"].as_str() != Some("pending")
}

/// `a`'s scope is `b`'s with some filters added (and nothing else different).
fn narrower(a: &Map<String, Json>, b: &Map<String, Json>) -> bool {
    a.len() > b.len() && b.iter().all(|(k, v)| a.get(k) == Some(v))
}

/// `query` with companion items added (see the module docs).
pub fn complete(query: &Json) -> Completion {
    let Some(original) = query["items"].as_array() else {
        return Completion {
            query: query.clone(),
            added: Vec::new(),
        };
    };
    let mut a = Adder {
        items: original.clone(),
        added: Vec::new(),
    };
    for item in original {
        let ask = &item["ask"];
        let n = name(item);
        match op(item) {
            "rank" => {
                let metric_output = if ask["metric"] == "count" {
                    "count"
                } else {
                    "amount"
                };
                match ask["output"].as_str() {
                    Some("label") => a.add(
                        "rank-amount",
                        &format!("{n}_{metric_output}"),
                        with(ask, "output", json!(metric_output)),
                    ),
                    Some("amount") | Some("count") => a.add(
                        "rank-label",
                        &format!("{n}_label"),
                        with(ask, "output", json!("label")),
                    ),
                    Some("labels") => {
                        let top = ask["n"].as_u64().unwrap_or(1);
                        for p in 1..=top {
                            let mut at = with(ask, "output", json!(metric_output));
                            at = with(&at, "position", json!(p));
                            if let Some(o) = at.as_object_mut() {
                                o.remove("n");
                            }
                            a.add("top-amounts", &format!("{n}_{p}"), at);
                        }
                    }
                    _ => {}
                }
            }
            "pick" => {
                for out in ["merchant", "amount", "date"] {
                    if ask["output"] != out {
                        a.add(
                            "pick-others",
                            &format!("{n}_{out}"),
                            with(ask, "output", json!(out)),
                        );
                    }
                }
            }
            "recurring" => match ask["output"].as_str() {
                Some("price_before" | "price_after" | "price_from") => {
                    for out in ["price_from", "price_before", "price_after"] {
                        a.add(
                            "price-steps",
                            &format!("{n}_{out}"),
                            with(ask, "output", json!(out)),
                        );
                    }
                }
                Some("names") => a.add(
                    "names-total",
                    &format!("{n}_total"),
                    with(ask, "output", json!("latest_total")),
                ),
                Some("latest_total") => a.add(
                    "total-names",
                    &format!("{n}_names"),
                    with(ask, "output", json!("names")),
                ),
                _ => {}
            },
            "average" => {
                let sum = json!({"op": "sum", "measure": ask["measure"], "scope": ask["scope"]});
                a.add("average-parts", &format!("{n}_total"), sum);
                let measure = if ask["measure"] == "spending" {
                    json!("purchases")
                } else {
                    ask["measure"].clone()
                };
                a.add(
                    "average-parts",
                    &format!("{n}_count"),
                    json!({"op": "count", "measure": measure, "scope": ask["scope"]}),
                );
            }
            "count" => {
                a.add(
                    "count-sum",
                    &format!("{n}_total"),
                    json!({"op": "sum", "measure": ask["measure"], "scope": ask["scope"]}),
                );
            }
            "sum" if ask["scope"]["amounts"] == "original" => {
                let mut scope = ask["scope"].clone();
                if let Some(o) = scope.as_object_mut() {
                    o.remove("amounts");
                }
                a.add(
                    "original-charged",
                    &format!("{n}_charged"),
                    with(ask, "scope", scope),
                );
            }
            "sum" if ask["measure"] == "money_out" || ask["measure"] == "money_in" => {
                let (part, label) = if ask["measure"] == "money_out" {
                    ("spending", "spending")
                } else {
                    ("income", "income")
                };
                a.add(
                    "money-split",
                    &format!("{n}_{label}"),
                    with(ask, "measure", json!(part)),
                );
            }
            "derive" if ask["formula"] == "difference" => {
                a.add(
                    "difference-change",
                    &format!("{n}_change"),
                    with(ask, "formula", json!("percent_change")),
                );
            }
            _ => {}
        }
    }

    // Pairs of money sums the model wrote.
    let sums: Vec<&Json> = original.iter().filter(|i| is_sum_of_money(i)).collect();
    let derived_from = |x: &str, y: &str| {
        original.iter().any(|i| {
            op(i) == "derive"
                && i["ask"]["of"]
                    .as_array()
                    .is_some_and(|of| of.iter().any(|v| v == x) && of.iter().any(|v| v == y))
        })
    };
    if sums.len() == 2 {
        let (x, y) = (sums[0], sums[1]);
        let (nx, ny) = (name(x), name(y));
        let (sx, sy) = (&x["ask"]["scope"], &y["ask"]["scope"]);
        let same_measure = x["ask"]["measure"] == y["ask"]["measure"];
        let part = match (sx.as_object(), sy.as_object()) {
            (Some(ox), Some(oy)) if same_measure && sx["period"] == sy["period"] => {
                if narrower(ox, oy) {
                    Some((nx, ny))
                } else if narrower(oy, ox) {
                    Some((ny, nx))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some((p, whole)) = part {
            a.add(
                "part-share",
                "share",
                json!({"op": "derive", "formula": "share", "of": [p, whole]}),
            );
        } else {
            if !derived_from(nx, ny) {
                a.add(
                    "pair-compare",
                    "difference",
                    json!({"op": "derive", "formula": "difference", "of": [nx, ny]}),
                );
                a.add(
                    "pair-compare",
                    "change",
                    json!({"op": "derive", "formula": "percent_change", "of": [nx, ny]}),
                );
            }
            if same_measure {
                a.add(
                    "pair-compare",
                    "larger",
                    json!({"op": "derive", "formula": "larger", "of": [nx, ny]}),
                );
            }
        }
    }
    // Two counts the model wrote separately, and their total.
    let counts: Vec<&Json> = original
        .iter()
        .filter(|i| op(i) == "count" || (op(i) == "recurring" && i["ask"]["output"] == "count"))
        .collect();
    if counts.len() == 2 {
        let (nx, ny) = (name(counts[0]), name(counts[1]));
        if !derived_from(nx, ny) {
            a.add(
                "counts-sum",
                "total",
                json!({"op": "derive", "formula": "sum", "of": [nx, ny]}),
            );
        }
    }
    Completion {
        query: json!({ "items": a.items }),
        added: a.added,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(q: &Json) -> Vec<(String, Json)> {
        q["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (name(i).to_owned(), i["ask"].clone()))
            .collect()
    }

    #[test]
    fn a_ranked_label_gets_its_amount_and_the_original_is_untouched() {
        let q = json!({"items": [{"name": "month", "ask": {"op": "rank", "measure": "spending",
            "scope": {"period": {"year": 2025}}, "by": "month", "order": "highest", "output": "label"}}]});
        let c = complete(&q).query;
        let got = items(&c);
        assert_eq!(got[0], items(&q)[0]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].0, "month_amount");
        assert_eq!(got[1].1["output"], "amount");
        // Completing again adds nothing.
        assert_eq!(complete(&c).query, c);
    }

    #[test]
    fn top_n_gets_each_amount_and_counts_stay_counts() {
        let q = json!({"items": [{"name": "top", "ask": {"op": "rank", "measure": "purchases",
            "scope": {"period": "all_time"}, "by": "merchant", "order": "highest", "metric": "count",
            "n": 2, "output": "labels"}}]});
        let got = items(&complete(&q).query);
        assert_eq!(got.len(), 3);
        assert_eq!(
            (got[1].1["output"].as_str(), got[1].1["position"].as_u64()),
            (Some("count"), Some(1))
        );
        assert_eq!(got[2].1["position"], 2);
    }

    #[test]
    fn two_sums_get_compared_and_a_part_gets_its_share() {
        let pair = json!({"items": [
            {"name": "a", "ask": {"op": "sum", "measure": "spending", "scope": {"period": {"month": "2025-01"}}}},
            {"name": "b", "ask": {"op": "sum", "measure": "spending", "scope": {"period": {"month": "2025-02"}}}}]});
        let names: Vec<String> = items(&complete(&pair).query)
            .into_iter()
            .map(|i| i.0)
            .collect();
        assert_eq!(names, ["a", "b", "difference", "change", "larger"]);

        let part = json!({"items": [
            {"name": "rent", "ask": {"op": "sum", "measure": "spending", "scope": {"period": {"year": 2025}, "categories": ["housing.rent"]}}},
            {"name": "all", "ask": {"op": "sum", "measure": "spending", "scope": {"period": {"year": 2025}}}}]});
        let got = items(&complete(&part).query);
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[2].1,
            json!({"op": "derive", "formula": "share", "of": ["rent", "all"]})
        );
    }

    #[test]
    fn every_added_item_says_which_rule_added_it() {
        let q = json!({"items": [
            {"name": "month", "ask": {"op": "rank", "measure": "spending", "scope": {"period": {"year": 2025}},
              "by": "month", "order": "highest", "output": "label"}},
            {"name": "a", "ask": {"op": "sum", "measure": "spending", "scope": {"period": {"month": "2025-01"}}}},
            {"name": "b", "ask": {"op": "sum", "measure": "spending", "scope": {"period": {"month": "2025-02"}}}}]});
        let c = complete(&q);
        let added: Vec<(&str, &str)> = c.added.iter().map(|x| (x.name.as_str(), x.rule)).collect();
        assert_eq!(
            added,
            [
                ("month_amount", "rank-amount"),
                ("difference", "pair-compare"),
                ("change", "pair-compare"),
                ("larger", "pair-compare")
            ]
        );
        let names: Vec<&str> = c.query["items"].as_array().unwrap()[3..]
            .iter()
            .map(name)
            .collect();
        assert_eq!(names, ["month_amount", "difference", "change", "larger"]);
        assert!(c.added.iter().all(|x| !x.why.is_empty()));
        // Rule ids are unique.
        let ids: std::collections::BTreeSet<&str> = RULES.iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), RULES.len());
    }

    #[test]
    fn two_counts_get_their_total() {
        let q = json!({"items": [
            {"name": "bills", "ask": {"op": "recurring", "select": {"kinds": ["bill"]}, "output": "count"}},
            {"name": "subs", "ask": {"op": "recurring", "select": {"kinds": ["subscription"]}, "output": "count"}}]});
        let got = items(&complete(&q).query);
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[2].1,
            json!({"op": "derive", "formula": "sum", "of": ["bills", "subs"]})
        );
    }

    #[test]
    fn other_companions() {
        let q = json!({"items": [
            {"name": "when", "ask": {"op": "pick", "measure": "purchases", "scope": {"period": "all_time"}, "order": "latest", "output": "date"}},
            {"name": "now", "ask": {"op": "recurring", "select": {"name": "Gym"}, "output": "price_after"}},
            {"name": "eur", "ask": {"op": "sum", "measure": "spending", "scope": {"period": "all_time", "original_currency": "EUR", "amounts": "original"}}},
            {"name": "avg", "ask": {"op": "average", "measure": "spending", "scope": {"period": {"year": 2025}}, "per": "transaction"}}]});
        let names: Vec<String> = items(&complete(&q).query)
            .into_iter()
            .map(|i| i.0)
            .collect();
        assert_eq!(
            names,
            [
                "when",
                "now",
                "eur",
                "avg",
                "when_merchant",
                "when_amount",
                "now_price_from",
                "now_price_before",
                "eur_charged",
                "avg_total",
                "avg_count"
            ]
        );
    }
}
