# views: spending views (T3.1)

Spending and income by category, group, merchant, month or account. Monthly series,
trends, and period-over-period changes. These are pure functions over a `Ledger`, with
no storage, no clock and no floats. T4.1 (structured queries) calls them; T3.3 (the
dashboard) renders their results. Every input and result is serde, so each can be
read or written as JSON.

| Function | Answers |
|---|---|
| `breakdown(ledger, query)` | One period, one row per key: "spending by category in March", "top 5 merchants this year" (`top: Some(5)`) |
| `monthly(ledger, query)` | One column per calendar month, every month present (zero-filled) |
| `trend(ledger, query)` | `monthly`, plus each row's average, median and latest value over the complete months |
| `compare(ledger, query, base)` | `query.period` against an earlier period, per key, with exact ratios |
| `month_over_month(ledger, query)` | `compare` with the month before; `query.period` must be one calendar month |

A `Query` has these fields:
- `period`: inclusive dates.
- `filter`: accounts, categories, groups, merchant and currency.
- `group_by`: category, group, merchant, month or account.
- `measure`: spending or income.
- `top`: optional; keeps the largest rows and adds up the rest.
- `as_of`: "today". It is passed in, never read from the clock.

## Counting rules (SPEC decision 42)

1. A transaction counts on its purchase date when the bank gives one, otherwise on its
   posted date.
2. Only posted transactions count. Pending transactions are reported on their own line
   (`totals.pending`) and are never part of a total.
3. Spending in a category is net of refunds filed to that category. It can be negative
   in a month where a refund lands after the purchase.
4. Transfers never count as spending or income; they get their own lines
   (`transfers_out`, `transfers_in`). Income is the Income bucket only.
5. Amounts are in the account's currency, and each result is in a single currency. If
   the accounts in scope hold more than one currency, the query must set
   `filter.currency`.

## What a number is built from

- **Bucket:** the transaction's one applied bucket decision. A transaction with none,
  or with more than one, counts as **Needs review** and is shown on that line.
- **Category:** a merged category counts toward the category it was merged into.
  Hidden categories still count.
- **Merchant:** the latest applied merchant decision. Without one, the raw description
  is used and the key is marked `from_description`.
- **Merchant filter:** compared after the rules engine's normalisation, so `"h-e-b"`
  matches `H-E-B`.
- **Traceability:** every row, line and change lists its transaction ids (ascending).
- **Identities** (checked in tests on every TD.1 month):
  - `net_flow = income + transfers_in − transfers_out − spending + needs_review`
  - the rows plus `other` add up to `total`
- **Partial months:** a month on or after `as_of`'s month is flagged
  (`partial_month`, `Column::partial`, `current_partial`/`base_partial`). So is a month
  the period covers only in part. Trend averages and medians use complete months only.
- **Rounding:** ratios (in basis points), averages and medians round half away from
  zero. Ratios also keep the exact fraction (`numerator`, `denominator`).

## Proof

```
cargo test -p fm-core --lib views -- --nocapture
```

- `views_match_td1_ground_truth_exactly`
  - Labels each TD.1 household from `truth.json`.
  - Folds the expected figures straight from the JSON files, without using this code:
    - category × month
    - group × month
    - merchant × year
    - income, refunds, transfers, pending and net flow per month
  - Every cell and count must match exactly. Prints one `T3.1-TD1` line per household.
- `views_reproduce_td3_answers`
  - Recomputes 36 of the freelancer's TD.3 answers through the views: totals, income,
    merchants, refunds, transfers, pending, month over month with the ratio,
    quarter-on-quarter, the top month and the top 3 merchants, an average, and the
    6-month median.
  - For every answer whose evidence is a list of transactions, the view's transaction
    ids must equal that list.

Errors carry ids and codes only, never amounts, merchants or descriptions. Test failure
messages follow the same rule.
