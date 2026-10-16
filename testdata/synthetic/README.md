# Synthetic ledgers with ground truth (TD.1)

Four invented households, 24 months each (2024-10-01 to 2026-09-30), generated from a seed. Each comes with its **ground truth**: the right bucket and clean merchant name for every transaction, which transactions are transfer pairs, which belong to recurring series, and which are one-offs. Later tasks measure themselves against it: the bank export corpus (TD.2) and the question set (TD.3) are built from these ledgers, and transfer detection (T1.4), categorisation (T2.2, T2.5), recurring detection (T3.2) and the views (T3.1) are scored on them.

Nothing here is real. People's names are invented and obviously fake (`FAKE_SURNAMES` in `src/catalog.rs`: Placeholder, Exampleton, Testworth…). Merchants are real US brands or invented local businesses, written the way they appear on statements.

## Running it

This folder is its own Cargo workspace, kept out of the app's build. Run everything from here:

```sh
cargo run --release              # regenerate data/ with the default seed
cargo run --release -- --check   # exit 1 if data/ differs from a fresh run
cargo test --release             # determinism, round trips, truth consistency, freshness
cargo run --release -- --seed 7 --out /tmp/synth   # another seed, elsewhere
```

## Determinism

- The output depends only on the seed (`DEFAULT_SEED = 20261016` for the committed data) and the generator code. There is no clock, no hash-map ordering and no platform-dependent floating point.
- Randomness comes from SplitMix64, written out in `src/rng.rs` and checked against its reference values, so a dependency upgrade can't change the output.
- Each part of a persona draws from its own stream (`<persona>/habit/coffee`, `<persona>/series/renter.rent`…). Editing one habit doesn't reshuffle the rest.
- `generator_version` (in `manifest.json` and every truth file) is bumped whenever a code change alters the output for the same seed. The `committed_data_matches_the_generator` test fails until `data/` is regenerated.

## The households

| Key | Accounts | Transactions | Transfer pairs | Recurring series | One-offs |
|---|---|---|---|---|---|
| `renter` | 4 | 2,133 | 72 | 16 | 7 |
| `family` | 4 | 2,220 | 71 | 28 | 7 |
| `freelancer` | 4 | 1,672 | 95 | 18 | 6 |
| `retiree` | 4 | 1,351 | 71 | 21 | 7 |

Each has a checking account, a savings account and two credit cards at different institutions, so descriptions come in different bank layouts (Chase, Bank of America, Wells Fargo, a credit union, Ally, Amex, Capital One). The full descriptions are in `manifest.json`. In short:

- **renter**: a salaried analyst renting in San Francisco. Biweekly pay with raises and bonuses; Ally savings fed by an automatic transfer; dinners split over Venmo and Zelle; subscriptions that rise in price, get cancelled or get replaced; a laptop, furniture, a New York trip.
- **family**: two incomes (biweekly and semimonthly), two kids, a mortgage in a Columbus suburb. HOA paid quarterly; utilities that follow the seasons; after-school care in term time only and summer camp; a babysitter paid by Zelle; checks for piano, lawn care (April to October) and church; a car loan paid off in November 2025; a furnace replacement and a Disney World trip.
- **freelancer**: a self-employed designer in Austin. A retainer plus irregular project payments; a quarter of each large payment set aside in savings; quarterly estimated tax; a card balance carried through lean months, with interest charged; software subscriptions on monthly and annual plans; ten days in Lisbon, partly paid in euros on a card that charges a foreign-transaction fee.
- **retiree**: a homeowner in Tucson. Social Security (on the second Wednesday, rising with cost-of-living adjustments) and a pension; a yearly IRA withdrawal from an account outside the ledger; property tax twice a year; insurance paid every six or twelve months; checks for church and a cleaner; two weeks in Japan paid in yen (a zero-decimal currency); a new roof and hearing aids.

## Files

```
data/
  manifest.json          seed, generator version, period, taxonomy version, per-persona counts and SHA-256 of each file
  <persona>/ledger.json  the ledger, as fm_core::model::Ledger
  <persona>/truth.json   its ground truth
```

The JSON is written one record per line, so diffs stay readable. It is ordinary JSON.

### `ledger.json`

This is a `fm_core::model::Ledger` in its stable JSON form (`crates/fm-core/src/model/README.md`). It holds only what a bank would report: there are no answers in it.

- `taxonomy` is the shipped default list, `crates/fm-core/assets/taxonomy/default-v1.json` (version `1+r0`), loaded as is.
- `decisions` and `rules` are empty.
- `accounts`: all `own_account: true`, in USD, with institution and last digits.
- `imports`: one `manual` import per account. The ledger is generated rather than read from files, so each import's bytes are just a one-line note saying which generator, seed, persona and account produced it. Realistic bank files are TD.2's job.
- `transactions`: ids run 1…n in posting order. Each points at its account's import (`source.index` counts 0…n within it) and has a random `external_id`. Amounts are signed cents (negative is money out of that account, for cards too). Card and debit-card purchases have a separate `transaction_date`. Purchases abroad carry `original_amount` in EUR or JPY. Transactions that haven't posted by 2026-09-30 are `pending`. `payee` and `memo` are always `null`, because everything is in `description`.

### `truth.json`

```json
{"persona": "renter", "description": "…", "seed": 20261016, "generator_version": 1,
 "taxonomy_version": {"base": "1", "revision": 0}, "start": "2024-10-01", "end": "2026-09-30",
 "transactions": [ … ], "transfer_pairs": [ … ], "recurring": [ … ]}
```

**`transactions`** has one entry per ledger transaction, in the same order:

| Field | Meaning |
|---|---|
| `id` | The ledger transaction id |
| `bucket` | Where its money belongs. Never `needs_review`. Categories are named by stable **key**, not id: `{"kind":"category","key":"food.coffee"}`, `{"kind":"transfer","counterpart":id\|null}`, `{"kind":"income"}`, `{"kind":"refund","key":"shopping.general","original":id\|null}` |
| `merchant` | Clean merchant or counterparty name (the merchant clean-up answer), e.g. `"Trader Joe's"`, `"IRS"`, `"Quinn Placeholder"`. `null` for transfers between own accounts and for cash |
| `recurring` | The series id, if it belongs to one |
| `one_off` | A large, irregular amount that a monthly spending baseline should leave out (SPEC decision 32): furniture, a laptop, trip flights and lodging, a furnace, a roof, dental work, bonuses, a tax balance due |
| `tags` | Provenance and hard-case tags (below) |
| `pending` | How the transaction looked while pending, when that differed from the posted one: `{"amount", "description"}`. Restaurant bills before the tip, pay-at-pump fuel holds of $175. Lets TD.2 build overlapping exports where a pending item later posts with a different amount |

A `transfer` with a `counterpart` is one side of a pair in the ledger. `counterpart: null` means money moved to or from one of the user's own accounts that isn't in the ledger (a brokerage, a 529, an IRA).

**`transfer_pairs`**: `{"out": id, "in": id, "kind": "own_transfer" | "card_payment"}`, sorted by `out`. `out` is the negative side and `in` the positive side. The two amounts add up to exactly zero, `in` posts 0 to 5 days after `out`, and `card_payment` means `in` is on a credit card.

**`recurring`**: one entry per series:

| Field | Meaning |
|---|---|
| `id` | `<persona>.<name>`, e.g. `renter.netflix` |
| `kind` | `subscription`, `bill`, `income`, `transfer` or `interest` |
| `merchant`, `account_id` | Who and which account (for transfers, the sending account) |
| `cadence` | `weekly`, `biweekly`, `semimonthly`, `monthly`, `quarterly`, `semiannual` or `annual` |
| `amount` | `{"kind":"fixed","steps":[{"from":date,"amount":Money}…]}`: the same amount each time, with each price change as a new step. Or `{"kind":"variable","min","max"}` for utilities, interest and card-funded tax set-asides |
| `first`, `last` | Dates of the first and last occurrence in the period |
| `ended` | `true` if it stopped before the period ended (a cancellation, a paid-off loan, a course that finished) |
| `transaction_ids` | Every member, in date order |

"Recurring" means a regular cadence with a fixed or bounded amount from one counterparty. Groceries every week are a habit, not a series. Card payments are listed in `transfer_pairs`, not as series. A cancel-and-restart is two series (`freelancer.zoom` and `freelancer.zoom_restart`).

### Tags

| Tag | Meaning |
|---|---|
| `pending` | Still pending at the end of the period (its `status` is `pending`) |
| `tip_adjusted`, `pump_hold` | Pending amount differed from the posted amount (see `pending`) |
| `refund`, `partial_refund` | A merchant refund linked to its purchase (`original`), in full or in part |
| `reimbursement` | Someone paying the user back by Zelle. The truth is a `refund` of the shared cost's category with `original: null` |
| `card_payment`, `partial_payment` | Either side of a card payment; a payment of less than the statement balance |
| `card_interest` | Interest charged on a carried card balance |
| `foreign_currency`, `foreign_fee` | Purchase abroad (`original_amount` set); the card's separate foreign-transaction fee line |
| `lagged_transfer` | A transfer whose receiving side posted 3 or more days later |
| `untracked_counterpart` | Transfer to or from an own account outside the ledger |
| `near_miss_transfer` | Looks like a transfer but isn't: a Zelle payment to a friend for the same amount on the same day as a savings transfer, or a quarterly tax payment equal to the savings withdrawal that funded it the day before |
| `ambiguous_descriptor` | The description is shared with a different category (`APPLE.COM/BILL` is Apple Music or iCloud; only the amount tells them apart) |
| `check` | A paper check: the statement shows only the number; the payee and category are in the truth only |
| `p2p`, `no_counterparty_name` | Zelle, Venmo or PayPal with a person; Venmo from a bank account doesn't show who was paid |
| `atm` | A cash withdrawal |
| `trip`, `bonus`, `babysitter`, `summer_camp`, `tax_set_aside`, `sep_ira`, `gift`, `concert_tickets` | Story context, for reading the data |

## Labelling conventions

These match `crates/fm-core/assets/taxonomy/README.md`, the shared reference for the rules engine and model prompts.

1. Card payments and moves between own accounts are **transfers**. `counterpart` is the other side when it's in the ledger, and `null` for an own account that isn't tracked.
2. Mortgage and auto-loan payments to lenders that aren't tracked are spending (`housing.mortgage`, `transport.auto_loan`).
3. An incoming P2P reimbursement for a shared cost is a **refund** in that cost's category, with `original: null`.
4. A tax refund is a refund of `taxes.income`.
5. Salary, interest, pension, Social Security and freelance client payments are **income**.
6. ATM cash is `financial.cash_withdrawal`. Card interest is `financial.interest`. Foreign-transaction and ATM fees are `financial.bank_fees`.
7. Paper checks are labelled by what they paid for, which only the truth knows.

65 of the 66 category keys appear in the truth. The one missing is `other.misc`, a catch-all that no ground truth should point at.

## Realism and simplifications

- **Descriptions** follow each institution's public layout: Amex pads columns and truncates names to 20 characters; Wells Fargo wraps card purchases in "PURCHASE AUTHORIZED ON MM/DD … CARD 6170"; the credit union upper-cases and cuts at 40 characters. Merchant descriptors vary (`AMZN Mktp US*…`, `Amazon.com*…`), store numbers mostly stay the same with an occasional other branch, and some purchases happen in nearby towns. Layouts are close imitations, not byte-exact copies of any bank.
- **Card statements** close monthly. The holder pays the statement balance (or, in the freelancer's lean months, 35% of it) from checking after 22 to 25 days. Interest is charged at the next close on the balance carried over. This is simpler than average-daily-balance interest, but it produces the same transactions.
- **Savings interest** is paid on the month-end balance at a rate that changes over time.
- **Timing**: card purchases post 1 to 2 days later. ACH debits move to the next business day and paychecks to the previous one. Bank holidays are ignored.
- **Balances** aren't modelled. Spending roughly matches income for each household over the 24 months, but no account is checked against an overdraft.

## Changing the generator

Edit a persona script in `src/personas/`, or the engine in `src/sim.rs`. Then:

1. bump `GENERATOR_VERSION` in `src/sim.rs` if the output changes;
2. run `cargo run --release`, then `cargo test --release`;
3. commit the code and `data/` together.

Category keys are frozen for taxonomy version 1. The tests fail if the truth uses a key the shipped list doesn't have.
