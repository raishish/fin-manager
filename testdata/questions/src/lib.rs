//! The 100-question test set (TD.3).
//!
//! Each question about the freelancer persona's two years of money comes with its
//! expected answer(s), computed here from TD.1's committed ledger and ground truth,
//! never written by hand. Sums carry the transaction ids behind them, so a test can
//! re-add them. `README.md` documents the file and the counting conventions.

pub mod facts;
mod queries;
mod questions;

use std::collections::BTreeMap;

use fm_core::model::{Date, Money, Sha256Hash};
use serde::{Deserialize, Serialize};

use crate::facts::{Fact, Kind, Period};

/// How an answer's evidence adds up to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    /// Minus the sum of the transactions' amounts (spending: money out is negative).
    NegatedSum,
    /// The sum of the amounts (income, refunds, money in).
    Sum,
    /// The sum of the original foreign-currency amounts, negated.
    NegatedOriginalSum,
    /// The number of transactions.
    Count,
    /// Computed from other answers or values (an average, a difference, a median).
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub rule: Rule,
    pub transaction_ids: Vec<i64>,
}

/// One expected value. A question can have several (a comparison has both sides and
/// the difference).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Answer {
    Money {
        name: String,
        amount: Money,
        evidence: Evidence,
    },
    Count {
        name: String,
        value: i64,
        evidence: Evidence,
    },
    /// Basis points: 1234 = 12.34%.
    Percent {
        name: String,
        basis_points: i64,
    },
    Date {
        name: String,
        value: Date,
    },
    Text {
        name: String,
        value: String,
    },
    /// An ordered list (top merchants, cancelled subscriptions).
    Texts {
        name: String,
        values: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub question: String,
    /// `totals`, `comparisons`, `merchants`, `recurring`, `time_ranges` or `hard`.
    #[serde(rename = "type")]
    pub kind: String,
    /// What makes it hard, or what it exercises (`refunds`, `transfers`, `relative_date`…).
    pub tags: Vec<String>,
    pub answers: Vec<Answer>,
    /// How the answer is meant to be read, when that isn't obvious.
    pub note: Option<String>,
    /// The structured query (T4.1) a perfect interpreter would write. `None` when the
    /// question can't be expressed (`null_reason` says why).
    pub expected_query: Option<fm_core::query::Query>,
    /// Set when the answers came from a ground-truth tag or series the ledger doesn't
    /// record, and the query reaches the same transactions another way. A model can't be
    /// expected to guess these, so accuracy is reported with and without them.
    pub proxy: Option<String>,
    /// Set when the query is right but doesn't give the answer yet, because of a known
    /// limit elsewhere (which, and why).
    pub known_gap: Option<String>,
    /// Why the question has no query.
    pub null_reason: Option<String>,
}

/// `questions.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionSet {
    pub version: u32,
    pub persona: String,
    pub today: Date,
    pub currency: String,
    /// The counting rules every answer follows.
    pub conventions: BTreeMap<String, String>,
    /// TD.1's manifest hash: the data the answers were computed from.
    pub synthetic_manifest_sha256: Sha256Hash,
    pub questions: Vec<Question>,
}

/// Bumped whenever a change alters the answers.
pub const VERSION: u32 = 1;

/// A total with the transactions behind it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sum {
    pub minor: i64,
    pub ids: Vec<i64>,
}

/// The persona's facts, and the queries questions are written with.
pub struct Book {
    pub facts: Vec<Fact>,
    pub truth: fm_synth::truth::PersonaTruth,
}

impl Book {
    fn select<'a>(
        &'a self,
        p: Period,
        f: impl Fn(&Fact) -> bool + 'a,
    ) -> impl Iterator<Item = &'a Fact> + 'a {
        self.facts
            .iter()
            .filter(move |x| x.counted() && p.contains(x.date()) && f(x))
    }

    /// Spending net of refunds: minus the sum of spending and refund amounts.
    pub fn spent(&self, p: Period, f: impl Fn(&Fact) -> bool) -> Sum {
        let mut s = Sum::default();
        for x in self.select(p, |x| x.is_spending()).filter(|x| f(x)) {
            s.minor -= x.amount;
            s.ids.push(x.id);
        }
        s
    }

    /// Plain sum of amounts of the matching transactions (any kind).
    pub fn total(&self, p: Period, f: impl Fn(&Fact) -> bool) -> Sum {
        let mut s = Sum::default();
        for x in self.select(p, f) {
            s.minor += x.amount;
            s.ids.push(x.id);
        }
        s
    }

    pub fn earned(&self, p: Period, f: impl Fn(&Fact) -> bool) -> Sum {
        self.total(p, |x| x.kind == Kind::Income && f(x))
    }

    pub fn count(&self, p: Period, f: impl Fn(&Fact) -> bool) -> Sum {
        let ids: Vec<i64> = self.select(p, f).map(|x| x.id).collect();
        Sum {
            minor: ids.len() as i64,
            ids,
        }
    }

    /// Spending grouped by a key (merchant, month…), largest first, ties by key.
    pub fn spent_by(
        &self,
        p: Period,
        f: impl Fn(&Fact) -> bool,
        key: impl Fn(&Fact) -> Option<String>,
    ) -> Vec<(String, Sum)> {
        let mut groups: BTreeMap<String, Sum> = BTreeMap::new();
        for x in self.select(p, |x| x.is_spending()).filter(|x| f(x)) {
            if let Some(k) = key(x) {
                let s = groups.entry(k).or_default();
                s.minor -= x.amount;
                s.ids.push(x.id);
            }
        }
        let mut v: Vec<(String, Sum)> = groups.into_iter().collect();
        v.sort_by(|a, b| b.1.minor.cmp(&a.1.minor).then(a.0.cmp(&b.0)));
        v
    }

    pub fn series(&self, id: &str) -> &fm_synth::truth::SeriesTruth {
        self.truth
            .recurring
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("no series {id}"))
    }

    pub fn fact(&self, id: i64) -> &Fact {
        self.facts.iter().find(|x| x.id == id).expect("fact exists")
    }
}

pub fn month_key(x: &Fact) -> Option<String> {
    let d = x.date();
    Some(format!("{:04}-{:02}", d.year(), d.month()))
}

pub fn usd(minor: i64) -> Money {
    Money::new(minor, fm_synth::sim::usd())
}

pub fn money(name: &str, s: &Sum, rule: Rule) -> Answer {
    Answer::Money {
        name: name.into(),
        amount: usd(s.minor),
        evidence: Evidence {
            rule,
            transaction_ids: s.ids.clone(),
        },
    }
}

/// Money out from a plain sum of negative amounts: the answer is minus the sum.
pub fn money_out(name: &str, s: &Sum) -> Answer {
    money(
        name,
        &Sum {
            minor: -s.minor,
            ids: s.ids.clone(),
        },
        Rule::NegatedSum,
    )
}

pub fn spending(name: &str, s: &Sum) -> Answer {
    money(name, s, Rule::NegatedSum)
}

pub fn derived(name: &str, minor: i64) -> Answer {
    Answer::Money {
        name: name.into(),
        amount: usd(minor),
        evidence: Evidence {
            rule: Rule::Derived,
            transaction_ids: Vec::new(),
        },
    }
}

pub fn count(name: &str, s: &Sum) -> Answer {
    Answer::Count {
        name: name.into(),
        value: s.minor,
        evidence: Evidence {
            rule: Rule::Count,
            transaction_ids: s.ids.clone(),
        },
    }
}

pub fn text(name: &str, v: impl Into<String>) -> Answer {
    Answer::Text {
        name: name.into(),
        value: v.into(),
    }
}

pub fn date(name: &str, d: Date) -> Answer {
    Answer::Date {
        name: name.into(),
        value: d,
    }
}

pub fn percent(name: &str, part: i64, whole: i64) -> Answer {
    Answer::Percent {
        name: name.into(),
        basis_points: facts::div_round(part * 10_000, whole),
    }
}

/// Builds the whole set from TD.1's committed data.
pub fn build() -> QuestionSet {
    let dir = facts::synthetic_data_dir();
    let (ledger, truth) = facts::load(&dir);
    let book = Book {
        facts: facts::facts(&ledger, &truth),
        truth,
    };
    let manifest = std::fs::read(dir.join("manifest.json")).expect("TD.1 manifest is readable");
    let mut questions = questions::all(&book);
    for (i, q) in questions.iter_mut().enumerate() {
        q.id = format!("q{:03}", i + 1);
        match queries::expected(&q.id) {
            queries::Expected::Query(query) => q.expected_query = Some(query),
            queries::Expected::Proxy(query, why) => {
                q.expected_query = Some(query);
                q.proxy = Some(why.to_string());
            }
            queries::Expected::KnownGap(query, why) => {
                q.expected_query = Some(query);
                q.known_gap = Some(why.to_string());
            }
            queries::Expected::Null(why) => q.null_reason = Some(why.to_string()),
        }
    }
    let conventions = [
        ("date_basis", match facts::DATE_BASIS {
            facts::DateBasis::PurchaseThenPosted => "purchase date when the bank gives one, else posted date",
            facts::DateBasis::Posted => "posted date",
        }),
        ("pending", if facts::COUNT_PENDING { "pending transactions count" } else { "pending transactions are left out of every total; only questions that ask about pending items include them" }),
        ("spending", "net of refunds and reimbursements in the same category; total spending is the sum over categories"),
        ("transfers", "never spending or income: card payments, moves between own accounts, money to or from own accounts outside the ledger"),
        ("income", "the income bucket only: paychecks, client payments, interest"),
        ("currency", "amounts in the account currency (USD); foreign-currency questions use the original amount the bank recorded"),
        ("today", "2026-10-01, fixed: last month = September 2026, this year = 2026"),
        ("periods", "calendar months and quarters; last N months = the N full months before the current month; last N days = the N days before today; weeks run Monday to Sunday"),
        ("rounding", "averages, medians and percentages round half away from zero, to the cent or basis point"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    QuestionSet {
        version: VERSION,
        persona: facts::PERSONA.to_string(),
        today: facts::today(),
        currency: facts::CURRENCY.to_string(),
        conventions,
        synthetic_manifest_sha256: Sha256Hash::of(&manifest),
        questions,
    }
}

/// `questions.json` bytes.
pub fn render() -> Vec<u8> {
    fm_synth::json::to_lines(&build())
}
