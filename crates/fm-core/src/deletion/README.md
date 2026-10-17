# Deleting data (`fm_core::deletion`)

This module implements decision 28 (deletion by import, account or date range with
everything derived; "delete everything" destroys the keys first; warn about old
backups), decision 42 (which date a transaction belongs to) and decision 43 (original
files only partly in scope).

Storage carries out a deletion in one transaction (`Unlocked::delete_planned`, T0.10).
This module decides what goes:
- which original files are deleted whole;
- which are only partly in scope;
- what the user sees before confirming.

| File | Contents |
|---|---|
| `mod.rs` | `plan`, `preview`, `delete`, `delete_everything`, the backup offer |
| `tests.rs` | Plan, cascade, delete-everything and backup-offer tests on real encrypted ledgers |

The app side is `app/src-tauri/src/deletion.rs` (commands) and `app/src/routes/data/`
(the screen).

## Deleting part of the data
**Targets.**
- **An original file:** the file, all its transactions, and everything derived from them.
- **An account:** its transactions, its own files, the rules scoped to it, and the
  account itself.
- **A date range** (optionally in one account): transactions whose date is in the range,
  both ends included. A transaction's date is its purchase date when the bank gives one,
  otherwise its posted date (decision 42), so "delete March" matches what the views call
  March.

**What else goes.** Everything derived from the deleted transactions: categorisations
and other decisions about them or pointing at them (transfer counterparts, refund
originals), rules learned from them, and the decisions those rules made. A surviving
decision that replaced a deleted one stays, with `replaces` cleared. Deleting one side
of a transfer removes the decisions pointing at it; the other side stays and falls back
to "needs review".

**Original files (decision 43).** A file is only ever deleted whole, because original
files are kept exactly as they came in.
- A file whose rows are all in scope is deleted.
- A file only partly in scope is listed before confirming, with its rows inside and
  outside the deletion. It is kept unless the user ticks "Also delete these files whole"
  (`PartialFiles::DeleteWhole`), which removes its out-of-scope transactions too.
- A kept file records the rows deleted from it (`deleted_rows`), so they never come back
  from that file. The screen says: "The original file still contains these
  transactions, encrypted in your ledger; they won't come back from it."
- When deleting an account, a file recorded as that account's own must go whole even if
  it holds other rows, because the account can't be deleted while a file refers to it.
  It is listed as such.

**Preview.** `preview` runs the same plan as a dry run inside storage's transaction and
rolls it back. The counts shown are exactly what the deletion will remove, and the
ledger is left unchanged.

## Delete everything
In this order, keys first (decision 28):
1. The Touch ID copy of the data key in the Keychain is deleted.
2. The ledger is locked, which wipes the data key from memory.
3. The keyfile, which holds the password and recovery-key slots, is overwritten with
   zeros, synced and deleted (`Vault::shred_keys`).
4. The database and the ledger folder are removed.

Any copy left behind (APFS snapshots, Time Machine, a synced copy of `ledger.db`) can
never be decrypted, and the saved recovery key no longer unlocks anything. The test
`deleting_everything_destroys_the_keys_first_and_leftovers_stay_unreadable` checks:
- the order of the steps;
- that a copy of the database taken beforehand is refused by the password and by the
  recovery key;
- that it holds no plaintext.

**Honest limits.** On SSDs and copy-on-write file systems, the old blocks of deleted files
may survive physically, and snapshots and backups keep their own copies. The guarantee
is that what is left can't be read, not that no copy exists.

**The Keychain must be cleared first.**
- If deleting the Touch ID copy fails, nothing is destroyed (`KeychainNotCleared`).
- An unsigned or development build can't see an item that a signed build of the app
  stored, so it can't vouch that the key is gone. It stops with `KeychainUnverifiable`
  and destroys nothing until the user, told to remove the "fin-manager" item in
  Keychain Access (or to delete from the signed app), chooses to go ahead
  (`an_unverifiable_keychain_needs_the_users_go_ahead`).

**Confirmation.** The user types "delete everything"; the app checks the phrase again in
Rust.

## Old backups (decision 28)
Deleting data doesn't reach older backups. After any deletion the result carries a
`BackupOffer`: this ledger's backups in the backup folder. The screen shows:

> Your older backups still contain the data you just deleted, and they open with the
> password or recovery key that was current when they were made. Delete the N backups
> in *folder* too? Copies you moved or copied elsewhere are outside the app's reach.

Accepting calls `backup::delete_all(folder, ledger_id)`, which deletes this ledger's
backups only. After "delete everything" the folder and ledger id are read before the
ledger is destroyed.

In the app, the offer is kept server-side. The page can only accept it: it never passes
a folder or an id, the offer works once, and it still works when no ledger is left.

## Tests (done when)
**Cascades** (`check_deletion`) run every target on every TD.1 ledger (every account,
several ranges, three files each, both file options) and on the sample ledger, which has
transfers, a refund, learned rules and a `replaces` chain. Each case checks, against an
untouched copy of the same ledger:
- **Preview:** its counts equal the deletion's, and the preview changed nothing.
- **Transactions:** exactly the in-scope ones, plus those of files deleted whole, are
  gone.
- **Derived data:** every removed decision and rule was derived from removed data, and
  the foreign-key check passes (nothing derived is left behind).
- **Survivors:** every surviving row is unchanged, apart from a cleared `replaces`.
- **Kept files:** they record exactly their deleted rows.

The cases assert that they actually met decisions, rules, recorded rows and a cleared
`replaces`.

**Plan:** checked against an independent computation over the ledger data
(`the_plan_matches_an_independent_computation`).

**Delete everything:** see above. **IPC:** `delete_everything_then_the_backup_offer` and
`preview_and_delete_an_account_through_the_app` (fm-app).

## Files covering a deleted period (decision 44)
A different file covering a deleted period, such as a re-saved copy or a later
overlapping export, is imported in full, with a warning: "This file covers a period you
deleted earlier" (`OVERLAP_WARNING`).

**What is recorded.** Each real deletion, never a preview, records what it covered in the
ledger's `meta` table: one row `deletion.scope.<n>` = `account|from|to`. That is an
account id (or empty for any account) and two ISO dates, with no transaction details
(`deleted_scopes_hold_no_transaction_details`).
- **A range:** its account and dates.
- **A file:** its account and its transactions' date span.
- **An account:** any account, over that account's date span. A file imported again
  later creates a new account with a new id, so matching on the deleted id would miss
  exactly the re-import the warning is for. Over-warning is the safer side.

The scope is read before the deletion and written right after it commits. If the app
stops in between, that one deletion goes unrecorded and a later overlapping file isn't
flagged; the data itself is deleted either way.

**Checking an import.** `overlaps_deleted(ledger, account, from, to)` takes the new file's
account (`None` if not known yet) and statement period, and says whether to show the
warning; the import goes ahead either way. The import summary (T5.5) calls it.

**TODO (decision 44):** refine this, for example by skipping the overlapping rows by
default with a "bring them back" choice.

## Known gaps
- **Overlapping files are only flagged.** A different file covering a deleted period
  brings the deleted rows back, with the warning above; refining this is the decision-44
  TODO.
