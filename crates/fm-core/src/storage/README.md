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
| `vault.finish_create(password)` | Completes a create that crashed after the keyfile was written (`VaultState::Incomplete`) |
| `vault.shred_keys()` | Overwrites with zeros and deletes every keyfile (current, `.next`, `.tmp`). The database can never be opened again. The full delete-everything flow is T5.3 |

**One instance at a time:** opening a ledger takes an exclusive advisory lock on `ledger.lock` (`flock` on Unix). A second app instance gets `StorageError::InUse` instead of racing the first one's keyfile writes. The OS releases the lock if the process dies.

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
- Version 1 creates only the `meta` key/value table. The ledger tables come in T0.8.

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
