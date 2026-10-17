# Output contract checker (`fm_core::contract`)

SPEC §6 defines eight hard constraints that the app's output must never break (decision 20). A violation is a bug. This module checks a whole `Ledger` against them:

```rust
let report = fm_core::contract::check(&ledger);   // constraints 1, 2, 4 and 8 so far
assert!(report.is_ok(), "{report}");
```

To check a ledger in an encrypted database, run it on `unlocked.load_ledger()?`.

## Framework
- **`Check`** is one constraint's check. It reads a `LedgerIndex` (the ledger plus shared lookups) and pushes `Violation`s.
- **`Checker`** is a set of checks. `Checker::all()` has every check implemented so far; `Checker::empty().with(...)` builds a custom set. Later tasks add constraints 2, 3, 5, 6 and 7 the same way:

  | Constraint | Added by |
  |---|---|
  | 2 | T2.2 (done) |
  | 3, 5, 7 | T2.5 |
  | 6 | T4.3 |

- **`Report`** lists every violation, sorted by constraint, subject and code. The same ledger always gives the same report, whatever order its rows are in.
- **`Violation`** holds the constraint, the subject (the ledger or an account, transaction, decision or rule id), a stable `code` and a fixed message. It never holds amounts, descriptions or merchant names, so a report can go into logs and crash reports (SPEC §7).

## What is checked

### Constraint 1: numbers are exact
Amounts are integer minor units by type, so this checks what could still make totals inexact or ambiguous:

| Code | Subject | Meaning |
|---|---|---|
| `currency_mismatch` | transaction | The amount isn't in its account's currency (summing it would mix currencies) |
| `unknown_account` | transaction | Its account isn't in the ledger |
| `original_same_currency` | transaction | A "foreign" amount is in the account's own currency |
| `original_sign_mismatch` | transaction | The foreign amount and the amount have opposite signs |
| `total_overflow` | account or ledger | Money in or out of an account, or of the whole ledger in one currency, doesn't fit in `i64` minor units |
| `duplicate_account_id` | account | Two accounts share an id |
| `totals_not_reproducible` | ledger | Summing in reverse order gave different totals. A self-check of the arithmetic: no ledger can trigger it, so it has no broken fixture |

### Constraint 2: user inputs always win
Sources rank user > rule > fixed logic (transfer matcher, merchant clean-up) > model > the Needs-review placeholder (`categorize` module docs). Every decision is about one assignment kind of one transaction: its bucket or its merchant name.

| Code | Subject | Meaning |
|---|---|---|
| `user_input_overridden` | decision | An applied decision replaced (`replaces`) a user's correction or a rule's decision, from a weaker source. The transfer matcher's pair replacing a rule's unpaired transfer is a refinement and allowed |
| `user_choice_not_in_effect` | transaction | The user's newest correction for a kind isn't applied, and no newer correction is |
| `model_over_matching_rule` | decision | An applied model decision where one of the user's enabled rules matches (and its bucket fits the transaction) |

Account designations (`own_account`) are plain fields no model writes, so they need no check.

### Constraint 4: everything adds up
Only **applied** decisions count; suggested, dismissed and superseded ones don't.

| Code | Subject | Meaning |
|---|---|---|
| `no_bucket` | transaction | No applied bucket. Even an undecided transaction must be in Needs review |
| `several_buckets` | transaction | More than one applied bucket |
| `several_merchants` | transaction | More than one applied merchant name |
| `unknown_category` | transaction | A category or refund bucket names a category not in the list. Merged and hidden categories are in the list |
| `unknown_refund_original` | transaction | A refund's original purchase isn't in the ledger (or is the refund itself) |
| `outflow_not_spending_or_transfer` | transaction | Money out is filed as income or a refund. Money out must be in a category, a transfer or Needs review |
| `unknown_counterpart`, `self_counterpart` | transaction | A transfer's counterpart is missing, or is the transaction itself |
| `counterpart_not_reciprocal` | transaction | The counterpart isn't a transfer pointing back |
| `same_account_transfer` | transaction | Both sides are on one account |
| `transfer_not_net_zero` | transaction | A matched pair doesn't add up to exactly zero (in one currency) |
| `transfer_from_foreign_account` | transaction | A transfer on an account the user hasn't marked as their own |
| `duplicate_transaction_id` | transaction | Two transactions share an id, so one would be counted twice |
| `outflows_dont_add_up` | ledger | Per currency, category, transfer and Needs-review outflows don't sum to total money out |
| `matched_transfers_not_net_zero` | ledger | Per currency, matched transfers don't net to zero |

An unmatched transfer (`counterpart: null`) is money moving to or from an own account outside the ledger, so it isn't required to net to zero.

### Constraint 8: every decision shows its source
The type guarantees every decision has a source. This checks that the source can be followed:

| Code | Subject | Meaning |
|---|---|---|
| `unknown_transaction` | decision | It's about a transaction that isn't in the ledger |
| `unknown_rule` | decision | It names a rule that isn't in the ledger |
| `model_id_missing`, `model_hash_missing`, `prompt_version_missing` | decision | A model decision doesn't fully identify the model: a blank id, an all-zero GGUF hash, or a blank prompt version |
| `taxonomy_version_missing` | decision | It doesn't say which category list it was made against |
| `unknown_replaced_decision`, `replaces_itself`, `replaces_other_transaction` | decision | Its `replaces` link is broken |
| `duplicate_decision_id`, `duplicate_rule_id` | decision, rule | Two rows share an id, so neither can be traced |
| `unknown_correction_transaction`, `unknown_correction_decision` | rule | A correction-learned rule's source is missing |
| `correction_about_other_transaction`, `correction_not_by_user` | rule | The correction it names is about another transaction, or wasn't made by the user |

## Tests (`tests.rs`)
- **Must pass:**
  - The T0.5 sample ledger, which covers every type, variant, source and state.
  - TD.1's four households, with one applied user decision per ground-truth bucket and merchant, built from `truth.json` in the test.
- **Must fail:** every code above except `totals_not_reproducible` has a broken fixture (constraint 2's include a rule, not only the model, replacing a correction). Each one is the sample with one thing broken, and the test checks that exact code, on that subject, under that constraint.
- **Also checked:** a transfer pair refining a rule's unpaired transfer passes constraint 2. An unlabelled TD.1 ledger fails `no_bucket` for every transaction. Reports are identical when rows are reversed, and contain no financial data. A custom check plugs into `Checker`.
