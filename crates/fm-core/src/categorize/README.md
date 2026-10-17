# Categorisation by fixed logic (`fm_core::categorize`)

Merchant clean-up, the user's rules, and corrections that become rules (T2.2). The model fallback (T2.5) runs after this, for what's left. The module docs in `mod.rs` hold the policy in full: who wins (constraint 2), seen decisions (constraint 7), matching, fitting a rule's bucket to the transaction's sign, and corrections.

```text
import (T1.3) → transfers::detect (T1.4) → categorize::run (rules, clean-up) → model (T2.5)
```

## Files
| File | What |
|---|---|
| `normalize.rs` | Text normalisation and whole-word matching |
| `merchant.rs` | `builtin/merchant_cleaner` v1. Aliases are in `assets/merchants/aliases.json`, versioned with it |
| `rules.rs` | Matching rules, choosing winners (bucket and merchant separately), fitting buckets to the sign |
| `plan.rs` | The pure planner and `decide`, the precedence policy T2.5 shares |
| `apply.rs` | Writing plans to a `Ledger` or, in one savepoint, to the database; `run` |
| `correct.rs` | Corrections: the user decision, transfer pairing and unpairing, the learned rule, the re-run. Income or a refund for money going out is refused (`DoesNotFit`, constraint 4) |
| `actions.rs` | The user's actions (T2.6): `set_rule_enabled`, `delete_rule` (its decisions go by the storage cascade; those transactions are planned again), `undo_rule_decisions` (supersedes what a rule decided and plans those transactions without it), `accept_suggestion` and `dismiss_suggestion` (both sides of a transfer pair together; a dismissal sticks: fixed logic and the model never bring the same assignment back, though the user's own rules can); `confirm_decisions` and `reject_decisions`, the review queue's bulk approve and reject (SPEC decision 46: a decision is seen once the user acts on it). Confirming marks decisions seen and skips those already seen; rejecting sends a model's or fixed logic's bucket back to Needs review, both sides of a pair, and sticks like a dismissal. Each batch is all or none |
| `tests.rs` | The policy row by row, rules against pairs and user decisions, suggestions, corrections; the done criterion through an encrypted database with TD.1's renter |
| `td1_eval.rs` | The rules-only baseline below |

The constraint 2 check is `contract/user_inputs_win.rs`.

## Baseline on TD.1
`cargo test -p fm-core --lib td1 -- --nocapture` prints it (lines starting `T2.2-BASELINE`). For each household, a simulated user corrects their top k merchants (by number of transactions), each time the first transaction still wrong, with the truth bucket and the default learned rule. Refunds count as correct when the category is right (a rule can't know the original purchase).

Two scenarios over all 7,376 transactions of the four households, 618 of which are halves of transfer pairs:
- **Rules alone** (`T2.2-BASELINE`): the pairs stay in Needs review.
- **Transfer detection, then rules** (`T2.2-BASELINE+T1.4`): the order the app uses after an import. T1.4 pairs all 618 correctly.

| k per household | Corrections | Rules alone: correct / wrong / review | T1.4 then rules: correct / wrong / review |
|---|---|---|---|
| 0 | 0 | 0.0% / 0.0% / 100.0% | 8.4% / 0.0% / 91.6% |
| 5 | 20 | 21.3% / 0.0% / 78.7% | 29.6% / 0.0% / 70.4% |
| 10 | 40 | 31.2% / 0.3% / 68.5% | 39.6% / 0.3% / 60.1% |
| **20** | **80** | **46.0% / 0.4% / 53.6%** | **54.4% / 0.4% / 45.2%** |
| 40 | 160 | 62.3% / 0.4% / 37.3% | 70.7% / 0.4% / 28.9% |

The wrong ones are TD.1's hard cases:
- `APPLE.COM/BILL` is Apple Music or iCloud, told apart only by the amount (24). A learned rule has no amount condition.
- DoorDash orders from Chipotle, where the newer "CHIPOTLE" rule wins (6).

**Merchant clean-up:** 64.4% of the 6,684 transactions with a merchant get exactly the truth name (33.2% without the alias table). Most misses are truncated names (`Blue Bottle Coff`) or names that run into the city; one merchant correction fixes them all through a merchant rule.
