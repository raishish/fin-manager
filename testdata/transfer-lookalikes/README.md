# Transfer look-alikes (TD.6)

These ledgers test transfer detection's **precision**. TD.1 has no same-amount pairs across own accounts except real transfers, so it can't show whether a matcher wrongly pairs a bill with a savings deposit. These can.

- Three households over 2025, each a `fm_core::model::Ledger` in its stable JSON form, plus a truth file.
- **250 look-alike candidates** (same amount, opposite sign, own accounts, near dates, and not a transfer) and **272 true transfers**.
- Some of the true transfers are deliberately hard to see: weak or no wording, long-weekend gaps, equal amounts days apart, card to card.

Everything is synthetic: invented people with obviously fake surnames, and account numbers that start with `9999`. Output depends only on the seed.

## Running it

A standalone Cargo workspace, like the other test-data folders:

```sh
cargo run --release              # regenerate data/
cargo run --release -- --check   # exit 1 if data/ is out of date
cargo test --release
```

## Files

```
data/
  manifest.json          seed, window, per-household counts and SHA-256s, counts per pattern
  <household>/ledger.json   fm_core::model::Ledger (decisions and rules empty)
  <household>/truth.json    the truth
```

Notes on `ledger.json`:
- Transactions carry `posted_date` only; no purchase date, `trn_type` or `check_number`.
- All evidence is in the descriptions, written in each bank's style (Chase, Bank of America, Wells Fargo, a credit union, Ally, Amex, Capital One, Citi, Discover).
- Each account has one `manual` import, as in TD.1.

`truth.json`:

| Field | Meaning |
|---|---|
| `transactions` | One per ledger transaction, in the same order, with its `bucket`, its `pattern`, and its `merchant` (clean name, where there is one). The bucket uses TD.1's `TruthBucket` form; a transfer's `counterpart` is its pair. |
| `transfer_pairs` | Every true transfer: `out`, `in`, `kind`, `pattern`, `wording`, `lag_days`. `kind` is `own_transfer`, `card_payment` or `card_to_card`. `wording` is `strong`, `one_sided` or `none`. |
| `lookalikes` | **Every candidate pair in the ledger that is not a transfer**: `out`, `in`, `pattern`, `lag_days` and `flags`. |
| `candidate_window_days` | 7. |

**A candidate** has all of these:
- a negative and a positive transaction of exactly opposite amount;
- on two different own accounts;
- the positive side posted 0 to 7 days after the negative one.

The window is wider than a matcher's, so near misses are listed too. The truth lists every candidate exactly once: as a true pair or as a look-alike. A test re-derives them from the ledger alone to prove it.

`flags` are informational:
- `p2p`: either side is a Zelle, Venmo or PayPal payment.
- `leaves_card`: the money-out side is on a card.

Some matchers rule these out before scoring.

## Patterns

**Look-alikes: not transfers.**

| Pattern | Count | What happens |
|---|---|---|
| `equal_recurring_crossed` | 72 | Two equal scheduled transfers (e.g. $250.00) two days apart. The first lands four days later, after the second has landed. The crossed pairings (first out with second in, and the reverse) are within the window. |
| `card_refund_vs_bill` | 36 | A merchant refund on a card one to three days after an unrelated bill of the same amount from checking. |
| `pmt_bill_vs_savings_move` | 24 | **A tie.** A bill whose bank line says `WEB PMT` is paid the same day, from the same checking, as an equal savings move. The move's out side says only `ONLINE TRANSFER`; its in side names the checking account. Both candidates have transfer wording on both sides and the same dates, and the bill is listed first. |
| `bill_vs_savings_move` | 24 | A bill and a savings move of the same amount, the same day, from the same checking. |
| `card_charge_vs_deposit` | 24 | A card purchase and a checking deposit of the same amount, the same day (`leaves_card`). |
| `zelle_out_vs_zelle_in` | 24 | Zelle out of one own account to a friend, and Zelle into another own account from a different friend, same amount, a day apart (`p2p`). |
| `deposit_vs_savings_outflow` | 12 | Savings sends money to an outside brokerage (a one-sided transfer, `counterpart: null`), and a tax refund of the same amount lands in checking the next day. |
| `payroll_vs_equal_transfer` | 12 | A paycheck (or pension) lands in checking the day the same amount moves from savings into checking. The savings move pairs with the transfer, not the paycheck. |
| `rent_split_repayment` | 12 | A roommate repays half the rent into a second checking, by plain ACH (no P2P words) or by Zelle, the day the same amount moves from checking to savings. |
| `balance_transfer_vs_payments` | 10 | A balance transfer from one card to another, on the day checking pays both cards the same amount. Every crossed pairing among the three true pairs is a look-alike. |

**True transfers.**

| Pattern | Count | What happens |
|---|---|---|
| `card_payment` | 76 | Checking pays a card, in the issuer's usual wording. |
| `savings_move` | 96 | Checking to savings and back, worded as the banks word them. Includes the true sides of the look-alike scenarios above. |
| `equal_recurring` | 72 | The two equal scheduled transfers whose landings cross. |
| `long_weekend` | 9 | ACH started on the Friday (or Wednesday) before a 2025 long weekend, landing 4 or 5 days later: MLK, Presidents' Day, Memorial Day, Independence Day, Labor Day, Columbus Day, Thanksgiving, Christmas. |
| `weak_none` | 9 | No transfer wording and no account names on either side: `WITHDRAWAL #…` and `DEPOSIT #…`. |
| `weak_one_sided` | 8 | Transfer wording on the out side only: `ONLINE TRANSFER REF #…` against `DEPOSIT …`. |
| `balance_transfer` | 2 | Card to card (`BALANCE TRANSFER - CAPITAL ONE` and `BALANCE TRANSFER CREDIT`). Money leaves a card, which some matchers rule out. By labelling convention 1 (moves between own accounts are transfers), it is a transfer. |

Everything else is everyday spending (pattern `background`), in cents amounts that don't collide with the scenarios. There are no `incidental` look-alikes.

## Checks (`tests/corpus.rs`)

- **Freshness.** The committed data is byte-identical to a fresh render. Rendering is deterministic, and another seed gives other data.
- **Ledgers.** The taxonomy is valid, transactions line up one-to-one with the truth, accounts are own accounts, and account numbers start `9999`.
- **Completeness.** Every candidate is re-derived from the ledger alone and must be listed exactly once, as a true pair or a look-alike.
- **True pairs.**
  - They net to zero, lag 0–5 days, and point at each other.
  - Each `kind` matches its account kinds.
  - No look-alike's sides point at each other.
- **Coverage.**
  - At least 200 look-alikes and 50 true pairs.
  - A minimum count for every pattern.
  - Long-weekend lags of 4 and 5 days.
  - The `p2p` and `leaves_card` flags where expected.
- **Difficulty.** A matcher using amount and date alone (nearest date first, each transaction once) gets 146 of its pairs wrong: precision 0.60. The test requires at least 40 wrong, so the corpus keeps its teeth.

## Changing it

1. Edit a scenario in `src/patterns.rs` or a household in `src/households.rs`.
2. Bump `VERSION` in `src/lib.rs` if the output changes.
3. Run `cargo run --release`, then `cargo test --release`.
4. Commit the code and `data/` together.
