# query: structured queries (T4.1)

A typed description of a question, a JSON schema for it, and an executor over the ledger.

```text
question → T4.2 model (constrained by schema.json) → Query → run(ledger, query, ctx) → answers
                                                       ↑ shown to the user as interpreted
```

```rust
let answers = query::run(&ledger, &query, &Context { as_of, labeler: Some(&model) })?;
```

## Shape

A `Query` is a list of named **items**, and each item gives exactly **one answer**:
money, a count, a text, a list of texts, a date or a percent.
- Money and counts carry the transaction ids behind them, ascending. They're empty for
  derived values.
- T4.3 checks every number against the ledger (SPEC constraint 6), and the UI shows
  each answer's sources.
- TD.3 answers map onto items one to one, by name.

```json
{"items": [
  {"name": "september_2026", "ask": {"op": "sum", "measure": "spending", "scope": {"period": {"month": "2026-09"}}}},
  {"name": "august_2026",    "ask": {"op": "sum", "measure": "spending", "scope": {"period": {"month": "2026-08"}}}},
  {"name": "change",         "ask": {"op": "derive", "formula": "percent_change", "of": ["september_2026", "august_2026"]}}
]}
```

| `op` | Gives | |
|---|---|---|
| `sum` | money | The measure added up |
| `count` | count | How many transactions the measure counts |
| `average` | money | `per`: `transaction` (a refund lowers the total but isn't an occurrence) or `month` (complete months only) |
| `median` | money | `per`: `transaction` or `month` (complete months only); the mean of the middle two for an even count |
| `rank` | texts, text, money or count | Groups `by` category, group, merchant, month or account, ordered `highest`/`lowest` by `metric` (`amount` or `count`). Reports the top `n` labels, or the label, amount or count at `position` (1-based). Ties go to the label that sorts first |
| `keys` | texts | Every group's label: months in calendar order, the others largest first |
| `distinct` | count | How many different groups |
| `pick` | text, money or date | One transaction: `largest`, `smallest`, `latest` or `earliest`, reported as its merchant, amount or date. Ties go to the earlier id, except `latest`, which takes the later |
| `recurring` | (by `output`) | T3.2's series; see below |
| `derive` | money, count, percent or text | `difference` (a − b), `sum`, `percent_change` ((a − b) ÷ \|b\|), `share` (a ÷ b) or `larger` (the name of the larger item). `of` names other items, before or after this one |

**Measures.** Money out and money in are both reported as positive amounts.

| `measure` | |
|---|---|
| `spending` | Spending net of refunds (SPEC decision 42) |
| `purchases` | Purchases before refunds; also "how many times did I buy" |
| `refunds` | Refunds |
| `income` | The Income bucket only |
| `transfers_in`, `transfers_out` | Transfers, never spending or income |
| `money_in`, `money_out` | Every posted amount into or out of the account(s), whatever its bucket |
| `pending` | Pending transactions, as money out. Never part of any other measure |

**Scope.** Every field narrows the transactions counted.
- `period`: see below.
- `categories`, `groups`: by **key** (`food.groceries`, `food`), so a model's query means the same in every install.
- `merchant`: compared after the rules engine's normalisation, so `h-e-b` matches `H-E-B`.
- `accounts`: by kind and/or id.
- `counterpart_accounts`: transfers whose other side is on those accounts, for example "moved from checking to savings".
- `description`: words that must appear together in the bank's description, on word boundaries.
- `original_currency` and `amounts: original`: a purchase abroad, in the currency the bank recorded it in.

**Periods.**
- **Absolute:** `{"range": {"from", "to"}}`, `{"month": "2025-07"}`, `{"months": {"from", "to"}}`, `{"quarter": {"year", "quarter"}}`, `{"year": 2025}`.
- **Relative to `as_of`:**
  - `{"last_months": n}`: the n full months before the current one.
  - `{"last_days": n}`: the n days before today.
  - `"last_week"`: Monday to Sunday.
  - `"last_quarter"`.
  - `"year_to_date"`.
  - `"all_time"`: from the ledger's first transaction.
- `as_of` comes from the app in the `Context`, never from the model or the clock.

**Recurring.**
- `select` narrows the series by:
  - `name`: the label, else the merchant key, normalised;
  - `categories`: more than half the members in them;
  - `kinds`: subscription, bill, income, interest, transfer;
  - `cadences`;
  - `status`: `active`, `ended`, `cancelled` or `any`. Cancelled means ended with no running series of the same name, so a cancel-and-restart isn't counted.
- `output` is one of:
  - `names` (sorted), `count`;
  - `latest`: the most recent matching series' latest amount;
  - `latest_total`: the latest amounts added up ("what my subscriptions cost now");
  - `price_from`, `price_before`, `price_after`: the latest price change;
  - `member_count`, `member_total`: the members in `select.period`.
- Kinds come from the app's labeler when it has one (T3.2's `Labeler` hook). A charge the labeler didn't name is then neither a subscription nor a bill. Without a labeler, fixed logic can't tell them apart, so a charge counts as both.

## How it counts

- **Decision 42 lives in the views.**
  - Each item asks `views::breakdown` which transactions in the period are spending, refunds, income, transfers, Needs review or pending: dates (purchase, else posted), posted only, merges and account currency, all as T3.1 counts them.
  - This module only narrows further: description, original currency and transfer counterpart. It also signs each amount for the measure.
- **Recurring series** come from `recurring::detect`, read-only, at `as_of`.
- **No floats.** Averages, medians and percents (basis points) round half away from zero.
- **Errors** (`QueryError`) name items, keys and codes only, never amounts, merchants or descriptions.

## Schema

`schema.json` (draft 2020-12, exported as `query::SCHEMA`) is written by hand for T4.2's constrained output. `schema_tests.rs` keeps it honest:
- **Enums and ops:** each enum and each `op` in the schema equals the serde names of the Rust types. The variant lists use exhaustive matches, so adding a variant fails to compile until the list is updated.
- **Shapes:** a query using every op and every period shape, with every optional field set, validates and round-trips.
- **Rejections:** the schema and serde reject the same mistakes: unknown fields, bad enum values, a period with two shapes.
- **TD.3:** all 98 expected queries validate.

The validator covers only the keywords the schema uses, and fails on any other.

## Tests

- `tests.rs`: one rule each, on a small hand-built ledger:
  - every measure, with its evidence ids;
  - every scope filter;
  - period resolution, including weeks, quarters across the year boundary and bad periods;
  - averages and medians over complete months;
  - ranking, ties and positions;
  - picks and their ties;
  - derive, including references forward, loops, a zero base and mixed kinds;
  - recurring with and without a labeler, price steps, cancellations and member totals;
  - error texts with no digits.
- **Done-when** (`testdata/questions/tests/answers.rs`). Run with `cargo test --release` in that folder:
  - TD.3's 98 expected queries run on the truth-labelled freelancer ledger.
  - 96 give every answer exactly, including evidence ids.
  - q062 and q065 are pinned known gaps: T3.2 misses the US estimated-tax series, which T3.4 adds.
  - q089 and q093 have no query (no trip tags or one-off marks in the ledger yet).
- **Mutation-checked.** Each break below is caught, by the TD.3 test or by a unit test:
  - purchases keeping refunds (TD.3 q086);
  - weeks starting on Sunday;
  - averages counting refunds as occurrences;
  - the `as_of` month counted as complete.
