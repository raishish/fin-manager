//! The categorisation prompt (version [`PROMPT_VERSION`]) and the choice sets compared.
//!
//! A prompt is split for [`fm_llm::Session`]: the **prefix** (instructions, the choice
//! list, the household's own corrections as examples) is the same for every
//! transaction of a household; the **suffix** is one transaction.

use std::collections::HashMap;

use fm_core::model::{Account, Bucket, Ledger, Taxonomy, Transaction, TransactionId};
use fm_llm::{Choices, UNCATEGORISED};

pub const PROMPT_VERSION: &str = "t2.4-categorise-v1";

/// Which answers the model may give besides the categories.
///
/// SPEC decision 45: the categoriser chooses from the categories, Income and
/// `uncategorised`, never Transfer (transfers come only from transfer detection, the
/// user's rules and corrections). [`Variant::WithIncome`] is that rule; categories only
/// is reported beside it to show what Income adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    /// SPEC decision 45: the categories, `income` and `uncategorised`.
    WithIncome,
    /// The categories and `uncategorised` only.
    Categories,
}

pub const VARIANTS: [Variant; 2] = [Variant::WithIncome, Variant::Categories];

pub const INCOME: &str = "income";

impl Variant {
    pub fn name(self) -> &'static str {
        match self {
            Self::WithIncome => "categories+income",
            Self::Categories => "categories",
        }
    }

    pub fn from_name(name: &str) -> Option<Variant> {
        VARIANTS.into_iter().find(|v| v.name() == name)
    }

    fn extras(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Categories => &[],
            Self::WithIncome => &[(
                INCOME,
                "Income: salary, interest, pensions, Social Security, client payments",
            )],
        }
    }

    /// The closed list for the grammar: selectable category keys, then the extras.
    pub fn choices(self, taxonomy: &Taxonomy) -> Choices {
        let mut keys: Vec<String> = taxonomy.selectable().map(|c| c.key.clone()).collect();
        keys.extend(self.extras().iter().map(|(k, _)| (*k).to_string()));
        Choices::new(keys).expect("category keys are distinct and never `uncategorised`")
    }

    /// What an answer means for this transaction, as rules::fit does for rules (SPEC
    /// decision 45): a category on money in is a refund of it; income on money out
    /// doesn't fit and stays in review.
    pub fn bucket(self, key: &str, taxonomy: &Taxonomy, t: &Transaction) -> Bucket {
        let money_in = t.amount.minor() > 0;
        match key {
            UNCATEGORISED => Bucket::NeedsReview,
            INCOME if money_in => Bucket::Income,
            INCOME => Bucket::NeedsReview,
            key => {
                let category_id = taxonomy
                    .categories
                    .iter()
                    .find(|c| c.key == key)
                    .expect("the grammar only allows listed keys")
                    .id;
                if money_in {
                    Bucket::Refund {
                        category_id,
                        original: None,
                    }
                } else {
                    Bucket::Category { category_id }
                }
            }
        }
    }

    /// The answer key a truth bucket corresponds to under this variant, for examples.
    /// `None` when the variant has no key for it (then the example is left out).
    fn key_for(self, bucket: Bucket, taxonomy: &Taxonomy) -> Option<String> {
        let key_of = |id| {
            taxonomy
                .categories
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.key.clone())
        };
        match bucket {
            Bucket::Category { category_id } | Bucket::Refund { category_id, .. } => {
                key_of(category_id)
            }
            Bucket::Income if self == Self::WithIncome => Some(INCOME.into()),
            _ => None,
        }
    }
}

/// The instructions and choice list, without examples.
fn instructions(variant: Variant, taxonomy: &Taxonomy) -> String {
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
         financial.interest; \
         foreign-transaction and ATM fees are financial.bank_fees.\n\
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
    for (key, meaning) in variant.extras() {
        out.push_str(&format!("{key}: {meaning}\n"));
    }
    out.push_str(&format!("{UNCATEGORISED}: none of these, or not sure\n"));
    out
}

/// One transaction, as the model sees it.
pub fn describe(t: &Transaction, account: Option<&Account>) -> String {
    let kind = account
        .map(|a| {
            serde_json::to_value(a.kind)
                .ok()
                .and_then(|v| v.as_str().map(|s| s.replace('_', " ")))
                .unwrap_or_default()
        })
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

/// The shared prefix and a function making each transaction's suffix, in the model's
/// chat format. `examples` are the household's own corrections.
pub struct Prompt {
    pub prefix: String,
    /// What the chat template puts after the user's text (the assistant's turn).
    tail: String,
    /// Put before the answer, e.g. an empty thinking block for thinking models.
    answer_lead: &'static str,
}

impl Prompt {
    pub fn new(
        model: &fm_llm::Model,
        variant: Variant,
        ledger: &Ledger,
        examples: &[(TransactionId, Bucket)],
        answer_lead: &'static str,
    ) -> Prompt {
        let accounts: HashMap<_, _> = ledger.accounts.iter().map(|a| (a.id, a)).collect();
        let by_id: HashMap<_, _> = ledger.transactions.iter().map(|t| (t.id, t)).collect();
        let mut system = instructions(variant, &ledger.taxonomy);
        let shown: Vec<String> = examples
            .iter()
            .filter_map(|(id, bucket)| {
                let t = by_id.get(id)?;
                let key = variant.key_for(*bucket, &ledger.taxonomy)?;
                Some(format!(
                    "{}Answer: {key}\n",
                    describe(t, accounts.get(&t.account_id).copied())
                ))
            })
            .collect();
        if !shown.is_empty() {
            system.push_str("\nExamples the user has already categorised:\n\n");
            system.push_str(&shown.join("\n"));
        }
        const MARK: &str = "\u{1}";
        let full = model
            .chat_prompt(&system, MARK)
            .expect("the model has a chat template");
        let (prefix, tail) = full
            .split_once(MARK)
            .expect("the user text is in the prompt");
        Prompt {
            prefix: prefix.to_string(),
            tail: tail.to_string(),
            answer_lead,
        }
    }

    /// The suffix for one transaction: its description and the assistant's turn.
    pub fn suffix(&self, t: &Transaction, account: Option<&Account>) -> String {
        format!(
            "Categorise this transaction.\n{}{}{}",
            describe(t, account),
            self.tail,
            self.answer_lead
        )
    }
}
