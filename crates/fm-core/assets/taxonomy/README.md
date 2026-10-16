# Default category list

`default-v1.json` is the category list every install starts with (SPEC decision 27):
17 groups and 66 categories, in the `Taxonomy` JSON form described in
`crates/fm-core/src/model/README.md`. Its version is `{"base": "1", "revision": 0}`.

## Licence check
The list was written for this project. The keys were first proposed by the synthetic
test data task (TD.1); the display names were written here. No third-party taxonomy was
copied, including Plaid's personal-finance categories, so no outside licence applies.
The file is covered by the crate's licence (AGPL-3.0-or-later).

## Keys are stable
- A key (`group` or `group.category`) never changes once shipped. The test data's ground
  truth, the model evals and users' decisions all refer to keys or to ids derived from them.
- Display names can change freely; that's a new **revision**, not a new base.
- Adding, removing or re-keying a category means a new base version (`default-v2.json`),
  a migration for existing installs, and an update to the TD.1 ground truth.
- Income, Transfer, Refund and Needs review are buckets (`Bucket`), not categories, so
  they don't appear in this file.

## Labelling conventions
Used by the TD.1 ground truth, the rules engine and the model prompts, so everyone
labels the same transaction the same way:

1. **Moves between the user's own accounts are transfers,** including credit-card
   payments. The `counterpart` is the other side when that account is in the ledger, and
   `null` when it's an own account that isn't tracked (for example a brokerage or 529
   contribution).
2. **Mortgage and car-loan payments to a lender whose account isn't tracked are spending:**
   `housing.mortgage` or `transport.auto_loan`. Once the loan account is tracked, the
   payment becomes a transfer.
3. **Incoming person-to-person reimbursement for a shared cost** (a roommate's half of
   the rent, a friend's share of dinner) is a **refund** in that cost's category.
4. **A tax refund** is a refund of `taxes.income`.
5. **Salary, interest, pensions, Social Security and freelance client payments** are
   **income**.
6. **ATM cash** is `financial.cash_withdrawal`, **card interest** is `financial.interest`,
   and **foreign-transaction and ATM fees** are `financial.bank_fees`.
7. **Paper checks** ("CHECK 1234") are labelled by what they paid for. Without a memo the
   app can't know this, so expect them in the review queue.
