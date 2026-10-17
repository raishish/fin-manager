//! Unit tests on a small hand-built ledger, one rule each.

use super::*;
use crate::model::*;

const USD: Currency = Currency::USD;
const CHECKING: AccountId = AccountId(1);
const CARD: AccountId = AccountId(2);
const EURO: AccountId = AccountId(3);

const FOOD: CategoryGroupId = CategoryGroupId(1);
const HOME: CategoryGroupId = CategoryGroupId(2);
const GROCERIES: CategoryId = CategoryId(10);
const RESTAURANTS: CategoryId = CategoryId(11);
/// Merged into RESTAURANTS.
const DINING: CategoryId = CategoryId(12);
const RENT: CategoryId = CategoryId(20);

fn date(s: &str) -> Date {
    s.parse().unwrap()
}

fn ym(s: &str) -> YearMonth {
    s.parse().unwrap()
}

fn usd(minor: i64) -> Money {
    Money::new(minor, USD)
}

fn ids(v: &[i64]) -> Vec<TransactionId> {
    v.iter().copied().map(TransactionId).collect()
}

fn ledger() -> Ledger {
    let account = |id, kind, currency| Account {
        id,
        name: format!("account {}", id.0),
        kind,
        currency,
        institution: None,
        masked_number: None,
        external_id: None,
        own_account: true,
        created_at: Timestamp::from_unix_seconds(0),
    };
    let group = |id, key: &str| CategoryGroup {
        id,
        key: key.into(),
        name: key.into(),
        hidden: false,
    };
    let category = |id, group_id, key: &str, merged_into| Category {
        id,
        group_id,
        key: key.into(),
        name: key.into(),
        hidden: false,
        merged_into,
    };
    Ledger {
        accounts: vec![
            account(CHECKING, AccountKind::Checking, USD),
            account(CARD, AccountKind::CreditCard, USD),
        ],
        imports: vec![],
        transactions: vec![],
        taxonomy: Taxonomy {
            version: TaxonomyVersion {
                base: "1".into(),
                revision: 1,
            },
            groups: vec![group(FOOD, "food"), group(HOME, "home")],
            categories: vec![
                category(GROCERIES, FOOD, "food.groceries", None),
                category(RESTAURANTS, FOOD, "food.restaurants", None),
                category(DINING, FOOD, "food.dining", Some(RESTAURANTS)),
                category(RENT, HOME, "home.rent", None),
            ],
        },
        decisions: vec![],
        rules: vec![],
    }
}

struct Build(Ledger);

impl Build {
    fn new() -> Build {
        Build(ledger())
    }

    /// A posted transaction; `purchased` is the bank's purchase date, if any.
    fn tx(
        mut self,
        id: i64,
        account: AccountId,
        posted: &str,
        purchased: Option<&str>,
        minor: i64,
    ) -> Build {
        let currency = self
            .0
            .accounts
            .iter()
            .find(|a| a.id == account)
            .map_or(USD, |a| a.currency);
        self.0.transactions.push(Transaction {
            id: TransactionId(id),
            account_id: account,
            source: SourceRef {
                import_id: ImportId(1),
                index: u32::try_from(id).unwrap(),
                external_id: None,
            },
            status: TransactionStatus::Posted,
            posted_date: date(posted),
            transaction_date: purchased.map(date),
            amount: Money::new(minor, currency),
            original_amount: None,
            description: format!("DESC {id}"),
            payee: None,
            memo: None,
            trn_type: None,
            check_number: None,
        });
        self
    }

    fn pending(mut self, id: i64) -> Build {
        let t = self
            .0
            .transactions
            .iter_mut()
            .find(|t| t.id.0 == id)
            .unwrap();
        t.status = TransactionStatus::Pending;
        self
    }

    fn decide(mut self, tx: i64, assignment: Assignment, state: DecisionState) -> Build {
        let id = DecisionId(i64::try_from(self.0.decisions.len()).unwrap() + 1);
        self.0.decisions.push(Decision {
            id,
            transaction_id: TransactionId(tx),
            assignment,
            source: DecisionSource::User,
            taxonomy_version: self.0.taxonomy.version.clone(),
            decided_at: Timestamp::from_unix_seconds(0),
            state,
            seen_at: None,
            replaces: None,
        });
        self
    }

    fn bucket(self, tx: i64, bucket: Bucket) -> Build {
        self.decide(tx, Assignment::Bucket(bucket), DecisionState::Applied)
    }

    fn cat(self, tx: i64, category_id: CategoryId) -> Build {
        self.bucket(tx, Bucket::Category { category_id })
    }

    fn refund(self, tx: i64, category_id: CategoryId) -> Build {
        self.bucket(
            tx,
            Bucket::Refund {
                category_id,
                original: None,
            },
        )
    }

    fn merchant(self, tx: i64, name: &str) -> Build {
        self.decide(
            tx,
            Assignment::Merchant(name.into()),
            DecisionState::Applied,
        )
    }
}

fn query(from: &str, to: &str, group_by: GroupBy) -> Query {
    Query {
        period: Period::new(date(from), date(to)).unwrap(),
        filter: Filter::default(),
        group_by,
        measure: Measure::Spending,
        top: None,
        as_of: date("2026-01-01"),
    }
}

fn row<'a>(b: &'a Breakdown, key: &Key) -> &'a Row {
    b.rows.iter().find(|r| &r.key == key).unwrap()
}

fn month(s: &str) -> Key {
    Key::Month { month: ym(s) }
}

fn category(id: CategoryId) -> Key {
    Key::Category { id }
}

/// The identities every breakdown keeps. Also used on TD.1 data, so failures name the
/// identity, never an amount.
pub(super) fn assert_adds_up(b: &Breakdown) {
    let t = &b.totals;
    let m = |l: &Line| l.amount.minor();
    assert!(
        t.net_flow.minor()
            == m(&t.income) + m(&t.transfers_in) - m(&t.transfers_out) - m(&t.spending)
                + m(&t.needs_review),
        "net flow identity"
    );
    let rows: i64 = b.rows.iter().chain(&b.other).map(|r| r.total.minor()).sum();
    assert!(rows == b.total.minor(), "rows add up to the total");
    let measure = match b.measure {
        Measure::Spending => &t.spending,
        Measure::Income => &t.income,
    };
    assert!(b.total == measure.amount, "total equals its line");
    for r in b.rows.iter().chain(&b.other) {
        assert!(
            r.total.minor() == r.gross.minor() - r.refunds.minor(),
            "row total is gross less refunds"
        );
        assert!(r.count == r.transaction_ids.len(), "row count");
    }
}

#[test]
fn purchase_date_decides_the_month() {
    let l = Build::new()
        .tx(1, CARD, "2025-02-01", Some("2025-01-31"), -1000)
        .tx(2, CARD, "2025-02-01", None, -500)
        .cat(1, GROCERIES)
        .cat(2, GROCERIES)
        .0;
    let b = breakdown(&l, &query("2025-01-01", "2025-02-28", GroupBy::Month)).unwrap();
    assert_eq!(row(&b, &month("2025-01")).total, usd(1000));
    assert_eq!(row(&b, &month("2025-02")).total, usd(500));
    assert_eq!(row(&b, &month("2025-01")).transaction_ids, ids(&[1]));
    assert_adds_up(&b);
}

#[test]
fn pending_is_its_own_line_and_never_in_a_total() {
    let l = Build::new()
        .tx(1, CARD, "2025-01-10", None, -1000)
        .tx(2, CARD, "2025-01-11", None, -4427)
        .pending(2)
        .cat(1, RESTAURANTS)
        .cat(2, RESTAURANTS)
        .0;
    let b = breakdown(&l, &query("2025-01-01", "2025-01-31", GroupBy::Category)).unwrap();
    assert_eq!(b.total, usd(1000));
    assert_eq!(b.totals.pending.amount, usd(-4427));
    assert_eq!(b.totals.pending.transaction_ids, ids(&[2]));
    assert_eq!(b.totals.net_flow, usd(-1000));
    assert_eq!(row(&b, &category(RESTAURANTS)).transaction_ids, ids(&[1]));
    assert_adds_up(&b);
}

#[test]
fn refunds_net_against_their_category_and_can_go_negative() {
    let l = Build::new()
        .tx(1, CARD, "2025-01-10", None, -10_000)
        .tx(2, CARD, "2025-02-03", None, 3_000)
        .tx(3, CARD, "2025-02-04", None, -500)
        .cat(1, GROCERIES)
        .refund(2, GROCERIES)
        .cat(3, RESTAURANTS)
        .0;
    let jan = breakdown(&l, &query("2025-01-01", "2025-01-31", GroupBy::Category)).unwrap();
    assert_eq!(row(&jan, &category(GROCERIES)).total, usd(10_000));

    let feb = breakdown(&l, &query("2025-02-01", "2025-02-28", GroupBy::Category)).unwrap();
    let groceries = row(&feb, &category(GROCERIES));
    assert_eq!(groceries.total, usd(-3_000));
    assert_eq!(groceries.gross, usd(0));
    assert_eq!(groceries.refunds, usd(3_000));
    assert_eq!(feb.total, usd(-2_500));
    assert_eq!(feb.totals.refunds.amount, usd(3_000));
    // Largest first: restaurants (5.00) before groceries (−30.00).
    assert_eq!(feb.rows[0].key, category(RESTAURANTS));
    assert_adds_up(&feb);

    let both = breakdown(&l, &query("2025-01-01", "2025-02-28", GroupBy::Category)).unwrap();
    let g = row(&both, &category(GROCERIES));
    assert_eq!(
        (g.total, g.gross, g.refunds, g.count),
        (usd(7_000), usd(10_000), usd(3_000), 2)
    );
}

#[test]
fn transfers_and_income_are_never_spending() {
    let l = Build::new()
        .tx(1, CHECKING, "2025-03-01", None, 500_000)
        .tx(2, CHECKING, "2025-03-05", None, -80_000)
        .tx(3, CARD, "2025-03-05", None, 80_000)
        .tx(4, CHECKING, "2025-03-06", None, -150_000)
        .tx(5, CARD, "2025-03-07", None, -2_000)
        .bucket(1, Bucket::Income)
        .bucket(
            2,
            Bucket::Transfer {
                counterpart: Some(TransactionId(3)),
            },
        )
        .bucket(
            3,
            Bucket::Transfer {
                counterpart: Some(TransactionId(2)),
            },
        )
        .cat(4, RENT)
        .bucket(5, Bucket::NeedsReview)
        .0;
    let b = breakdown(&l, &query("2025-03-01", "2025-03-31", GroupBy::Account)).unwrap();
    assert_eq!(b.total, usd(150_000));
    assert_eq!(b.rows.len(), 1);
    assert_eq!(b.rows[0].key, Key::Account { id: CHECKING });
    let t = &b.totals;
    assert_eq!(t.income.amount, usd(500_000));
    assert_eq!(t.transfers_out.amount, usd(80_000));
    assert_eq!(t.transfers_in.amount, usd(80_000));
    assert_eq!(t.needs_review.amount, usd(-2_000));
    assert_eq!(t.needs_review.transaction_ids, ids(&[5]));
    assert_eq!(t.net_flow, usd(348_000));
    assert_adds_up(&b);

    let mut q = query("2025-03-01", "2025-03-31", GroupBy::Month);
    q.measure = Measure::Income;
    let income = breakdown(&l, &q).unwrap();
    assert_eq!(income.total, usd(500_000));
    assert_eq!(income.rows[0].transaction_ids, ids(&[1]));
    assert_adds_up(&income);
}

#[test]
fn no_bucket_or_two_buckets_count_as_needs_review() {
    let rent = Assignment::Bucket(Bucket::Category { category_id: RENT });
    let l = Build::new()
        .tx(1, CARD, "2025-01-10", None, -100)
        .tx(2, CARD, "2025-01-11", None, -200)
        .tx(3, CARD, "2025-01-12", None, -400)
        .cat(2, GROCERIES)
        .cat(2, RENT)
        // Only applied decisions count.
        .decide(3, rent.clone(), DecisionState::Superseded)
        .decide(3, rent, DecisionState::Suggested)
        .cat(3, GROCERIES)
        .0;
    let b = breakdown(&l, &query("2025-01-01", "2025-01-31", GroupBy::Category)).unwrap();
    assert_eq!(b.totals.needs_review.transaction_ids, ids(&[1, 2]));
    assert_eq!(b.totals.needs_review.amount, usd(-300));
    assert_eq!(b.rows.len(), 1);
    assert_eq!(row(&b, &category(GROCERIES)).transaction_ids, ids(&[3]));
    assert_adds_up(&b);
}

#[test]
fn a_merged_category_counts_toward_its_target() {
    let l = Build::new()
        .tx(1, CARD, "2025-01-10", None, -100)
        .tx(2, CARD, "2025-01-11", None, -200)
        .cat(1, DINING)
        .cat(2, RESTAURANTS)
        .0;
    let b = breakdown(&l, &query("2025-01-01", "2025-01-31", GroupBy::Category)).unwrap();
    assert_eq!(b.rows.len(), 1);
    let r = row(&b, &category(RESTAURANTS));
    assert_eq!((r.total, r.label.as_str()), (usd(300), "food.restaurants"));

    // Filtering by the merged-away id selects the target.
    let mut q = query("2025-01-01", "2025-01-31", GroupBy::Group);
    q.filter.categories = Some(vec![DINING]);
    let b = breakdown(&l, &q).unwrap();
    assert_eq!(b.rows[0].key, Key::Group { id: FOOD });
    assert_eq!(b.total, usd(300));
}

#[test]
fn category_and_group_filters_keep_only_spending() {
    let l = Build::new()
        .tx(1, CHECKING, "2025-01-01", None, 100_000)
        .tx(2, CHECKING, "2025-01-02", None, -50_000)
        .tx(3, CARD, "2025-01-03", None, -1_000)
        .bucket(1, Bucket::Income)
        .cat(2, RENT)
        .cat(3, GROCERIES)
        .0;
    let mut q = query("2025-01-01", "2025-01-31", GroupBy::Category);
    q.filter.groups = Some(vec![FOOD]);
    let b = breakdown(&l, &q).unwrap();
    assert_eq!(b.total, usd(1_000));
    assert_eq!(b.totals.income.count, 0);
    assert_eq!(b.totals.net_flow, usd(-1_000));
    assert_adds_up(&b);

    q.filter.groups = Some(vec![]);
    assert_eq!(breakdown(&l, &q).unwrap().totals.spending.count, 0);
}

#[test]
fn merchant_is_the_applied_name_else_the_description_and_filters_ignore_case() {
    let l = Build::new()
        .tx(1, CARD, "2025-01-10", None, -100)
        .tx(2, CARD, "2025-01-11", None, -200)
        .tx(3, CARD, "2025-01-12", None, -400)
        .cat(1, GROCERIES)
        .cat(2, GROCERIES)
        .cat(3, GROCERIES)
        .merchant(1, "H-E-B")
        .merchant(2, "Corner Shop")
        // The latest applied merchant decision wins.
        .merchant(2, "H-E-B")
        .0;
    let b = breakdown(&l, &query("2025-01-01", "2025-01-31", GroupBy::Merchant)).unwrap();
    let heb = Key::Merchant {
        name: "H-E-B".into(),
        from_description: false,
    };
    assert_eq!(row(&b, &heb).transaction_ids, ids(&[1, 2]));
    let raw = Key::Merchant {
        name: "DESC 3".into(),
        from_description: true,
    };
    assert_eq!(row(&b, &raw).total, usd(400));

    let mut q = query("2025-01-01", "2025-01-31", GroupBy::Merchant);
    // Normalised like the rules engine: same words, any case and punctuation.
    for (typed, expected) in [
        ("h-e-b", 300),
        ("H E B", 300),
        ("heb", 0),
        ("desc 3", 400),
        ("--", 0),
    ] {
        q.filter.merchant = Some(typed.into());
        assert_eq!(breakdown(&l, &q).unwrap().total, usd(expected), "{typed:?}");
    }
}

#[test]
fn one_currency_per_view() {
    let mut l = Build::new()
        .tx(1, CARD, "2025-01-10", None, -100)
        .cat(1, GROCERIES)
        .0;
    l.accounts.push(Account {
        id: EURO,
        currency: Currency::EUR,
        ..l.accounts[0].clone()
    });
    let l = Build(l)
        .tx(2, EURO, "2025-01-10", None, -700)
        .cat(2, GROCERIES)
        .0;

    let mut q = query("2025-01-01", "2025-01-31", GroupBy::Category);
    assert_eq!(breakdown(&l, &q), Err(ViewError::MixedCurrency));
    q.filter.currency = Some(Currency::EUR);
    let b = breakdown(&l, &q).unwrap();
    assert_eq!(
        (b.currency, b.total),
        (Currency::EUR, Money::new(700, Currency::EUR))
    );
    q.filter.currency = None;
    q.filter.accounts = Some(vec![CARD]);
    assert_eq!(breakdown(&l, &q).unwrap().total, usd(100));

    // An amount in the wrong currency is an error, not a silent sum.
    let mut bad = l.clone();
    bad.transactions[1].amount = usd(-700);
    q.filter.accounts = None;
    q.filter.currency = Some(Currency::EUR);
    assert_eq!(
        breakdown(&bad, &q),
        Err(ViewError::AmountCurrency(TransactionId(2)))
    );
    // A currency no account holds is an empty view, not an error.
    assert_eq!(breakdown(&ledger(), &q).unwrap().rows, vec![]);
    let mut empty = ledger();
    empty.accounts.clear();
    q.filter.currency = None;
    assert_eq!(breakdown(&empty, &q), Err(ViewError::NoAccounts));
}

#[test]
fn top_n_keeps_the_largest_and_the_rest_still_adds_up() {
    let mut b = Build::new();
    for (id, amount) in [(1, 500), (2, 100), (3, 300), (4, 200), (5, 400)] {
        b = b
            .tx(id, CARD, "2025-01-10", None, -amount)
            .cat(id, GROCERIES)
            .merchant(id, &format!("m{id}"));
    }
    let l = b.0;
    let mut q = query("2025-01-01", "2025-01-31", GroupBy::Merchant);
    q.top = Some(2);
    let b = breakdown(&l, &q).unwrap();
    let names: Vec<&str> = b.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(names, ["m1", "m5"]);
    let other = b.other.as_ref().unwrap();
    assert_eq!(
        (other.key.clone(), other.total, other.count),
        (Key::Other, usd(600), 3)
    );
    assert_eq!(other.transaction_ids, ids(&[2, 3, 4]));
    assert_eq!(b.total, usd(1500));
    assert_adds_up(&b);

    q.top = Some(10);
    assert!(breakdown(&l, &q).unwrap().other.is_none());
    q.top = Some(0);
    let b = breakdown(&l, &q).unwrap();
    assert!(b.rows.is_empty());
    assert_eq!(b.other.as_ref().unwrap().total, usd(1500));
    assert_adds_up(&b);

    // By month, the kept rows stay in calendar order.
    let l = Build::new()
        .tx(1, CARD, "2025-01-10", None, -100)
        .tx(2, CARD, "2025-02-10", None, -300)
        .tx(3, CARD, "2025-03-10", None, -200)
        .cat(1, GROCERIES)
        .cat(2, GROCERIES)
        .cat(3, GROCERIES)
        .0;
    let mut q = query("2025-01-01", "2025-03-31", GroupBy::Month);
    q.top = Some(2);
    let b = breakdown(&l, &q).unwrap();
    let keys: Vec<Key> = b.rows.iter().map(|r| r.key.clone()).collect();
    assert_eq!(keys, [month("2025-02"), month("2025-03")]);

    // The same cut in a series and a comparison.
    let s = monthly(&l, &q).unwrap();
    let other = s.other.unwrap();
    assert_eq!(other.values, [usd(100), usd(0), usd(0)]);
    let totals: Vec<Money> = s.columns.iter().map(|c| c.total).collect();
    assert_eq!(totals, [usd(100), usd(300), usd(200)]);
}

#[test]
fn month_over_month_flags_a_partial_month_and_gives_exact_ratios() {
    let l = Build::new()
        .tx(1, CARD, "2025-11-10", None, -30_000)
        .tx(2, CARD, "2025-12-10", None, -40_000)
        .tx(3, CARD, "2025-12-11", None, -999)
        .tx(4, CARD, "2025-11-12", None, -1)
        .cat(1, GROCERIES)
        .cat(2, GROCERIES)
        .cat(3, RENT)
        .cat(4, RESTAURANTS)
        .0;
    let mut q = query("2025-12-01", "2025-12-31", GroupBy::Category);
    q.as_of = date("2025-12-15");
    let c = month_over_month(&l, &q).unwrap();
    assert_eq!(c.base, Period::month(ym("2025-11")));
    assert!(c.current_partial);
    assert!(!c.base_partial);
    assert_eq!(c.total.current, usd(40_999));
    assert_eq!(c.total.base, usd(30_001));
    assert_eq!(c.total.change, usd(10_998));
    // 10998 / 30001 = 0.366587... → 3666 bp.
    assert_eq!(
        c.total.ratio,
        Some(Ratio {
            numerator: 10_998,
            denominator: 30_001,
            basis_points: 3_666
        })
    );
    // Largest change first; keys from either month appear.
    let keys: Vec<Key> = c.rows.iter().map(|r| r.key.clone()).collect();
    assert_eq!(
        keys,
        [category(GROCERIES), category(RENT), category(RESTAURANTS)]
    );
    assert_eq!(c.rows[1].delta.ratio, None);
    assert_eq!(c.rows[2].delta.change, usd(-1));
    assert_eq!(c.rows[2].base_ids, ids(&[4]));

    q.as_of = date("2026-01-01");
    assert!(!month_over_month(&l, &q).unwrap().current_partial);

    q.period = Period::new(date("2025-12-01"), date("2025-12-30")).unwrap();
    assert_eq!(month_over_month(&l, &q), Err(ViewError::NotOneMonth));
}

#[test]
fn ratios_round_half_away_from_zero() {
    let l = Build::new()
        .tx(1, CARD, "2025-01-10", None, -8)
        .tx(2, CARD, "2025-02-10", None, -9)
        .tx(3, CARD, "2025-03-10", None, -7)
        .cat(1, GROCERIES)
        .cat(2, GROCERIES)
        .cat(3, GROCERIES)
        .0;
    let bp = |c: Comparison| c.total.ratio.unwrap().basis_points;
    // +1/8 = 1250 bp exactly.
    let q = query("2025-02-01", "2025-02-28", GroupBy::Category);
    assert_eq!(bp(month_over_month(&l, &q).unwrap()), 1250);
    // −2/9 = −2222.2 bp → −2222.
    let q = query("2025-03-01", "2025-03-31", GroupBy::Category);
    assert_eq!(bp(month_over_month(&l, &q).unwrap()), -2222);
    // +1/7 = 1428.57 bp → 1429.
    let q = query("2025-01-01", "2025-01-31", GroupBy::Category);
    assert_eq!(
        bp(compare(&l, &q, Period::month(ym("2025-03"))).unwrap()),
        1429
    );
}

#[test]
fn monthly_fills_every_month_and_flags_partial_ones() {
    let l = Build::new()
        .tx(1, CARD, "2025-01-10", None, -100)
        .tx(2, CARD, "2025-03-10", None, -300)
        .tx(3, CARD, "2025-03-11", None, -50)
        .cat(1, GROCERIES)
        .cat(2, GROCERIES)
        .cat(3, RENT)
        .0;
    let mut q = query("2025-01-15", "2025-03-31", GroupBy::Category);
    q.as_of = date("2025-03-20");
    let s = monthly(&l, &q).unwrap();
    let months: Vec<(YearMonth, bool, Money)> = s
        .columns
        .iter()
        .map(|c| (c.month, c.partial, c.total))
        .collect();
    assert_eq!(
        months,
        [
            // Starts mid-month (and transaction 1 is before the period).
            (ym("2025-01"), true, usd(0)),
            (ym("2025-02"), false, usd(0)),
            // The month of as_of.
            (ym("2025-03"), true, usd(350)),
        ]
    );
    assert_eq!(s.rows[0].key, category(GROCERIES));
    assert_eq!(s.rows[0].values, [usd(0), usd(0), usd(300)]);
    assert_eq!(s.total, usd(350));
}

#[test]
fn trend_averages_complete_months_only() {
    let mut b = Build::new();
    // January to April are complete; May is the month of as_of.
    for (id, posted, amount) in [
        (1, "2025-01-10", 100),
        (2, "2025-02-10", 201),
        (3, "2025-03-10", 400),
        (4, "2025-04-10", 50),
        (5, "2025-05-02", 9_999),
    ] {
        b = b.tx(id, CARD, posted, None, -amount).cat(id, GROCERIES);
    }
    let mut q = query("2025-01-01", "2025-05-31", GroupBy::Category);
    q.as_of = date("2025-05-15");
    let t = trend(&b.0, &q).unwrap();
    let s = &t.rows[0].stats;
    assert_eq!(s.complete_months, 4);
    assert_eq!(s.complete_total, usd(751));
    // 751 / 4 = 187.75 → 188.
    assert_eq!(s.average, Some(usd(188)));
    // Middle two of 50, 100, 201, 400: (100 + 201) / 2 = 150.5 → 151.
    assert_eq!(s.median, Some(usd(151)));
    assert_eq!(s.latest, Some(usd(50)));
    assert_eq!(s.latest_vs_average, Some(usd(-138)));
    assert_eq!(t.overall, *s);

    // Nothing complete yet.
    q.period = Period::month(ym("2025-05"));
    let t = trend(&b.0, &q).unwrap();
    assert_eq!(t.overall.average, None);
    assert_eq!(t.overall.median, None);
}

#[test]
fn bad_queries_are_errors() {
    let l = ledger();
    let mut q = query("2025-01-01", "2025-01-31", GroupBy::Category);
    q.measure = Measure::Income;
    assert_eq!(breakdown(&l, &q), Err(ViewError::GroupingNotApplicable));
    assert_eq!(
        Period::new(date("2025-02-01"), date("2025-01-31")),
        Err(ViewError::InvalidPeriod)
    );
    let mut q = query("2025-01-01", "2025-01-31", GroupBy::Category);
    q.period = Period {
        from: date("2025-02-01"),
        to: date("2025-01-31"),
    };
    assert_eq!(monthly(&l, &q), Err(ViewError::InvalidPeriod));
}

#[test]
fn queries_read_from_json() {
    let q: Query = serde_json::from_str(
        r#"{"period": {"from": "2025-01-01", "to": "2025-12-31"},
            "filter": {"merchant": "h-e-b", "categories": [10]},
            "group_by": "merchant", "top": 3, "as_of": "2026-10-01"}"#,
    )
    .unwrap();
    assert_eq!(q.period, Period::year(2025).unwrap());
    assert_eq!(q.measure, Measure::Spending);
    assert_eq!(q.filter.categories, Some(vec![GROCERIES]));
    let json = serde_json::to_value(&q).unwrap();
    assert_eq!(serde_json::from_value::<Query>(json).unwrap(), q);
    // Unknown fields are rejected, so a typo can't silently widen a query.
    let typo = r#"{"period": {"from": "2025-01-01", "to": "2025-12-31"},
        "group_by": "merchant", "as_of": "2026-10-01", "filters": {}}"#;
    assert!(serde_json::from_str::<Query>(typo).is_err());
}

#[test]
fn results_serialise_with_tagged_keys() {
    let l = Build::new()
        .tx(1, CARD, "2025-01-10", None, -100)
        .cat(1, GROCERIES)
        .0;
    let b = breakdown(&l, &query("2025-01-01", "2025-01-31", GroupBy::Category)).unwrap();
    let v = serde_json::to_value(&b).unwrap();
    assert_eq!(
        v["rows"][0]["key"],
        serde_json::json!({"kind": "category", "id": 10})
    );
    assert_eq!(
        v["total"],
        serde_json::json!({"minor": 100, "currency": "USD"})
    );
}
