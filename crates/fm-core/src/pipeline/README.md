# Import pipeline (`fm_core::pipeline`)

A downloaded bank file goes in; the ledger's accounts and transactions come out. Call it like this:

```rust
let report = pipeline::import_file(&mut unlocked, &ImportRequest {
    bytes, file_name: Some("chase.csv"), target: Target::Account(id),
    csv_currency: Currency::USD, csv_profiles: &profiles, now: SystemTime::now(),
})?;
```

## Steps
Steps 1–4 run in one `SAVEPOINT`: an import is stored completely or not at all.

1. **Raw layer.** The bytes and their SHA-256 are stored as a `RawImport`.
   - Fields set: `source` (`csv`, `ofx`, or `qfx` when the file carries `INTU.BID`), the file name, the CSV mapping profile, and the account when the file has one statement.
   - **The same bytes twice are refused** (`AlreadyImported`) and nothing is written. A re-saved copy with different bytes goes through, and every row in it comes out as a duplicate.
   - **Rows deleted out of a kept import stay deleted (SPEC decision 43).** `Unlocked::deleted_rows(import)` lists the source indexes a deletion removed while keeping the file. Anything that re-parses a stored raw import (a re-import, a parser upgrade, a repair) must skip those rows. No code re-parses stored imports yet. Known gap: a re-saved copy with different bytes is a new import, so rows the user deleted from the original come back from it as new transactions. Detecting that is policy for T5.3.
2. **Parse.** A file whose header says OFX goes to `fm-import-ofx`. Anything else is read as CSV by `fm-import-csv`, which recognises the layout from its profiles. Both return `fm_import_types::Statement`s.
3. **Accounts.**
   - With `Target::Account(id)`: that account.
   - With `Target::NewAccount { name, kind, currency }`: a new account.
   - With `Target::Auto`, in order:
     1. The account whose `external_id` is the statement's account number.
     2. An account with the same last four digits at the same institution.
     3. A new account, with its kind, currency, institution, last four digits and number taken from the file.
   - A CSV that says nothing about its account gives `AccountNeeded`, and the UI asks the user. A statement in another currency than its account's gives `CurrencyMismatch`.
4. **Transactions.** Each record is matched against the account's existing transactions (`matcher.rs`, below). Duplicates are skipped and pending → posted pairs are settled. Everything new is inserted with an **applied Needs review decision** (source `builtin/import_pipeline v1`), so every transaction is in a bucket (constraint 4).
   - The record's description, payee, memo, both dates, amount, original amount, status, external id, `trn_type` and `check_number` are kept.
   - Balance and bank category aren't stored per transaction. The raw bytes keep them.
5. **Backup.** After the commit, `backup::after_import` runs (T5.2) and its result goes in the report.

The `ImportReport` gives each record's outcome by position (`Inserted`, `Duplicate`, `PostedPending` or `PendingAlreadyPosted`, each with a transaction id), plus totals.

## Matching
Wrongly merging two real transactions makes one silently disappear. That is worse than a duplicate the user can see, so every match needs agreeing evidence, and anything ambiguous falls through to new.

- **Per account, deterministic, and order-independent.** The whole statement is classified at once.
- **Multiset matching.** Each existing transaction can be claimed by at most one record per import, so three identical coffees on one day stay three transactions.
- **No self-matching.** Rows of the import being matched never match each other by content. The one exception: OFX files can hold overlapping statements of one account, so a row in another statement of the same file matches by bank id, as an exact repeat (same status, amount and posted date).

**Text.**
- **Fields:** descriptions are compared with descriptions and memos with memos. Every field both rows have must agree, and at least one must be compared. A generic description ("POS PURCHASE", with the merchant in the memo) with different memos is two merchants, and so is a generic memo with different descriptions.
- **Combined text:** when only one row has a memo, its description and memo together may equal the other's description. Some banks' CSVs print NAME + MEMO as one text (PNC, TD.5 case 13).
- **Agreement:** after normalising (upper case, non-alphanumeric runs to one space), texts agree **exactly**, or by **prefix** when the shorter is at least 24 characters. That length is where OFX cuts `NAME` at 32 characters, so a short generic text never prefix-matches a longer one.
- **Ranking:** a match is exact only if every compared field is exact, and exact ranks above prefix.

**Check numbers.** When both rows give one, they must be equal.

**Dates.** Every row knows which dates its *source* gives:
- **Posted only:** OFX without `DTUSER`, or a CSV with only a posted-date column.
- **Purchase only:** a CSV with only a transaction-date column.
- **Both:** OFX with `DTUSER`, or a CSV with both columns.
- **Unknown:** a deleted CSV profile, or a manual entry.

New rows take the shape from their file. Existing rows take it from their own import's source and profile.

Dates are compared like with like, strongest first:
- **Both:** both rows give both dates, and both agree.
- **Posted:** both give a posted date and they agree. If both also give a purchase date, it must agree too.
- **Purchase:** both give a purchase date and they agree.
- **Lagged(d):** a purchase-only row and a posted-only row, posted 0 to 3 days after the purchase. Accepted **only when each is the other's unique candidate** (same amount and text) within ±3 days. A daily fare is ambiguous and stays new.

Consequences:
- A posted date is never compared with a purchase date. A Chase QFX ride posted 02-28 is not the CSV ride bought 02-28 and posted 03-01.
- When both rows give both dates, the purchase dates must agree.

| Pass | Record | Rule | Outcome |
|---|---|---|---|
| 1, in file order | any | Same bank id (FITID or CSV reference), **and** the same amount and check number, a date within 3 days, **and** agreeing text or an exact date. A recycled short id on another merchant two days later is not it. For a pending/posted pair, the settlement rule below must also agree | Duplicate, settles, or already posted. Otherwise the id was reused and the record falls through |
| 2, the whole statement | posted | Same amount, agreeing check number, a same-kind date match (not Lagged) and agreeing text as an existing *posted* transaction. Pairs are assigned best first: (date, text, record, id) | Duplicate |
| 2 | pending | The same against *any* transaction | Duplicate, or already posted |
| 3, the whole statement | any | A Lagged pair that is unique on both sides | Duplicate, or already posted |
| 4, the whole statement | posted | It **settles** an existing pending transaction: same sign; dated 0–7 days later; amount 10%–135% of the pending one (tips up, fuel holds down); agreeing text. Pairs are assigned best first across the statement: nearest date, then stronger text, then nearer amount. A later visit at the pending price can't take a pending row from its real tipped settlement | `post_pending`: the pending transaction becomes this one, keeping its id and decision |
| 5, the whole statement | pending | The same rule, reversed, against existing posted transactions | Already posted, so dropped |
| 6, the whole statement | pending + posted rows of *this* statement | The settlement rule between two rows of the file. The earlier row is written: a pending row first is inserted and the posted row settles it; a posted row first is inserted and the pending row points at it | One transaction (TD.5 case 10) |
| — | any | Otherwise | Inserted |

A pending transaction that never posts stays pending. Transactions are immutable, and removing stale pending ones belongs to deletion (T5.3).

**Accounts with `Target::Auto`.** A full account number (at least 8 characters, unmasked) matches on its own. A short or masked one (`XXXX1234`), or the last four digits, matches only at the same institution and with the same kind of account (when the file says), because a checking account and a card at one bank can share them. OFX's terse identities (`ORG C1`, `INTU.BID 7492`) are mapped to the names the CSV profiles use ("Capital One"), so a bank's QFX and CSV land on the same account (`institution.rs`). An unknown bank is named by ORG and FID together.

**Named targets.** `Target::Account` and `Target::NewAccount` take a file whose statements are all one account (several statement periods), resolved or created once. Files with several accounts need `Target::Auto`.

**Atomicity.** The import runs in a savepoint whose guard rolls back on any early return or panic, and also when `RELEASE` itself fails. An import refuses to start inside another transaction (`TransactionOpen`).

## Tests (`tests.rs`, `matcher.rs`)
- **The whole TD.2 corpus, per persona, in file order and in reverse.** That is 50 files: CSV, QFX and OFX, 1 to 6 overlapping exports per TD.1 account. Every row is imported to its TD.1 account. Then:
  - All rows of one `truth_id` land on one transaction, and different truths on different transactions: zero duplicates and zero false merges.
  - Posted transactions carry TD.1's amount on a date one of the files gave.
  - The contract check passes.
  - Re-importing all 50 files is refused file by file and leaves the ledger unchanged.
- **The 7 pending → posted cases** (Citi tips, USAA and Schwab posting days later, a Citi pending settled from an Amex export of the same TD.1 account) each merge into the pending transaction at the posted amount.
- **Other tests:**
  - Automatic account matching for OFX by number, and masked numbers needing the same institution.
  - Two statements of one file.
  - A CSV with no account.
  - Rollback on failure, and the savepoint closed on every path (error, panic, open transaction).
  - Source facts kept.
- **TD.4 and TD.5's adversarial fixtures** (`testdata/import-edge-cases`, 17 cases): every row's expected outcome in `import_order`, then the expected final transactions in import order and again in reverse order on a fresh ledger. Under `Target::Auto` (case 04), the files resolve to exactly the case's accounts.
- **Matcher unit tests** for every rule above, including review 1's cases:
  - the daily-fare month boundary;
  - generic memos;
  - a reused bank id;
  - the nearest settlement winning regardless of row order.

## For the UI (T3.3, T5.5)
A row matched to an existing transaction creates no decision of its own, so a wrong match would be invisible. The `ImportReport` gives every record's outcome. The import screen should show "N rows matched existing transactions" with a drill-down, so the user can spot a wrong merge.
