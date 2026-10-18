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
| Date basis | The purchase date when the bank gives one, otherwise the posted date | SPEC decision 42 (`DATE_BASIS` in `src/facts.rs`) |
| Pending transactions | Left out of every total. Only the question about pending items includes them | SPEC decision 42 (`COUNT_PENDING`) |
| Spending | Net of refunds and reimbursements in the same category. Total spending is the sum over categories | SPEC decision 42 |
| Transfers | Never spending or income: card payments, moves between own accounts, money to or from own accounts outside the ledger | SPEC decision 42 |
| Income | The income bucket only: paychecks, client payments, interest | SPEC decision 42 |
| Currency | Amounts in USD, the account currency. Foreign-currency questions use the original amount the bank recorded | SPEC decision 42 |
| Today | 2026-10-01, fixed. Last month = September 2026; this year = 2026 | Fixture choice |
| Periods | Calendar months and quarters. Last N months = the N full months before the current month. Last N days = the N days before today. Weeks run Monday to Sunday | Fixture choice |
| Rounding | Averages, medians and percentages round half away from zero, to the cent or basis point | Fixture choice |

To change a rule, change its constant and run `cargo run --release`. Every answer is recomputed.

## `data/questions.json`

```json
{"version": 1, "persona": "freelancer", "today": "2026-10-01", "currency": "USD",
 "conventions": {"date_basis": "…", "pending": "…", …},
 "synthetic_manifest_sha256": "…",
 "questions": [
   {"id": "q001", "question": "How much did I spend in total in 2025?", "type": "totals", "tags": [],
    "answers": [{"kind": "money", "name": "total", "amount": {"minor": 8129956, "currency": "USD"},
                 "evidence": {"rule": "negated_sum", "transaction_ids": [ … ]}}],
    "note": null,
    "expected_query": {"items": [{"name": "total", "ask": {"op": "sum", "measure": "spending",
                                  "scope": {"period": {"year": 2025}, …}}}]},
    "proxy": null, "known_gap": null, "null_reason": null}
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
- `expected_query` is the structured query (T4.1, `fm_core::query`) a perfect interpreter would write. It has one item per answer, with the same names. The queries are written in `src/queries.rs` and generated into the file like the answers. 98 questions have one:
  - `proxy` (9 questions: q044, q058, q059, q060, q064, q090, q094, q097, q100) says why the query is a stand-in. The answers came from a ground-truth tag or series the ledger doesn't record (the trip, the fee lines, the set-asides), and the query reaches the same transactions another way. A model can't be expected to guess these, so T4.2 reports accuracy with and without them.
  - `known_gap` says why a correct query doesn't give the answer yet, because of a known limit elsewhere. None does today: q062 and q065 had one until T3.4 taught recurring detection the US estimated-tax calendar.
- `null_reason` is set on the two questions with no query (q089, the Lisbon trip, and q093, without one-offs): the ledger has no trip tags or one-off marks yet.

## Checks (`tests/questions.rs`)

- **Freshness.** The committed file is byte-identical to a fresh build, and building is deterministic.
- **Shape.**
  - 100 questions, with no repeated question text.
  - At least 15 of each type.
  - The hard-case tags are all present.
  - Every question has a query or a reason for none.
- **Evidence.** Every `money` and `count` answer that has evidence (117 of them) is re-added straight from TD.1's ledger, without the generator's code, and must match. Every comparison's `difference` must be the first value minus the second.
- **An independent recount.** 2025 spending and September 2026 spending are recomputed from the raw ledger and truth with the conventions written out again in the test. The results must equal the answers, and the twelve 2025 months must add up to the year.
- **The conventions hold.**
  - No pending transaction is in any total, except the pending question.
  - No transfer or income transaction counts as spending, except in questions that ask about transfers.
  - The food group's total equals its categories added up.

Mutation-checked: making refunds not offset spending fails the independent recount.

## The expected queries (`tests/answers.rs`)

- **Every expected query gives its answers.**
  - Each query runs through `fm_core::query::run` on the freelancer's ledger, labelled from truth, as of 2026-10-01.
  - Every answer must match exactly, and so must its evidence ids for sums and counts.
  - Recurring questions name their series through a labeler that reads truth. It stands in for the model behind T3.2's `Labeler` hook.
  - The test prints `T4.1-TD3 queries=98 exact=98 known_gaps=0 nulls=2 proxies=9`.
- **Known gaps live in one place**, `KNOWN_GAPS` at the top of the test (empty today). Each entry pins what its query gives, so the test fails if that changes, until the limit is fixed and the entry deleted.
- **Every query round-trips through JSON.**
- fm-core's `query::schema_tests` also validates every expected query against `schema.json`.

## Question answering with the local model (`nl-eval`, T4.2)

`src/bin/nl-eval.rs` measures T4.2:
- Each question goes through the local model (fm-llm's `nl_query`).
- The query the model writes runs on the freelancer's ledger, labelled from truth (`src/labelled.rs`), as of 2026-10-01.
- The answers are compared with this set's.

It sits behind the `nl-eval` feature, so the default build and CI never compile llama.cpp here.

```sh
cargo run --release --features nl-eval --bin nl-eval -- \
  --model <file.gguf> --sha256 <pinned hex> --id <model id> \
  [--lead '<think>\n\n</think>\n\n'] [--only q001,q022] [--out nl-eval/<model id>.md]
```

- `--sha256` is required: the file is verified before it's loaded.
- `--lead` is the text a thinking model's answer starts with (Qwen3.5 takes an empty think block).

**A question is correct** when:
- every expected answer is matched, one to one, by an answer of the model's query with the same value. Money must also match in currency; counts, percents, dates and texts must match exactly. Names and order don't matter. Extra answers are allowed but counted (a query has at most 6 items).
- **and** no item of the model's query covers a period disjoint from every period the expected query covers.

Special cases:
- **Nulls** (q089, q093): wrong unless matched anyway.

**The report** gives:
- correct out of 100;
- correct without the 9 proxies;
- correct per type;
- each failure class: wrong value, stray period, run error, no parse, no answer;
- the extra-item rate;
- whether evidence ids agree;
- confidence when right and when wrong;
- time per question;
- every failure with the model's query and its plain-English reading.

Reports are kept in `nl-eval/`.

## Changing the set

1. Edit `src/questions.rs`.
2. Bump `VERSION` in `src/lib.rs` if answers change.
3. Run `cargo run --release`, then `cargo test --release`.
4. Commit the code and `data/questions.json` together.

If TD.1's data changes, regenerate here too. The freshness test fails until you do.
