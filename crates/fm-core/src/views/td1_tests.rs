//! T3.1's done-when: the views match TD.1 ground truth exactly.
//!
//! Each household is labelled from `truth.json` (one applied decision per truth bucket
//! and merchant). The expected figures come from a separate, plain fold over the raw
//! JSON of `ledger.json` and `truth.json`, not from the views code: per category and
//! month, per group and month, per merchant and year, income per month, transfers,
//! pending and net flow per month. Every cell and count must match exactly.
//!
//! Then TD.3's answers for the freelancer (`testdata/questions`) are recomputed through
//! the views, amounts and evidence transaction ids both.
//!
//! Failure messages name the check and a category key or month, never an amount or a
//! merchant. Run with `-- --nocapture` for the `T3.1-TD1` summary lines.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use super::tests::assert_adds_up;
use super::*;
use crate::model::*;

const PERSONAS: [&str; 4] = ["renter", "family", "freelancer", "retiree"];

fn testdata(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(rel)
}

fn read(rel: &str) -> Value {
    serde_json::from_slice(&fs::read(testdata(rel)).unwrap()).unwrap()
}

fn date(s: &str) -> Date {
    s.parse().unwrap()
}

fn str_of<'a>(v: &'a Value, field: &str) -> &'a str {
    v[field].as_str().unwrap()
}

struct Household {
    ledger: Ledger,
    raw: Value,
    truth: Value,
}

/// The household with one applied user decision per truth bucket and merchant.
fn household(persona: &str) -> Household {
    let raw = read(&format!("synthetic/data/{persona}/ledger.json"));
    let truth = read(&format!("synthetic/data/{persona}/truth.json"));
    let mut ledger: Ledger = serde_json::from_value(raw.clone()).unwrap();
    let by_key: HashMap<String, CategoryId> = ledger
        .taxonomy
        .categories
        .iter()
        .map(|c| (c.key.clone(), c.id))
        .collect();
    let category = |b: &Value| by_key[str_of(b, "key")];
    let other = |v: &Value| v.as_i64().map(TransactionId);
    let mut decisions = Vec::new();
    for t in truth["transactions"].as_array().unwrap() {
        let id = TransactionId(t["id"].as_i64().unwrap());
        let b = &t["bucket"];
        let bucket = match str_of(b, "kind") {
            "category" => Bucket::Category {
                category_id: category(b),
            },
            "refund" => Bucket::Refund {
                category_id: category(b),
                original: other(&b["original"]),
            },
            "transfer" => Bucket::Transfer {
                counterpart: other(&b["counterpart"]),
            },
            "income" => Bucket::Income,
            kind => panic!("unexpected truth bucket kind {kind}"),
        };
        let mut assignments = vec![Assignment::Bucket(bucket)];
        if let Some(m) = t["merchant"].as_str() {
            assignments.push(Assignment::Merchant(m.into()));
        }
        for assignment in assignments {
            decisions.push(Decision {
                id: DecisionId(i64::try_from(decisions.len()).unwrap() + 1),
                transaction_id: id,
                assignment,
                source: DecisionSource::User,
                taxonomy_version: ledger.taxonomy.version.clone(),
                decided_at: Timestamp::from_unix_seconds(1_760_000_000),
                state: DecisionState::Applied,
                seen_at: None,
                replaces: None,
            });
        }
    }
    ledger.decisions = decisions;
    Household { ledger, raw, truth }
}

/// (sum, count)
type Cell = (i64, usize);

fn bump<K: Ord>(map: &mut BTreeMap<K, Cell>, key: K, amount: i64) {
    let cell = map.entry(key).or_default();
    cell.0 += amount;
    cell.1 += 1;
}

/// The expected figures, folded straight from the JSON files.
#[derive(Default)]
struct Expected {
    /// (category key, YYYY-MM) → net spending.
    category_month: BTreeMap<(String, String), Cell>,
    /// (group key, YYYY-MM) → net spending.
    group_month: BTreeMap<(String, String), Cell>,
    /// (merchant, YYYY) → net spending.
    merchant_year: BTreeMap<(String, String), Cell>,
    income_month: BTreeMap<String, Cell>,
    refunds_month: BTreeMap<String, Cell>,
    transfers_out_month: BTreeMap<String, Cell>,
    transfers_in_month: BTreeMap<String, Cell>,
    pending_month: BTreeMap<String, Cell>,
    net_flow_month: BTreeMap<String, Cell>,
}

fn fold(raw: &Value, truth: &Value) -> Expected {
    let truth: HashMap<i64, &Value> = truth["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["id"].as_i64().unwrap(), t))
        .collect();
    let mut e = Expected::default();
    for t in raw["transactions"].as_array().unwrap() {
        let day = t["transaction_date"]
            .as_str()
            .unwrap_or_else(|| str_of(t, "posted_date"));
        let (year, month) = (day[..4].to_owned(), day[..7].to_owned());
        let amount = t["amount"]["minor"].as_i64().unwrap();
        if str_of(t, "status") == "pending" {
            bump(&mut e.pending_month, month, amount);
            continue;
        }
        bump(&mut e.net_flow_month, month.clone(), amount);
        let truth = truth[&t["id"].as_i64().unwrap()];
        let bucket = &truth["bucket"];
        match str_of(bucket, "kind") {
            kind @ ("category" | "refund") => {
                let key = str_of(bucket, "key");
                let group = key.split('.').next().unwrap();
                let merchant = truth["merchant"]
                    .as_str()
                    .unwrap_or_else(|| str_of(t, "description"));
                bump(
                    &mut e.category_month,
                    (key.to_owned(), month.clone()),
                    -amount,
                );
                bump(
                    &mut e.group_month,
                    (group.to_owned(), month.clone()),
                    -amount,
                );
                bump(&mut e.merchant_year, (merchant.to_owned(), year), -amount);
                if kind == "refund" {
                    bump(&mut e.refunds_month, month, amount);
                }
            }
            "income" => bump(&mut e.income_month, month, amount),
            "transfer" if amount < 0 => bump(&mut e.transfers_out_month, month, -amount),
            "transfer" => bump(&mut e.transfers_in_month, month, amount),
            kind => panic!("unexpected truth bucket kind {kind}"),
        }
    }
    e
}

/// Asserts two maps are equal, naming the check and (if `show_key`) the first key that
/// differs. Never prints an amount.
fn same<K: Ord + std::fmt::Debug, V: PartialEq>(
    check: &str,
    got: &BTreeMap<K, V>,
    want: &BTreeMap<K, V>,
    show_key: bool,
) {
    let differing: Vec<&K> = got
        .keys()
        .chain(want.keys())
        .filter(|k| got.get(k) != want.get(k))
        .collect();
    if let Some(first) = differing.first() {
        let at = if show_key {
            format!(", first at {first:?}")
        } else {
            String::new()
        };
        panic!("{check}: {} entries differ{at}", differing.len());
    }
}

fn nonzero<K: Ord + Clone>(m: &BTreeMap<K, Cell>) -> BTreeMap<K, i64> {
    m.iter()
        .filter(|(_, c)| c.0 != 0)
        .map(|(k, c)| (k.clone(), c.0))
        .collect()
}

/// Per key over all months: (sum, count).
fn per_key<K: Ord + Clone>(m: &BTreeMap<(K, String), Cell>) -> BTreeMap<K, Cell> {
    let mut out: BTreeMap<K, Cell> = BTreeMap::new();
    for ((k, _), (sum, count)) in m {
        let cell = out.entry(k.clone()).or_default();
        cell.0 += sum;
        cell.1 += count;
    }
    out
}

const AS_OF: &str = "2026-10-01";

fn query(period: Period, group_by: GroupBy, measure: Measure) -> Query {
    Query {
        period,
        filter: Filter::default(),
        group_by,
        measure,
        top: None,
        as_of: date(AS_OF),
    }
}

#[test]
fn views_match_td1_ground_truth_exactly() {
    for persona in PERSONAS {
        let Household { ledger, raw, truth } = household(persona);
        let e = fold(&raw, &truth);
        let full = Period::new(date(str_of(&truth, "start")), date(str_of(&truth, "end"))).unwrap();
        let tax = &ledger.taxonomy;
        let cat_key: HashMap<CategoryId, &str> = tax
            .categories
            .iter()
            .map(|c| (c.id, c.key.as_str()))
            .collect();
        let group_key: HashMap<CategoryGroupId, &str> =
            tax.groups.iter().map(|g| (g.id, g.key.as_str())).collect();
        let check = |what: &str| format!("{persona} {what}");

        // Category × month and group × month, from one monthly series each.
        let mut cells = 0;
        for (group_by, want) in [
            (GroupBy::Category, &e.category_month),
            (GroupBy::Group, &e.group_month),
        ] {
            let s = monthly(&ledger, &query(full, group_by, Measure::Spending)).unwrap();
            let name = |k: &Key| match k {
                Key::Category { id } => cat_key[id].to_owned(),
                Key::Group { id } => group_key[id].to_owned(),
                other => panic!("unexpected key kind {other:?}"),
            };
            let mut got = BTreeMap::new();
            let mut got_rows = BTreeMap::new();
            for row in &s.rows {
                for (col, v) in s.columns.iter().zip(&row.values) {
                    if v.minor() != 0 {
                        got.insert((name(&row.key), col.month.to_string()), v.minor());
                    }
                }
                got_rows.insert(name(&row.key), (row.total.minor(), row.count));
            }
            same(
                &check(&format!("{group_by:?} × month")),
                &got,
                &nonzero(want),
                true,
            );
            same(
                &check(&format!("{group_by:?} rows")),
                &got_rows,
                &per_key(want),
                true,
            );
            assert!(s.other.is_none());
            cells += got.len();
        }

        // Merchant × year.
        let mut merchants = BTreeMap::new();
        for year in 2024..=2026 {
            let b = breakdown(
                &ledger,
                &query(
                    Period::year(year).unwrap(),
                    GroupBy::Merchant,
                    Measure::Spending,
                ),
            )
            .unwrap();
            for row in &b.rows {
                let Key::Merchant { name, .. } = &row.key else {
                    panic!("{persona}: merchant view returned another key kind");
                };
                merchants.insert(
                    (name.clone(), year.to_string()),
                    (row.total.minor(), row.count),
                );
            }
            assert_adds_up(&b);
        }
        same(
            &check("merchant × year"),
            &merchants,
            &e.merchant_year,
            false,
        );

        // Income per month.
        let s = monthly(&ledger, &query(full, GroupBy::Month, Measure::Income)).unwrap();
        let got: BTreeMap<String, i64> = s
            .columns
            .iter()
            .filter(|c| c.total.minor() != 0)
            .map(|c| (c.month.to_string(), c.total.minor()))
            .collect();
        same(
            &check("income × month"),
            &got,
            &nonzero(&e.income_month),
            true,
        );

        // Every month on its own: rows, the other lines, and the identities.
        let lines = |l: &Line| (l.amount.minor(), l.count);
        let at = |m: &BTreeMap<String, Cell>, k: &str| m.get(k).copied().unwrap_or_default();
        let mut spending_total = 0;
        for month in full.year_months() {
            let key = month.to_string();
            let b = breakdown(
                &ledger,
                &query(Period::month(month), GroupBy::Category, Measure::Spending),
            )
            .unwrap();
            let rows: BTreeMap<String, Cell> = b
                .rows
                .iter()
                .map(|r| {
                    let Key::Category { id } = r.key else {
                        panic!("{persona}: category view returned another key kind");
                    };
                    (cat_key[&id].to_owned(), (r.total.minor(), r.count))
                })
                .collect();
            let want: BTreeMap<String, Cell> = e
                .category_month
                .iter()
                .filter(|((_, m), _)| *m == key)
                .map(|((k, _), c)| (k.clone(), *c))
                .collect();
            same(&check(&format!("{key} rows")), &rows, &want, true);
            let t = &b.totals;
            let pairs = [
                ("income", lines(&t.income), at(&e.income_month, &key)),
                ("refunds", lines(&t.refunds), at(&e.refunds_month, &key)),
                (
                    "transfers out",
                    lines(&t.transfers_out),
                    at(&e.transfers_out_month, &key),
                ),
                (
                    "transfers in",
                    lines(&t.transfers_in),
                    at(&e.transfers_in_month, &key),
                ),
                ("pending", lines(&t.pending), at(&e.pending_month, &key)),
                ("needs review", lines(&t.needs_review), (0, 0)),
                (
                    "net flow",
                    (t.net_flow.minor(), at(&e.net_flow_month, &key).1),
                    at(&e.net_flow_month, &key),
                ),
            ];
            for (line, got, want) in pairs {
                assert!(got == want, "{persona} {key}: {line} differs");
            }
            assert!(b.partial_month == (key.as_str() >= &AS_OF[..7]));
            assert_adds_up(&b);
            spending_total += b.total.minor();
        }
        let whole = breakdown(&ledger, &query(full, GroupBy::Category, Measure::Spending)).unwrap();
        assert!(
            whole.total.minor() == spending_total,
            "{persona}: months add up to the whole"
        );
        assert_adds_up(&whole);

        println!(
            "T3.1-TD1 {persona:<10} transactions={} months={} category_and_group_cells={cells} merchant_years={} income_months={} exact",
            ledger.transactions.len(),
            full.year_months().len(),
            merchants.len(),
            got.len(),
        );
    }
}

// ---- TD.3: the freelancer's question set, recomputed through the views.

struct Td3 {
    answers: HashMap<String, Vec<Value>>,
}

impl Td3 {
    fn load() -> Td3 {
        let q = read("questions/data/questions.json");
        assert_eq!(q["persona"], "freelancer");
        assert_eq!(q["today"], AS_OF);
        let answers = q["questions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| {
                (
                    str_of(q, "id").to_owned(),
                    q["answers"].as_array().unwrap().clone(),
                )
            })
            .collect();
        Td3 { answers }
    }

    fn answer(&self, id: &str, i: usize) -> &Value {
        &self.answers[id][i]
    }

    fn minor(&self, id: &str, i: usize) -> i64 {
        self.answer(id, i)["amount"]["minor"].as_i64().unwrap()
    }

    fn evidence(&self, id: &str, i: usize) -> Vec<TransactionId> {
        let mut ids: Vec<TransactionId> = self.answer(id, i)["evidence"]["transaction_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| TransactionId(v.as_i64().unwrap()))
            .collect();
        ids.sort_unstable();
        ids
    }
}

fn ids_of(rows: &[Row]) -> Vec<TransactionId> {
    let mut ids: Vec<TransactionId> = rows
        .iter()
        .flat_map(|r| r.transaction_ids.clone())
        .collect();
    ids.sort_unstable();
    ids
}

#[test]
fn views_reproduce_td3_answers() {
    let ledger = household("freelancer").ledger;
    let td3 = Td3::load();
    let tax = &ledger.taxonomy;
    let cat = |key: &str| tax.categories.iter().find(|c| c.key == key).unwrap().id;
    let group = |key: &str| tax.groups.iter().find(|g| g.key == key).unwrap().id;
    let accounts = |kind: AccountKind| -> Vec<AccountId> {
        ledger
            .accounts
            .iter()
            .filter(|a| a.kind == kind)
            .map(|a| a.id)
            .collect()
    };
    let ym = |s: &str| s.parse::<YearMonth>().unwrap();
    let months = |a: &str, b: &str| Period::months(ym(a), ym(b)).unwrap();
    let year = |y| Period::year(y).unwrap();
    let q = |period: Period, filter: Filter, group_by: GroupBy, measure: Measure| Query {
        period,
        filter,
        group_by,
        measure,
        top: None,
        as_of: date(AS_OF),
    };
    let spend = |period, filter| {
        breakdown(
            &ledger,
            &q(period, filter, GroupBy::Month, Measure::Spending),
        )
        .unwrap()
    };
    let earn = |period, filter| {
        breakdown(&ledger, &q(period, filter, GroupBy::Month, Measure::Income)).unwrap()
    };
    let categories = |keys: &[&str]| Filter {
        categories: Some(keys.iter().map(|k| cat(k)).collect()),
        ..Filter::default()
    };
    let merchant = |name: &str| Filter {
        merchant: Some(name.into()),
        ..Filter::default()
    };
    let on = |kind| Filter {
        accounts: Some(accounts(kind)),
        ..Filter::default()
    };

    let mut checked = 0;
    // (question, answer index, amount, evidence ids if the view lists them)
    let mut expect = |id: &str, i: usize, got: i64, ids: Option<Vec<TransactionId>>| {
        assert!(got == td3.minor(id, i), "{id} answer {i}: amount differs");
        if let Some(ids) = ids {
            assert!(
                ids == td3.evidence(id, i),
                "{id} answer {i}: evidence differs"
            );
        }
        checked += 1;
    };

    // Totals by period, category, group and merchant.
    for (id, b) in [
        ("q001", spend(year(2025), Filter::default())),
        ("q002", spend(year(2025), categories(&["food.groceries"]))),
        (
            "q003",
            spend(
                months("2026-01", "2026-03"),
                categories(&["food.restaurants"]),
            ),
        ),
        ("q005", spend(year(2025), categories(&["housing.rent"]))),
        (
            "q014",
            spend(
                year(2025),
                Filter {
                    groups: Some(vec![group("food")]),
                    ..Filter::default()
                },
            ),
        ),
        // A user-typed name: lower case, normalised like the rules engine.
        ("q036", spend(year(2025), merchant("h-e-b"))),
        (
            "q066",
            spend(months("2026-09", "2026-09"), Filter::default()),
        ),
        (
            "q067",
            spend(months("2026-01", "2026-09"), Filter::default()),
        ),
        ("q083", spend(year(2025), on(AccountKind::CreditCard))),
    ] {
        assert!(!b.partial_month, "{id}: complete months only");
        expect(id, 0, b.total.minor(), Some(ids_of(&b.rows)));
    }
    for (id, b) in [
        ("q015", earn(year(2025), Filter::default())),
        (
            "q016",
            earn(months("2025-01", "2025-01"), Filter::default()),
        ),
        ("q017", earn(year(2025), merchant("Tailspin Toys"))),
    ] {
        expect(id, 0, b.total.minor(), Some(ids_of(&b.rows)));
    }

    // Refunds, transfers and pending: the other lines.
    let amazon = spend(year(2025), merchant("amazon"));
    let r = &amazon.rows;
    let sum = |f: fn(&Row) -> Money| r.iter().map(|row| f(row).minor()).sum::<i64>();
    expect("q086", 0, sum(|row| row.gross), None);
    expect(
        "q086",
        1,
        sum(|row| row.refunds),
        Some(amazon.totals.refunds.transaction_ids.clone()),
    );
    expect("q086", 2, amazon.total.minor(), Some(ids_of(r)));
    let y2025 = spend(year(2025), Filter::default());
    let refunds = &y2025.totals.refunds;
    expect(
        "q087",
        0,
        refunds.amount.minor(),
        Some(refunds.transaction_ids.clone()),
    );
    let savings = spend(year(2025), on(AccountKind::Savings))
        .totals
        .transfers_in;
    expect(
        "q082",
        0,
        savings.amount.minor(),
        Some(savings.transaction_ids),
    );
    let cards = spend(year(2025), on(AccountKind::CreditCard))
        .totals
        .transfers_in;
    expect("q084", 0, cards.amount.minor(), Some(cards.transaction_ids));
    let all = Period::new(date("2024-10-01"), date("2026-09-30")).unwrap();
    let pending = spend(all, Filter::default()).totals.pending;
    assert!(pending.count == 3, "q081: pending count differs");
    expect(
        "q081",
        1,
        -pending.amount.minor(),
        Some(pending.transaction_ids),
    );

    // Month over month and period comparisons, with the ratio.
    let sep = q(
        months("2026-09", "2026-09"),
        Filter::default(),
        GroupBy::Category,
        Measure::Spending,
    );
    let c = month_over_month(&ledger, &sep).unwrap();
    let flat = |rows: &[Change], current: bool| {
        let mut ids: Vec<TransactionId> = rows
            .iter()
            .flat_map(|r| {
                if current {
                    r.current_ids.clone()
                } else {
                    r.base_ids.clone()
                }
            })
            .collect();
        ids.sort_unstable();
        ids
    };
    expect(
        "q022",
        0,
        c.total.current.minor(),
        Some(flat(&c.rows, true)),
    );
    expect("q022", 1, c.total.base.minor(), Some(flat(&c.rows, false)));
    expect("q022", 2, c.total.change.minor(), None);
    let bp = td3.answer("q022", 3)["basis_points"].as_i64().unwrap();
    assert!(
        c.total.ratio.map(|r| r.basis_points) == Some(bp),
        "q022: ratio differs"
    );
    assert!(!c.current_partial && !c.base_partial);

    let groceries = Query {
        filter: categories(&["food.groceries"]),
        ..sep.clone()
    };
    let c = month_over_month(&ledger, &groceries).unwrap();
    expect("q034", 0, c.total.current.minor(), None);
    expect("q034", 1, c.total.base.minor(), None);
    expect("q034", 2, c.total.change.minor(), None);

    let q2 = q(
        months("2026-04", "2026-06"),
        Filter::default(),
        GroupBy::Group,
        Measure::Spending,
    );
    let c = compare(&ledger, &q2, months("2025-04", "2025-06")).unwrap();
    expect(
        "q033",
        0,
        c.total.current.minor(),
        Some(flat(&c.rows, true)),
    );
    expect("q033", 1, c.total.base.minor(), Some(flat(&c.rows, false)));
    expect("q033", 2, c.total.change.minor(), None);

    // Rankings: the top month, the lowest income month, the top merchants.
    let mut top = q(
        year(2025),
        Filter::default(),
        GroupBy::Month,
        Measure::Spending,
    );
    top.top = Some(1);
    let b = breakdown(&ledger, &top).unwrap();
    assert!(
        b.rows[0].label == str_of(td3.answer("q025", 0), "value"),
        "q025: month differs"
    );
    expect(
        "q025",
        1,
        b.rows[0].total.minor(),
        Some(b.rows[0].transaction_ids.clone()),
    );
    assert_adds_up(&b);

    let b = earn(year(2025), Filter::default());
    let lowest = b
        .rows
        .iter()
        .min_by_key(|r| (r.total.minor(), r.key.clone()))
        .unwrap();
    assert!(
        lowest.label == str_of(td3.answer("q026", 0), "value"),
        "q026: month differs"
    );
    expect(
        "q026",
        1,
        lowest.total.minor(),
        Some(lowest.transaction_ids.clone()),
    );

    let mut top = q(
        year(2025),
        Filter::default(),
        GroupBy::Merchant,
        Measure::Spending,
    );
    top.top = Some(3);
    let b = breakdown(&ledger, &top).unwrap();
    let names: Vec<&str> = b.rows.iter().map(|r| r.label.as_str()).collect();
    let want: Vec<&str> = td3.answer("q037", 0)["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(names == want, "q037: merchants differ");
    for (i, row) in b.rows.iter().enumerate() {
        expect(
            "q037",
            i + 1,
            row.total.minor(),
            Some(row.transaction_ids.clone()),
        );
    }
    assert_adds_up(&b);

    // Trends: an average over 12 complete months, and the 6-month median baseline.
    let t = trend(
        &ledger,
        &q(
            year(2025),
            categories(&["food.groceries"]),
            GroupBy::Category,
            Measure::Spending,
        ),
    )
    .unwrap();
    expect("q029", 0, t.overall.average.unwrap().minor(), None);
    expect("q029", 1, t.overall.complete_total.minor(), None);
    assert!(t.overall.complete_months == 12);
    let t = trend(
        &ledger,
        &q(
            months("2026-04", "2026-09"),
            Filter::default(),
            GroupBy::Category,
            Measure::Spending,
        ),
    )
    .unwrap();
    expect("q035", 0, t.overall.median.unwrap().minor(), None);

    println!("T3.1-TD3 freelancer answers_checked={checked} exact");
    assert_eq!(checked, 36);
}
