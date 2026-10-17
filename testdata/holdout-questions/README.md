# Held-out question set (TD.7)

25 plain-English questions about the same household as TD.3, with their expected answers
and queries. They were written apart from question answering (T4.2): the writer never saw
its prompt, its evaluation or its failures. TD.3 is what T4.2 is developed against; this
set measures how it does on phrasings it was never tuned on.

**Don't tune on this set.** Look at its results only to report them, never to change the
prompt or the examples. A question that T4.2 gets wrong here stays wrong until a change
made for other reasons fixes it.

The persona, the counting conventions and the file format are TD.3's
(`testdata/questions/README.md`). This set reuses TD.3's crate for the format, TD.1's
facts and the answer helpers, and its `conventions` are TD.3's word for word (a test
checks this). Every answer is computed by `src/questions.rs` from TD.1's committed ledger
and ground truth. None is typed by hand.

## Running it

A standalone Cargo workspace, like the other test-data folders:

```sh
cargo run --release              # regenerate data/questions.json
cargo run --release -- --check   # exit 1 if the committed file is out of date
cargo test --release             # the checks below
```

## The questions

Ids are `h01` to `h25`. The mix follows TD.3's proportions:

| Type | Count | |
|---|---|---|
| `totals` | 5 | categories, a group of bills, income in a quarter |
| `comparisons` | 4 | which is larger, this year against last, the cheapest month, a share |
| `merchants` | 4 | all-time total, visits, the largest single purchase, the top two last month |
| `recurring` | 4 | a bill's current price, two price changes, a membership named only by what it is |
| `time_ranges` | 4 | the past 30 and 90 days, a season, last quarter |
| `hard` | 4 | fees, a refund, money moved between my own accounts, card payments |

None repeats a TD.3 question (a test checks the text). Phrasings are casual, the way a
person would type them: "What did fast food set me back", "Where did most of my money go
last month? Just the top two places.", "What does my coworking membership cost per
month?".

`h17` is a **proxy**, as in TD.3:
- the question says "coworking membership" and never names Capital Factory;
- that space's category, professional services, also holds the tax preparer.

So the query names the series.

## Checks

`tests/questions.rs`:
- **Freshness.** The committed file is byte-identical to a fresh build, and building is
  deterministic.
- **Shape.**
  - 25 questions in the mix above, with no repeated text and none of TD.3's.
  - Every question has a query.
  - The tags `refunds`, `transfers`, `relative_date`, `income`, `percent` and
    `price_change` are all present.
  - The conventions and "today" are TD.3's.
- **Evidence.** Every sum and count is re-added straight from TD.1's ledger.
- **An independent recount.** Summer 2025, the past 30 days and 2025 spending are
  recomputed from the raw ledger with the conventions written out again.
- **The conventions hold.**
  - No pending item is in a total.
  - Transfer questions count only transfers.
  - No other spending total counts a transfer or income.
- **Differences** are the first value minus the second.

`tests/answers.rs`, as TD.3's:
- **Every expected query gives its answers.** Each runs through `fm_core::query::run` on
  the freelancer's ledger, labelled from truth, as of 2026-10-01. Every answer must match
  exactly, and so must the evidence ids behind sums and counts.
- **Every query round-trips through JSON.**

## Changing the set

1. Edit `src/questions.rs` and `src/queries.rs`.
2. Bump `VERSION` in `src/lib.rs` if answers change.
3. Run `cargo run --release`, then `cargo test --release`.
4. Commit the code and `data/questions.json` together.

Keep the set apart from T4.2's work. If a question has to change, change it for a reason
that has nothing to do with how T4.2 answers it, and say so in the commit.

If TD.1's data or TD.3's conventions change, regenerate here too. The freshness test fails
until you do.
