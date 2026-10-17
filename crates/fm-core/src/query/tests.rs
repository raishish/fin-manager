//! Unit tests on a small hand-built ledger, one rule each.

use super::*;
use crate::model::*;
use crate::recurring::{Label, LabelKind, Labeler, SeriesSketch};

const USD: Currency = Currency::USD;
const CHECKING: AccountId = AccountId(1);
const CARD: AccountId = AccountId(2);
const SAVINGS: AccountId = AccountId(3);
const GROCERIES: CategoryId = CategoryId(10);
const RESTAURANTS: CategoryId = CategoryId(11);
const RENT: CategoryId = CategoryId(20);
const FEES: CategoryId = CategoryId(30);

fn date(s: &str) -> Date {
    s.parse().unwrap()
}

fn usd(minor: i64) -> Money {
    Money::new(minor, USD)
}

fn tids(v: &[i64]) -> Vec<TransactionId> {
    v.iter().copied().map(TransactionId).collect()
}

struct Build(Ledger);

impl Build {
    fn new() -> Build {
        let account = |id, name: &str, kind| Account {
            id,
            name: name.into(),
            kind,
            currency: USD,
            institution: None,
            masked_number: None,
            external_id: None,
            own_account: true,
            created_at: Timestamp::from_unix_seconds(0),
        };
        let group = |id, key: &str| CategoryGroup {
            id: CategoryGroupId(id),
            key: key.into(),
            name: key.into(),
            hidden: false,
        };
        let category = |id, group, key: &str| Category {
            id,
            group_id: CategoryGroupId(group),
            key: key.into(),
            name: key.into(),
            hidden: false,
            merged_into: None,
        };
        Build(Ledger {
            accounts: vec![
                account(CHECKING, "Checking", AccountKind::Checking),
                account(CARD, "Card", AccountKind::CreditCard),
                account(SAVINGS, "Savings", AccountKind::Savings),
            ],
            imports: vec![],
            transactions: vec![],
            taxonomy: Taxonomy {
                version: TaxonomyVersion {
                    base: "1".into(),
                    revision: 0,
                },
                groups: vec![group(1, "food"), group(2, "housing"), group(3, "financial")],
                categories: vec![
                    category(GROCERIES, 1, "food.groceries"),
                    category(RESTAURANTS, 1, "food.restaurants"),
                    category(RENT, 2, "housing.rent"),
                    category(FEES, 3, "financial.bank_fees"),
                ],
            },
            decisions: vec![],
            rules: vec![],
        })
    }

    fn tx(mut self, id: i64, account: AccountId, day: &str, minor: i64, desc: &str) -> Build {
        self.0.transactions.push(Transaction {
            id: TransactionId(id),
            account_id: account,
            source: SourceRef {
                import_id: ImportId(1),
                index: u32::try_from(id).unwrap(),
                external_id: None,
            },
            status: TransactionStatus::Posted,
            posted_date: date(day),
            transaction_date: None,
            amount: usd(minor),
            original_amount: None,
            description: desc.into(),
            payee: None,
            memo: None,
            trn_type: None,
            check_number: None,
        });
        self
    }

    fn last(&mut self) -> &mut Transaction {
        self.0.transactions.last_mut().unwrap()
    }

    fn pending(mut self) -> Build {
        self.last().status = TransactionStatus::Pending;
        self
    }

    fn eur(mut self, minor: i64) -> Build {
        self.last().original_amount = Some(Money::new(minor, Currency::EUR));
        self
    }

    fn decide(mut self, tx: i64, assignment: Assignment) -> Build {
        let id = DecisionId(i64::try_from(self.0.decisions.len()).unwrap() + 1);
        self.0.decisions.push(Decision {
            id,
            transaction_id: TransactionId(tx),
            assignment,
            source: DecisionSource::User,
            taxonomy_version: self.0.taxonomy.version.clone(),
            decided_at: Timestamp::from_unix_seconds(0),
            state: DecisionState::Applied,
            seen_at: None,
            replaces: None,
        });
        self
    }

    fn cat(self, tx: i64, category_id: CategoryId, merchant: &str) -> Build {
        self.decide(tx, Assignment::Bucket(Bucket::Category { category_id }))
            .decide(tx, Assignment::Merchant(merchant.into()))
    }

    fn refund(self, tx: i64, category_id: CategoryId, merchant: &str) -> Build {
        self.decide(
            tx,
            Assignment::Bucket(Bucket::Refund {
                category_id,
                original: None,
            }),
        )
        .decide(tx, Assignment::Merchant(merchant.into()))
    }

    fn bucket(self, tx: i64, bucket: Bucket) -> Build {
        self.decide(tx, Assignment::Bucket(bucket))
    }
}

/// A month of everything: purchases, a refund, income, a transfer pair, a fee, a
/// purchase in euros, a pending purchase and one in Needs review.
fn ledger() -> Ledger {
    Build::new()
        .tx(1, CARD, "2025-03-03", -5_000, "HEB #1")
        .tx(2, CARD, "2025-03-10", -2_500, "HEB #2")
        .tx(3, CARD, "2025-03-12", 1_000, "HEB REFUND")
        .tx(4, CARD, "2025-03-15", -4_000, "CAFE LISBOA")
        .eur(-3_700)
        .tx(5, CHECKING, "2025-03-01", 300_000, "PAYROLL")
        .tx(6, CHECKING, "2025-03-02", -150_000, "RENT")
        .tx(7, CHECKING, "2025-03-20", -50_000, "ONLINE TRANSFER TO SAV")
        .tx(8, SAVINGS, "2025-03-20", 50_000, "ONLINE TRANSFER FROM CHK")
        .tx(9, CARD, "2025-03-21", -300, "FOREIGN TRANSACTION FEE")
        .tx(10, CARD, "2025-03-22", -250, "LATE FEE")
        .tx(11, CARD, "2025-03-30", -999, "SOMETHING")
        .tx(12, CARD, "2025-03-31", -1_234, "HEB #3")
        .pending()
        .tx(13, CARD, "2025-02-10", -2_000, "HEB #0")
        .cat(1, GROCERIES, "H-E-B")
        .cat(2, GROCERIES, "H-E-B")
        .refund(3, GROCERIES, "H-E-B")
        .cat(4, RESTAURANTS, "Cafe Lisboa")
        .bucket(5, Bucket::Income)
        .cat(6, RENT, "Landlord")
        .bucket(
            7,
            Bucket::Transfer {
                counterpart: Some(TransactionId(8)),
            },
        )
        .bucket(
            8,
            Bucket::Transfer {
                counterpart: Some(TransactionId(7)),
            },
        )
        .cat(9, FEES, "Card bank")
        .cat(10, FEES, "Card bank")
        .cat(12, GROCERIES, "H-E-B")
        .cat(13, GROCERIES, "H-E-B")
        .0
}

const AS_OF: &str = "2025-04-15";

fn ctx() -> Context<'static> {
    Context {
        as_of: date(AS_OF),
        labeler: None,
    }
}

fn march() -> Scope {
    Scope {
        period: PeriodSpec::Month("2025-03".parse().unwrap()),
        ..Scope::default()
    }
}

fn ask(l: &Ledger, ask: Ask) -> Result<Value, QueryError> {
    run_items(l, vec![("x", ask)], &ctx()).map(|mut v| v.remove(0).value)
}

fn run_items(
    l: &Ledger,
    items: Vec<(&str, Ask)>,
    ctx: &Context<'_>,
) -> Result<Vec<Answer>, QueryError> {
    let query = Query {
        items: items
            .into_iter()
            .map(|(name, ask)| Item {
                name: name.into(),
                ask,
            })
            .collect(),
    };
    run(l, &query, ctx)
}

fn money(v: Value) -> (i64, Vec<TransactionId>) {
    match v {
        Value::Money {
            amount,
            transaction_ids,
        } => (amount.minor(), transaction_ids),
        other => panic!("not money: {other:?}"),
    }
}

fn sum(measure: Measure, scope: Scope) -> Ask {
    Ask::Sum { measure, scope }
}

#[test]
fn measures_follow_decision_42() {
    let l = ledger();
    let m = |measure| money(ask(&l, sum(measure, march())).unwrap());
    assert_eq!(
        m(Measure::Spending),
        (161_050, tids(&[1, 2, 3, 4, 6, 9, 10]))
    );
    assert_eq!(m(Measure::Purchases), (162_050, tids(&[1, 2, 4, 6, 9, 10])));
    assert_eq!(m(Measure::Refunds), (1_000, tids(&[3])));
    assert_eq!(m(Measure::Income), (300_000, tids(&[5])));
    assert_eq!(m(Measure::TransfersOut), (50_000, tids(&[7])));
    assert_eq!(m(Measure::TransfersIn), (50_000, tids(&[8])));
    // Every posted amount, whatever its bucket (the Needs-review 11 too).
    assert_eq!(m(Measure::MoneyIn), (351_000, tids(&[3, 5, 8])));
    assert_eq!(
        m(Measure::MoneyOut),
        (213_049, tids(&[1, 2, 4, 6, 7, 9, 10, 11]))
    );
    assert_eq!(m(Measure::Pending), (1_234, tids(&[12])));
}

#[test]
fn scope_filters_narrow() {
    let l = ledger();
    let m = |scope: Scope| money(ask(&l, sum(Measure::Spending, scope)).unwrap());
    let groceries = Scope {
        categories: Some(vec!["food.groceries".into()]),
        ..march()
    };
    assert_eq!(m(groceries).0, 6_500);
    let food = Scope {
        groups: Some(vec!["food".into()]),
        ..march()
    };
    assert_eq!(m(food).0, 10_500);
    let heb = Scope {
        merchant: Some("h e b".into()),
        ..march()
    };
    assert_eq!(m(heb).1, tids(&[1, 2, 3]));
    let checking = Scope {
        accounts: Some(AccountSelect {
            kinds: vec![AccountKind::Checking],
            ids: vec![],
        }),
        ..march()
    };
    assert_eq!(m(checking).1, tids(&[6]));
    // Whole words of the description: "FEE" matches both fees, "TRANSACTION FEE" one.
    let fee = |words: &str| Scope {
        description: Some(words.into()),
        ..march()
    };
    assert_eq!(m(fee("fee")).1, tids(&[9, 10]));
    assert_eq!(m(fee("transaction fee")).1, tids(&[9]));
    assert_eq!(m(fee("FE")).1, tids(&[]));
    // Paid in euros, in euros or in dollars.
    let eur = Scope {
        original_currency: Some("EUR".into()),
        ..march()
    };
    assert_eq!(m(eur.clone()), (4_000, tids(&[4])));
    let v = ask(
        &l,
        sum(
            Measure::Spending,
            Scope {
                amounts: Amounts::Original,
                ..eur
            },
        ),
    )
    .unwrap();
    assert_eq!(
        v,
        Value::Money {
            amount: Money::new(3_700, Currency::EUR),
            transaction_ids: tids(&[4]),
        }
    );
    // Transfers by where the other side went.
    let to_savings = Scope {
        counterpart_accounts: Some(AccountSelect {
            kinds: vec![AccountKind::Savings],
            ids: vec![],
        }),
        ..march()
    };
    assert_eq!(
        money(ask(&l, sum(Measure::TransfersOut, to_savings.clone())).unwrap()).1,
        tids(&[7])
    );
    assert_eq!(
        money(ask(&l, sum(Measure::Spending, to_savings)).unwrap()),
        (0, tids(&[]))
    );
}

#[test]
fn scope_mistakes_are_errors() {
    let l = ledger();
    let bad = |scope: Scope| ask(&l, sum(Measure::Spending, scope)).unwrap_err();
    assert_eq!(
        bad(Scope {
            categories: Some(vec!["food.grocery".into()]),
            ..march()
        }),
        QueryError::UnknownCategory("food.grocery".into())
    );
    assert_eq!(
        bad(Scope {
            groups: Some(vec!["fod".into()]),
            ..march()
        }),
        QueryError::UnknownGroup("fod".into())
    );
    assert_eq!(
        bad(Scope {
            amounts: Amounts::Original,
            ..march()
        }),
        QueryError::OriginalNeedsCurrency
    );
    assert_eq!(
        bad(Scope {
            original_currency: Some("XXX".into()),
            ..march()
        }),
        QueryError::UnknownCurrency
    );
}

fn period(spec: PeriodSpec) -> (Date, Date) {
    let p = super::period::resolve(&spec, date("2026-10-01"), date("2024-10-02")).unwrap();
    (p.from, p.to)
}

#[test]
fn periods_resolve_against_as_of() {
    let d = date;
    assert_eq!(
        period(PeriodSpec::LastMonths(1)),
        (d("2026-09-01"), d("2026-09-30"))
    );
    assert_eq!(
        period(PeriodSpec::LastMonths(12)),
        (d("2025-10-01"), d("2026-09-30"))
    );
    assert_eq!(
        period(PeriodSpec::LastDays(14)),
        (d("2026-09-17"), d("2026-09-30"))
    );
    // 2026-10-01 is a Thursday.
    assert_eq!(
        period(PeriodSpec::LastWeek),
        (d("2026-09-21"), d("2026-09-27"))
    );
    assert_eq!(
        period(PeriodSpec::LastQuarter),
        (d("2026-07-01"), d("2026-09-30"))
    );
    assert_eq!(
        period(PeriodSpec::YearToDate),
        (d("2026-01-01"), d("2026-10-01"))
    );
    assert_eq!(
        period(PeriodSpec::AllTime),
        (d("2024-10-02"), d("2026-10-01"))
    );
    assert_eq!(
        period(PeriodSpec::Quarter {
            year: 2025,
            quarter: 4
        }),
        (d("2025-10-01"), d("2025-12-31"))
    );
    // In the first quarter, last quarter is the previous year's fourth.
    let p =
        super::period::resolve(&PeriodSpec::LastQuarter, d("2026-02-10"), d("2020-01-01")).unwrap();
    assert_eq!((p.from, p.to), (d("2025-10-01"), d("2025-12-31")));
    // On a Monday, last week is the seven days before.
    let p =
        super::period::resolve(&PeriodSpec::LastWeek, d("2026-09-28"), d("2020-01-01")).unwrap();
    assert_eq!((p.from, p.to), (d("2026-09-21"), d("2026-09-27")));
    for bad in [
        PeriodSpec::LastMonths(0),
        PeriodSpec::LastDays(0),
        PeriodSpec::Quarter {
            year: 2025,
            quarter: 5,
        },
        PeriodSpec::Range {
            from: d("2025-02-01"),
            to: d("2025-01-01"),
        },
    ] {
        assert_eq!(
            super::period::resolve(&bad, d("2026-10-01"), d("2020-01-01")),
            Err(QueryError::InvalidPeriod)
        );
    }
}

#[test]
fn averages_and_medians() {
    let l = ledger();
    let groceries = |period| Scope {
        period,
        categories: Some(vec!["food.groceries".into()]),
        ..Scope::default()
    };
    // Per transaction: the refund lowers the total but isn't counted: 6500 / 2.
    let v = ask(
        &l,
        Ask::Average {
            measure: Measure::Spending,
            scope: groceries(march().period),
            per: Per::Transaction,
        },
    )
    .unwrap();
    assert_eq!(money(v).0, 3_250);
    // Per month over Feb–Apr: April is the month of as_of, so not complete. (2000 + 6500) / 2.
    let feb_apr = PeriodSpec::Months {
        from: "2025-02".parse().unwrap(),
        to: "2025-04".parse().unwrap(),
    };
    let per_month = |op: fn(Measure, Scope, Per) -> Ask| {
        money(
            ask(
                &l,
                op(Measure::Spending, groceries(feb_apr.clone()), Per::Month),
            )
            .unwrap(),
        )
        .0
    };
    assert_eq!(
        per_month(|measure, scope, per| Ask::Average {
            measure,
            scope,
            per
        }),
        4_250
    );
    assert_eq!(
        per_month(|measure, scope, per| Ask::Median {
            measure,
            scope,
            per
        }),
        4_250
    );
    // Median per transaction of 5000, 2500, −1000 (the refund): 2500.
    let v = ask(
        &l,
        Ask::Median {
            measure: Measure::Spending,
            scope: groceries(march().period),
            per: Per::Transaction,
        },
    )
    .unwrap();
    assert_eq!(money(v).0, 2_500);
    // No complete month: nothing to average.
    let april = groceries(PeriodSpec::Month("2025-04".parse().unwrap()));
    let none = ask(
        &l,
        Ask::Average {
            measure: Measure::Spending,
            scope: april,
            per: Per::Month,
        },
    );
    assert_eq!(none, Err(QueryError::NothingFound("x".into())));
}

fn rank(
    by: By,
    order: Order,
    metric: Metric,
    output: RankOutput,
    n: usize,
    position: usize,
) -> Ask {
    Ask::Rank {
        measure: Measure::Spending,
        scope: march(),
        by,
        order,
        metric,
        n,
        output,
        position,
    }
}

#[test]
fn rankings_keys_and_distinct() {
    let l = ledger();
    let r = |by, order, metric, output, n, position| {
        ask(&l, rank(by, order, metric, output, n, position)).unwrap()
    };
    use RankOutput::*;
    assert_eq!(
        r(By::Merchant, Order::Highest, Metric::Amount, Labels, 2, 1),
        Value::Texts {
            values: vec!["Landlord".into(), "H-E-B".into()]
        }
    );
    assert_eq!(
        r(By::Merchant, Order::Highest, Metric::Count, Label, 1, 1),
        Value::Text {
            value: "H-E-B".into()
        }
    );
    assert_eq!(
        money(r(By::Category, Order::Lowest, Metric::Amount, Amount, 1, 1)),
        (550, tids(&[9, 10]))
    );
    assert_eq!(
        r(By::Group, Order::Highest, Metric::Amount, Count, 1, 2),
        Value::Count {
            value: 4,
            transaction_ids: tids(&[1, 2, 3, 4])
        }
    );
    // Past the end.
    assert_eq!(
        ask(
            &l,
            rank(By::Merchant, Order::Highest, Metric::Amount, Label, 1, 9)
        ),
        Err(QueryError::NothingFound("x".into()))
    );
    // Transfers have no category.
    let transfers = Ask::Rank {
        measure: Measure::TransfersOut,
        scope: march(),
        by: By::Category,
        order: Order::Highest,
        metric: Metric::Amount,
        n: 1,
        output: Label,
        position: 1,
    };
    assert_eq!(ask(&l, transfers), Err(QueryError::GroupingNotApplicable));

    let all_time = Scope::default();
    let keys = |by| {
        ask(
            &l,
            Ask::Keys {
                measure: Measure::Spending,
                scope: all_time.clone(),
                by,
            },
        )
        .unwrap()
    };
    assert_eq!(
        keys(By::Month),
        Value::Texts {
            values: vec!["2025-02".into(), "2025-03".into()]
        }
    );
    assert_eq!(
        keys(By::Account),
        Value::Texts {
            values: vec!["Checking".into(), "Card".into()]
        }
    );
    let distinct = ask(
        &l,
        Ask::Distinct {
            measure: Measure::Purchases,
            scope: march(),
            by: By::Merchant,
        },
    )
    .unwrap();
    assert_eq!(
        distinct,
        Value::Count {
            value: 4,
            transaction_ids: vec![]
        }
    );
}

#[test]
fn picks_break_ties_by_id() {
    let l = Build::new()
        .tx(1, CARD, "2025-03-05", -500, "A")
        .tx(2, CARD, "2025-03-05", -500, "B")
        .tx(3, CARD, "2025-03-01", -100, "C")
        .cat(1, GROCERIES, "A")
        .cat(2, GROCERIES, "B")
        .cat(3, GROCERIES, "C")
        .0;
    let p = |order, output| {
        ask(
            &l,
            Ask::Pick {
                measure: Measure::Purchases,
                scope: march(),
                order,
                output,
            },
        )
        .unwrap()
    };
    let text = |s: &str| Value::Text { value: s.into() };
    assert_eq!(p(PickOrder::Largest, PickOutput::Merchant), text("A"));
    assert_eq!(p(PickOrder::Latest, PickOutput::Merchant), text("B"));
    assert_eq!(p(PickOrder::Earliest, PickOutput::Merchant), text("C"));
    assert_eq!(
        money(p(PickOrder::Smallest, PickOutput::Amount)),
        (100, tids(&[3]))
    );
    assert_eq!(
        p(PickOrder::Latest, PickOutput::Date),
        Value::Date {
            value: date("2025-03-05")
        }
    );
}

fn derive(formula: DeriveOp, of: &[&str]) -> Ask {
    Ask::Derive {
        formula,
        of: of.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn derived_items() {
    let l = ledger();
    let groceries = Scope {
        categories: Some(vec!["food.groceries".into()]),
        ..march()
    };
    let feb = Scope {
        categories: Some(vec!["food.groceries".into()]),
        period: PeriodSpec::Month("2025-02".parse().unwrap()),
        ..Scope::default()
    };
    // Derived items may refer to items after them.
    let out = run_items(
        &l,
        vec![
            ("change", derive(DeriveOp::PercentChange, &["mar", "feb"])),
            ("diff", derive(DeriveOp::Difference, &["mar", "feb"])),
            ("both", derive(DeriveOp::Sum, &["mar", "feb"])),
            ("share", derive(DeriveOp::Share, &["feb", "mar"])),
            ("larger", derive(DeriveOp::Larger, &["feb", "mar"])),
            ("mar", sum(Measure::Spending, groceries)),
            ("feb", sum(Measure::Spending, feb)),
        ],
        &ctx(),
    )
    .unwrap();
    let v: Vec<&Value> = out.iter().map(|a| &a.value).collect();
    // (6500 − 2000) / 2000 = 225%.
    assert_eq!(
        v[0],
        &Value::Percent {
            basis_points: 22_500
        }
    );
    assert_eq!(money(v[1].clone()), (4_500, vec![]));
    assert_eq!(money(v[2].clone()), (8_500, vec![]));
    // 2000 / 6500 = 30.769% → 3077 bp.
    assert_eq!(
        v[3],
        &Value::Percent {
            basis_points: 3_077
        }
    );
    assert_eq!(
        v[4],
        &Value::Text {
            value: "mar".into()
        }
    );

    let zero = Scope {
        period: PeriodSpec::Month("2025-01".parse().unwrap()),
        ..Scope::default()
    };
    let err = run_items(
        &l,
        vec![
            ("jan", sum(Measure::Spending, zero)),
            ("mar", sum(Measure::Spending, march())),
            ("change", derive(DeriveOp::PercentChange, &["mar", "jan"])),
        ],
        &ctx(),
    );
    assert_eq!(err, Err(QueryError::ZeroBase("change".into())));
    let loop_ = run_items(
        &l,
        vec![
            ("a", derive(DeriveOp::Sum, &["b"])),
            ("b", derive(DeriveOp::Sum, &["a"])),
        ],
        &ctx(),
    );
    assert!(matches!(loop_, Err(QueryError::BadReference(_))));
    let missing = run_items(&l, vec![("a", derive(DeriveOp::Sum, &["nope"]))], &ctx());
    assert_eq!(missing, Err(QueryError::BadReference("nope".into())));
    let mixed = run_items(
        &l,
        vec![
            ("m", sum(Measure::Spending, march())),
            (
                "c",
                Ask::Count {
                    measure: Measure::Spending,
                    scope: march(),
                },
            ),
            ("d", derive(DeriveOp::Difference, &["m", "c"])),
        ],
        &ctx(),
    );
    assert_eq!(mixed, Err(QueryError::WrongKind("d".into())));
}

#[test]
fn items_need_distinct_names() {
    let l = ledger();
    assert_eq!(run_items(&l, vec![], &ctx()), Err(QueryError::BadItems));
    let twice = vec![
        ("a", sum(Measure::Spending, march())),
        ("a", sum(Measure::Income, march())),
    ];
    assert_eq!(run_items(&l, twice, &ctx()), Err(QueryError::BadItems));
}

/// Four monthly charges at one price, three at a higher one, ending in June.
fn subscription_ledger() -> Ledger {
    let mut b = Build::new();
    let days = [
        "2025-01-05",
        "2025-02-05",
        "2025-03-05",
        "2025-04-05",
        "2025-05-05",
        "2025-06-05",
        "2025-07-05",
    ];
    for (i, day) in days.iter().enumerate() {
        let id = i64::try_from(i).unwrap() + 1;
        let amount = if i < 4 { -999 } else { -1_099 };
        b = b
            .tx(id, CARD, day, amount, "STREAMCO 555 CA")
            .cat(id, RESTAURANTS, "StreamCo");
    }
    b.0
}

struct Names(Option<LabelKind>);

impl Labeler for Names {
    fn label(&self, _: &SeriesSketch<'_>) -> Option<Label> {
        self.0.map(|kind| Label {
            name: "StreamCo".into(),
            kind,
        })
    }
}

fn recurring(select: SeriesSelect, output: SeriesOutput) -> Ask {
    Ask::Recurring { select, output }
}

#[test]
fn recurring_series_with_and_without_a_labeler() {
    let l = subscription_ledger();
    let as_of = date("2025-12-01");
    let count_of = |kinds: Vec<SeriesKindSel>, labeler: Option<&dyn Labeler>| {
        let ctx = Context { as_of, labeler };
        let select = SeriesSelect {
            kinds: Some(kinds),
            ..SeriesSelect::default()
        };
        match run_items(
            &l,
            vec![("n", recurring(select, SeriesOutput::Count))],
            &ctx,
        )
        .unwrap()
        .remove(0)
        .value
        {
            Value::Count { value, .. } => value,
            other => panic!("not a count: {other:?}"),
        }
    };
    use SeriesKindSel::{Bill, Subscription};
    // Fixed logic alone can't tell a subscription from a bill: a charge is both.
    assert_eq!(count_of(vec![Subscription], None), 1);
    assert_eq!(count_of(vec![Bill], None), 1);
    // A labeler decides.
    let sub = Names(Some(LabelKind::Subscription));
    assert_eq!(count_of(vec![Subscription], Some(&sub)), 1);
    assert_eq!(count_of(vec![Bill], Some(&sub)), 0);
    // A charge the labeler didn't name is neither.
    let unsure = Names(None);
    assert_eq!(count_of(vec![Subscription, Bill], Some(&unsure)), 0);

    let ctx = Context {
        as_of,
        labeler: Some(&sub),
    };
    let named = || SeriesSelect {
        name: Some("streamco".into()),
        ..SeriesSelect::default()
    };
    let out = run_items(
        &l,
        vec![
            ("latest", recurring(named(), SeriesOutput::Latest)),
            ("from", recurring(named(), SeriesOutput::PriceFrom)),
            ("before", recurring(named(), SeriesOutput::PriceBefore)),
            ("after", recurring(named(), SeriesOutput::PriceAfter)),
            (
                "cancelled",
                recurring(
                    SeriesSelect {
                        status: SeriesStatus::Cancelled,
                        ..SeriesSelect::default()
                    },
                    SeriesOutput::Names,
                ),
            ),
            (
                "h1",
                recurring(
                    SeriesSelect {
                        period: Some(PeriodSpec::Months {
                            from: "2025-01".parse().unwrap(),
                            to: "2025-06".parse().unwrap(),
                        }),
                        ..named()
                    },
                    SeriesOutput::MemberTotal,
                ),
            ),
            (
                "rent",
                recurring(
                    SeriesSelect {
                        categories: Some(vec!["housing.rent".into()]),
                        ..SeriesSelect::default()
                    },
                    SeriesOutput::Count,
                ),
            ),
        ],
        &ctx,
    )
    .unwrap();
    let v: Vec<Value> = out.into_iter().map(|a| a.value).collect();
    assert_eq!(money(v[0].clone()), (1_099, tids(&[7])));
    assert_eq!(
        v[1],
        Value::Date {
            value: date("2025-05-05")
        }
    );
    assert_eq!(money(v[2].clone()).0, 999);
    assert_eq!(money(v[3].clone()).0, 1_099);
    // Ended in July: no charge for 1.5 months before as_of.
    assert_eq!(
        v[4],
        Value::Texts {
            values: vec!["StreamCo".into()]
        }
    );
    assert_eq!(
        money(v[5].clone()),
        (4 * 999 + 2 * 1_099, tids(&[1, 2, 3, 4, 5, 6]))
    );
    // Its members are restaurants, not rent.
    assert_eq!(
        v[6],
        Value::Count {
            value: 0,
            transaction_ids: vec![]
        }
    );
    let missing = run_items(
        &l,
        vec![(
            "x",
            recurring(
                SeriesSelect {
                    name: Some("nobody".into()),
                    ..SeriesSelect::default()
                },
                SeriesOutput::Latest,
            ),
        )],
        &ctx,
    );
    assert_eq!(missing, Err(QueryError::NothingFound("x".into())));
}

#[test]
fn errors_carry_no_financial_data() {
    // Display texts name items, keys and codes only.
    let all = [
        QueryError::BadItems,
        QueryError::UnknownCategory("food.x".into()),
        QueryError::UnknownGroup("x".into()),
        QueryError::UnknownCurrency,
        QueryError::BadReference("total".into()),
        QueryError::WrongKind("total".into()),
        QueryError::ZeroBase("change".into()),
        QueryError::OriginalNeedsCurrency,
        QueryError::GroupingNotApplicable,
        QueryError::NothingFound("total".into()),
        QueryError::InvalidPeriod,
        QueryError::Overflow,
        QueryError::View(crate::views::ViewError::MixedCurrency),
    ];
    for e in all {
        let text = e.to_string();
        assert!(!text.chars().any(|c| c.is_ascii_digit()), "{text}");
    }
}

#[test]
fn answers_serialise_with_a_kind_tag() {
    let l = ledger();
    let out = run_items(&l, vec![("total", sum(Measure::Refunds, march()))], &ctx()).unwrap();
    assert_eq!(
        serde_json::to_value(&out).unwrap(),
        serde_json::json!([{"name": "total", "kind": "money",
            "amount": {"minor": 1000, "currency": "USD"}, "transaction_ids": [3]}])
    );
}
