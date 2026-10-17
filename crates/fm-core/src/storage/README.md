# Encrypted storage (`fm_core::storage`)

This module implements SPEC decision 16. A ledger is stored encrypted at rest, and its key exists only in memory while the ledger is unlocked.

## On disk
A ledger is a directory with two files:

| File | Contents |
|---|---|
| `ledger.db` | SQLCipher 4 database, using SQLCipher's defaults: AES-256-CBC pages with HMAC-SHA512, 4 KiB pages. |
| `ledger.keys` | The keyfile: the random data key, wrapped once for each way of unlocking. |
| `ledger.keys.next` | Exists only during a data-key rotation (see below). |
| `ledger.lock` | Advisory lock (no contents), held while the ledger is open. |

**Permissions (Unix):** the folder is 0700 and every file in it is 0600, so other users on the same machine can't read them. The database file is created 0600 before SQLite opens it, and SQLite gives its journals the database's mode. Keyfiles are written through a 0600 temporary file, so the mode carries over on rename. Unlocking re-applies these modes if something has loosened them.

**Both files are needed.** Without the keyfile the database can never be opened again; deleting it is how a ledger is crypto-shredded (T5.3, SPEC decision 28). **Backups must include it** (T5.2).

## Keys
```
password ──Argon2id(salt, m=64 MiB, t=3, p=4)──► KEK ──XChaCha20-Poly1305──► data key (256-bit, random)
recovery key ──Argon2id(own salt)──► KEK' ─────────┘                            │
                                                              PRAGMA key = "x'…'" (raw key, no SQLCipher KDF)
                                                                                  ▼
                                                                              ledger.db
```
- **Data key:** 32 random bytes from the OS (`getrandom`). It never changes, so changing the password rewrites one keyfile slot and leaves the database untouched.
- **Key slots:** one per secret (password, and the recovery key once T5.1 sets one up). Each slot stores its own Argon2id parameters, salt and nonce, so the parameters can be raised later without a migration. The binary layout is documented at the top of `keyfile.rs`.
- **Binding:** each slot's AEAD additional data covers the keyfile header (magic, version, random keyfile id) and the slot's kind, parameters and salt. Changing any of them, or moving a slot into another keyfile, makes the slot fail to open.
- **Failed unlocks:** every failure to open a slot is reported as `StorageError::WrongSecret`, so the error never says why.
- **Bounds on read:** Argon2 memory ≤ 256 MiB, iterations ≤ 16, lanes ≤ 16. A crafted keyfile can't make an unlock allocate or spin without limit.
- **Floor on write:** new slots need at least 19 MiB and 2 passes (the OWASP minimum for Argon2id). Only fm-core's own unit tests may go lower, because Argon2 is slow in debug builds.
- **Debug output:** printing a keyfile, slot or unlocked ledger shows slot kinds and parameters only, never salts, nonces, wrapped keys or the data key.
- **Atomic writes:** the keyfile is written to a temporary file, fsynced, then renamed over the old one. A crash leaves the old keyfile or the new one, never half of each.

**Argon2id cost.** The parameters are m = 64 MiB, t = 3, p = 4, the second recommended setting in RFC 9106 §4. Unlocking takes about 85 ms in a release build on an Apple M2, which includes opening the database. The `argon2` crate computes lanes sequentially, so p = 4 adds no threads.

## Lifecycle
| Call | Effect |
|---|---|
| `Vault::at(dir)` | A locked ledger: paths only, no key material |
| `vault.create(password)` | Writes the keyfile, then creates the database and runs the migrations |
| `vault.unlock(password)` / `vault.unlock_with_recovery_key(key)` | Returns an `Unlocked` ledger: the open connection plus the data key |
| `unlocked.lock()`, or dropping it | Closes the connection (SQLCipher wipes its own key copy on close) and zeroizes the data key |
| `unlocked.set_password(current, new)` / `set_recovery_key(current_password, key)` | Re-wraps one slot. The current password is required |
| `unlocked.rotate_data_key(password, new_recovery_key)` | New data key: the whole database is re-encrypted (`PRAGMA rekey`) and a new keyfile with a new id is written. The old recovery key stops working; pass a new one or set one later |
| `unlocked.reset_password(recovery_key, new)` | After unlocking with the recovery key: checks the recovery key and re-wraps the password slot |
| `vault.finish_create(password)` | Completes a create that crashed after the keyfile was written (`VaultState::Incomplete`) |
| `vault.shred_keys()` | Overwrites with zeros and deletes every keyfile (current, `.next`, `.tmp`). The database can never be opened again. The full delete-everything flow is T5.3 |

**One instance at a time:** opening a ledger takes an exclusive advisory lock on `ledger.lock` (`flock` on Unix). A second app instance gets `StorageError::InUse` instead of racing the first one's keyfile writes. The OS releases the lock if the process dies.

Taking the lock retries for up to 200 ms (`LOCK_RETRY`) before reporting `InUse`. A `flock` belongs to an open file description, and a child process spawned by any thread shares that description until its `exec` closes it. A lock released in that window stays held by the child for a moment, which is slow on macOS under load. A ledger that another instance really has open is still refused, 200 ms later (T0.12).

**Rotation is crash-safe:**
1. The new keyfile is written to `ledger.keys.next`.
2. SQLCipher re-encrypts the database in a single transaction.
3. `.next` is renamed over `ledger.keys`.

If the app stops partway, the next unlock repairs it. If the current keyfile opens the database, step 2 never happened and the stale `.next` is discarded. If not, and `.next` opens it, the rotation is finished.

Every connection also turns on `foreign_keys` and `secure_delete`, so deleted rows are overwritten inside the file (T5.3).

**Out of scope here:** password strength rules, recovery key generation and display, auto-lock and Touch ID belong to T5.1.

## Migrations
- `MIGRATIONS` is an ordered list of versions 1, 2, 3, and so on. The current version is stored in `PRAGMA user_version`.
- Each migration runs in its own transaction together with the version bump, so a failure leaves the previous version intact.
- The app refuses a database newer than itself (`StorageError::NewerSchema`).
- Released migrations are never edited; a change is always a new migration.
- Version 1 creates only the `meta` key/value table.
- Version 2 (`schema/v2_ledger.sql`) creates the ledger tables. When a version-1 ledger is opened, it gains the ledger tables and the default category list.
- Version 3 (`schema/v3_source_facts.sql`) makes two changes:
  - It adds `trn_type` and `check_number` to `transactions`.
  - It lets `decisions.builtin_logic` be `import_pipeline`. SQLite can't change a `CHECK` in place, so the decisions table is rebuilt with [SQLite's procedure](https://sqlite.org/lang_altertable.html): create, copy, drop, rename, then recreate the indexes and triggers.
- Version 4 (`schema/v4_deleted_rows.sql`) adds `deleted_rows`: the rows deleted out of an import that was kept (SPEC decision 43, see below).
- Version 5 (`schema/v5_reject_state.sql`) recreates the decisions trigger so that a decision the user has seen may move applied → dismissed (SPEC decision 46).
- A migration that rebuilds a table runs with `foreign_keys_off`. The runner switches foreign keys off around its transaction, so the `DROP TABLE` can't cascade into rules, and requires `foreign_key_check` to come back empty before it commits.

## Ledger tables and the repository
The tables mirror the T0.5 model (`crates/fm-core/src/model/README.md`). All of them are `STRICT` and use relational columns, never JSON blobs. Each enum is stored as its JSON name (`"credit_card"`), each `Bucket` as `(bucket, bucket_category_id, bucket_transaction_id)`, and each `DecisionSource` as one column per field. `CHECK` constraints enforce each variant's shape. `repo.rs` is the only code that reads or writes these tables, through methods on `Unlocked`:

| Methods | Notes |
|---|---|
| `insert_*`, `account`, `transaction`, `decision`, `raw_import`, `accounts`, `transactions`, `transactions_for_account`, `decisions`, `decisions_for_transaction`, `rules`, `raw_imports`, `imports_with_sha256` | Writes keep the caller's ids; `next_id(Table)` gives a free one. Reads rebuild validated model types (rules through `Rule::validate`, the category list through `Taxonomy::validate`): a row that breaks an invariant is `StorageError::InvalidRow`, which names the table and the reason, never the row's contents |
| `taxonomy`, `save_taxonomy` | The whole category list. Saving upserts by id, so a rename re-applies to past decisions. A category still in use can't be removed (`RESTRICT`); merge it instead. A new ledger starts with `assets/taxonomy/default-v1.json` |
| `save_ledger`, `load_ledger` | Write a whole `Ledger` into an empty ledger, or read it all back. Used for fixtures and the TD.1 ground truth |
| `set_decision_state`, `mark_decision_seen` | The only changes a stored decision can take (see below) |
| `post_pending` | The only change a stored transaction can take: pending → posted |
| `delete_import`, `delete_planned` (crate-private) | Cascading deletion (see below) |
| `deleted_rows(import)` (crate-private) | The source indexes deleted out of a kept import |

**What the database refuses.** These rules hold for any SQL on the app's connection, not just for the repository:

| Rows | Rule |
|---|---|
| Decisions | Append-only. An `UPDATE` may only move `state` (applied → superseded; applied → dismissed once `seen_at` is set, a user's rejection, SPEC decision 46; suggested → applied, dismissed or superseded) and set `seen_at` once, from `NULL`. An update that changes nothing is refused too. A `DELETE` happens only through the deletion API |
| Transactions | Immutable, with one exception: `post_pending` turns a pending transaction into the posted one. That may change everything except the id and the account, including the import it points to. A `DELETE` happens only through the deletion API |
| Raw imports | The id, `sha256` and `bytes` never change; metadata such as the file name can. A `DELETE` happens only through the deletion API |
| Deleted rows | Inserted only by the deletion API, never updated, and removed only with their import |
| Rules | Once a rule has made decisions, its matcher, action, origin and id are fixed (`priority` and `enabled` can still change). Changing what it does means a new rule |
| Accounts | The id and currency never change |

Violations come back as `StorageError::ForbiddenChange`.

**How it holds:**
- **`fm_permit(kind)`** is an SQL function the app registers on each connection. It returns 1 only while the matching Rust API call runs: `'delete'` during `delete_planned` (and `delete_import`, which is a one-import plan), `'post_pending'` during `post_pending`. A drop guard turns it off again. No SQL can turn it on.
- **Failing closed:** a connection without the function (another tool opening the file with the key) can't change or delete protected rows at all.
- **`recursive_triggers` is on,** so `REPLACE` / `INSERT OR REPLACE` can't get around the DELETE triggers through their implicit delete. A control test shows the bypass works when the setting is off.
- **An authorizer on the connection** refuses:
  - creating, dropping or altering tables, indexes, views and triggers (temporary triggers too);
  - `ATTACH` and `DETACH`;
  - every PRAGMA write (`foreign_keys`, `recursive_triggers`, `writable_schema`, `key`, `user_version`, ...).

  Reads and `defer_foreign_keys` are allowed. Only migrations and rekeying get a Rust-side schema permit that lifts these restrictions.
- **SQLite's defensive mode is on.**
- **`Unlocked::connection()` is crate-private.** Code outside fm-core goes through repository functions. Code inside fm-core is still policed by the triggers and the authorizer. It could re-register `fm_permit` through the Rust API, and the only defence against that is code review.

**Deleting an import removes everything derived from it (SPEC decision 28),** through foreign-key cascades, in one transaction:
```
raw_imports ─► transactions ─► decisions about them
                     │       ─► decisions that point at them (transfer counterpart, refund original)
                     └─────► rules learned from them (Correction origin) ─► decisions those rules made
decisions.replaces ─► SET NULL: a surviving decision that replaced a deleted one still stands
```
A transaction can end up with no applied decision; categorisation (T2.2) and transfer detection (T1.4) decide it again.

**A deletion plan** (`DeletionPlan`, run by `delete_planned`) removes, in one transaction and in this order:
1. **The transactions in a scope:** all of them, narrowed to one account and an inclusive date range when given. A transaction's date is its purchase date when the bank gives one, otherwise its posted date (SPEC decision 42). Each goes with the cascade above.
2. **The listed imports,** whole, with their transactions.
3. **The listed accounts,** with the rules scoped to them and those rules' decisions. An account that still has transactions or imports is refused (`AccountInUse`); transactions and imports are `RESTRICT` on accounts.

Any failure (an unknown id is `NotFound`) rolls the whole plan back. An empty plan, an inverted date range and an id listed twice are refused (`InvalidRequest`). A **dry run** does everything and rolls back, so its `DeletedCounts` are exactly what the real run removes; the tests check that the file's bytes don't change. Which files and accounts go with a scope is policy, decided above storage (T5.3, SPEC decision 43); the plan only says what to remove.

**Rows deleted out of a kept import never come back (SPEC decision 43).** When the scope deletes a transaction whose import isn't also in the plan, `(import_id, source_index)` goes into `deleted_rows`, in the same transaction (`DeletedCounts::recorded_rows`). The records go with their import (`ON DELETE CASCADE`). **Any code that re-reads a raw import's bytes must skip the rows `deleted_rows(import)` lists.**

## Hooks for T5.1 (crate-private)
- `Unlocked::data_key_copy()` returns a zeroizing copy of the raw data key, for the Keychain item behind Touch ID.
- `Vault::unlock_with_data_key(&key)` takes the same lock and runs the same permission and migration steps as `unlock`. A stale or wrong key is `DatabaseKeyRejected`; the caller falls back to the password, which also completes an interrupted rotation. After `rotate_data_key`, T5.1 must replace the Keychain copy.
- `Unlocked::reset_password(recovery_key, new_password)` checks the recovery-key slot and rewrites the password slot, for a user who unlocked with the recovery key.

Neither raw-key hook is public outside fm-core; compile-fail doctests on `Vault` check that.

## Crypto backend
SQLCipher 4.14.0 (community edition, BSD-3-Clause) is bundled by `libsqlite3-sys`. It is linked against **OpenSSL 3.6.x built from source and linked statically** (`bundled-sqlcipher-vendored-openssl`, Apache-2.0), on both macOS and Linux. That gives the same crypto everywhere, pinned by `Cargo.lock` (SPEC decision 35). Keeping `openssl-src` current is our job, and the advisory check in T0.6 flags it when it falls behind. The `sqlcipher_settings` test checks the provider and versions.

## Threat model notes
- **Old keyfile copies keep working until rotation.** Changing the password re-wraps the same data key, so any earlier copy of `ledger.keys` (a filesystem snapshot, Time Machine, a cloud-synced folder, a T5.2 backup) plus the *old* password still opens the *current* database. After a password change made because of suspected compromise, the app must call `rotate_data_key`. Rotation re-encrypts the database under a new key, and from then on old keyfile copies open only old database copies.
- **Shredding can't reach every copy.** `shred_keys` overwrites the keyfiles before deleting them. On copy-on-write filesystems (APFS, Btrfs) and wear-levelled SSDs, the old blocks may survive physically, and snapshots and backups keep their own copies. The guarantee is "the app's own files are gone", not "no copy exists anywhere". T5.3 tells the user this, as SPEC decision 28 requires.
- **Deleting the keyfile isn't prevented.** The set of slots isn't authenticated as a whole, so someone who can write the folder can remove a slot. That is no worse than deleting the keyfile, which they could also do.

## Known gaps
- **A copy of the key in SQLite's parser.** The key reaches SQLCipher as `PRAGMA key = "x'<hex>'"`. Our buffer is sized exactly and zeroized, and rusqlite passes it without copying. SQLite's parser, however, keeps a transient copy of the SQL text and frees it without wiping. `PRAGMA cipher_memory_security = ON` would wipe all freed memory, but slows every query, so it stays off; the `sqlcipher_settings` test asserts that. The fix is `sqlite3_key_v2` over FFI, which needs a narrowly scoped `unsafe` block, so it is deferred to a later hardening task. Exploiting the gap requires reading process memory while the ledger is unlocked, which falls under "malware while unlocked", a case the threat model (SPEC §7) already lists as not protected against.
- **Swap, core dumps and stack copies.** Key memory isn't `mlock`ed, so it could reach swap or a crash dump. The key and the KEK also pass through short-lived stack copies that aren't wiped. Fixing either needs `unsafe` or a platform crate, so both are deferred along with the gap above.
- **Errors from statements holding the key** (`PRAGMA key` and `PRAGMA rekey`) always produce the fixed message `StorageError::KeySetup`. SQLite's error text could otherwise quote the statement, and with it the key.
