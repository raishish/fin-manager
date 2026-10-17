# Backups (`fm_core::backup`)

This module implements SPEC §8 and decisions 17 and 28: encrypted copies of the ledger,
saved to a folder the user chooses. A backup is made daily and after each import, the
last 7 daily, 4 weekly and 12 monthly are kept, and a backup can be restored on a fresh
install with the password or the recovery key. We never host backups. The folder is
only a local path: no network is used, and a cloud-synced folder is the user's own
choice.

| File | Contents |
|---|---|
| `format.rs` | The backup file format, streamed in both directions |
| `name.rs` | File names, which carry the time |
| `rotation.rs` | Which backups to keep, as a pure function |
| `mod.rs` | Making, listing, rotating, deleting and restoring backups; settings and schedule |
| `tests.rs` | End-to-end tests on a real encrypted ledger |

The app side is `app/src-tauri/src/backup.rs` (commands, the daily step on the
5-second timer, restore under the session lock) and `app/src/routes/backup/` (`/backup`
settings, `/backup/restore` for a fresh install).

## A backup file
`fin-manager-YYYYMMDDTHHMMSSZ.fmbackup`, or `…Z-N.fmbackup` for a second backup in the
same second. The time is UTC.

**Format.** A small header (magic, format version, time, app version, lengths), then
`ledger.keys` and `ledger.db` byte for byte, then a SHA-256 over everything before it.
The layout is at the top of `format.rs`.

**Nothing is re-encrypted.** Both files are already encrypted at rest, so a backup is
unreadable without the password or the recovery key, wherever it is stored. The hash
catches truncation and corruption, such as a half-synced cloud copy or a bad drive.
Deliberate tampering is caught anyway, by the keyfile's AEAD and SQLCipher's page
HMACs.

**Streaming.** Copying, verifying and restoring go through a 64 KiB buffer, so a backup
never has to fit in memory. Declared lengths are checked against the file size before
anything is read, and a keyfile over 1 MiB is refused.

## Making a backup
1. Check the folder: an absolute path to a folder that exists and isn't inside the
   ledger's own folder. A missing folder (an unplugged drive) is an error and is never
   created.
2. Remove temporary files from earlier backups that never finished (older than
   10 minutes).
3. Take `BEGIN IMMEDIATE` on the ledger's connection, so nothing can write to the
   database. The keyfile can't change either: password and key changes need
   `&mut Unlocked`, and the backup holds `&Unlocked`. The journal mode is the default
   rollback journal, so every committed change is in `ledger.db`.
4. Stream both files into a hidden temporary file (`.fin-manager-….fmbackup.tmp`, mode
   0600), fsync it, then `ROLLBACK` (it was a read-only transaction).
5. Rename it into place and fsync the folder.
6. Read the backup back and check its hash. A backup that fails the check is deleted
   and reported. Cloud folders sometimes fail without an error.
7. Apply the rotation.

**When backups run.** Only while the ledger is unlocked, which is the only time data
can change.
- `daily()`: called from the app's timer; makes a backup if the last one is at least
  24 hours old, or if the clock has moved back past it.
- `after_import()`: for T1.3 to call after each import; always makes one.
- `back_up_now()`: the button on the settings screen.

All three record the outcome in the ledger's `meta` table: `backup.folder`,
`backup.last_at`, and `backup.last_error` (cleared by the next success). A failure
never stops the app; the settings screen shows it.

## Rotation (decision 28)
The rotation keeps:
- the newest backup of each of the 7 most recent UTC days that have one;
- the newest of each of the 4 most recent ISO weeks that have one;
- the newest of each of the 12 most recent months that have one;
- the newest backup overall.

It counts "periods that have a backup", not calendar days. A machine that was off for a
month therefore still keeps a week of dailies, and a wrong clock can't make the rule
delete everything.

Only files whose names parse as ours are considered. Temporary files and everything
else in the folder are never touched (`rotation_keeps_7_4_12_and_never_touches_other_files`).

## Restore (fresh install)
`restore(file, vault)` only acts when the vault is `VaultState::Missing` and its folder
is absent, empty, or holds just the lock file. The steps:
1. Stream the backup into a new private staging folder next to the target (0700
   folder, 0600 files), checking the hash.
2. fsync it.
3. Rename it into place in one step.

So an interrupted or failed restore leaves nothing behind, and nothing that exists is
ever overwritten (`restore_refuses_where_a_ledger_exists`,
`restore_never_touches_a_folder_with_other_files`, `a_damaged_backup_restores_nothing`).

In the app, `restore_backup` is the only data-changing command allowed while locked. It
runs under the session lock and only when the status is `NoLedger`; the IPC test
`restore_backup_only_runs_on_a_fresh_install` covers it. The restored ledger then opens
through the normal unlock.

**A backup opens with the password that was current when it was made.** Changing the
password re-wraps the key slot in the live keyfile only. Older backups keep the old
slot and need the old password, or the old recovery key if it was replaced since. The
`/backup` and `/backup/restore` screens say so; the test is
`a_backup_from_before_a_password_change_needs_the_old_password`.

## Deleting data and old backups (decision 28, for T5.3)
Deleting data from the ledger doesn't reach older backups. After a deletion, T5.3 calls
`list(folder)` to count them and offers `delete_all(folder)`, which deletes only our
backup files.

Suggested wording:

> Your older backups still contain the data you just deleted. Delete the N backups in
> *folder* too? Copies you moved or copied elsewhere are outside the app's reach.

`delete_all` unlinks files rather than overwriting them first. On SSDs, copy-on-write
file systems and cloud folders, an overwrite wouldn't reach the old blocks or the
provider's version history anyway, and every backup is encrypted. After "delete
everything" (keys destroyed first), old backups still open with the old password or
recovery key, so deleting them is the only way to make them go away.

## Known gaps
- **UTC days.** "Daily", the rotation's days and the file names use UTC, so a day can
  be a few hours off from local midnight. There is no time-zone dependency.
- **Folder by path.** The folder is typed as a path for now. The native folder picker
  comes with T5.5's first-run flow.
- **Cloud folders.** Cloud providers may keep their own version history of deleted
  backups, which the app can't reach. The warning above mentions copies outside the
  app's reach.
