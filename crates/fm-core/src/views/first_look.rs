//! The first look at a ledger (T5.5, SPEC decision 34): money in and out by month and
//! the merchants the money went to, **before anything is categorised**.
//!
//! On first run nearly every transaction is in Needs review, so [`breakdown`]'s spending
//! (categorised only) is close to zero. This view doesn't use categories at all:
//!
//! - **Money in / money out:** every posted transaction that isn't a transfer, by the sign
//!   of its amount, whatever its bucket (Needs review, spending, refunds, income). Refunds
//!   aren't known yet, so a refund is money in, not less spending: the UI must say "money
//!   out (excluding transfers)", never "spending".
//! - **Transfers** are their own lines and never money in or out (SPEC decision 42).
//! - **Pending** is its own line and in no other (decision 42).
//! - Dates follow decision 42: the purchase date when the bank gives one, else posted.
//! - **Merchants by money out:** the applied merchant decision, else the raw description
//!   (`from_description`), as in the other views.
//!
//! Identities, for the whole period and for each month:
//! `money_in − money_out + transfers_in − transfers_out = net_flow`, the same net flow as
//! [`breakdown`]'s, and the merchant rows plus `other` add up to `money_out`.
//!
//! [`breakdown`]: super::breakdown

use std::collections::BTreeMap;

use serde::Serialize;

use super::breakdown::Line;
use super::scope::{Kind, Scope};
use super::{Filter, Key, Period, ViewError};
use crate::model::{Currency, Date, Ledger, Money, TransactionId, YearMonth};

/// What [`first_look`] is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstLookQuery {
    pub period: Period,
    pub filter: Filter,
    /// Merchants kept before "Other".
    pub top: usize,
    /// "Today", to flag months not over yet.
    pub as_of: Date,
}

/// One month's flows. Every amount is positive except `pending` and `net_flow`, which
/// keep their sign.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlowMonth {
    pub month: YearMonth,
    /// The month isn't over on `as_of`, or the period covers only part of it.
    pub partial: bool,
    pub money_in: Line,
    pub money_out: Line,
    pub transfers_in: Line,
    pub transfers_out: Line,
    pub pending: Line,
    pub net_flow: Money,
}

/// Money out to one merchant (positive).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MerchantOut {
    pub key: Key,
    pub label: String,
    pub money_out: Money,
    pub count: usize,
    /// Ascending.
    pub transaction_ids: Vec<TransactionId>,
}

/// The result of [`first_look`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FirstLook {
    pub currency: Currency,
    pub period: Period,
    /// Every calendar month of the period, oldest first, zero-filled.
    pub months: Vec<FlowMonth>,
    pub money_in: Line,
    pub money_out: Line,
    pub transfers_in: Line,
    pub transfers_out: Line,
    pub pending: Line,
    pub net_flow: Money,
    /// The largest `top` merchants by money out, largest first (ties by label).
    pub merchants: Vec<MerchantOut>,
    /// Everything after them, added up.
    pub other: Option<MerchantOut>,
}

#[derive(Default)]
struct Acc {
    total: i64,
    ids: Vec<TransactionId>,
}

impl Acc {
    fn add(&mut self, id: TransactionId, amount: i64) -> Result<(), ViewError> {
        self.total = self.total.checked_add(amount).ok_or(ViewError::Overflow)?;
        self.ids.push(id);
        Ok(())
    }

    fn line(mut self, c: Currency) -> Line {
        self.ids.sort_unstable();
        Line {
            amount: Money::new(self.total, c),
            count: self.ids.len(),
            transaction_ids: self.ids,
        }
    }
}

#[derive(Default)]
struct MonthAcc {
    money_in: Acc,
    money_out: Acc,
    transfers_in: Acc,
    transfers_out: Acc,
    pending: Acc,
    net: i64,
}

impl MonthAcc {
    fn line(self, c: Currency, month: YearMonth, partial: bool) -> FlowMonth {
        FlowMonth {
            month,
            partial,
            net_flow: Money::new(self.net, c),
            money_in: self.money_in.line(c),
            money_out: self.money_out.line(c),
            transfers_in: self.transfers_in.line(c),
            transfers_out: self.transfers_out.line(c),
            pending: self.pending.line(c),
        }
    }
}

/// Money in and out by month, and merchants by money out, regardless of categories.
pub fn first_look(ledger: &Ledger, query: &FirstLookQuery) -> Result<FirstLook, ViewError> {
    let scope = Scope::collect(ledger, &query.period, &query.filter)?;
    let c = scope.currency;
    let mut months: BTreeMap<YearMonth, MonthAcc> = query
        .period
        .year_months()
        .into_iter()
        .map(|m| (m, MonthAcc::default()))
        .collect();
    let mut all = MonthAcc::default();
    let mut merchants: BTreeMap<(String, bool), Acc> = BTreeMap::new();

    for item in &scope.items {
        let month = months.entry(item.date.year_month()).or_default();
        let amount = item.amount;
        let magnitude = amount.checked_abs().ok_or(ViewError::Overflow)?;
        for acc in [&mut *month, &mut all] {
            match item.kind {
                Kind::Pending => acc.pending.add(item.id, amount)?,
                Kind::TransferOut => acc.transfers_out.add(item.id, magnitude)?,
                Kind::TransferIn => acc.transfers_in.add(item.id, magnitude)?,
                Kind::Spending | Kind::Refund | Kind::Income | Kind::NeedsReview => {
                    if amount < 0 {
                        acc.money_out.add(item.id, magnitude)?;
                    } else if amount > 0 {
                        acc.money_in.add(item.id, amount)?;
                    }
                }
            }
            if item.kind != Kind::Pending {
                acc.net = acc.net.checked_add(amount).ok_or(ViewError::Overflow)?;
            }
        }
        let is_out = amount < 0
            && matches!(
                item.kind,
                Kind::Spending | Kind::Refund | Kind::Income | Kind::NeedsReview
            );
        if is_out {
            merchants
                .entry((item.merchant.to_string(), item.merchant_from_description))
                .or_default()
                .add(item.id, magnitude)?;
        }
    }

    let mut ranked: Vec<MerchantOut> = merchants
        .into_iter()
        .map(|((name, from_description), acc)| {
            let line = acc.line(c);
            MerchantOut {
                key: Key::Merchant {
                    name: name.clone(),
                    from_description,
                },
                label: name,
                money_out: line.amount,
                count: line.count,
                transaction_ids: line.transaction_ids,
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.money_out
            .minor()
            .cmp(&a.money_out.minor())
            .then_with(|| a.label.cmp(&b.label))
    });
    let other = if ranked.len() > query.top {
        let rest = ranked.split_off(query.top);
        let mut acc = Acc::default();
        for m in rest {
            acc.total = acc
                .total
                .checked_add(m.money_out.minor())
                .ok_or(ViewError::Overflow)?;
            acc.ids.extend(m.transaction_ids);
        }
        let line = acc.line(c);
        Some(MerchantOut {
            key: Key::Other,
            label: "Other".to_owned(),
            money_out: line.amount,
            count: line.count,
            transaction_ids: line.transaction_ids,
        })
    } else {
        None
    };

    let as_of_month = query.as_of.year_month();
    let months = months
        .into_iter()
        .map(|(m, acc)| {
            let covered = Period::month(m);
            let partial = m >= as_of_month
                || covered.from < query.period.from
                || covered.to > query.period.to;
            acc.line(c, m, partial)
        })
        .collect();
    Ok(FirstLook {
        currency: c,
        period: query.period,
        months,
        net_flow: Money::new(all.net, c),
        money_in: all.money_in.line(c),
        money_out: all.money_out.line(c),
        transfers_in: all.transfers_in.line(c),
        transfers_out: all.transfers_out.line(c),
        pending: all.pending.line(c),
        merchants: ranked,
        other,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::*;
    use crate::views::{GroupBy, Measure, Query, breakdown};

    fn query(period: Period) -> FirstLookQuery {
        FirstLookQuery {
            period,
            filter: Filter::default(),
            top: 5,
            as_of: "2026-10-17".parse().unwrap(),
        }
    }

    /// TD.1's ledger as imported: no decisions at all, so everything is Needs review.
    fn td1(persona: &str) -> Ledger {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/synthetic/data")
            .join(persona)
            .join("ledger.json");
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    /// TD.1's transfer pairs labelled from `truth.json`, the rest left in Needs review:
    /// what the ledger looks like after transfer detection on first run.
    fn td1_with_transfers(persona: &str) -> Ledger {
        let mut ledger = td1(persona);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/synthetic/data")
            .join(persona)
            .join("truth.json");
        let truth: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for (i, t) in truth["transactions"].as_array().unwrap().iter().enumerate() {
            if t["bucket"]["kind"] != "transfer" {
                continue;
            }
            ledger.decisions.push(Decision {
                id: DecisionId(i64::try_from(i).unwrap() + 1),
                transaction_id: TransactionId(t["id"].as_i64().unwrap()),
                assignment: Assignment::Bucket(Bucket::Transfer {
                    counterpart: t["bucket"]["counterpart"].as_i64().map(TransactionId),
                }),
                source: DecisionSource::Builtin {
                    logic: BuiltinLogic::TransferMatcher,
                    version: 1,
                },
                taxonomy_version: ledger.taxonomy.version.clone(),
                decided_at: Timestamp::from_unix_seconds(0),
                state: DecisionState::Applied,
                seen_at: None,
                replaces: None,
            });
        }
        ledger
    }

    fn check_identities(look: &FirstLook) {
        let m = |l: &Line| l.amount.minor();
        for month in look.months.iter().map(|f| {
            (
                f.month,
                m(&f.money_in) - m(&f.money_out) + m(&f.transfers_in) - m(&f.transfers_out),
                f.net_flow.minor(),
            )
        }) {
            assert_eq!(month.1, month.2, "net flow identity in {}", month.0);
        }
        assert_eq!(
            m(&look.money_in) - m(&look.money_out) + m(&look.transfers_in) - m(&look.transfers_out),
            look.net_flow.minor(),
            "net flow identity over the period"
        );
        let merchants: i64 = look
            .merchants
            .iter()
            .chain(&look.other)
            .map(|r| r.money_out.minor())
            .sum();
        assert_eq!(
            merchants,
            m(&look.money_out),
            "merchants add up to money out"
        );
        let sum = |f: fn(&FlowMonth) -> i64| look.months.iter().map(f).sum::<i64>();
        assert_eq!(sum(|f| f.money_out.amount.minor()), m(&look.money_out));
        assert_eq!(sum(|f| f.money_in.amount.minor()), m(&look.money_in));
        assert_eq!(sum(|f| f.pending.amount.minor()), m(&look.pending));
    }

    /// The identities on every TD.1 household, uncategorised (as imported) and with
    /// transfers detected; and the same net flow, transfers and pending as T3.1's
    /// breakdown, month by month.
    #[test]
    fn first_look_matches_breakdown_on_td1() {
        for persona in ["renter", "family", "freelancer", "retiree"] {
            for ledger in [td1(persona), td1_with_transfers(persona)] {
                let period =
                    Period::months("2025-10".parse().unwrap(), "2026-09".parse().unwrap()).unwrap();
                let look = first_look(&ledger, &query(period)).unwrap();
                check_identities(&look);
                assert_eq!(look.months.len(), 12);
                assert!(
                    look.money_out.amount.minor() > 0,
                    "{persona}: money went out"
                );
                assert!(!look.merchants.is_empty());
                for f in &look.months {
                    let b = breakdown(
                        &ledger,
                        &Query {
                            period: Period::month(f.month),
                            filter: Filter::default(),
                            group_by: GroupBy::Group,
                            measure: Measure::Spending,
                            top: None,
                            as_of: "2026-10-17".parse().unwrap(),
                        },
                    )
                    .unwrap();
                    let t = &b.totals;
                    assert_eq!(f.net_flow, t.net_flow, "{persona} {}: net flow", f.month);
                    assert_eq!(f.transfers_out, t.transfers_out, "{persona} {}", f.month);
                    assert_eq!(f.transfers_in, t.transfers_in, "{persona} {}", f.month);
                    assert_eq!(f.pending, t.pending, "{persona} {}: pending", f.month);
                    assert!(!f.partial, "{persona} {}: complete", f.month);
                }
            }
        }
    }

    /// The reason this view exists: uncategorised, `breakdown`'s spending is zero while
    /// money plainly went out.
    #[test]
    fn money_out_doesnt_wait_for_categories() {
        let ledger = td1("renter");
        let period = Period::month("2026-03".parse().unwrap());
        let b = breakdown(
            &ledger,
            &Query {
                period,
                filter: Filter::default(),
                group_by: GroupBy::Group,
                measure: Measure::Spending,
                top: None,
                as_of: "2026-10-17".parse().unwrap(),
            },
        )
        .unwrap();
        assert_eq!(b.totals.spending.amount.minor(), 0);
        let look = first_look(&ledger, &query(period)).unwrap();
        assert!(look.money_out.amount.minor() > 0);
        assert_eq!(
            look.money_out.count + look.money_in.count,
            b.totals.needs_review.count
        );
    }

    /// A small hand-made case: transfers and pending stay out of money in and out, a
    /// refund is money in, merchant names come from decisions or descriptions, and the
    /// top list cuts to `top` with the rest in Other.
    #[test]
    fn transfers_and_pending_are_their_own_lines() {
        let mut ledger = td1("renter");
        ledger.decisions.clear();
        let card = ledger.accounts[0].id;
        let tx = |id: i64, day: u8, minor: i64, desc: &str, pending: bool| Transaction {
            id: TransactionId(id),
            account_id: card,
            source: SourceRef {
                import_id: ImportId(1),
                index: u32::try_from(id).unwrap(),
                external_id: None,
            },
            status: if pending {
                TransactionStatus::Pending
            } else {
                TransactionStatus::Posted
            },
            posted_date: Date::from_ymd(2026, 9, day).unwrap(),
            transaction_date: None,
            amount: Money::new(minor, Currency::USD),
            original_amount: None,
            description: desc.into(),
            payee: None,
            memo: None,
            trn_type: None,
            check_number: None,
        };
        ledger.transactions = vec![
            tx(1, 1, -1_000, "COFFEE ONE", false),
            tx(2, 2, -2_000, "COFFEE ONE", false),
            tx(3, 3, -500, "BOOKS", false),
            tx(4, 4, 300, "BOOKS REFUND", false),
            tx(5, 5, -10_000, "TO SAVINGS", false),
            tx(6, 6, -700, "PENDING DINER", true),
            tx(7, 7, -50, "TINY", false),
        ];
        let decide = |id: i64, tx: i64, assignment: Assignment| Decision {
            id: DecisionId(id),
            transaction_id: TransactionId(tx),
            assignment,
            source: DecisionSource::User,
            taxonomy_version: ledger.taxonomy.version.clone(),
            decided_at: Timestamp::from_unix_seconds(0),
            state: DecisionState::Applied,
            seen_at: None,
            replaces: None,
        };
        ledger.decisions = vec![
            decide(
                1,
                5,
                Assignment::Bucket(Bucket::Transfer { counterpart: None }),
            ),
            decide(2, 1, Assignment::Merchant("Coffee One".into())),
            decide(3, 2, Assignment::Merchant("Coffee One".into())),
        ];
        let look = first_look(
            &ledger,
            &FirstLookQuery {
                top: 1,
                ..query(Period::month("2026-09".parse().unwrap()))
            },
        )
        .unwrap();
        check_identities(&look);
        assert_eq!(look.money_out.amount.minor(), 1_000 + 2_000 + 500 + 50);
        assert_eq!(look.money_in.amount.minor(), 300, "a refund is money in");
        assert_eq!(look.transfers_out.amount.minor(), 10_000);
        assert_eq!(look.pending.amount.minor(), -700);
        assert_eq!(look.pending.transaction_ids, [TransactionId(6)]);
        assert_eq!(look.merchants.len(), 1);
        assert_eq!(look.merchants[0].label, "Coffee One");
        assert_eq!(look.merchants[0].money_out.minor(), 3_000);
        assert_eq!(
            look.merchants[0].key,
            Key::Merchant {
                name: "Coffee One".into(),
                from_description: false
            }
        );
        let other = look.other.as_ref().unwrap();
        assert_eq!(other.money_out.minor(), 550);
        assert_eq!(other.transaction_ids, [TransactionId(3), TransactionId(7)]);
    }

    #[test]
    fn a_month_not_over_is_partial() {
        let ledger = td1("renter");
        let look = first_look(
            &ledger,
            &FirstLookQuery {
                as_of: "2026-09-15".parse().unwrap(),
                ..query(
                    Period::months("2026-08".parse().unwrap(), "2026-09".parse().unwrap()).unwrap(),
                )
            },
        )
        .unwrap();
        assert!(!look.months[0].partial);
        assert!(look.months[1].partial);
    }
}
