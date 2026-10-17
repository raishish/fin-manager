//! The query language: what a question asks, as data. T4.2's model writes it (constrained
//! by `schema.json`), the user sees it, and [`run`](super::run) answers it.

use serde::{Deserialize, Serialize};

use crate::model::{AccountId, AccountKind, Date, YearMonth};

/// A question as a list of named items. Each item gives one answer, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    /// The answer's name, unique in the query (`"total"`, `"august_2026"`). `derive`
    /// items refer to other items by it.
    pub name: String,
    pub ask: Ask,
}

/// What one item computes. Serialised with an `"op"` tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ask {
    /// The measure added up. Money.
    Sum { measure: Measure, scope: Scope },
    /// How many transactions the measure counts. Count.
    Count { measure: Measure, scope: Scope },
    /// The measure per transaction or per complete month, rounded half away from zero
    /// to the minor unit. Money.
    Average {
        measure: Measure,
        scope: Scope,
        per: Per,
    },
    /// The middle monthly total over the complete months (the mean of the middle two
    /// for an even count). Money.
    Median {
        measure: Measure,
        scope: Scope,
        per: Per,
    },
    /// Groups by `by`, orders by `metric`, and reports the top `n` (`labels`) or the
    /// group at `position` (1-based; `label`, `amount`, `count`). Ties go to the label
    /// that sorts first.
    Rank {
        measure: Measure,
        scope: Scope,
        by: By,
        order: Order,
        #[serde(default)]
        metric: Metric,
        #[serde(default = "one")]
        n: usize,
        output: RankOutput,
        #[serde(default = "one")]
        position: usize,
    },
    /// Every group's label, in order: months in calendar order, others largest first.
    /// Texts.
    Keys {
        measure: Measure,
        scope: Scope,
        by: By,
    },
    /// How many different groups. Count.
    Distinct {
        measure: Measure,
        scope: Scope,
        by: By,
    },
    /// One transaction: the largest, smallest, latest or earliest. Ties go to the
    /// earlier transaction id, except `latest`, which takes the later.
    Pick {
        measure: Measure,
        scope: Scope,
        order: PickOrder,
        output: PickOutput,
    },
    /// Recurring series (T3.2): subscriptions, bills, income, interest, transfers.
    Recurring {
        select: SeriesSelect,
        output: SeriesOutput,
    },
    /// Arithmetic on other items' answers.
    Derive { formula: DeriveOp, of: Vec<String> },
}

fn one() -> usize {
    1
}

/// What is added up. Money out is reported as a positive amount, and so is money in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    /// Spending net of refunds (SPEC decision 42).
    Spending,
    /// Purchases only, before refunds. Also what `count` means by "times I bought".
    Purchases,
    Refunds,
    /// The Income bucket only.
    Income,
    TransfersIn,
    TransfersOut,
    /// Every posted amount into the account(s), whatever its bucket.
    MoneyIn,
    /// Every posted amount out of the account(s), whatever its bucket.
    MoneyOut,
    /// Pending transactions, as money out. Never part of any other measure.
    Pending,
}

/// Which transactions an item looks at. Every field narrows; an empty list matches
/// nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scope {
    pub period: PeriodSpec,
    /// Category keys (`"food.groceries"`), stable across installs.
    pub categories: Option<Vec<String>>,
    /// Group keys (`"food"`).
    pub groups: Option<Vec<String>>,
    /// Merchant name, compared after normalisation (case and punctuation ignored).
    pub merchant: Option<String>,
    pub accounts: Option<AccountSelect>,
    /// Transfers whose other side is on one of these accounts.
    pub counterpart_accounts: Option<AccountSelect>,
    /// Words that must appear together in the bank's description (normalised).
    pub description: Option<String>,
    /// Only transactions the bank recorded in this currency (a purchase abroad).
    pub original_currency: Option<String>,
    /// `original` reports amounts as the bank recorded them in `original_currency`.
    pub amounts: Amounts,
}

/// Accounts by kind or id. Both given: an account matching either.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AccountSelect {
    pub kinds: Vec<AccountKind>,
    pub ids: Vec<AccountId>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Amounts {
    #[default]
    Account,
    Original,
}

/// A period, absolute or relative to the `as_of` day the app passes in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PeriodSpec {
    /// Inclusive dates.
    Range {
        from: Date,
        to: Date,
    },
    Month(YearMonth),
    Months {
        from: YearMonth,
        to: YearMonth,
    },
    Quarter {
        year: i32,
        quarter: u8,
    },
    Year(i32),
    /// The `n` full calendar months before the month of `as_of`.
    LastMonths(u32),
    /// The `n` days before `as_of` (not including it).
    LastDays(u32),
    /// Monday to Sunday of the week before the week of `as_of`.
    LastWeek,
    /// The full calendar quarter before the quarter of `as_of`.
    LastQuarter,
    /// From 1 January of `as_of`'s year to `as_of`.
    YearToDate,
    /// From the ledger's first transaction to `as_of`.
    #[default]
    AllTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Per {
    Transaction,
    Month,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    Category,
    Group,
    Merchant,
    Month,
    Account,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    Highest,
    Lowest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    #[default]
    Amount,
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankOutput {
    /// The top `n` labels. Texts.
    Labels,
    /// The label at `position`. Text.
    Label,
    /// The amount at `position`. Money.
    Amount,
    /// The number of transactions at `position`. Count.
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickOrder {
    Largest,
    Smallest,
    Latest,
    Earliest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickOutput {
    Merchant,
    Amount,
    Date,
}

/// Which recurring series. Every field narrows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SeriesSelect {
    /// The series' name (its label, else its merchant key), normalised.
    pub name: Option<String>,
    /// Series most of whose members are in these categories ("my rent").
    pub categories: Option<Vec<String>>,
    pub kinds: Option<Vec<SeriesKindSel>>,
    pub cadences: Option<Vec<CadenceSel>>,
    pub status: SeriesStatus,
    /// For `member_count` and `member_total`: only members dated in this period.
    pub period: Option<PeriodSpec>,
}

/// A series kind as the user names it. The labeler's answer when there is one;
/// otherwise fixed logic, where a charge is both a subscription and a bill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesKindSel {
    Subscription,
    Bill,
    Income,
    Interest,
    Transfer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CadenceSel {
    Weekly,
    Biweekly,
    Semimonthly,
    Monthly,
    Quarterly,
    Semiannual,
    Annual,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesStatus {
    /// Still running at `as_of`.
    Active,
    Ended,
    /// Ended, and no series of the same name is still running (a cancel-and-restart
    /// doesn't count).
    Cancelled,
    #[default]
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesOutput {
    /// The names, sorted. Texts.
    Names,
    /// How many series. Count.
    Count,
    /// The latest amount of the series (the most recent one if several match). Money.
    Latest,
    /// The latest amounts added up ("what my subscriptions cost now"). Money.
    LatestTotal,
    /// The date of the first charge at the latest price. Date.
    PriceFrom,
    /// The price before the latest change. Money.
    PriceBefore,
    /// The price after the latest change. Money.
    PriceAfter,
    /// How many members, in `select.period`. Count.
    MemberCount,
    /// The members added up, in `select.period`. Money.
    MemberTotal,
}

/// Arithmetic over other items, named in `of`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeriveOp {
    /// `of[0] − of[1]`. Money or count.
    Difference,
    /// All of `of` added up. Money or count.
    Sum,
    /// `(of[0] − of[1]) ÷ |of[1]|`, in basis points. Percent.
    PercentChange,
    /// `of[0] ÷ of[1]`, in basis points. Percent.
    Share,
    /// The name of the larger of `of[0]` and `of[1]` (the first on a tie). Text.
    Larger,
}
