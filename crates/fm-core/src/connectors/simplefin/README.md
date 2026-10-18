# SimpleFIN connector (T5.4)

Fetches the user's bank data from their own SimpleFIN Bridge account (SPEC decision 12).
SimpleFIN and its aggregator, MX, see the data. The app's makers don't, because
requests go straight from the user's computer to the bridge. The app screen
(`app/src/routes/connectors/`) says this plainly before anything is connected.

## Flow

| Step | Code | Network |
|---|---|---|
| Paste a setup token (base64 of a one-time claim URL); the app shows where it points | `inspect_token` | none |
| The user confirms that host | `claim(…, confirmed_host)` refuses any other | none |
| Claim it: `POST` the claim URL and receive the access URL | `claim` | the claim's origin, allowed only for the claim; the log shows `/<setup token>`, none of the path |
| Keep the access URL in the encrypted ledger (`meta`, `connector.simplefin.access_url`) | `save` | none |
| Start a sync: the windows from the last sync to now | `SyncRun::start` | none |
| Per window: count the request, before sending | `SyncRun::next` | none |
| Per window: `GET {base}/accounts?start-date&end-date&pending=1&version=2` | `fetch` | the bridge's origin, `Authorization: Basic`; the log shows the path only |
| Per window: store the response as a raw import (`simplefin`) and import it | `SyncRun::store` | none |
| Report | `SyncRun::finish` | none |

`connect` and `sync` run all their steps in one call. The app runs them separately. It
holds the ledger only for the steps marked "none", never while the network is slow, so
locking waits for one window's import at most. If the ledger is locked between steps,
the sync stops; what was stored stays, and the next sync carries on.

## Hosts

Both URLs come from outside: the setup token is pasted by the user, and the access URL
is the bridge's answer. A crafted token must not be able to point the app at this
machine or the local network.

- Both must be `https` to a **public** host (`token::is_public_host`). Refused:
  - `localhost` and `*.localhost`;
  - loopback, private (RFC 1918), shared (RFC 6598), link-local and other special
    addresses, in IPv4 and IPv6 (unique local, link-local, IPv4-mapped, SIIT
    `::ffff:0:0:0/96`, NAT64, 6to4 `2002::/16`);
  - names that are local by their form (`.local`, `.lan`, `.internal`, `.home.arpa`,
    single labels);
  - numeric forms a resolver reads as an address (`2130706433`, `0x7f.1`, `127.1`).
- **The user confirms the host before the claim.** The screen shows it, warns when it
  isn't SimpleFIN's own (`simplefin.org`), and the claim refuses any host but the
  confirmed one.
- **The access URL must be on the confirmed server** (same scheme, host and port, T5.10).
  A claim server can't hand back a server the user never saw, such as a public name
  that resolves to this machine. Otherwise the claim fails with `BridgeHostChanged`
  before anything is saved or allowed.
- Not caught: a public name that resolves to a private address (DNS rebinding). The
  confirmation step is the defence there.
- The tests' fake bridge is plain `http` to `127.0.0.1` or `[::1]` with a port, and
  nothing else (not `localhost`, no private range). It is accepted only:
  - in this crate's own tests;
  - with the `fake-bridge` feature, which fm-app turns on for its tests only, as a
    dev-dependency.

  Guards keep it out of anything shipped:
  - an optimised build with the feature doesn't compile (`compile_error!` in
    `lib.rs`), so `cargo test --release -p fm-app` doesn't either; CI runs those
    tests in debug;
  - `fm_core::FAKE_BRIDGE` says whether it's on;
  - the release workflow fails if `fake-bridge` appears in fm-app's normal and build
    dependency graph.

## Credentials

The access URL `https://user:password@bridge/path` is a long-lived credential.

- It is stored only in the encrypted ledger, and read back into wiped memory
  (`Zeroizing`), as are the decoded user and password.
- It is never logged, and it never appears in an error.
- Requests go to the URL without the credentials, which travel in an
  `Authorization: Basic` header. fm-netguard refuses credentials in URLs and never logs
  headers.
- `Credentials`, `Claimed` and `SyncRun` have redacted `Debug` output.

## Network allowlist

Nothing is allowed by default. The claim's origin is allowed for the claim only. After
the claim, only the bridge's origin is allowed:

- The app allows it while the ledger is open and connected (`BridgeAllowance`, driven
  by the unlock observer).
- It is revoked on lock and on disconnect.
- The allowlist is shared by the whole app and has no reference counts. `claim` adds
  the claim's origin and takes it away again afterwards, unless it was already allowed
  or is also the bridge's. Nothing else in the app allows a SimpleFIN origin today; a
  feature that does would need counting.

## After a sync (T5.13)

A sync runs the same follow-up steps as a file import, from the app's `after_import`
(`app/src-tauri/src/importing.rs`):
- pairing transfers (`transfers::detect`);
- the rules (`categorize::run`);
- the model fallback, once T2.5 lands;
- one backup.

They run once per sync, over the rows it added or settled. Each window's import skips
its own backup (`SyncRun::set_backup(false)`); one backup for the whole sync is enough.

- **The unfinished mark.**
  - The app sets it (`simplefin-sync`) before the first window commits, so a lock or
    a crash leaves the steps to finish (`finish_import`, or the next sync or import).
  - There is one mark for files and syncs. If another import's mark was there, the
    sync replaces it and runs the rules over everything, so neither is lost. A file
    import that finds a sync's mark does the same.
  - Each run clears only its own mark.
- **Stopping.** Each window's import can be cancelled (`SyncRun::store_cancellable`). A
  lock rolls that window back (`SimpleFinError::Interrupted`), keeps the windows before
  it, and leaves the mark. The step runner also checks the session generation, so
  nothing runs on a session other than the one the sync started on.

## Disconnecting

`disconnect` forgets the access URL and every `connector.simplefin.*` value and revokes
the bridge's origin; imported transactions stay. The app refuses it while a sync is
running (`Busy`). A sync also checks before every step that the connection it started
with is still the ledger's, so after a disconnect (or a new connection) it stores
nothing more.

## Limits

- **24 requests a day** (`quota.rs`). The bridge says "24 requests or fewer per day" and
  disables tokens that keep using more. Every `/accounts` request is counted in a
  rolling 24 hours before it is sent, and the count is kept in the ledger, so it
  survives a restart.
  - Each request is counted at the time it is sent (`SyncRun::next` takes the step's
    time), never before the sync's start or an earlier request, so a slow sync's later
    requests count for the full day after they go out.
  - Unreadable entries count as now.
  - A stamp in the future still counts, so setting the clock back frees nothing. A
    stamp more than a day ahead counts as a day ahead, so it blocks for two days at
    most.
  - The claim is not an `/accounts` request and isn't counted.
  - The bridge counts per access token, so a new connection starts with a fresh local
    count.
- **Windows** (`quota.rs`). The bridge allows at most 90 days per request and warns past
  45 days: "Requested date range exceeds recommended range of 45 days. In the future,
  this may be capped." So windows are at most 45 days.
  - The first sync fetches 90 days of history in two requests.
  - Each later sync starts 5 days before the end of the last one, so late-posting
    transactions aren't missed. The pipeline drops the repeats.
  - A sync plans at most a day's requests. If today's requests can't cover every
    window, the oldest are fetched and the next sync carries on (`complete: false`).
  - All time arithmetic saturates, whatever the clock or the stored values say.
- **Size.** A response over 16 MiB isn't read (`MAX_RESPONSE_BYTES`), and a sync asks
  for no more windows once its responses add up to 64 MiB (`MAX_SYNC_BYTES`). Only one
  response is in memory at a time.
- **Messages.** At most 20 distinct bridge messages of at most 300 characters each are
  kept (`MAX_MESSAGES`, `MAX_MESSAGE_CHARS`).

## Mapping (`parse.rs`)

- One statement per SimpleFIN account.
  - The account id becomes the statement's account number, which the pipeline stores
    as `external_id` and uses to find the account on every later sync.
  - No last four digits are derived from that opaque id.
  - A new account takes SimpleFIN's account name. An existing account is never
    renamed.
- Each transaction id becomes the record's external id.
- Transactions with `pending: true` are imported as pending. Their posted version
  settles them later.
- Accounts in a custom currency (the protocol allows a URL, e.g. points) are skipped,
  with a message saying so.
- `errlist` and the deprecated `errors` are shown to the user as they are, as the
  bridge asks ("Always show those errors to your end users"). Repeats are removed, and
  the number and length are capped (see Limits).
- A malformed response is refused without echoing any of it.

## Dates

SimpleFIN gives Unix timestamps. Each is read as its **UTC calendar date**
(`parse::utc_date`, the only place this happens).

What the demo bridge sends: one live run (2026-10-17, public demo token, 1 claim and 1
`/accounts` request) returned 340 transactions with 680 `posted`/`transacted_at`
stamps. Every stamp fell at one of three UTC hours:

| UTC hour | stamps | Same calendar date as … |
|---|---|---|
| 12:00 | 328 | every zone from UTC−11 to UTC+11 |
| 20:00 | 328 | US zones (the next day from UTC+4 eastward) |
| 08:00 | 24 | continental US zones (the previous day in Alaska and Hawaii) |

SimpleFIN serves US banks through MX, so for US time zones the UTC date is the bank's
date at every observed hour, except 08:00 stamps in Alaska and Hawaii. SPEC decision 47
uses the UTC date for now, to be revisited after user feedback. Changing it means
changing `utc_date` only.

## Tests

- Unit tests for token, quota and parse.
- `tests.rs` runs the whole flow against a fake bridge on loopback: claim, the
  confirmed host, refused access URLs, Basic auth, no secrets in URLs or the log, raw
  storage, matching by id, pending to posted, repeats, windows, one window at a time,
  size limits, quota across a restart and a clock change, partial syncs, bridge errors,
  the allowlist after a restart, disconnecting, and a disconnect during a sync.
- `simplefin_demo` calls the public demo bridge and is `#[ignore]`d, so CI never runs
  it:

  ```
  cargo test -p fm-core --lib connectors::simplefin::tests::simplefin_demo -- --ignored --nocapture
  ```
