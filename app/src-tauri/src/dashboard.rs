//! The dashboard (T3.3): one month's totals, spending by group and category with the change
//! from the month before, top merchants, a 12-month trend, and subscriptions and bills.
//!
//! Every figure is an fm-core result (`fm_core::views`, `fm_core::recurring`), passed through
//! unchanged. Counting follows SPEC decision 42: pending transactions are their own line,
//! never in a total; Needs review is its own line and the review queue's input.

use std::collections::{BTreeMap, BTreeSet};

use fm_core::model::{Currency, Date, Ledger, TransactionId, YearMonth};
use fm_core::recurring::{self, Cadence, Direction, SeriesAmount, SeriesKind};
use fm_core::views::{
    self, Breakdown, Change, Filter, GroupBy, Key, Line, Measure, Period, Query, Row,
};
use serde::Serialize;

use crate::ledger_view::{
    MoneyView, Result, counting_date, local_today, locked, percent, ui_error,
};
use crate::unlock::AppUnlock;

/// Months of history in the trend table, ending with the month shown.
pub const TREND_MONTHS: i32 = 12;
/// Merchants listed before "Other".
pub const TOP_MERCHANTS: usize = 5;

fn ids(v: &[TransactionId]) -> Vec<i64> {
    v.iter().map(|t| t.0).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineView {
    pub amount: MoneyView,
    pub count: usize,
    pub transaction_ids: Vec<i64>,
}

impl From<&Line> for LineView {
    fn from(l: &Line) -> Self {
        Self {
            amount: l.amount.into(),
            count: l.count,
            transaction_ids: ids(&l.transaction_ids),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TotalsView {
    pub spending: LineView,
    pub refunds: LineView,
    pub income: LineView,
    pub transfers_out: LineView,
    pub transfers_in: LineView,
    pub needs_review: LineView,
    /// Never part of any total (SPEC decision 42).
    pub pending: LineView,
    pub net_flow: MoneyView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowView {
    pub label: String,
    pub total: MoneyView,
    pub gross: MoneyView,
    pub refunds: MoneyView,
    pub count: usize,
    pub transaction_ids: Vec<i64>,
    /// A merchant row named by the raw description (no merchant decision).
    pub from_description: bool,
}

impl From<&Row> for RowView {
    fn from(r: &Row) -> Self {
        Self {
            label: r.label.clone(),
            total: r.total.into(),
            gross: r.gross.into(),
            refunds: r.refunds.into(),
            count: r.count,
            transaction_ids: ids(&r.transaction_ids),
            from_description: matches!(
                r.key,
                Key::Merchant {
                    from_description: true,
                    ..
                }
            ),
        }
    }
}

/// The change from the month before, as fm-core computed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeView {
    pub base: MoneyView,
    pub change: MoneyView,
    /// `None` when the month before was zero.
    pub percent: Option<String>,
}

impl From<&views::Delta> for ChangeView {
    fn from(d: &views::Delta) -> Self {
        Self {
            base: d.base.into(),
            change: d.change.into(),
            percent: d.ratio.as_ref().map(|r| percent(r.basis_points)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    pub group_id: Option<i64>,
    pub row: RowView,
    pub change: Option<ChangeView>,
    pub categories: Vec<CategoryRowView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRowView {
    pub category_id: i64,
    pub row: RowView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendRowView {
    pub label: String,
    pub values: Vec<MoneyView>,
    pub average: Option<MoneyView>,
    pub median: Option<MoneyView>,
    pub latest: Option<MoneyView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendView {
    /// `YYYY-MM`, oldest first, with `partial` set for a month not yet over.
    pub months: Vec<MonthColumn>,
    pub rows: Vec<TrendRowView>,
    pub total: TrendRowView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthColumn {
    pub month: String,
    pub partial: bool,
    pub total: MoneyView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecurringView {
    pub name: String,
    pub cadence: &'static str,
    /// The latest occurrence's amount.
    pub amount: MoneyView,
    /// Different every time (utilities): `amount` is the latest of a range.
    pub variable: bool,
    pub last: String,
    pub next_expected: Option<String>,
    pub ended: bool,
    pub accounts: Vec<String>,
    pub transaction_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardView {
    pub as_of: String,
    /// Currencies with transactions; one dashboard shows one (SPEC decision 42, rule 5).
    pub currencies: Vec<&'static str>,
    pub currency: Option<&'static str>,
    /// Months with transactions in `currency`, newest first.
    pub months: Vec<String>,
    pub month: Option<String>,
    /// The month isn't over yet (or the view covers part of it).
    pub partial: bool,
    pub totals: Option<TotalsView>,
    pub spending_change: Option<ChangeView>,
    pub groups: Vec<GroupView>,
    pub merchants: Vec<RowView>,
    pub merchants_other: Option<RowView>,
    pub trend: Option<TrendView>,
    pub recurring: Vec<RecurringView>,
}

impl DashboardView {
    fn empty(as_of: Date) -> Self {
        Self {
            as_of: as_of.to_string(),
            currencies: vec![],
            currency: None,
            months: vec![],
            month: None,
            partial: false,
            totals: None,
            spending_change: None,
            groups: vec![],
            merchants: vec![],
            merchants_other: None,
            trend: None,
            recurring: vec![],
        }
    }
}

fn cadence(c: Cadence) -> &'static str {
    match c {
        Cadence::Weekly => "weekly",
        Cadence::Biweekly => "every 2 weeks",
        Cadence::Semimonthly => "twice a month",
        Cadence::Monthly => "monthly",
        Cadence::Quarterly => "quarterly",
        Cadence::Semiannual => "every 6 months",
        Cadence::Annual => "yearly",
    }
}

/// Builds the dashboard for `month` (`YYYY-MM`; the latest month with transactions when
/// `None`) in `currency` (the one with the most transactions when `None`).
pub fn build(
    ledger: &Ledger,
    month: Option<&str>,
    currency: Option<&str>,
    as_of: Date,
) -> Result<DashboardView> {
    let currency_of: BTreeMap<_, Currency> =
        ledger.accounts.iter().map(|a| (a.id, a.currency)).collect();
    let mut per_currency: BTreeMap<&'static str, usize> = BTreeMap::new();
    for t in &ledger.transactions {
        if let Some(c) = currency_of.get(&t.account_id) {
            *per_currency.entry(c.code()).or_default() += 1;
        }
    }
    if per_currency.is_empty() {
        return Ok(DashboardView::empty(as_of));
    }
    let currency = match currency {
        Some(code) => {
            Currency::new(code).map_err(|e| ui_error("InvalidCurrency", e.to_string()))?
        }
        None => {
            let (code, _) = per_currency
                .iter()
                .max_by_key(|(code, n)| (**n, std::cmp::Reverse(**code)))
                .expect("not empty");
            Currency::new(code).expect("an account's currency is valid")
        }
    };
    let months: BTreeSet<YearMonth> = ledger
        .transactions
        .iter()
        .filter(|t| currency_of.get(&t.account_id) == Some(&currency))
        .map(|t| counting_date(t).year_month())
        .collect();
    let latest = *months
        .last()
        .ok_or_else(|| ui_error("NoTransactions", "no transactions in this currency"))?;
    let month: YearMonth = match month {
        Some(m) => m
            .parse()
            .map_err(|e: fm_core::model::ModelError| ui_error("InvalidMonth", e.to_string()))?,
        None => latest,
    };

    let filter = Filter {
        currency: Some(currency),
        ..Filter::default()
    };
    let query = |group_by, top| Query {
        period: Period::month(month),
        filter: filter.clone(),
        group_by,
        measure: Measure::Spending,
        top,
        as_of,
    };

    let by_group: Breakdown = views::breakdown(ledger, &query(GroupBy::Group, None))?;
    let by_category = views::breakdown(ledger, &query(GroupBy::Category, None))?;
    let group_change = views::month_over_month(ledger, &query(GroupBy::Group, None))?;
    let merchants = views::breakdown(ledger, &query(GroupBy::Merchant, Some(TOP_MERCHANTS)))?;

    let changes: BTreeMap<&Key, &Change> = group_change.rows.iter().map(|c| (&c.key, c)).collect();
    let group_of = |category| ledger.taxonomy.resolve(category).map(|c| c.group_id);
    let groups = by_group
        .rows
        .iter()
        .map(|g| {
            let group_id = match g.key {
                Key::Group { id } => Some(id),
                _ => None,
            };
            let categories = by_category
                .rows
                .iter()
                .filter_map(|c| match c.key {
                    Key::Category { id } if group_id.is_some() && group_of(id) == group_id => {
                        Some(CategoryRowView {
                            category_id: id.0,
                            row: c.into(),
                        })
                    }
                    _ => None,
                })
                .collect();
            GroupView {
                group_id: group_id.map(|g| g.0),
                row: g.into(),
                change: changes.get(&g.key).map(|c| ChangeView::from(&c.delta)),
                categories,
            }
        })
        .collect();

    let first = month
        .checked_add_months(-(TREND_MONTHS - 1))
        .ok_or_else(|| ui_error("InvalidMonth", "month out of range"))?;
    let trend = views::trend(
        ledger,
        &Query {
            period: Period::months(first, month)?,
            ..query(GroupBy::Group, None)
        },
    )?;
    let trend_row =
        |label: String, values: &[fm_core::model::Money], s: &views::TrendStats| TrendRowView {
            label,
            values: values.iter().map(|m| (*m).into()).collect(),
            average: s.average.map(Into::into),
            median: s.median.map(Into::into),
            latest: s.latest.map(Into::into),
        };
    let trend = TrendView {
        months: trend
            .series
            .columns
            .iter()
            .map(|c| MonthColumn {
                month: c.month.to_string(),
                partial: c.partial,
                total: c.total.into(),
            })
            .collect(),
        rows: trend
            .series
            .rows
            .iter()
            .zip(&trend.rows)
            .map(|(r, t)| trend_row(r.label.clone(), &r.values, &t.stats))
            .collect(),
        total: trend_row(
            "Total".into(),
            &trend
                .series
                .columns
                .iter()
                .map(|c| c.total)
                .collect::<Vec<_>>(),
            &trend.overall,
        ),
    };

    let account_name: BTreeMap<_, &str> = ledger
        .accounts
        .iter()
        .map(|a| (a.id, a.name.as_str()))
        .collect();
    let recurring = recurring::detect(
        &ledger.accounts,
        &ledger.transactions,
        &ledger.decisions,
        as_of,
    )
    .into_iter()
    .filter(|s| {
        s.kind == SeriesKind::Charge
            && s.direction == Direction::Out
            && s.amount.latest().currency() == currency
    })
    .map(|s| RecurringView {
        name: s
            .label
            .as_ref()
            .map_or_else(|| s.merchant_key.clone(), |l| l.name.clone()),
        cadence: cadence(s.cadence),
        amount: s.amount.latest().into(),
        variable: matches!(s.amount, SeriesAmount::Variable { .. }),
        last: s.last.to_string(),
        next_expected: s.next_expected.map(|d| d.to_string()),
        ended: s.ended,
        accounts: s
            .accounts
            .iter()
            .map(|a| {
                account_name
                    .get(a)
                    .map_or_else(|| format!("account {}", a.0), |n| (*n).into())
            })
            .collect(),
        transaction_ids: ids(&s.members),
    })
    .collect();

    let t = &by_group.totals;
    Ok(DashboardView {
        as_of: as_of.to_string(),
        currencies: per_currency.keys().copied().collect(),
        currency: Some(currency.code()),
        months: months.iter().rev().map(ToString::to_string).collect(),
        month: Some(month.to_string()),
        partial: by_group.partial_month,
        totals: Some(TotalsView {
            spending: (&t.spending).into(),
            refunds: (&t.refunds).into(),
            income: (&t.income).into(),
            transfers_out: (&t.transfers_out).into(),
            transfers_in: (&t.transfers_in).into(),
            needs_review: (&t.needs_review).into(),
            pending: (&t.pending).into(),
            net_flow: t.net_flow.into(),
        }),
        spending_change: Some((&group_change.total).into()),
        groups,
        merchants: merchants.rows.iter().map(Into::into).collect(),
        merchants_other: merchants.other.as_ref().map(Into::into),
        trend: Some(trend),
        recurring,
    })
}

impl AppUnlock {
    /// `today` is the user's local date from the UI (see [`local_today`]).
    pub fn dashboard(
        &self,
        month: Option<&str>,
        currency: Option<&str>,
        today: &str,
    ) -> Result<DashboardView> {
        let as_of = local_today(today, fm_core::model::Timestamp::now())?;
        self.with_ledger(|open| {
            let ledger = open.ledger().load_ledger()?;
            build(&ledger, month, currency, as_of)
        })
        .ok_or_else(locked)?
    }
}

pub mod commands {
    use tauri::State;

    use super::DashboardView;
    use crate::ledger_view::Result;
    use crate::unlock::AppUnlock;

    #[tauri::command(async)]
    pub fn dashboard(
        unlock: State<'_, AppUnlock>,
        month: Option<String>,
        currency: Option<String>,
        today: String,
    ) -> Result<DashboardView> {
        unlock.dashboard(month.as_deref(), currency.as_deref(), &today)
    }
}
