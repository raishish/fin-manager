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
8. **Clean-up.**
   - Exact chains inside a variable series of the same merchant fold into it (interest that repeats an amount for a few months).
   - Leftover rows of the merchant that sit in an empty slot of a series join it, if within a price step of their neighbours.
   - A series whose amount changes more often than every fourth occurrence is described as variable.

## Tests (`tests.rs`)
**TD.1, end to end:** every transaction starts at Needs review, then T1.4's `transfers::detect`, then `recurring::detect` as of 2026-09-30. A detected series is correct when at least 80% of its members belong to one truth series. A truth series is found when a correct series covers at least 80% of its members.

| Measure | Result |
|---|---|
| Series precision | 0.8913 (82 of 92) |
| Series recall | 0.9880 (82 of 83); no truth series split |
| Member precision / recall | 0.9819 / 0.9915 |
| Found by kind | bills 37/38, subscriptions 29/29, income 6/6, interest 4/4, transfers 6/6 |
| Of the 82 correct series | cadence right 82, ended right 82, kind right 79 |
| One-offs in any series | 0 of 27 |

- **The missed series:** `freelancer.estimated_tax`, US quarterly estimated tax on Jan 15, Apr 15, Jun 15 and Sep 15. The gaps are 61 to 122 days, outside the quarterly band.
- **The three wrong kinds:** Robinhood and 529 contributions are transfers to accounts outside the ledger. T1.4 can't pair them, so they read as `Charge`. The IRA withdrawal reads as `Income`.
- **The 10 detected series not in TD.1's truth.** Most would count as recurring in real life:
  - annual: a Wikimedia donation, a Vanguard purchase, an IRS payment each April, an eye exam, and two Zelle payments to the same person on the same date each year;
  - semiannual: youth soccer registration;
  - weekly: two runs of summer-camp payments at the YMCA.
  - One is a plain coincidence: the same Starbucks amount a year apart.

**Sensitivity**, one tolerance at a time from the defaults (`td1_sensitivity`):

| Tolerance | Values → precision / recall |
|---|---|
| `split_days` | 100: .883/.976 · 150: .893/.976 · **175–250: .891/.988** · 300: .879/.964 · 400: .889/.964 |
| `variable_band` | 1.5: .897/.940 · **2 to 100: .891/.988** (flat; dominance does the work on TD.1) |
| `step_band` | 1.02: .919/.952 · 1.05: .900/.976 · **1.10: .891/.988** · 1.2: .863/.988 · 1.5: .812/.988 |
| `dominance` | 0.5: .790/.988 · 0.6: .828/.988 · **0.75: .891/.988** · 0.9: .911/.988 · 1.0: .906/.916 |
| `regular_share` | 0.5: .891/.988 · 0.6: .872/.988 · **0.75: .891/.988** · 0.9: .890/.976 · 1.0: .884/.916 |

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
- **Known failure: two $9.99 Apple subscriptions on the 3rd and the 17th** read as one semimonthly $9.99 series. The rows can't tell that apart from one twice-monthly charge. The monthly total is still right for a forecast, and a labeler that knows the services can split it.

## Limits
- Unevenly spaced calendar schedules (US estimated tax) aren't recognised.
- A series needs 3 occurrences (4 for weekly and biweekly, 6 for variable monthly), except annual, which needs 2. A new subscription shows up from its third charge.
- An annual series has only two points, so it is the weakest kind: most of the "extra" series above are annual.
- Series are recomputed on each call and not stored, so a user's "this isn't recurring" needs storage (a later task).
