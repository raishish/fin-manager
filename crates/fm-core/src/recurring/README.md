# Recurring series (`fm_core::recurring`)

Finds subscriptions, bills, paychecks, interest and regular transfers in a ledger: what comes back, how often, for how much, and whether it has stopped. It uses fixed logic only. A language model can name each series and say whether it is a subscription or a bill, through the `Labeler` hook.

```rust
let mut series = recurring::detect_in(&ledger, as_of)?; // or detect(accounts, transactions, decisions, as_of)
recurring::label_all(&mut series, &labeler);         // optional
```

Detection is pure and deterministic. It writes nothing, and the same ledger always gives the same series in the same order.

## A `Series`
- **Where and which way:** `accounts` (more than one when the user moved it to another card) and `direction` (out or in).
- **When:** `cadence` (weekly, biweekly, semimonthly, monthly, quarterly, semiannual or annual), `members` in date order, `first` and `last`, `next_expected`, and `ended`.
  - `ended` means no occurrence for 1.5 periods before `as_of`.
  - Dates follow SPEC decision 42: the purchase date when the bank gives one, otherwise the posted date.
- **How much:** `amount`, either `Fixed { steps }` (one step per price change or raise) or `Variable { min, max, latest }`.
- **What:** `kind`, from fixed logic: `Charge`, `Income`, `Interest` (money in whose text says interest or dividend) or `Transfer` (money out with an applied transfer decision).
- **For the labeler:** `merchant_key`, up to three `descriptions`, and `label`.

Income and transfers are detected too, not only charges. Cash-flow planning (affordability, savings goals) needs paychecks and regular transfers out. The UI can show just `Charge` for "subscriptions and bills".

## How it works (`detect.rs`)
1. **Rows.** Every transaction except these:
   - both sides of a card payment, and the receiving side of a transfer between own accounts (from T1.4's applied transfer decisions): that money is already counted on its sending side;
   - zero amounts, such as a free trial.
2. **Merchant keys** (`key.rs`):
   - **The key** is the first meaningful word of the description:
     - Web domains lose their suffix: `NETFLIX.COM` is `NETFLIX`.
     - `. & ' -` are joined rather than split on, so `AT&T` is `ATT` and `T-MOBILE` is `TMOBILE`.
     - Words with digits and generic bank words (`ACH`, `POS`, `DES`, `SQ`, ...) are skipped.
     - `CITY OF ...` and similar take two words.
   - **The fine key** adds one more word, so `AMAZON PRIME` is told apart from `AMAZON MKTPL`.
3. **Exact-amount chains.** Within each (account, direction, exact amount), rows are chained one period apart, trying the shortest cadence first, so a biweekly series is never read as two monthly ones. A row joins a chain in three ways:
   - **regular:** one period after the last member;
   - **jitter:** early or late, but two periods after the member before last, as with checks that clear on different days;
   - **pause:** after a silence of up to `split_days` (200), once the chain has its minimum count, as with a summer or winter break. A longer silence ends the series, so a cancel-and-restart is two series.

   Rows prefer a chain with the same key, which unions alternating names (HBO MAX and MAX.COM, MSF and DOCTORS WITHOUT BORDERS). A key that occurs only once in a chain of several is dropped, as a stray coincidence. At least 75% of steps must be regular or jitter (`regular_share`).
4. **Long cadences** (quarterly, semiannual, annual) must also fall on about the same day of the month, within 6 days. Real bills are scheduled; the same meal bought every three months or so isn't.
5. **Judged together** (`accept`). Candidate chains at one merchant (account, direction, a shared fine key, overlapping in time) are kept or dropped as a group:
   - Between them they must hold at least 75% (`dominance`) of the merchant's rows over their span, at any amount. Two Apple subscriptions, or checks for church and for piano lessons, explain their merchant; a café's visits don't.
   - No amount may have more than two chains side by side, and at most one chain may vary in amount. A café visited every Monday, Wednesday and Friday at one price makes three parallel weekly chains and is dropped.
6. **Price steps and card changes** (`merge_steps`). An exact chain that starts about one period after another ends is merged into it, when it has the same key and cadence and either:
   - is on the same account with an amount change of at most 1.5× (a price rise);
   - or has the same amount on another account (a new card).
7. **Variable amounts**, from the rows no exact chain took, grouped by fine key:
   - Monthly and quarterly series need at least 6 and 4 occurrences, each amount within 4× of the previous one (`variable_band`). That lets utilities drift with the seasons but rejects one-off spikes.
   - Semiannual and annual series need every amount within 1.10× (`step_band`): a price step, as with insurance or property tax.
8. **The estimated-tax calendar** (T3.4). US estimated tax is due around Jan 15, Apr 15, Jun 15 and Sep 15 (IRS Form 1040-ES; many states use the same dates). The gaps of 3, 2, 3 and 4 months fit no cadence, so these payments are matched to the calendar instead, with no cadence band loosened.
   - Only rows no other series took, money out to a payee, not a transfer between own accounts. Moving money to checking ahead of a due date funds the payment; counting it too would double the commitment.
   - A payment counts for a due date from 7 days before it to 5 days after: early payments, and due dates moved past a weekend or holiday to as late as the 18th, plus a day or two of posting.
   - Each due date takes at most one payment per fine merchant key: the one nearest in amount to the previous member. A balance-due payment made the same April day as the first estimate stays out.
   - A series needs at least 4 payments, a year's worth. At least 75% of steps must be from one due date to the next, and a silence longer than `split_days` ends the series.
   - Candidates are then judged like any other (step 5), so a shop that also happens to be visited near the due dates is not a tax series.
   - `next_expected` is the next due date on the calendar.
9. **Clean-up.**
   - Exact chains inside a variable series of the same merchant fold into it (interest that repeats an amount for a few months).
   - Leftover rows of the merchant that sit in an empty slot of a series join it, if within a price step of their neighbours.
   - A series whose amount changes more often than every fourth occurrence is described as variable.

## Tests (`tests.rs`)
**TD.1, end to end:** every transaction starts at Needs review, then T1.4's `transfers::detect`, then `recurring::detect` as of 2026-09-30. A detected series is correct when at least 80% of its members belong to one truth series. A truth series is found when a correct series covers at least 80% of its members.

| Measure | Result |
|---|---|
| Series precision | 0.8925 (83 of 93) |
| Series recall | 1.0000 (83 of 83); no truth series split |
| Member precision / recall | 0.9820 / 0.9960 |
| Found by kind | bills 38/38, subscriptions 29/29, income 6/6, interest 4/4, transfers 6/6 |
| Of the 83 correct series | cadence right 83, ended right 83, kind right 80 |
| One-offs in any series | 0 of 27 |

- **No series is missed.** `freelancer.estimated_tax` (gaps of 61 to 122 days), missed before T3.4, is found on the estimated-tax calendar with exactly its 8 payments; the April 2025 balance-due payment stays out.
- **The three wrong kinds:** Robinhood and 529 contributions are transfers to accounts outside the ledger. T1.4 can't pair them, so they read as `Charge`. The IRA withdrawal reads as `Income`.
- **The 10 detected series not in TD.1's truth.** Most would count as recurring in real life:
  - annual: a Wikimedia donation, a Vanguard purchase, an IRS payment each April, an eye exam, and two Zelle payments to the same person on the same date each year;
  - semiannual: youth soccer registration;
  - weekly: two runs of summer-camp payments at the YMCA.
  - One is a plain coincidence: the same Starbucks amount a year apart.

**Sensitivity**, one tolerance at a time from the defaults (`td1_sensitivity`):

| Tolerance | Values → precision / recall |
|---|---|
| `split_days` | 100: .885/.976 · 150: .894/.988 · **175–250: .893/1.000** · 300: .880/.976 · 400: .890/.976 |
| `variable_band` | 1.5: .898/.952 · **2 to 100: .893/1.000** (flat; dominance does the work on TD.1) |
| `step_band` | 1.02: .920/.964 · 1.05: .901/.988 · **1.10: .893/1.000** · 1.2: .865/1.000 · 1.5: .814/1.000 |
| `dominance` | 0.5: .790/.988 · 0.6: .830/1.000 · **0.75: .893/1.000** · 0.9: .911/.988 · 1.0: .906/.916 |
| `regular_share` | 0.5: .893/1.000 · 0.6: .874/1.000 · **0.75: .893/1.000** · 0.9: .891/.988 · 1.0: .885/.928 |

**Hand-written cases** (`hand_written_cases`), in shapes TD.1 doesn't have:
- A gym billing on the 28th or the 1st alternately: monthly.
- A quarterly bill moved a day by Christmas: quarterly.
- The same $5 coffee three Mondays running: no series.
- The same $4.50 coffee every Monday, Wednesday and Friday: no series.
- A subscription moved to another card: one series on both cards.
- A $0 free trial, then paid: the paid charges only.
- A laptop from a shop that also bills a subscription: not in the series.
- An annual membership with a price rise among the same shop's purchases: annual.
- A gas bill six times higher in winter: monthly, variable.
- Federal and state estimated tax on the 1040-ES calendar: two quarterly series. Payments are early, on time and moved past a weekend, and a same-day balance due stays out.
- Three estimated-tax payments: not yet a series.
- A shop visited all year, including near the tax due dates: no series.
- An estimated-tax series as of October expects January 15 next.
- **Known failure: two $9.99 Apple subscriptions on the 3rd and the 17th** read as one semimonthly $9.99 series. The rows can't tell that apart from one twice-monthly charge. The monthly total is still right for a forecast, and a labeler that knows the services can split it.

## Limits
- The only uneven calendar known is US estimated tax. Other fixed-date schedules (a school's term fees, say) aren't recognised unless they fit a cadence.
- A series needs 3 occurrences (4 for weekly and biweekly, 6 for variable monthly), except annual, which needs 2. A new subscription shows up from its third charge.
- An annual series has only two points, so it is the weakest kind: most of the "extra" series above are annual.
- Series are recomputed on each call and not stored, so a user's "this isn't recurring" needs storage (a later task).
