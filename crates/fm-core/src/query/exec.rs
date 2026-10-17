//! Runs a [`Query`]: each item in turn, `derive` items after what they refer to.

use std::collections::{BTreeMap, HashMap};

use super::facts::{Fact, Facts, gather};
use super::period::resolve;
use super::series;
use super::{
    Answer, Ask, By, DeriveOp, Item, Measure, Metric, Order, Per, PickOrder, PickOutput, Query,
    QueryError, RankOutput, Scope, Value,
};
use crate::contract::LedgerIndex;
use crate::model::{Date, Ledger, Money, TransactionId, YearMonth};
use crate::recurring::{self, Labeler, Series};

/// What a query runs against besides the ledger.
pub struct Context<'a> {
    /// "Today", from the app (never the model, never the clock inside fm-core).
    pub as_of: Date,
    /// Names recurring series and tells subscriptions from bills (T3.2's hook). Without
    /// it, `recurring` items use fixed logic only.
    pub labeler: Option<&'a dyn Labeler>,
}

/// Answers every item of `query`, in order.
pub fn run(ledger: &Ledger, query: &Query, ctx: &Context<'_>) -> Result<Vec<Answer>, QueryError> {
    let mut names = std::collections::HashSet::new();
    if query.items.is_empty() || !query.items.iter().all(|i| names.insert(i.name.as_str())) {
        return Err(QueryError::BadItems);
    }
    let mut run = Run {
        ledger,
        ix: LedgerIndex::new(ledger),
        ctx,
        items: query.items.iter().map(|i| (i.name.as_str(), i)).collect(),
        done: HashMap::new(),
        series: None,
        earliest: ledger
            .transactions
            .iter()
            .map(|t| t.transaction_date.unwrap_or(t.posted_date))
            .min()
            .unwrap_or(ctx.as_of),
    };
    query
        .items
        .iter()
        .map(|i| {
            Ok(Answer {
                name: i.name.clone(),
                value: run.value(&i.name, &mut Vec::new())?,
            })
        })
        .collect()
}

struct Run<'a, 'c> {
    ledger: &'a Ledger,
    ix: LedgerIndex<'a>,
    ctx: &'c Context<'c>,
    items: HashMap<&'a str, &'a Item>,
    done: HashMap<String, Value>,
    series: Option<Vec<Series>>,
    earliest: Date,
}

fn add(a: i64, b: i64) -> Result<i64, QueryError> {
    a.checked_add(b).ok_or(QueryError::Overflow)
}

/// `n / d` rounded half away from zero; `d` must be positive.
fn div_round(n: i128, d: i128) -> Result<i64, QueryError> {
    let (q, r) = (n / d, n % d);
    let q = if 2 * r.abs() >= d { q + n.signum() } else { q };
    i64::try_from(q).map_err(|_| QueryError::Overflow)
}

fn count(n: usize) -> Result<i64, QueryError> {
    i64::try_from(n).map_err(|_| QueryError::Overflow)
}

fn median(mut v: Vec<i64>) -> Result<Option<i64>, QueryError> {
    v.sort_unstable();
    let n = v.len();
    Ok(match n {
        0 => None,
        _ if n % 2 == 1 => Some(v[n / 2]),
        _ => Some(div_round(
            i128::from(v[n / 2 - 1]) + i128::from(v[n / 2]),
            2,
        )?),
    })
}

/// One group of a `rank`, `keys` or `distinct`.
#[derive(Default)]
struct Group {
    amount: i64,
    ids: Vec<TransactionId>,
}

impl<'a> Run<'a, '_> {
    fn value(&mut self, name: &str, path: &mut Vec<String>) -> Result<Value, QueryError> {
        if let Some(v) = self.done.get(name) {
            return Ok(v.clone());
        }
        if path.iter().any(|p| p == name) {
            return Err(QueryError::BadReference(name.to_owned()));
        }
        let item = *self
            .items
            .get(name)
            .ok_or_else(|| QueryError::BadReference(name.to_owned()))?;
        path.push(name.to_owned());
        let value = self.compute(item, path)?;
        path.pop();
        self.done.insert(name.to_owned(), value.clone());
        Ok(value)
    }

    fn facts(&self, measure: Measure, scope: &Scope) -> Result<Facts<'a>, QueryError> {
        let period = resolve(&scope.period, self.ctx.as_of, self.earliest)?;
        gather(
            self.ledger,
            &self.ix,
            measure,
            scope,
            period,
            self.ctx.as_of,
        )
    }

    fn money(f: &Facts<'_>, minor: i64, mut ids: Vec<TransactionId>) -> Value {
        ids.sort_unstable();
        Value::Money {
            amount: Money::new(minor, f.currency),
            transaction_ids: ids,
        }
    }

    fn compute(&mut self, item: &Item, path: &mut Vec<String>) -> Result<Value, QueryError> {
        let name = item.name.as_str();
        let none = || QueryError::NothingFound(name.to_owned());
        match &item.ask {
            Ask::Sum { measure, scope } => {
                let f = self.facts(*measure, scope)?;
                let total = f.list.iter().try_fold(0, |a, x| add(a, x.value))?;
                Ok(Self::money(
                    &f,
                    total,
                    f.list.iter().map(|x| x.id).collect(),
                ))
            }
            Ask::Count { measure, scope } => {
                let f = self.facts(*measure, scope)?;
                Ok(Value::Count {
                    value: count(f.list.len())?,
                    transaction_ids: f.list.iter().map(|x| x.id).collect(),
                })
            }
            Ask::Average {
                measure,
                scope,
                per,
            } => {
                let f = self.facts(*measure, scope)?;
                let values = match per {
                    Per::Month => self.complete_months(&f)?,
                    // A refund lowers the total but isn't an occurrence of its own.
                    Per::Transaction => f.list.iter().map(|x| x.value).collect::<Vec<_>>(),
                };
                let occurrences = match per {
                    Per::Month => values.len(),
                    Per::Transaction => f.list.iter().filter(|x| !x.refund).count(),
                };
                let total = values.iter().try_fold(0, |a, v| add(a, *v))?;
                if occurrences == 0 {
                    return Err(none());
                }
                let avg = div_round(i128::from(total), i128::from(count(occurrences)?))?;
                Ok(Self::money(&f, avg, Vec::new()))
            }
            Ask::Median {
                measure,
                scope,
                per,
            } => {
                let f = self.facts(*measure, scope)?;
                let values = match per {
                    Per::Month => self.complete_months(&f)?,
                    Per::Transaction => f.list.iter().map(|x| x.value).collect(),
                };
                let m = median(values)?.ok_or_else(none)?;
                Ok(Self::money(&f, m, Vec::new()))
            }
            Ask::Rank {
                measure,
                scope,
                by,
                order,
                metric,
                n,
                output,
                position,
            } => {
                let f = self.facts(*measure, scope)?;
                let mut groups: Vec<(String, Group)> = self.group(&f, *by)?.into_iter().collect();
                let score = |g: &Group| match metric {
                    Metric::Amount => g.amount,
                    Metric::Count => i64::try_from(g.ids.len()).unwrap_or(i64::MAX),
                };
                groups.sort_by(|a, b| {
                    let by_score = score(&a.1).cmp(&score(&b.1));
                    let by_score = if *order == Order::Highest {
                        by_score.reverse()
                    } else {
                        by_score
                    };
                    by_score.then_with(|| a.0.cmp(&b.0))
                });
                if *output == RankOutput::Labels {
                    return Ok(Value::Texts {
                        values: groups.into_iter().take(*n).map(|g| g.0).collect(),
                    });
                }
                let (label, g) = position
                    .checked_sub(1)
                    .and_then(|i| groups.into_iter().nth(i))
                    .ok_or_else(none)?;
                Ok(match output {
                    RankOutput::Label | RankOutput::Labels => Value::Text { value: label },
                    RankOutput::Amount => Self::money(&f, g.amount, g.ids),
                    RankOutput::Count => {
                        let mut ids = g.ids;
                        ids.sort_unstable();
                        Value::Count {
                            value: count(ids.len())?,
                            transaction_ids: ids,
                        }
                    }
                })
            }
            Ask::Keys { measure, scope, by } => {
                let f = self.facts(*measure, scope)?;
                let mut groups: Vec<(String, Group)> = self.group(&f, *by)?.into_iter().collect();
                if *by != By::Month {
                    groups.sort_by(|a, b| b.1.amount.cmp(&a.1.amount).then_with(|| a.0.cmp(&b.0)));
                }
                Ok(Value::Texts {
                    values: groups.into_iter().map(|g| g.0).collect(),
                })
            }
            Ask::Distinct { measure, scope, by } => {
                let f = self.facts(*measure, scope)?;
                Ok(Value::Count {
                    value: count(self.group(&f, *by)?.len())?,
                    transaction_ids: Vec::new(),
                })
            }
            Ask::Pick {
                measure,
                scope,
                order,
                output,
            } => {
                let f = self.facts(*measure, scope)?;
                // `f.list` is in id order, so `min_by_key`/`max_by_key` break ties as
                // documented: the earlier id, except `latest` (the later).
                let it = f.list.iter();
                let x: &Fact<'_> = match order {
                    PickOrder::Largest => it.max_by_key(|x| (x.value, std::cmp::Reverse(x.id))),
                    PickOrder::Smallest => it.min_by_key(|x| (x.value, x.id)),
                    PickOrder::Latest => it.max_by_key(|x| (x.date, x.id)),
                    PickOrder::Earliest => it.min_by_key(|x| (x.date, x.id)),
                }
                .ok_or_else(none)?;
                Ok(match output {
                    PickOutput::Merchant => Value::Text {
                        value: x.merchant.to_owned(),
                    },
                    PickOutput::Amount => Self::money(&f, x.value, vec![x.id]),
                    PickOutput::Date => Value::Date { value: x.date },
                })
            }
            Ask::Recurring { select, output } => {
                if self.series.is_none() {
                    let l = self.ledger;
                    let mut all = recurring::detect(
                        &l.accounts,
                        &l.transactions,
                        &l.decisions,
                        self.ctx.as_of,
                    );
                    if let Some(labeler) = self.ctx.labeler {
                        recurring::label_all(&mut all, labeler);
                    }
                    self.series = Some(all);
                }
                let all = self.series.as_deref().unwrap_or_default();
                series::answer(
                    &self.ix,
                    all,
                    select,
                    *output,
                    self.ctx.as_of,
                    self.ctx.labeler.is_some(),
                    name,
                )
            }
            Ask::Derive { formula, of } => {
                let values = of
                    .iter()
                    .map(|n| self.value(n, path))
                    .collect::<Result<Vec<_>, _>>()?;
                derive(*formula, of, &values, name)
            }
        }
    }

    /// Monthly totals over the complete months of the facts' period: months before the
    /// month of `as_of`, covered in full. Empty months count as zero.
    fn complete_months(&self, f: &Facts<'_>) -> Result<Vec<i64>, QueryError> {
        let current = self.ctx.as_of.year_month();
        let p = f.period;
        let mut totals: BTreeMap<YearMonth, i64> = p
            .year_months()
            .into_iter()
            .filter(|m| *m < current && p.from <= m.first_day() && m.last_day() <= p.to)
            .map(|m| (m, 0))
            .collect();
        for x in &f.list {
            if let Some(t) = totals.get_mut(&x.date.year_month()) {
                *t = add(*t, x.value)?;
            }
        }
        Ok(totals.into_values().collect())
    }

    /// The facts grouped by `by`, keyed by display label.
    fn group(&self, f: &Facts<'_>, by: By) -> Result<BTreeMap<String, Group>, QueryError> {
        let mut groups: BTreeMap<String, Group> = BTreeMap::new();
        for x in &f.list {
            let label = match by {
                By::Merchant => x.merchant.to_owned(),
                By::Month => x.date.year_month().to_string(),
                By::Account => x.account.name.clone(),
                By::Category => x
                    .category
                    .ok_or(QueryError::GroupingNotApplicable)?
                    .name
                    .clone(),
                By::Group => {
                    let c = x.category.ok_or(QueryError::GroupingNotApplicable)?;
                    self.ledger
                        .taxonomy
                        .groups
                        .iter()
                        .find(|g| g.id == c.group_id)
                        .ok_or(QueryError::GroupingNotApplicable)?
                        .name
                        .clone()
                }
            };
            let g = groups.entry(label).or_default();
            g.amount = add(g.amount, x.value)?;
            g.ids.push(x.id);
        }
        Ok(groups)
    }
}

fn derive(op: DeriveOp, of: &[String], values: &[Value], name: &str) -> Result<Value, QueryError> {
    let wrong = || QueryError::WrongKind(name.to_owned());
    // Every operand as (minor, currency) for money, or (value, None) for a count.
    let nums = values
        .iter()
        .map(|v| match v {
            Value::Money { amount, .. } => Ok((amount.minor(), Some(amount.currency()))),
            Value::Count { value, .. } => Ok((*value, None)),
            _ => Err(wrong()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let same_kind = nums.windows(2).all(|w| w[0].1 == w[1].1);
    if nums.is_empty() || !same_kind {
        return Err(wrong());
    }
    let pair = || match nums.as_slice() {
        [a, b] => Ok((a.0, b.0)),
        _ => Err(wrong()),
    };
    let wrap = |v: i64| match nums[0].1 {
        Some(c) => Value::Money {
            amount: Money::new(v, c),
            transaction_ids: Vec::new(),
        },
        None => Value::Count {
            value: v,
            transaction_ids: Vec::new(),
        },
    };
    let bp = |num: i64, den: i64| {
        if den == 0 {
            return Err(QueryError::ZeroBase(name.to_owned()));
        }
        Ok(Value::Percent {
            basis_points: div_round(i128::from(num) * 10_000, i128::from(den))?,
        })
    };
    match op {
        DeriveOp::Difference => {
            let (a, b) = pair()?;
            Ok(wrap(a.checked_sub(b).ok_or(QueryError::Overflow)?))
        }
        DeriveOp::Sum => Ok(wrap(nums.iter().try_fold(0, |s, n| add(s, n.0))?)),
        DeriveOp::PercentChange => {
            let (a, b) = pair()?;
            let change = a.checked_sub(b).ok_or(QueryError::Overflow)?;
            bp(change, b.checked_abs().ok_or(QueryError::Overflow)?)
        }
        DeriveOp::Share => {
            let (a, b) = pair()?;
            // Keep the denominator positive for the rounding.
            if b < 0 {
                bp(
                    a.checked_neg().ok_or(QueryError::Overflow)?,
                    b.checked_neg().ok_or(QueryError::Overflow)?,
                )
            } else {
                bp(a, b)
            }
        }
        DeriveOp::Larger => {
            let (a, b) = pair()?;
            Ok(Value::Text {
                value: if a >= b { of[0].clone() } else { of[1].clone() },
            })
        }
    }
}
