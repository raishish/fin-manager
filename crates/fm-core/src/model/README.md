# Data model v0 (`fm_core::model`)

These are the types the ledger, storage, importers, categorisation, views, queries and contract checks all share. This document gives their JSON form, the units and the rules that hold everywhere. Rust doc comments on each type carry the same information in more detail.

The JSON form is stable. Test fixtures, the synthetic data generator's ground truth and storage's JSON columns use it, and `round_trip_tests.rs` pins it with golden JSON, so a change here means a change there.

## Conventions

| Topic | Rule |
|---|---|
| **Amounts** | `Money` = `{"minor": <i64>, "currency": "<ISO 4217>"}`. `minor` is an integer count of the currency's minor unit: cents for USD, yen for JPY (exponent 0), fils for KWD (exponent 3). No floats anywhere (SPEC constraint 1). The exponent comes from the currency table, never an assumed 2. |
| **Sign** | Seen from the account: **negative = money left the account, positive = money came in**, for every account type. A card purchase is negative on the card; the card payment is negative on checking and positive on the card. Importers normalise bank-specific signs to this. |
| **Currencies** | Three uppercase letters, and only codes in the built-in ISO 4217 table: active codes plus ANG, BGN, CUC, HRK, SLL and ZWL, which were withdrawn recently but still appear in old statements. `"usd"` and `"XXX"` are rejected. |
| **Bank dates** | `Date` = `"YYYY-MM-DD"`, a calendar date with no time or zone, range 0001-01-01 to 9999-12-31. Parsing accepts only this layout; importers convert bank layouts. Month arithmetic clamps to the month's end (2026-01-31 + 1 month = 2026-02-28). |
| **Months** | `YearMonth` = `"YYYY-MM"`, the unit of the spending views. |
| **App times** | `Timestamp` = integer Unix seconds, UTC. Used for when something happened in the app (`imported_at`, `decided_at`, `seen_at`), never for bank dates. |
| **Ids** | Plain integers (`AccountId`, `ImportId`, `TransactionId`, `CategoryGroupId`, `CategoryId`, `DecisionId`, `RuleId`). Storage assigns them as SQLite rowids. They are only meaningful inside one ledger. |
| **Hashes** | `Sha256Hash` = 64 lowercase hex digits. |
| **Confidence** | Integer basis points, 0 to 10000 (9150 = 91.5%). |
| **Optional fields** | Written as `null`; a missing key also reads as `null`. |
| **Unknown fields** | Rejected on every struct and every struct-like enum variant, so a typo in a fixture fails loudly. One serde limit: unit variants inside `"kind"`-tagged enums (`{"kind": "income"}`) ignore extra keys. |
| **Enums** | `snake_case` strings (`"credit_card"`, `"simplefin"`). Enums with data use a `"kind"` tag (`Bucket`, `DecisionSource`, `RuleOrigin`) or a one-key object (`Assignment`, `TextMatch`, `AmountMatch`), as shown below. |
| **i64 in JavaScript** | Amounts above 2^53 minor units (about $90 trillion) would lose precision in a JS `number`. Out of reach for real data, but the UI shouldn't do arithmetic on amounts anyway: the core computes every figure. |

**Validation.** Primitive types reject invalid values both when constructed and when deserialised: `Date`, `YearMonth`, `Currency`, `Sha256Hash` and `Confidence` all do, and so does `RawImport`, whose hash must match its bytes. Types that refer to other records have a `validate()` method, because whether a reference is good depends on the rest of the ledger: `Taxonomy::validate` checks unique ids and keys, that every group exists, and that merges have no chains or cycles; `Rule::validate` checks for a non-blank pattern, an action and a sane amount range. Checks across the whole ledger (every transaction's currency matches its account's, transfer pairs net to zero, one applied bucket per transaction, and so on) belong to the contract checker (T1.5), not here.

**Errors never echo financial data.** `ModelError` messages can include a bad date or currency code. They never include amounts, descriptions or file contents (SPEC §7: errors can end up in crash reports).

## Types

### `Account`
| Field | Type | Notes |
|---|---|---|
| `id` | AccountId | |
| `name` | string | Display name, user-editable |
| `kind` | `checking` \| `savings` \| `credit_card` \| `cash` \| `loan` \| `mortgage` \| `brokerage` \| `retirement` \| `other` | All kinds exist now (SPEC §2). `in_mvp()` is true for checking, savings, credit_card and cash. `is_liability()` is true for credit_card, loan and mortgage |
| `currency` | Currency | The currency transaction `amount`s on this account are in |
| `institution` | string? | |
| `masked_number` | string? | Last few digits only, never the full number |
| `external_id` | string? | Source's account id (SimpleFIN id, OFX `ACCTID`), to match later imports. May be a full account number; it lives only in the encrypted DB |
| `own_account` | bool | User designation for transfer matching. A user input, never changed by a model (constraint 2) |
| `created_at` | Timestamp | |

### `RawImport`
The original bytes of one file or SimpleFIN response, kept until the user deletes them (SPEC decision 28).

| Field | Type | Notes |
|---|---|---|
| `id` | ImportId | |
| `source` | `csv` \| `ofx` \| `qfx` \| `simplefin` \| `manual` | |
| `file_name` | string? | Original file name |
| `mapping_profile` | string? | CSV mapping profile used (owned by the CSV importer) |
| `account_id` | AccountId? | Set when the source covers one account (CSV/OFX); `null` for a multi-account SimpleFIN response |
| `imported_at` | Timestamp | |
| `sha256` | Sha256Hash | Always the SHA-256 of `bytes`: computed by `RawImport::new` and re-checked on deserialisation (`HashMismatch` if they differ). Identical files share it, so storage can refuse duplicate files |
| `bytes` | lowercase hex string | The original bytes. Read-only in Rust (`bytes()`); storage keeps them in a BLOB |

### `Transaction`
What the source reported, normalised. Derived facts such as the cleaned merchant and the category are `Decision`s, so these raw fields are never overwritten.

| Field | Type | Notes |
|---|---|---|
| `id` | TransactionId | |
| `account_id` | AccountId | |
| `source` | SourceRef | `{"import_id", "index", "external_id"}`: which raw import (needed for cascade deletion), the zero-based record position in it, and the source's own id (OFX `FITID`, SimpleFIN id), used to remove duplicates |
| `status` | `pending` \| `posted` | The pipeline replaces a pending transaction with the posted one |
| `posted_date` | Date | For pending: the date the source reports |
| `transaction_date` | Date? | Purchase date, when given separately |
| `amount` | Money | Signed, in the account's currency |
| `original_amount` | Money? | Foreign-currency amount when it differs from the account's (same sign) |
| `description` | string | Exactly as received |
| `payee` | string? | OFX `NAME` or similar |
| `memo` | string? | OFX `MEMO` or similar |

### `Taxonomy`, `CategoryGroup`, `Category`
```json
{"version": {"base": "1", "revision": 0},
 "groups": [{"id": 1, "key": "food", "name": "Food & drink", "hidden": false}],
 "categories": [{"id": 1, "group_id": 1, "key": "food.groceries", "name": "Groceries",
                 "hidden": false, "merged_into": null}]}
```
- `version.base` is the shipped default list's version (T2.1). `version.revision` counts the user's edits on top of it. Shown as `1+r0`.
- `key` is a stable machine name that survives renames. Keys from the default list are identical in every install, so evals can compare results across users.
- Renaming changes only `name`. Decisions refer to the id, so past transactions follow the rename automatically.
- Merging sets `merged_into`. `Taxonomy::resolve(id)` follows it, one level only (validation forbids chains).
- Hiding a category removes it from pickers and from the model's list, `Taxonomy::selectable()` (constraint 3). Existing decisions keep it.

### `Bucket`
Where a transaction's money counts. A transaction has exactly one applied bucket (constraint 4).

| JSON | Meaning |
|---|---|
| `{"kind": "category", "category_id": 4}` | Spending in a category |
| `{"kind": "transfer", "counterpart": 9}` | Between the user's own accounts (including card payments); `counterpart` is the other side if it is in the ledger, else `null` |
| `{"kind": "income"}` | |
| `{"kind": "refund", "category_id": 4, "original": 2}` | Offsets spending in `category_id`; `original` is the purchase, if known |
| `{"kind": "needs_review"}` | Not confident enough (constraint 5), including a model answering "uncategorised" |

### `Decision`
Every categorisation and merchant clean-up, from any source. Decisions are append-only: a change adds a new decision that `replaces` the old one, and the old one becomes `superseded`.

| Field | Type | Notes |
|---|---|---|
| `id` | DecisionId | |
| `transaction_id` | TransactionId | |
| `assignment` | `{"bucket": Bucket}` \| `{"merchant": "<name>"}` | |
| `source` | DecisionSource | See below (constraint 8) |
| `taxonomy_version` | TaxonomyVersion | Category list in force when it was decided |
| `decided_at` | Timestamp | |
| `state` | `applied` \| `suggested` \| `dismissed` \| `superseded` | Only `applied` counts in views. A model re-run that disagrees with a decision the user has seen becomes `suggested` (constraint 7) |
| `seen_at` | Timestamp? | When the user first saw it. Once set, no silent changes by a model |
| `replaces` | DecisionId? | The decision it replaced (or, for a suggestion, would replace) |

`DecisionSource`:

| JSON | Spec source |
|---|---|
| `{"kind": "user"}` | the user |
| `{"kind": "rule", "rule_id": 4}` | a rule (user-written or learned from a correction) |
| `{"kind": "builtin", "logic": "transfer_matcher" \| "merchant_cleaner", "version": 1}` | a rule: fixed logic shipped in the app, versioned |
| `{"kind": "model", "model_id": "...", "gguf_sha256": "<hex>", "prompt_version": "...", "confidence": 9150}` | the model, with exact identity and confidence |

`DecisionSource::is_user_input()` is true for `user` and `rule`. These always beat model decisions (constraint 2).

### `Rule`
```json
{"id": 1,
 "matcher": {"description": {"contains": "TRADER JOE"}, "account_id": 2,
             "amount": {"between": {"min": {"minor": 0, "currency": "USD"},
                                    "max": {"minor": 100000, "currency": "USD"}}}},
 "action": {"bucket": {"kind": "category", "category_id": 1}, "merchant": "Trader Joe's"},
 "origin": {"kind": "correction", "transaction_id": 2, "decision_id": 3},
 "priority": 10, "enabled": true, "created_at": 1760002000}
```
- `matcher.description` is one of `{"equals": s}`, `{"starts_with": s}` or `{"contains": s}`, matched case-insensitively against the raw description. The exact normalisation belongs to the rules engine (T2.2).
- `matcher.amount` is `{"exact": Money}` or `{"between": {"min", "max"}}`, both ends inclusive, on the signed amount. `null` means any amount.
- `action` sets a bucket, a merchant or both; at least one.
- `origin` is `{"kind": "user"}` or `{"kind": "correction", ...}`. A correction-learned rule names the transaction it came from, so deleting that data deletes the rule (SPEC decision 28).
- When several rules match, higher `priority` wins; ties go to the newer rule.

### `Ledger`
`{"accounts", "imports", "transactions", "taxonomy", "decisions", "rules"}`: one user's whole ledger in memory. Used by fixtures, TD.1 ground truth and the contract checker. Storage keeps the same data in tables.

## Left for later tasks
- **Storage layout** (tables, indexes, BLOBs): T0.3.
- **Matching, de-duplication and pending → posted:** T1.3. A transaction records one `source`; if T1.3 needs every sighting of a transaction across imports, it adds a sightings relation.
- **Transfer pairing:** T1.4.
- **Rule evaluation and merchant normalisation:** T2.2.
- **Recurring-charge types:** T3.2.
- **Device-sync identifiers:** v2.
