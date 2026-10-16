# Unlocking (`fm_core::unlock`)

This module implements the unlock rules of SPEC §7 and decision 41 on top of
encrypted storage (`fm_core::storage`). Storage decides how the data key is kept;
this module decides who may open the ledger, and when.

| File | Contents |
|---|---|
| `password.rs` | Master password strength and NFC normalisation |
| `recovery.rs` | Recovery key format, parsing, confirmation challenge |
| `autolock.rs` | Idle timer |
| `touch_id.rs` | When Touch ID may replace the password |
| `clock.rs` | Time read from the wall clock and the monotonic clock together |
| `session.rs` | `Session`: the state machine the app drives |
| `store.rs` | The two seams: storage (`LedgerStore` / `OpenLedger`) and the Keychain (`TouchIdKeychain`) |
| `macos.rs` | The Keychain item behind Touch ID |
| `tests.rs` | Session tests against an in-memory ledger and a scripted Keychain |

The UI is `app/src/routes/unlock/` (`/unlock` and `/unlock/settings`). The app's glue is
`app/src-tauri/src/unlock.rs`. The root layout (`app/src/routes/+layout.ts` and
`+layout.svelte`) redirects every other route to `/unlock` while the ledger isn't open,
reports activity to the auto-lock timer, and returns to `/unlock` when the core
auto-locks (`unlock://locked` event).

## Rules
**Master password (decision 41).**
- At least 12 characters (after normalisation), and a zxcvbn score of 4.
- Score 4 means roughly 10^10 guesses or more. zxcvbn rates score 3 as only "moderate protection" against an offline attack, and a stolen disk is an offline attack.
- A short passphrase ("purple monkey dishwasher") reaches 4. The UI explains a failure with zxcvbn's feedback and suggests a passphrase.

**Normalisation.** Passwords are NFC-normalised before they reach storage. The same text typed with composed or decomposed accents (keyboards and systems differ) then opens the same ledger.

**Recovery key.**
- 256 random bits, shown as 56 Crockford base32 characters in 14 groups of 4.
- The text carries a 3-byte SHA-256 checksum, so a typo is caught before the slow unlock runs.
- Input ignores case, spaces and dashes, and reads O as 0 and I/L as 1.

**Confirming the recovery key.**
- The key is shown once, when it is made. The user confirms by typing back 3 randomly chosen groups.
- Until then the ledger stays closed (`Status::ConfirmRecoveryKey`), and a flag in the ledger's `meta` table (`unlock.recovery_unconfirmed`) records that the key was never confirmed.
- If the app stops first, the next password unlock makes a new key and shows that one instead.

**Forgotten password.**
1. The recovery key unlocks the ledger. Nothing else is possible until a new password is set (`Status::NeedsNewPassword`).
2. The new password replaces the old one (`reset_password`, using the recovery key as proof).
3. A new recovery key is made and confirmed, because the old one has now been seen and used.
4. If the user ticks "someone else may have known my old password", the data key is rotated too.

**Changing the password.**
- Re-wraps the existing key slot.
- With "I think someone else knows my current password", the data key is also rotated: `rotate_data_key` re-encrypts the database, so old keyfile copies stop working (storage README, "Threat model notes"). A new recovery key is then made and confirmed.

**Auto-lock (decision 41).**
- 10 minutes without activity by default. The choices are 1, 5, 10, 30 and 60 minutes, stored per ledger in `meta` (`unlock.autolock_minutes`). A value that isn't one of the choices falls back to the nearest one, so a damaged setting can't turn auto-lock off.
- Idle time is measured on both clocks. Time asleep counts (the monotonic clock stops while a Mac sleeps), and a wall clock set backwards locks at once.
- The app checks every 5 seconds, and also locks when it quits.

**Touch ID (macOS).** It only re-opens a ledger that the password already unlocked in this run of the app. The password is needed again:
- after every restart (nothing about this is stored);
- after 5 failed scans in a row (a cancelled prompt doesn't count);
- 7 days after the password was last typed (both clocks; a wall clock set backwards also forces the password).

**Crash reports (T5.6).** While the ledger is open, its account names and institutions are scrubbed from crash reports. Merchant names will be added with T2.2. The list is cleared on every lock (manual, auto-lock, quit), so no names stay in memory (`app/src-tauri` `crash_reports_hide_account_names_only_while_unlocked`).

**Linux, and Macs without Touch ID:** password only (`NoTouchId`; `Keychain::available()` is false).

## The Touch ID Keychain item (macOS)
**What it is.** One generic-password item per ledger in the data-protection Keychain: service `fin-manager.touch-id`, account = the ledger's folder path. It holds a copy of the 32-byte data key.

**How it is protected.**
- Access control `BiometryCurrentSet`: reading it needs a fingerprint match. The Secure Enclave enforces this against the set of fingers enrolled when the item was written; enrolling or removing a finger makes the item unreadable for good.
- `kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly`: never included in backups restored to another Mac, and removed by the system if the Mac's login password is removed.
- Not synchronizable, so it never goes to iCloud Keychain.

**When it is written.** After every password unlock, a password reset after a recovery unlock, and a password change, the old item is deleted and a new one is written with the current data key. After a data-key rotation, the new key replaces the old one.

**When it is deleted** (`Session::forget_touch_id`):
- when the 7-day rule or the 5-failed-scans rule trips (checked on every timer tick, on lock and on every Touch ID attempt);
- when locking while a recovery key is still unconfirmed, or after a recovery-key unlock (both need the password next);
- when the fingers changed or the stored key no longer opens the ledger;
- whenever it is replaced (see above);
- by T5.3 when the ledger's keys are shredded, which must call `forget_touch_id`.

**A leftover item.** An item left by an earlier run is never offered: Touch ID needs a password unlock in the current run first, and that overwrites the item.

**Signing requirement.** The data-protection Keychain needs the `keychain-access-groups` entitlement:
```xml
<key>keychain-access-groups</key>
<array><string>$(TeamIdentifierPrefix)com.raishish.fin-manager</string></array>
```
macOS grants this restricted entitlement only to an app signed with an Apple Developer ID and an embedded provisioning profile that allows it. That is T5.7's signing setup.
- Unsigned development builds get `errSecMissingEntitlement` (-34018). Touch ID is then reported as unavailable and the password is used (`macos::tests::unsigned_builds_report_touch_id_unavailable`).
- An ad-hoc signature with the entitlement is killed by macOS at launch, checked on 2026-10-16.

**Prompt wording.** The Touch ID prompt uses the system's wording, because the safe `security-framework` API doesn't expose `kSecUseOperationPrompt`.

## SPEC §7 rules → tests and checks
Tests are in `fm-core` unless the path says otherwise. "Manual N" refers to the numbered
list under [Manual checks](#manual-checks); checks 13–19 need the T5.7 signing setup above.

| SPEC §7 rule | Enforced by | Test or check |
|---|---|---|
| The master password is required every time the app launches | A new `Session` starts locked; the Touch ID policy starts at "not since start" | `the_password_is_needed_after_a_restart`, `an_item_left_by_an_earlier_run_is_not_used`, `touch_id::tests::password_is_required_after_a_restart` |
| Argon2id KEK → wrapped random data key → SQLCipher | Storage (T0.3) | `storage::tests` |
| The password is never stored | The session passes it straight to storage; it is held as `Zeroizing<String>` | Storage tests (keyfile holds only wrapped keys) |
| The data key lives in memory only while unlocked and is wiped on lock | `Session::lock` drops `Unlocked`, which wipes the key (T0.3) | `locking_closes_the_ledger`, `storage::tests` (lock wipes the key) |
| … and on close | The app locks on exit | Manual check 6 |
| … and on idle timeout (auto-lock) | `AutoLock`, `Session::tick` | `the_ledger_locks_after_the_idle_timeout`, `the_idle_timeout_is_a_per_ledger_setting`, `sleep_counts_towards_the_idle_timeout`, `autolock::tests::*`; Manual 7–8 |
| The app enforces a minimum password strength (decision 41) | `password::strength` on create, change and reset | `password::tests::*`, `a_weak_password_cannot_create_a_ledger`, `a_weak_password_cannot_replace_the_current_one`, `the_recovery_key_unlocks_but_a_new_password_comes_first` |
| Touch ID: a copy of the data key in the Keychain, locked to the Secure Enclave, needing a *current* fingerprint | `macos::Keychain` (`BiometryCurrentSet`, this device only, not synced) | Fake: `touch_id_reopens_a_ledger_locked_in_this_run`, `the_keychain_holds_the_current_data_key`. **Manual 13–14** (signed build) |
| Enrolling a new fingerprint invalidates it | `BiometryCurrentSet`; `Invalidated` → item deleted | Fake: `enrolling_a_new_finger_invalidates_touch_id`. **Manual 17** (signed build) |
| The password is required after a restart | Policy `NotSinceStart` (memory only) | `the_password_is_needed_after_a_restart` |
| … after 5 failed scans | Policy count of failed scans; item deleted | `the_password_is_needed_after_five_failed_scans`, `the_keychain_copy_is_deleted_after_five_failed_scans`, `cancelling_the_prompt_is_not_a_failed_scan`. **Manual 16** (signed build) |
| … and every 7 days | Policy on both clocks; item deleted | `the_password_is_needed_every_seven_days`, `the_keychain_copy_is_deleted_when_the_week_runs_out`, `touch_id::tests::password_is_required_every_seven_days` (sleep and clock set back); **Manual 18** (signed build) |
| Macs without Touch ID use the password | `Keychain::available()`; failures fall back to the password | `macs_without_touch_id_use_the_password`, `macos::tests::unsigned_builds_report_touch_id_unavailable`; Manual 19 |
| Linux: password only | `NoTouchId` | `macs_without_touch_id_use_the_password` (the `NoTouchId` part); Manual 12 |
| A recovery key is required | Made at create; made again if missing or unconfirmed | `the_ledger_stays_closed_until_the_recovery_key_is_confirmed`, `an_unconfirmed_recovery_key_is_replaced_at_the_next_unlock`, `an_interrupted_create_is_finished_with_the_password` |
| It is shown once, and the user must confirm they saved it | Held in memory until confirmed; 3 groups typed back | `recovery::tests::challenges_*`, `the_ledger_stays_closed_until_the_recovery_key_is_confirmed`. Manual 3–5 |
| It also unlocks the data key | `unlock_with_recovery_key`; then a new password is required | `the_recovery_key_unlocks_but_a_new_password_comes_first`, `recovery_after_a_compromise_rotates_the_data_key` |
| Losing both means the data is gone | The UI says so when the key is shown and when the password is chosen | Manual 3 |
| (Storage note) Rotate after a password change made because of suspected compromise | The "compromised" option on change and reset | `a_compromised_password_also_rotates_the_data_key_and_recovery_key`, `recovery_after_a_compromise_rotates_the_data_key` |
| (Storage note) Show whether a recovery slot exists | `Session::has_recovery_key` → security page | `app/src-tauri` `create_confirm_lock_and_unlock_through_the_app_layer` |

## Manual checks
Run these on a release build (`npm run tauri build`) with a fresh ledger, by deleting the
app data folder first. Each check names the SPEC §7 rule it covers.

**Any build (password path; unsigned dev builds too)**
1. **First launch.** `/unlock` asks for a master password; the network log and other routes redirect to `/unlock`. *(password at launch)*
2. **Strength.** `Summer2024!!!` is refused, with zxcvbn's reason and the passphrase hint. `purple monkey dishwasher` is accepted. The two entries must match. *(minimum strength, decision 41)*
3. **Recovery key, shown once.** 14 groups of 4 are shown with the warning that losing both secrets loses the data. **Continue** stays disabled until the "I have written down…" box is ticked. *(recovery key required; data gone if both are lost)*
4. **Confirmation.** A wrong group is refused. The right three groups open the ledger. Lowercase input and O/0 or I/1 mix-ups are accepted. *(user confirms they saved it)*
5. **Unconfirmed key.** Repeat 1–3 on a fresh ledger, then quit before confirming. Relaunch and unlock with the password: a **different** key is shown, and the first one no longer unlocks (step 9). *(shown once; never trust an unseen key)*
6. **Lock on quit.** Quit and relaunch: the password is asked for. *(data key wiped on close)*
7. **Auto-lock.** In `/unlock/settings` choose 1 minute. Leave the app idle: after about 1 minute it returns to `/unlock`. Moving the mouse or typing before then keeps it open. *(idle timeout)*
8. **Sleep.** With a 1-minute timeout, sleep the Mac for 2 minutes and wake it: the app locks within 5 seconds of waking. *(time asleep counts)*
9. **Recovery.** On the lock screen choose "Forgot your password?" and paste the key with spaces or in lowercase. The app asks for a new password before showing anything. After saving, a new recovery key is shown and confirmed. The old password and old recovery key are then both refused, and the new password works. *(recovery key unlocks; mandatory new password)*
10. **Compromise.** In `/unlock/settings`, change the password with "I think someone else knows my current password" ticked. A new recovery key is shown. A copy of the old `ledger.keys`, taken before the change, together with the old password no longer opens the ledger. *(rotation after suspected compromise)*
11. **Recovery key status.** `/unlock/settings` shows "A recovery key is set up". *(show whether a recovery slot exists)*
12. **Linux.** No Touch ID button is ever shown. *(Linux: password only)*

**Signed build only** (Developer ID + provisioning profile with `keychain-access-groups`; T5.7)
13. **Touch ID offered.** After a password unlock, lock (**Lock now**): **Use Touch ID** appears. The right finger unlocks. *(Touch ID copy, current fingerprint)*
14. **Item protection.** Keychain Access → "fin-manager Touch ID unlock": the item is in the local items, marked this-device-only, and absent from iCloud. *(this device only, never synced)*
15. **Not after a restart.** Quit and relaunch: no Touch ID button until the password has been typed. *(password after restart)*
16. **Five failures.** Lock, then fail Touch ID 5 times (wrong finger, prompt by prompt): the button disappears, the item is gone from Keychain Access, and the password is needed. *(5 failed scans)*
17. **New finger.** Lock, add a finger in System Settings → Touch ID: Touch ID fails with "fingerprints changed", the password works, and Touch ID works again after the next lock. *(enrolling invalidates)*
18. **Seven days.** Set the clock forward 7 days while locked: within 5 seconds the button disappears and the item is deleted. Setting the clock back also requires the password. *(every 7 days)*
19. **No Touch ID hardware.** On a Mac without Touch ID, or with no fingers enrolled, the button never appears. *(Macs without Touch ID use the password)*

## Known gaps
- **Secrets in the web view.** Passwords and the recovery key pass through the UI's JavaScript as strings, which can't be wiped. On the Rust side they are `Zeroizing` from the moment the command receives them.
- **Sleep.** The app doesn't lock the moment the Mac sleeps or the screen locks; Tauri exposes no such event. Because time asleep counts as idle, the first timer tick after waking locks it if the sleep was longer than the timeout.
- **Touch ID on real hardware** can only be checked with a signed build (above). Until then every Touch ID rule is tested against the scripted Keychain.
