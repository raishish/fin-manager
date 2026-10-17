# Question test set (TD.3)

100 plain-English questions about one synthetic household's money, each with its expected answer(s). Question answering (T4.2, T4.3) is measured against it; the Stage 1 bar is 85% correct (SPEC decision 22).

The persona is TD.1's **freelancer**: 24 months of a self-employed designer in Austin (`testdata/synthetic/data/freelancer`). This household has the cases that trip up question answering:
- refunds that offset spending
- money moved to savings and the tax set-asides, card payments and a retirement contribution, none of which is spending
- euro purchases with foreign-transaction fees
- card interest from lean months
- irregular income
- subscriptions that rise in price, get cancelled and restart

**Every answer is computed by `src/questions.rs` from TD.1's committed ledger and ground truth.** None is typed by hand.

## Running it

A standalone Cargo workspace, like the other test-data folders:

```sh
cargo run --release              # regenerate data/questions.json
cargo run --release -- --check   # exit 1 if the committed file is out of date
cargo test --release             # the checks below
```

## Counting conventions

These rules decide what the right answer is. The spending views (T3.1) and question answering (T4.x) must follow the same rules. They are recorded in `questions.json` under `conventions`.

| Rule | Value | |
|---|---|---|
| Date basis | The purchase date when the bank gives one, otherwise the posted date | **Pending the user's decision** (coordinator's recommendation; `DATE_BASIS` in `src/facts.rs`) |
| Pending transactions | Left out of every total. Only the question about pending items includes them | **Pending the user's decision** (coordinator's recommendation; `COUNT_PENDING`) |
| Spending | Net of refunds and reimbursements in the same category. Total spending is the sum over categories | Approved |
| Transfers | Never spending or income: card payments, moves between own accounts, money to or from own accounts outside the ledger | Approved |
| Income | The income bucket only: paychecks, client payments, interest | Approved |
| Currency | Amounts in USD, the account currency. Foreign-currency questions use the original amount the bank recorded | Approved |
| Today | 2026-10-01, fixed. Last month = September 2026; this year = 2026 | Fixture choice |
| Periods | Calendar months and quarters. Last N months = the N full months before the current month. Last N days = the N days before today. Weeks run Monday to Sunday | Fixture choice |
| Rounding | Averages, medians and percentages round half away from zero, to the cent or basis point | Fixture choice |

If the user decides the first two rules differently, change the constant and run `cargo run --release`. Every answer is recomputed.

## `data/questions.json`

```json
{"version": 1, "persona": "freelancer", "today": "2026-10-01", "currency": "USD",
 "conventions": {"date_basis": "…", "pending": "…", …},
 "synthetic_manifest_sha256": "…",
 "questions": [
   {"id": "q001", "question": "How much did I spend in total in 2025?", "type": "totals", "tags": [],
    "answers": [{"kind": "money", "name": "total", "amount": {"minor": 8129956, "currency": "USD"},
                 "evidence": {"rule": "negated_sum", "transaction_ids": [ … ]}}],
    "note": null, "expected_query": null}
 ]}
```

- `type` is one of `totals`, `comparisons`, `merchants`, `recurring`, `time_ranges` or `hard`. There are 20 totals, 15 each of the next four, and 20 hard.
- `tags` say what a question exercises: `refunds`, `transfers`, `multi_currency`, `relative_date`, `pending`, `one_off`, `income`, `interest`, `median`, `percent` and others.
- `answers` holds one or more named values:

  | `kind` | Value |
  |---|---|
  | `money` | `amount` in minor units and a currency (USD, or EUR for questions asked in euros). Spending is positive. |
  | `count` | `value` |
  | `percent` | `basis_points` (1234 = 12.34%) |
  | `date` | `value`, `YYYY-MM-DD` |
  | `text` | `value`. Months are written `YYYY-MM`; merchants by their clean name. |
  | `texts` | `values`: an ordered list (top merchants) or a sorted one (cancelled subscriptions, months) |

  Comparisons give both sides and a `difference` (first minus second). Some also give a percent `change` relative to the second.
- `evidence` lists the TD.1 transactions a sum or count is made of, and the `rule` that adds them up:

  | `rule` | Meaning |
  |---|---|
  | `negated_sum` | Spending or money out: minus the sum of the transactions' amounts |
  | `sum` | Income, refunds, money in |
  | `negated_original_sum` | The euro amounts the bank recorded |
  | `count` | How many transactions |
  | `derived` | An average, median, difference or current price, with no list |

  The ids join with `testdata/synthetic/data/freelancer/truth.json`, so a wrong answer can be traced to the transactions it should have counted.
- `note` says how to read the answer where that isn't obvious, for example "Card payments are transfers and don't count".
- `expected_query` is `null` until T4.1 defines the structured query type. Then each question gets the query that should produce its answer.

## Checks (`tests/questions.rs`)

- **Freshness.** The committed file is byte-identical to a fresh build, and building is deterministic.
- **Shape.**
  - 100 questions, with no repeated question text.
  - At least 15 of each type.
  - The hard-case tags are all present.
  - `expected_query` is empty for now.
- **Evidence.** Every `money` and `count` answer that has evidence (117 of them) is re-added straight from TD.1's ledger, without the generator's code, and must match. Every comparison's `difference` must be the first value minus the second.
- **An independent recount.** 2025 spending and September 2026 spending are recomputed from the raw ledger and truth with the conventions written out again in the test. The results must equal the answers, and the twelve 2025 months must add up to the year.
- **The conventions hold.**
  - No pending transaction is in any total, except the pending question.
  - No transfer or income transaction counts as spending, except in questions that ask about transfers.
  - The food group's total equals its categories added up.

Mutation-checked: making refunds not offset spending fails the independent recount.

## Changing the set

1. Edit `src/questions.rs`.
2. Bump `VERSION` in `src/lib.rs` if answers change.
3. Run `cargo run --release`, then `cargo test --release`.
4. Commit the code and `data/questions.json` together.

If TD.1's data changes, regenerate here too. The freshness test fails until you do.
