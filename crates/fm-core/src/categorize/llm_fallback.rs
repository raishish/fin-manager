//! The model fallback (T2.5): a local model categorises what the rules and transfer
//! detection left in Needs review.
//!
//! ```text
//! import → transfers::detect → categorize::run (rules) → llm_fallback::run_fallback
//! ```
//!
//! The model is a parameter, behind [`Chooser`]: fm-core never depends on llama.cpp.
//! `fm_llm::Session` is wrapped in a `Chooser` where the model is loaded (fm-cli, the
//! app), and tests use a scripted one.
//!
//! **What it may answer** (SPEC decision 45): a selectable category, `income`, or
//! `uncategorised`, never a transfer. The adapter enforces this at generation time with
//! a grammar (constraint 3); this module checks it again and refuses anything else.
//!
//! **What the answer means**, as for rules ([`super::rules::fit`]): a category on money
//! in is a refund of it; income on money out doesn't fit and stays in review.
//!
//! **Below the threshold** ([`FALLBACK_THRESHOLD_BP`]), or for `uncategorised`, the
//! model's decision is Needs review (constraint 5). It is still recorded, with the
//! model's identity and confidence, so the review queue shows the model looked and
//! wasn't sure, and a re-run with the same model and prompt doesn't change anything.
//!
//! **Every write goes through [`super::plan::Planner::propose`]**, so the model never
//! replaces the user's decisions, a rule's or fixed logic's (constraint 2), a decision
//! the user has seen gets a suggestion instead of a change (constraint 7), and what the
//! user dismissed or rejected never comes back.

use std::collections::HashMap;
use std::fmt;

use super::apply::{CategorizeError, Loaded, in_savepoint, write_plan};
use super::plan::{Kind, LedgerView, Plan, Planner, RunReport, Scope};
use crate::model::{
    Account, Assignment, Bucket, BuiltinLogic, Confidence, DecisionSource, ModelProvenance,
    Sha256Hash, Taxonomy, Timestamp, Transaction, TransactionId,
};
use crate::storage::Unlocked;

/// Version of the prompt below, recorded on every model decision.
pub const PROMPT_VERSION: &str = "categorise-v1";

/// Confidence, in basis points, below which a model answer stays in Needs review
/// (constraint 5). SPEC decision 48: with Qwen3.5-4B, 7.8% of the answers applied at or
/// above 0.80 are wrong, 2.8% of the whole ledger on TD.1 (categorize/README.md).
pub const FALLBACK_THRESHOLD_BP: u16 = 8000;

/// The answer for money that comes in as income (SPEC decision 45).
pub const INCOME: &str = "income";
/// The answer for "none of these, or not sure".
pub const UNCATEGORISED: &str = "uncategorised";

/// How many of the user's own corrections the prompt shows as examples.
const EXAMPLES: usize = 20;

/// Why the model couldn't answer. Never holds financial data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChooserError(pub String);

impl fmt::Display for ChooserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A model that picks one key from a closed list.
pub trait Chooser {
    /// Name and quantisation, recorded with every decision.
    fn model_id(&self) -> &str;
    /// SHA-256 of the exact model file, recorded with every decision.
    fn gguf_sha256(&self) -> [u8; 32];
    /// The instructions and examples shared by every transaction of a run, given once.
    fn begin(&mut self, instructions: &str) -> Result<(), ChooserError>;
    /// One transaction (see [`describe`]): one of `keys` and the confidence in basis
    /// points. Must answer only from `keys`.
    fn choose(&mut self, transaction: &str, keys: &[String])
    -> Result<(String, u16), ChooserError>;
}

/// Settings for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FallbackOptions<'a> {
    pub threshold_bp: u16,
    pub scope: Scope<'a>,
}

impl Default for FallbackOptions<'_> {
    fn default() -> Self {
        Self {
            threshold_bp: FALLBACK_THRESHOLD_BP,
            scope: Scope::All,
        }
    }
}

/// What a run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FallbackReport {
    /// Transactions the model was asked about.
    pub asked: usize,
    /// Answers at or above the threshold, written as a category, refund or income.
    pub categorised: usize,
    /// Answers kept in Needs review: below the threshold, `uncategorised`, or income
    /// for money going out.
    pub needs_review: usize,
    /// The planner's counts: applied, suggested (the user had seen the decision), or
    /// kept (a stronger decision, or something the user dismissed).
    pub run: RunReport,
}

/// The keys the model may answer (SPEC decision 45): the selectable categories, then
/// `income`. `uncategorised` is always offered by the adapter's closed list.
pub fn choices(taxonomy: &Taxonomy) -> Vec<String> {
    let mut keys: Vec<String> = taxonomy.selectable().map(|c| c.key.clone()).collect();
    keys.push(INCOME.to_string());
    keys
}

/// What an answer means for this transaction, or `None` for one that isn't allowed.
pub fn bucket_for(key: &str, taxonomy: &Taxonomy, t: &Transaction) -> Option<Bucket> {
    let money_in = t.amount.minor() > 0;
    Some(match key {
        UNCATEGORISED => Bucket::NeedsReview,
        INCOME if money_in => Bucket::Income,
        INCOME => Bucket::NeedsReview,
        key => {
            let category_id = taxonomy.selectable().find(|c| c.key == key)?.id;
            if money_in {
                Bucket::Refund {
                    category_id,
                    original: None,
                }
            } else {
                Bucket::Category { category_id }
            }
        }
    })
}

/// One transaction, as the model sees it: description, signed amount, date, account
/// kind.
pub fn describe(t: &Transaction, account: Option<&Account>) -> String {
    let kind = account
        .and_then(|a| serde_json::to_value(a.kind).ok())
        .and_then(|v| v.as_str().map(|s| s.replace('_', " ")))
        .unwrap_or_default();
    let direction = if t.amount.minor() < 0 {
        "money out"
    } else {
        "money in"
    };
    format!(
        "Description: {}\nAmount: {} {} ({direction})\nDate: {}\nAccount: {kind}\n",
        t.description,
        t.amount.to_decimal_string(),
        t.amount.currency().code(),
        t.transaction_date.unwrap_or(t.posted_date),
    )
}

/// The instructions and the user's own corrections as examples ([`PROMPT_VERSION`]).
pub fn instructions(
    view: LedgerView<'_>,
    accounts: &[Account],
    examples: &[(TransactionId, Bucket)],
) -> String {
    let taxonomy = view.taxonomy;
    let groups: HashMap<_, _> = taxonomy
        .groups
        .iter()
        .map(|g| (g.id, g.name.as_str()))
        .collect();
    let mut out = String::from(
        "You categorise one bank transaction at a time for a personal finance app.\n\
         Answer with exactly one key from the list below. If none fits, or you are not \
         sure, answer \"uncategorised\": a person will look at it. A wrong answer is worse \
         than \"uncategorised\".\n\n\
         Conventions:\n\
         - Salary, payroll, client and platform payouts (Stripe, Upwork), interest earned, \
         pensions and Social Security coming in are income.\n\
         - Money coming back from a merchant is a refund: answer the category of what was \
         bought.\n\
         - Someone paying the user back for a shared cost (Zelle, Venmo) is a refund of \
         that cost's category.\n\
         - ATM cash is financial.cash_withdrawal; interest charged on a card is \
         financial.interest; foreign-transaction and ATM fees are financial.bank_fees.\n\
         - Mortgage or car-loan payments to a lender are housing.mortgage or \
         transport.auto_loan.\n\
         - A tax refund is taxes.income.\n\
         - Paper checks without a memo are usually \"uncategorised\".\n\n\
         Keys:\n",
    );
    for c in taxonomy.selectable() {
        out.push_str(&format!(
            "{}: {} ({})\n",
            c.key,
            c.name,
            groups.get(&c.group_id).copied().unwrap_or("")
        ));
    }
    out.push_str(&format!(
        "{INCOME}: Income: salary, interest, pensions, Social Security, client payments\n\
         {UNCATEGORISED}: none of these, or not sure\n"
    ));
    let by_id: HashMap<_, _> = view.transactions.iter().map(|t| (t.id, t)).collect();
    let account: HashMap<_, _> = accounts.iter().map(|a| (a.id, a)).collect();
    let shown: Vec<String> = examples
        .iter()
        .filter_map(|(id, bucket)| {
            let t = by_id.get(id)?;
            let key = match *bucket {
                Bucket::Category { category_id } | Bucket::Refund { category_id, .. } => {
                    taxonomy.category(category_id)?.key.clone()
                }
                Bucket::Income => INCOME.to_string(),
                _ => return None,
            };
            Some(format!(
                "{}Answer: {key}\n",
                describe(t, account.get(&t.account_id).copied())
            ))
        })
        .collect();
    if !shown.is_empty() {
        out.push_str("\nExamples the user has already categorised:\n\n");
        out.push_str(&shown.join("\n"));
    }
    out
}

/// The user's most recent bucket corrections, oldest first, as prompt examples.
pub fn user_examples(view: LedgerView<'_>) -> Vec<(TransactionId, Bucket)> {
    let mut corrections: Vec<_> = view
        .decisions
        .iter()
        .filter(|d| {
            d.source == DecisionSource::User && d.state == crate::model::DecisionState::Applied
        })
        .filter_map(|d| match d.assignment {
            Assignment::Bucket(b) => Some((d.id, d.transaction_id, b)),
            Assignment::Merchant(_) => None,
        })
        .collect();
    corrections.sort_by_key(|(id, _, _)| *id);
    let skip = corrections.len().saturating_sub(EXAMPLES);
    corrections
        .into_iter()
        .skip(skip)
        .map(|(_, t, b)| (t, b))
        .collect()
}

/// Whether the model may decide this transaction's bucket: it is in review (the
/// placeholder) or a model decided it before. Everything else is stronger.
fn candidate(applied: Option<&crate::model::Decision>) -> bool {
    match applied.map(|d| &d.source) {
        None => true,
        Some(DecisionSource::Model(_)) => true,
        Some(DecisionSource::Builtin {
            logic: BuiltinLogic::ImportPipeline,
            ..
        }) => true,
        Some(_) => false,
    }
}

/// Plans the model's decisions without writing them. Calls the model once per
/// candidate transaction in `scope`.
pub fn plan_fallback(
    view: LedgerView<'_>,
    accounts: &[Account],
    chooser: &mut dyn Chooser,
    options: &FallbackOptions<'_>,
    now: Timestamp,
) -> Result<(Plan, FallbackReport), CategorizeError> {
    let keys = choices(view.taxonomy);
    let mut allowed = keys.clone();
    allowed.push(UNCATEGORISED.to_string());
    let account: HashMap<_, _> = accounts.iter().map(|a| (a.id, a)).collect();

    let mut planner = Planner::new(view, now);
    let selected: Vec<&Transaction> = match options.scope {
        Scope::All => view.transactions.iter().collect(),
        Scope::Transactions(ids) => view
            .transactions
            .iter()
            .filter(|t| ids.contains(&t.id))
            .collect(),
    };
    let targets: Vec<&Transaction> = selected
        .into_iter()
        .filter(|t| candidate(planner.current().applied(t.id, Kind::Bucket)))
        .collect();
    let mut report = FallbackReport::default();
    if targets.is_empty() {
        return Ok((planner.finish(), report));
    }

    chooser
        .begin(&instructions(view, accounts, &user_examples(view)))
        .map_err(|e| CategorizeError::Model(e.0))?;
    for t in targets {
        let (key, confidence_bp) = chooser
            .choose(&describe(t, account.get(&t.account_id).copied()), &keys)
            .map_err(|e| CategorizeError::Model(e.0))?;
        // Constraint 3, checked again here: only a listed key is accepted.
        if !allowed.contains(&key) {
            return Err(CategorizeError::Model("an answer outside the list".into()));
        }
        let confidence = Confidence::from_basis_points(confidence_bp)
            .map_err(|_| CategorizeError::Model("a confidence above 10000".into()))?;
        let mut bucket =
            bucket_for(&key, view.taxonomy, t).ok_or(CategorizeError::UnknownCategory)?;
        // Constraint 5: not sure enough is Needs review.
        if confidence_bp < options.threshold_bp {
            bucket = Bucket::NeedsReview;
        }
        report.asked += 1;
        if bucket == Bucket::NeedsReview {
            report.needs_review += 1;
        } else {
            report.categorised += 1;
        }
        let source = DecisionSource::Model(ModelProvenance {
            model_id: chooser.model_id().to_string(),
            gguf_sha256: Sha256Hash::from_bytes(chooser.gguf_sha256()),
            prompt_version: PROMPT_VERSION.to_string(),
            confidence,
        });
        planner.propose(t.id, source, Assignment::Bucket(bucket));
    }
    let plan = planner.finish();
    report.run = plan.report;
    Ok((plan, report))
}

/// Runs the fallback over the stored ledger: plans with `chooser`, then writes in one
/// savepoint.
pub fn run_fallback(
    unlocked: &mut Unlocked,
    chooser: &mut dyn Chooser,
    options: &FallbackOptions<'_>,
    now: Timestamp,
) -> Result<FallbackReport, CategorizeError> {
    let data = Loaded::from(unlocked)?;
    let accounts = unlocked.accounts()?;
    let (plan, report) = plan_fallback(data.view(), &accounts, chooser, options, now)?;
    in_savepoint(unlocked, |u| write_plan(u, &plan, &[]))?;
    Ok(report)
}
