//! The prompt: what the query language means, the user's own categories and accounts,
//! today's date, and the worked examples. Versioned by [`PROMPT_VERSION`].

use std::fmt::Write as _;

use super::examples;
use super::gbnf::canonical;
use super::{Request, Vocabulary};

/// Bumped whenever the prompt or the examples change, and recorded with every
/// interpretation, so an accuracy figure says which prompt it was measured on.
pub const PROMPT_VERSION: &str = "nlq-6";

const LANGUAGE: &str = r#"You turn a question about the user's own money into a JSON query. Write only the query.

A query is {"items": [...]}: one item per thing the answer needs, each {"name": "<snake_case>", "ask": {...}}.

Asks ("op"):
- sum: the measure added up. count: how many transactions.
- average / median, "per": "month" (complete months only) or "transaction".
- rank: group "by" category|group|merchant|month|account, "order" highest|lowest, "metric" amount|count (default amount); "output" labels (the top "n") or label|amount|count (at "position", default 1).
- keys: every group's label. distinct: how many different groups.
- pick: one transaction, "order" largest|smallest|latest|earliest, "output" merchant|amount|date.
- recurring: series of subscriptions, bills, income; "select" by name, categories, kinds (subscription|bill|income|interest|transfer), cadences (monthly, annual, ...), status (active|ended|cancelled|any), period; "output" names|count|latest|latest_total|price_from|price_before|price_after|member_count|member_total.
- derive: "formula" difference (a - b)|sum|percent_change ((a - b) / b)|share (a / b)|larger (name of the larger), "of": [item names].

Measures: spending (net of refunds), purchases (before refunds; "times I bought"), refunds, income, transfers_in, transfers_out, money_in and money_out (every amount into or out of an account), pending. Amounts out and in are both positive.

Scope (every field narrows): "period" (always), categories, groups, merchant, accounts {"kinds": [...]} or {"ids": [...]}, counterpart_accounts (where a transfer went), description (words in the bank's text), original_currency with "amounts": "original" for money abroad.

Periods: {"year": 2025}, {"month": "2025-07"}, {"months": {"from": "2025-01", "to": "2025-06"}}, {"quarter": {"year": 2025, "quarter": 4}}, {"range": {"from": "2025-11-28", "to": "2025-12-25"}}; relative to today: {"last_months": n} (full months before this one), {"last_days": n}, "last_week" (Monday to Sunday), "last_quarter", "year_to_date", "all_time".

Rules:
- Transfers between the user's accounts and card payments are never spending or income.
- A question with several parts ("and", "or", "compared with", "which ... and how much") needs an item for each part. Write the plain items first and derive items last, after the items they use.
- Give everything a full answer shows, one item each:
  - comparisons: both sides, the difference (first minus second) and the percent change; for a change over time, the later period comes first;
  - "which"/"who"/"top" questions: the label(s), then the amount of each;
  - "what do my ... cost" about recurring payments: the names and the total;
  - "how many ... and how much": the count and the sum;
  - one transaction: the merchant, the amount and the date that the question asks about.
- Words like "monthly", "yearly", "running now", "cancelled" select cadences and status of recurring payments.
- original_currency only when the question is about money spent in a foreign currency or abroad.
- "On average per ride / bill / visit / purchase" is an average per transaction; "typical" is a median.
- What a subscription or merchant cost "in total" or "altogether" is the sum spent there over all time.
- Use a relative period for "last month", "this year", "so far", "lately"."#;

fn vocabulary_text(v: &Vocabulary) -> String {
    let mut out = String::from("The user's categories (group: key = name):\n");
    for g in &v.groups {
        let cats: Vec<String> = v
            .categories
            .iter()
            .filter(|c| c.group == g.key)
            .map(|c| format!("{} = {}", c.key, c.name))
            .collect();
        let _ = writeln!(out, "- {} ({}): {}", g.key, g.name, cats.join("; "));
    }
    out.push_str("The user's accounts:\n");
    for a in &v.accounts {
        let _ = writeln!(out, "- id {}: {} ({})", a.id, a.name, a.kind);
    }
    out
}

/// The system part of the prompt.
pub fn system(request: &Request<'_>) -> String {
    let mut out = String::from(LANGUAGE);
    let _ = write!(
        out,
        "\n\nToday is {} ({}).\n\n",
        request.as_of, request.weekday
    );
    out.push_str(&vocabulary_text(request.vocabulary));
    out.push_str("\nExamples:\n");
    for e in examples::all() {
        let _ = writeln!(out, "Q: {}\nA: {}", e.question, canonical(&e.query));
    }
    out
}
