//! SimpleFIN (T5.4, SPEC decision 12): the user's bank data through their own SimpleFIN
//! Bridge account. SimpleFIN and its aggregator (MX) see the data; this app's makers
//! don't, because requests go straight from this machine to the bridge.
//!
//! - **Connect.** The user pastes a setup token: base64 of a one-time claim URL, which
//!   must point at a public host over https. The user confirms that host
//!   ([`inspect_token`]), then a POST to the claim URL returns the access URL
//!   (`https://user:password@bridge/path`), a long-lived credential, kept only in the
//!   encrypted ledger (`meta`) and never logged. The claim's path is a secret too, so
//!   the network log shows none of it.
//! - **Sync.** `GET {base}/accounts` for each window (see `quota.rs`), credentials in an
//!   `Authorization: Basic` header, one window at a time ([`SyncRun`]). Every response
//!   is stored as it came, as a raw import (source `simplefin`), and its accounts and
//!   transactions go through the import pipeline: accounts match by their SimpleFIN id,
//!   repeats are dropped, a pending transaction settles when its posted version arrives.
//! - **Limits.** At most 24 requests in any 24 hours and 45 days per request, enforced
//!   here before anything is sent (`quota.rs`); at most 16 MiB per response and 64 MiB
//!   per sync.
//! - **Network.** Every request goes through `fm_netguard`. The claim's origin is
//!   allowed for the claim; afterwards only the bridge's origin, which the app allows
//!   again after each unlock with [`bridge_origin`].

mod parse;
mod quota;
mod token;

#[cfg(test)]
mod tests;

use std::fmt;
use std::time::SystemTime;

use fm_netguard::{NetGuard, Origin, Request};
use rusqlite::OptionalExtension as _;
use zeroize::Zeroizing;

use crate::model::ImportSource;
use crate::pipeline::{self, ImportError, ImportReport, StatementsImport, Target};
use crate::storage::{StorageError, Unlocked};

pub use quota::{HISTORY_DAYS, MAX_REQUESTS_PER_DAY, MAX_WINDOW_DAYS, OVERLAP_DAYS};

use quota::Quota;
use token::Credentials;

/// The network log's label for the claim.
pub const PURPOSE_CLAIM: &str = "simplefin-claim";
/// The network log's label for `/accounts` requests.
pub const PURPOSE_SYNC: &str = "simplefin-sync";

/// The largest `/accounts` response read: 45 days of several accounts is far less.
pub const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
/// A sync stops asking for more windows once its responses add up to this; the next
/// sync carries on.
pub const MAX_SYNC_BYTES: u64 = 64 * 1024 * 1024;
/// The bridge's messages kept per sync, and their length in characters.
pub const MAX_MESSAGES: usize = 20;
pub const MAX_MESSAGE_CHARS: usize = 300;

/// The access URL: a credential.
const KEY_ACCESS_URL: &str = "connector.simplefin.access_url";
/// Unix seconds of the requests in the last 24 hours (see `quota.rs`).
const KEY_REQUESTS: &str = "connector.simplefin.requests";
/// Unix seconds up to which transactions have been fetched.
const KEY_SYNCED_TO: &str = "connector.simplefin.synced_to";
/// Unix seconds of the last sync that reached the bridge.
const KEY_LAST_SYNC: &str = "connector.simplefin.last_sync";
/// The bridge's messages from the last sync (a JSON array of strings).
const KEY_MESSAGES: &str = "connector.simplefin.messages";
const ALL_KEYS: [&str; 5] = [
    KEY_ACCESS_URL,
    KEY_REQUESTS,
    KEY_SYNCED_TO,
    KEY_LAST_SYNC,
    KEY_MESSAGES,
];

/// A claim's answer is one URL.
const MAX_CLAIM_BYTES: u64 = 4096;

/// Why connecting or syncing failed. Never contains the token, the access URL or
/// anything from a response.
#[derive(Debug)]
#[non_exhaustive]
pub enum SimpleFinError {
    /// The setup token isn't a SimpleFIN setup token for a public server.
    InvalidToken,
    /// The host the user confirmed isn't the one in the setup token.
    HostNotConfirmed,
    /// The bridge answered the claim with something that isn't a usable access URL.
    BadAccessUrl,
    /// The bridge's access URL names a different server from the one the user
    /// confirmed. Nothing was saved or allowed.
    BridgeHostChanged,
    /// The setup token was claimed before (each works once). If this wasn't you,
    /// disable that token in SimpleFIN.
    TokenAlreadyUsed,
    /// Already connected: disconnect first.
    AlreadyConnected,
    /// Not connected, or disconnected (or reconnected) while a sync was running.
    NotConnected,
    /// The bridge no longer accepts this connection: connect again with a new token.
    AccessRevoked,
    /// The SimpleFIN subscription needs paying.
    PaymentRequired,
    /// The bridge answered with this unexpected HTTP status.
    Bridge {
        status: u16,
    },
    /// The 24-requests-a-day limit is reached; the next request is allowed at this
    /// Unix time.
    QuotaReached {
        next_allowed: i64,
    },
    /// The bridge's answer couldn't be read.
    Malformed,
    Network(fm_netguard::Error),
    Storage(StorageError),
    Import(ImportError),
}

impl fmt::Display for SimpleFinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToken => f.write_str(
                "that isn't a SimpleFIN setup token, or it points somewhere other than a \
                 public server",
            ),
            Self::HostNotConfirmed => {
                f.write_str("confirm the server this setup token connects to first")
            }
            Self::BadAccessUrl => {
                f.write_str("SimpleFIN's answer to the setup token wasn't usable")
            }
            Self::BridgeHostChanged => f.write_str(
                "the server answered with a different server than the one you confirmed, so \
                 nothing was saved; if this wasn't expected, disable the token in SimpleFIN",
            ),
            Self::TokenAlreadyUsed => f.write_str(
                "this setup token was already used; if that wasn't you, disable it in SimpleFIN \
                 and make a new one",
            ),
            Self::AlreadyConnected => f.write_str("SimpleFIN is already connected"),
            Self::NotConnected => f.write_str("SimpleFIN isn't connected"),
            Self::AccessRevoked => f.write_str(
                "SimpleFIN no longer accepts this connection; connect again with a new setup token",
            ),
            Self::PaymentRequired => f.write_str("SimpleFIN says the subscription needs paying"),
            Self::Bridge { status } => write!(f, "SimpleFIN answered with HTTP status {status}"),
            Self::QuotaReached { .. } => f.write_str(
                "SimpleFIN allows 24 updates a day and they're used up; try again later",
            ),
            Self::Malformed => f.write_str("SimpleFIN's answer couldn't be read"),
            Self::Network(e) => write!(f, "{e}"),
            Self::Storage(e) => write!(f, "{e}"),
            Self::Import(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SimpleFinError {}

impl From<StorageError> for SimpleFinError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

impl From<rusqlite::Error> for SimpleFinError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(StorageError::Database(e))
    }
}

impl From<fm_netguard::Error> for SimpleFinError {
    fn from(e: fm_netguard::Error) -> Self {
        Self::Network(e)
    }
}

/// The connection as the settings screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The bridge's origin, e.g. `https://beta-bridge.simplefin.org`.
    pub bridge: Option<String>,
    /// Unix seconds up to which transactions have been fetched.
    pub synced_to: Option<i64>,
    /// Unix seconds of the last sync that reached the bridge.
    pub last_sync: Option<i64>,
    pub requests_left: usize,
    /// When the next request is allowed, if the day's are used up.
    pub next_allowed: Option<i64>,
    /// The bridge's messages from the last sync, to show as they are.
    pub messages: Vec<String>,
}

impl Status {
    pub fn connected(&self) -> bool {
        self.bridge.is_some()
    }
}

/// One sync's result.
#[derive(Debug)]
pub struct SyncReport {
    /// One per response, oldest window first. A response identical to an earlier one
    /// is stored once and adds nothing, so it has no report.
    pub imports: Vec<ImportReport>,
    /// The bridge's messages (connections needing attention, skipped accounts), to show
    /// as they are. At most [`MAX_MESSAGES`].
    pub messages: Vec<String>,
    /// False when the day's requests or the sync's byte budget ran out before every
    /// window was fetched; the next sync continues from `synced_to`.
    pub complete: bool,
    pub synced_to: Option<i64>,
    pub requests_left: usize,
    pub next_allowed: Option<i64>,
}

impl SyncReport {
    pub fn inserted(&self) -> usize {
        self.sum(pipeline::StatementReport::inserted)
    }

    pub fn duplicates(&self) -> usize {
        self.sum(pipeline::StatementReport::duplicates)
    }

    pub fn posted_from_pending(&self) -> usize {
        self.sum(pipeline::StatementReport::posted_from_pending)
    }

    pub fn accounts_created(&self) -> usize {
        self.imports
            .iter()
            .flat_map(|i| &i.statements)
            .filter(|s| s.account_created)
            .count()
    }

    fn sum(&self, f: fn(&pipeline::StatementReport) -> usize) -> usize {
        self.imports.iter().flat_map(|i| &i.statements).map(f).sum()
    }
}

// ---- connecting -------------------------------------------------------------------------

/// Where a setup token points, for the user to confirm before it is claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenInfo {
    /// `host[:port]`.
    pub host: String,
    /// The host is SimpleFIN's own (`simplefin.org` or a subdomain).
    pub simplefin: bool,
}

/// Reads a setup token without contacting anything.
pub fn inspect_token(setup_token: &str) -> Result<TokenInfo, SimpleFinError> {
    let claim = token::claim_url(setup_token)?;
    Ok(TokenInfo {
        simplefin: token::is_simplefin_host(&claim.host),
        host: claim.host,
    })
}

/// Claims a setup token and keeps the access URL: [`claim`] then [`save`], refusing
/// first if already connected. The app runs the steps separately so the ledger isn't
/// held during the claim.
pub fn connect(
    ledger: &Unlocked,
    guard: &NetGuard,
    setup_token: &str,
    confirmed_host: &str,
) -> Result<Status, SimpleFinError> {
    if is_connected(ledger)? {
        return Err(SimpleFinError::AlreadyConnected);
    }
    let claimed = claim(guard, setup_token, confirmed_host)?;
    save(ledger, guard, claimed)
}

pub fn is_connected(ledger: &Unlocked) -> Result<bool, SimpleFinError> {
    Ok(ledger.connection().query_row(
        "SELECT EXISTS (SELECT 1 FROM meta WHERE key = ?1)",
        [KEY_ACCESS_URL],
        |r| r.get(0),
    )?)
}

/// A claimed access URL, not yet saved. A credential: never logged.
pub struct Claimed {
    access_url: Zeroizing<String>,
    origin: Origin,
}

impl Claimed {
    /// The bridge's origin, which [`claim`] allowed.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
}

impl fmt::Debug for Claimed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Claimed")
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

/// Claims a setup token (each works once), once the user has confirmed its host
/// (`confirmed_host`, as [`inspect_token`] showed it). Needs no ledger.
///
/// The access URL in the answer must be on the confirmed origin (scheme, host and
/// port): a claim server can't hand back a server the user never saw
/// ([`SimpleFinError::BridgeHostChanged`]).
///
/// The claim's origin is allowed on `guard` for the claim and stays allowed on
/// success, as the bridge's. The allowlist is shared and not reference counted: on
/// failure, the claim's origin is revoked again unless it was already allowed.
pub fn claim(
    guard: &NetGuard,
    setup_token: &str,
    confirmed_host: &str,
) -> Result<Claimed, SimpleFinError> {
    let claim = token::claim_url(setup_token)?;
    if !claim.host.eq_ignore_ascii_case(confirmed_host.trim()) {
        return Err(SimpleFinError::HostNotConfirmed);
    }
    let claim_origin: Origin = claim
        .origin
        .parse()
        .map_err(|_| SimpleFinError::InvalidToken)?;
    let added = guard.allow(claim_origin.clone());
    let answer = guard.send(
        Request::post(claim.url.as_str(), PURPOSE_CLAIM, Vec::new())
            .max_body_bytes(MAX_CLAIM_BYTES)
            .log_path(token::CLAIM_LOG_PATH),
    );
    let claimed = answer
        .map_err(SimpleFinError::from)
        .and_then(|response| match response.status {
            200..=299 => {
                let text = Zeroizing::new(
                    String::from_utf8(response.body).map_err(|_| SimpleFinError::BadAccessUrl)?,
                );
                let credentials = Credentials::parse(&text)?;
                let origin: Origin = credentials
                    .origin
                    .parse()
                    .map_err(|_| SimpleFinError::BadAccessUrl)?;
                if origin != claim_origin {
                    return Err(SimpleFinError::BridgeHostChanged);
                }
                Ok(Claimed {
                    access_url: Zeroizing::new(text.trim().to_string()),
                    origin,
                })
            }
            403 => Err(SimpleFinError::TokenAlreadyUsed),
            status => Err(SimpleFinError::Bridge { status }),
        });
    // On success the bridge's origin is the claim's, already allowed.
    if added && claimed.is_err() {
        guard.revoke(&claim_origin);
    }
    claimed
}

/// Keeps a claimed access URL, replacing any earlier connection (the token is spent
/// either way). A new access token starts with a fresh quota (the bridge counts per
/// token) and a first sync of the full history.
pub fn save(
    ledger: &Unlocked,
    guard: &NetGuard,
    claimed: Claimed,
) -> Result<Status, SimpleFinError> {
    if let Some(old) = bridge_origin(ledger)?
        && old != claimed.origin
    {
        guard.revoke(&old);
    }
    for key in ALL_KEYS {
        set(ledger, key, None)?;
    }
    set(ledger, KEY_ACCESS_URL, Some(&claimed.access_url))?;
    status(ledger, SystemTime::now())
}

/// The bridge's origin, for the app to allow on its `NetGuard` after unlocking. `None`
/// when SimpleFIN isn't connected.
pub fn bridge_origin(ledger: &Unlocked) -> Result<Option<Origin>, SimpleFinError> {
    Ok(match credentials(ledger) {
        Ok(c) => c.origin.parse().ok(),
        Err(SimpleFinError::NotConnected) => None,
        Err(e) => return Err(e),
    })
}

pub fn status(ledger: &Unlocked, now: SystemTime) -> Result<Status, SimpleFinError> {
    let now = unix(now);
    let bridge = match credentials(ledger) {
        Ok(c) => Some(c.origin.clone()),
        Err(SimpleFinError::NotConnected) => None,
        Err(e) => return Err(e),
    };
    let quota = load_quota(ledger, now)?;
    let messages = get(ledger, KEY_MESSAGES)?
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(|m| m.into_iter().take(MAX_MESSAGES).collect())
        .unwrap_or_default();
    Ok(Status {
        bridge,
        synced_to: number(ledger, KEY_SYNCED_TO)?,
        last_sync: number(ledger, KEY_LAST_SYNC)?,
        requests_left: quota.left(now),
        next_allowed: quota.next_allowed(now),
        messages,
    })
}

/// Forgets the access URL and everything about the connection, and stops allowing the
/// bridge. Transactions already imported stay (deleting them is T5.3's job). To stop
/// SimpleFIN itself sharing data, the user disables the token in SimpleFIN too. A sync
/// still running stores nothing more ([`SyncRun::store`] checks).
pub fn disconnect(ledger: &Unlocked, guard: &NetGuard) -> Result<(), SimpleFinError> {
    if let Some(origin) = bridge_origin(ledger).ok().flatten() {
        guard.revoke(&origin);
    }
    for key in ALL_KEYS {
        set(ledger, key, None)?;
    }
    Ok(())
}

// ---- syncing ----------------------------------------------------------------------------

/// Fetches what's new since the last sync and imports it, one window at a time:
/// [`SyncRun::start`], then [`SyncRun::next`], [`fetch`] and [`SyncRun::store`] per
/// window, then [`SyncRun::finish`]. The app runs the steps separately, holding the
/// ledger only for `start`, `next`, `store` and `finish`, never while the network is
/// slow, so locking waits for at most one window's import.
///
/// A failure stops the sync; the windows stored before it are kept.
pub fn sync(
    ledger: &mut Unlocked,
    guard: &NetGuard,
    now: SystemTime,
) -> Result<SyncReport, SimpleFinError> {
    let mut run = SyncRun::start(ledger, now)?;
    while let Some(window) = run.next(ledger, now)? {
        let body = fetch(guard, &run, window)?;
        run.store(ledger, window, body)?;
    }
    run.finish(ledger)
}

/// One `/accounts` request: Unix seconds `[from, to)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from: i64,
    pub to: i64,
}

/// A sync in progress.
pub struct SyncRun {
    credentials: Credentials,
    now: SystemTime,
    /// When the sync started (Unix seconds).
    at: i64,
    /// The latest step's time: when the last request was counted.
    latest: i64,
    windows: Vec<Window>,
    next: usize,
    bytes: u64,
    /// [`MAX_SYNC_BYTES`]; smaller in tests.
    budget: u64,
    report: SyncReport,
}

impl fmt::Debug for SyncRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncRun")
            .field("windows", &self.windows)
            .field("next", &self.next)
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl SyncRun {
    /// Starts a sync: the windows from the last sync (with the overlap) up to `now`.
    /// Refuses at once when today's requests are used up.
    pub fn start(ledger: &Unlocked, now: SystemTime) -> Result<Self, SimpleFinError> {
        let credentials = credentials(ledger)?;
        let at = unix(now);
        if let Some(next_allowed) = load_quota(ledger, at)?.next_allowed(at) {
            return Err(SimpleFinError::QuotaReached { next_allowed });
        }
        let synced_to = number(ledger, KEY_SYNCED_TO)?;
        let windows = quota::windows(synced_to, at)
            .into_iter()
            .map(|(from, to)| Window { from, to })
            .collect();
        Ok(Self {
            credentials,
            now,
            at,
            latest: at,
            windows,
            next: 0,
            bytes: 0,
            budget: MAX_SYNC_BYTES,
            report: SyncReport {
                imports: Vec::new(),
                messages: Vec::new(),
                complete: true,
                synced_to,
                requests_left: 0,
                next_allowed: None,
            },
        })
    }

    /// The next window to fetch, counted against today's limit and stored before
    /// anything is sent, so a crash or a failure never frees quota. `now` is the time
    /// of this step: the request is counted then (never before the sync's start or an
    /// earlier step), so a slow sync's later requests count for the full 24 hours
    /// after they are sent.
    /// `None` when the sync is done, or stops early because today's requests or the
    /// sync's byte budget ran out (the report then says `complete: false`).
    pub fn next(
        &mut self,
        ledger: &Unlocked,
        now: SystemTime,
    ) -> Result<Option<Window>, SimpleFinError> {
        self.check_connection(ledger)?;
        let Some(&window) = self.windows.get(self.next) else {
            return Ok(None);
        };
        // Never before the sync's start or an earlier step, if the clock went back.
        let sent_at = self.latest.max(unix(now));
        self.latest = sent_at;
        let mut quota = load_quota(ledger, sent_at)?;
        if quota.left(sent_at) == 0 || self.bytes >= self.budget {
            self.report.complete = false;
            return Ok(None);
        }
        quota.record(sent_at);
        set(ledger, KEY_REQUESTS, Some(&quota.encode()))?;
        self.next += 1;
        Ok(Some(window))
    }

    /// Stores and imports one window's response, and moves the sync forward past it.
    /// Stores nothing if SimpleFIN was disconnected (or reconnected) meanwhile.
    pub fn store(
        &mut self,
        ledger: &mut Unlocked,
        window: Window,
        body: Vec<u8>,
    ) -> Result<(), SimpleFinError> {
        self.check_connection(ledger)?;
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(body.len()).unwrap_or(u64::MAX));
        let parsed = parse::parse(&body, (window.from, window.to))?;
        for message in parsed.messages {
            // Each window repeats the connection's messages.
            if self.report.messages.len() < MAX_MESSAGES && !self.report.messages.contains(&message)
            {
                self.report.messages.push(message);
            }
        }
        let request = StatementsImport {
            bytes: &body,
            file_name: None,
            source: ImportSource::SimpleFin,
            statements: parsed.statements,
            account_names: parsed.account_names,
            target: Target::Auto,
            now: self.now,
            backup: true,
            cancel: &pipeline::never,
        };
        match pipeline::import_statements(ledger, &request) {
            Ok(imported) => self.report.imports.push(imported),
            // The same answer as before: nothing new in it.
            Err(ImportError::AlreadyImported { .. }) => {}
            Err(e) => return Err(SimpleFinError::Import(e)),
        }
        // Never moves back (a slower, older sync finishing late).
        let synced_to = self
            .report
            .synced_to
            .map_or(window.to, |t| t.max(window.to));
        set(ledger, KEY_SYNCED_TO, Some(&synced_to.to_string()))?;
        set(ledger, KEY_LAST_SYNC, Some(&self.at.to_string()))?;
        let messages =
            serde_json::to_string(&self.report.messages).map_err(|_| SimpleFinError::Malformed)?;
        set(ledger, KEY_MESSAGES, Some(&messages))?;
        self.report.synced_to = Some(synced_to);
        Ok(())
    }

    /// The sync's result.
    pub fn finish(mut self, ledger: &Unlocked) -> Result<SyncReport, SimpleFinError> {
        // Not up to now: stopped early, or the gap needed more than a day's windows.
        if !self.windows.is_empty() && self.report.synced_to.is_none_or(|t| t < self.at) {
            self.report.complete = false;
        }
        let quota = load_quota(ledger, self.latest)?;
        self.report.requests_left = quota.left(self.latest);
        self.report.next_allowed = quota.next_allowed(self.latest);
        Ok(self.report)
    }

    /// The connection this sync started with is still the ledger's.
    fn check_connection(&self, ledger: &Unlocked) -> Result<(), SimpleFinError> {
        let current = credentials(ledger)?;
        if current.base != self.credentials.base
            || *current.authorization() != *self.credentials.authorization()
        {
            return Err(SimpleFinError::NotConnected);
        }
        Ok(())
    }
}

/// Sends one window's `/accounts` request. Needs no ledger.
pub fn fetch(guard: &NetGuard, run: &SyncRun, window: Window) -> Result<Vec<u8>, SimpleFinError> {
    let url = format!(
        "{}/accounts?start-date={}&end-date={}&pending=1&version=2",
        run.credentials.base, window.from, window.to
    );
    let authorization = run.credentials.authorization();
    let response = guard.send(
        Request::get(url, PURPOSE_SYNC)
            .header("Authorization", authorization.as_str())
            .max_body_bytes(MAX_RESPONSE_BYTES),
    )?;
    match response.status {
        200..=299 => Ok(response.body),
        402 => Err(SimpleFinError::PaymentRequired),
        403 => Err(SimpleFinError::AccessRevoked),
        status => Err(SimpleFinError::Bridge { status }),
    }
}

// ---- storage ----------------------------------------------------------------------------

fn credentials(ledger: &Unlocked) -> Result<Credentials, SimpleFinError> {
    let url: Option<Zeroizing<String>> = ledger
        .connection()
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [KEY_ACCESS_URL],
            |r| r.get::<_, String>(0).map(Zeroizing::new),
        )
        .optional()?;
    Credentials::parse(&url.ok_or(SimpleFinError::NotConnected)?)
}

fn load_quota(ledger: &Unlocked, now: i64) -> Result<Quota, SimpleFinError> {
    Ok(Quota::decode(
        &get(ledger, KEY_REQUESTS)?.unwrap_or_default(),
        now,
    ))
}

fn number(ledger: &Unlocked, key: &str) -> Result<Option<i64>, SimpleFinError> {
    Ok(get(ledger, key)?.and_then(|s| s.parse().ok()))
}

fn unix(t: SystemTime) -> i64 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_secs()).map_or(i64::MIN, |s| -s),
    }
}

fn get(ledger: &Unlocked, key: &str) -> Result<Option<String>, SimpleFinError> {
    Ok(ledger
        .connection()
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

fn set(ledger: &Unlocked, key: &str, value: Option<&str>) -> Result<(), SimpleFinError> {
    let conn = ledger.connection();
    match value {
        Some(v) => conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [key, v],
        ),
        None => conn.execute("DELETE FROM meta WHERE key = ?1", [key]),
    }?;
    Ok(())
}
