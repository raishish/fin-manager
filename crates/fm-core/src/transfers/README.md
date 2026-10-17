# Transfer detection (`fm_core::transfers`)

Finds money moving between the user's own accounts (savings transfers and credit-card payments) and records both sides as `Transfer` decisions that point at each other. Transfers are neither spending nor income, so a missed one double-counts and a false one hides real money. The matcher favours precision.

```rust
let report = transfers::detect(&mut ledger, now)?; // TransferReport { applied, suggested }
```

## Pairing (`pairing.rs`, `find_pairs`)
A pure function over accounts and transactions.

**A candidate** has all of these:
- A negative and a positive transaction of exactly opposite amount, in one currency.
- Two different accounts, both marked `own_account`.
- The positive side posted 0 to 5 days after the negative one (`PARAMS.max_lag_days`).
- Neither side is a person-to-person payment (Zelle, Venmo, PayPal, Cash App, Square Cash, Apple Cash).
- **Kind:** non-card to card is a `CardPayment`; non-card to non-card is an `OwnTransfer`. Money leaving a card is never a transfer.

**Evidence** decides. The amount and dates only make a candidate. The score has one point for each of these:
- Transfer wording on the out side (`TRANSFER`, `XFER`, `AUTOPAY`, `EPAY`, `PMT`, `THANK YOU`, ...).
- Transfer wording on the in side.
- The out side names the in side's account (an institution word, a distinctive word of the account name, or its last four digits).
- The in side names the out side's account.
- The source's `trn_type` (`XFER`, `PAYMENT`, `DIRECTDEP`, `DIRECTDEBIT`), counted only when there is already text evidence.

A pair needs **2 points** (`PARAMS.min_score`): wording on one side alone is never enough. Candidates are assigned best first: highest score, then the nearest dates, then the lowest ids. Each transaction is used once.

## Recording (`detect.rs`, `detect`)
- **Who may be paired.** A transaction is open when its applied bucket came from the import pipeline's Needs review, from a model, or when it has none. It is closed when:
  - a user's or a rule's decision is applied (never overridden);
  - the matcher has any decision on it, in any state: a dismissed pair stays dismissed, and a second run changes nothing.
- **Each pair** gets a `Transfer { counterpart }` decision on both sides:
  - source `builtin/transfer_matcher v1`;
  - `replaces` set to the decision it takes over, which is then superseded.
- **Seen decisions aren't changed under the user.** If the user has seen either side's earlier decision (`seen_at`), both sides get a **suggestion** instead, and the earlier decisions stay applied.
- **One savepoint:** a run records everything or nothing, and it refuses to start inside another transaction (`DetectError::TransactionOpen`).

## Tests (`tests.rs`)

| Test | Result |
|---|---|
| TD.1, pairing alone (309 true pairs) | precision 1.0000, recall 1.0000 |
| TD.1 through the ledger, every transaction starting at Needs review | precision 1.0000, recall 1.0000; contract holds; second run is a no-op |
| TD.2: all 50 bank exports imported through the pipeline, in file order and in reverse (169 pairs with both sides exported) | precision 1.0000, recall 1.0000 both ways; contract holds |
| Hand-written cases in real banks' wording (not from TD.1 or TD.2): 6 transfers, 7 look-alikes | all right |
| User, rule and model decisions; seen → suggested; dismissal sticks; savepoint rollback | pass |

**Overfitting check** (`td1_near_misses_and_lags`, `td1_sensitivity_to_window_and_threshold`):
- **Evidence:** true pairs score 3 (208) or 4 (101). TD.1 has **no** false candidates at all within the window. The threshold of 2 sits in the gap, and every threshold from 1 to 3 gives the same result on TD.1. TD.1 therefore tests recall but **not** precision.
- **Precision comes from the hand-written look-alikes.** Each scores 0 or 1 and fails the threshold:
  - a refund alongside an ATM withdrawal;
  - two Zelle payments;
  - a bill alongside interest;
  - card to card;
  - wording on one side only;
  - an unknown card's payment.
- **Lag:** true pairs post 0 days apart (149), 1 day (83), 2 days (62) or 3 days (15).
  - Recall is 0.75 with a 1-day window, 0.95 with 2 days, and 1.0 from 3 days to 14.
  - Precision stays 1.0 across every window.
  - The window is 5 days, not TD.1's maximum of 3, to cover ACH over a long weekend (a hand-written case posts 5 days later).

## Limits
- **One-sided transfers are not attempted.** When the other account isn't in the ledger, there is no pair to find; TD.1 has 52 of these. They stay Needs review for categorisation (T2.2) or the user.
- **Transfers between currencies** (amounts that differ by an exchange rate) are not matched.
- **Payments into someone else's account** (e.g. a partner's card) look like a card payment only if that account is in the ledger as `own_account`.
