//! The connector against a fake bridge on this machine, and once by hand against
//! SimpleFIN's public demo bridge (`simplefin_demo`, ignored so CI never calls it).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fm_netguard::{Allowlist, LogEntry, NetGuard, Outcome};

use super::*;
use crate::model::{AccountId, TransactionStatus};
use crate::storage::{KdfParams, Vault};

const PASSWORD: &str = "velvet anchor mosaic tundra";
const DAY: i64 = 86_400;
/// 2026-10-16T12:00:00Z.
const NOW: i64 = 1_792_152_000;
const CLAIM_SECRET: &str = "CLAIMSECRET0123456789";
const USER: &str = "AB12USER";
const PASSWORD_IN_URL: &str = "s3cr%40t";
const PASSWORD_DECODED: &str = "s3cr@t";

fn at(secs: i64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(u64::try_from(secs).unwrap())
}

fn ledger() -> (tempfile::TempDir, Unlocked) {
    let dir = tempfile::tempdir().unwrap();
    let u = Vault::at(&dir.path().join("ledger"))
        .create_with_params(PASSWORD.as_bytes(), KdfParams::INSECURE_TEST)
        .unwrap();
    (dir, u)
}

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    target: String,
    authorization: Option<String>,
}

impl Seen {
    fn query(&self, name: &str) -> Option<i64> {
        let (_, query) = self.target.split_once('?')?;
        query
            .split('&')
            .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
            .and_then(|v| v.parse().ok())
    }
}

/// What the fake bridge answers.
struct Script {
    claim: (u16, String),
    accounts: Vec<(u16, String)>,
}

/// A SimpleFIN bridge on loopback: a claim endpoint and `/accounts`, answering
/// `/accounts` from a queue (the last answer repeats).
struct Bridge {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    script: Arc<Mutex<Script>>,
}

impl Bridge {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(Script {
            claim: (
                200,
                format!("http://{USER}:{PASSWORD_IN_URL}@127.0.0.1:{port}/simplefin"),
            ),
            accounts: vec![(200, accounts_body(&[TRN_COFFEE_PENDING]))],
        }));
        let (log, answers) = (Arc::clone(&seen), Arc::clone(&script));
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let target = parts.next().unwrap_or_default().to_string();
                let mut authorization = None;
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    let header = header.trim_end();
                    if header.is_empty() {
                        break;
                    }
                    let (name, value) = header.split_once(':').unwrap();
                    match name.trim().to_ascii_lowercase().as_str() {
                        "authorization" => authorization = Some(value.trim().to_string()),
                        "content-length" => length = value.trim().parse().unwrap(),
                        _ => {}
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let (status, body) = {
                    let mut script = answers.lock().unwrap();
                    if target.contains("/claim/") {
                        script.claim.clone()
                    } else if script.accounts.len() > 1 {
                        script.accounts.remove(0)
                    } else {
                        script.accounts[0].clone()
                    }
                };
                log.lock().unwrap().push(Seen {
                    method,
                    target,
                    authorization,
                });
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            }
        });
        Self { port, seen, script }
    }

    fn setup_token(&self) -> String {
        STANDARD.encode(format!(
            "http://127.0.0.1:{}/simplefin/claim/{CLAIM_SECRET}",
            self.port
        ))
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    fn origin(&self) -> Origin {
        format!("http://127.0.0.1:{}", self.port).parse().unwrap()
    }

    fn answer(&self, accounts: Vec<(u16, String)>) {
        self.script.lock().unwrap().accounts = accounts;
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn account_requests(&self) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|s| s.target.contains("/accounts"))
            .collect()
    }
}

const TRN_COFFEE_PENDING: &str = r#"{"id": "TRN-1", "posted": 0, "amount": "-4.50", "description": "COFFEE", "transacted_at": 1792065600, "pending": true}"#;
const TRN_COFFEE_POSTED: &str = r#"{"id": "TRN-1", "posted": 1792152000, "amount": "-4.50", "description": "COFFEE", "transacted_at": 1792065600}"#;
const TRN_RENT: &str =
    r#"{"id": "TRN-2", "posted": 1791979200, "amount": "-1500.00", "description": "RENT"}"#;

fn accounts_body(transactions: &[&str]) -> String {
    format!(
        r#"{{"errlist": [], "connections": [{{"conn_id": "CON-1", "name": "Demo Bank"}}],
            "accounts": [{{"id": "ACT-1", "name": "Everyday Checking", "conn_id": "CON-1",
              "currency": "USD", "balance": "100.00", "balance-date": {NOW},
              "transactions": [{}]}}]}}"#,
        transactions.join(",")
    )
}

fn connected() -> (tempfile::TempDir, Unlocked, Bridge, NetGuard) {
    let (dir, u) = ledger();
    let bridge = Bridge::start();
    let guard = NetGuard::new(Allowlist::new());
    connect(&u, &guard, &bridge.setup_token(), &bridge.host()).unwrap();
    (dir, u, bridge, guard)
}

fn secrets_absent(text: &str) {
    for secret in [CLAIM_SECRET, USER, PASSWORD_IN_URL, PASSWORD_DECODED] {
        assert!(!text.contains(secret), "a secret leaked: {text}");
    }
}

fn log_text(guard: &NetGuard) -> String {
    guard
        .log()
        .iter()
        .map(|e: &LogEntry| format!("{e:?}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn connecting_claims_the_token_and_keeps_only_the_bridge_allowed() {
    let (_dir, u) = ledger();
    let bridge = Bridge::start();
    let guard = NetGuard::new(Allowlist::new());
    let status = connect(&u, &guard, &bridge.setup_token(), &bridge.host()).unwrap();
    assert!(status.connected());
    assert_eq!(
        status.requests_left, 24,
        "the claim isn't an /accounts request"
    );
    assert_eq!(bridge_origin(&u).unwrap(), Some(bridge.origin()));
    assert_eq!(
        guard.allowlist().iter().cloned().collect::<Vec<_>>(),
        [bridge.origin()]
    );

    let seen = bridge.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert!(seen[0].target.ends_with(CLAIM_SECRET));

    // The network log shows the claim with none of its path (the token).
    let log = guard.log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].path, "/<setup token>");
    assert_eq!(log[0].purpose, PURPOSE_CLAIM);
    secrets_absent(&log_text(&guard));

    // The access URL is in the encrypted ledger, and only there.
    let stored = get(&u, KEY_ACCESS_URL).unwrap().unwrap();
    assert!(stored.contains(PASSWORD_IN_URL));
    assert!(matches!(
        connect(&u, &guard, &bridge.setup_token(), &bridge.host()),
        Err(SimpleFinError::AlreadyConnected)
    ));
}

#[test]
fn a_used_token_is_reported_and_its_origin_not_left_allowed() {
    let (_dir, u) = ledger();
    let bridge = Bridge::start();
    bridge.script.lock().unwrap().claim = (403, String::new());
    let guard = NetGuard::new(Allowlist::new());
    let err = connect(&u, &guard, &bridge.setup_token(), &bridge.host()).unwrap_err();
    assert!(matches!(err, SimpleFinError::TokenAlreadyUsed), "{err:?}");
    assert!(guard.allowlist().is_empty());
    assert_eq!(get(&u, KEY_ACCESS_URL).unwrap(), None);

    // A claim answered with something that isn't an access URL.
    bridge.script.lock().unwrap().claim = (200, "<html>hello</html>".into());
    let err = connect(&u, &guard, &bridge.setup_token(), &bridge.host()).unwrap_err();
    assert!(matches!(err, SimpleFinError::BadAccessUrl), "{err:?}");
    assert!(guard.allowlist().is_empty());

    // Not a token at all: nothing is sent.
    let before = bridge.seen().len();
    let err = connect(&u, &guard, "hello", "").unwrap_err();
    assert!(matches!(err, SimpleFinError::InvalidToken));
    assert_eq!(bridge.seen().len(), before);
}

#[test]
fn syncing_sends_basic_auth_and_stores_the_raw_response() {
    let (_dir, mut u, bridge, guard) = connected();
    let report = sync(&mut u, &guard, at(NOW)).unwrap();
    assert!(report.complete);
    assert_eq!(report.inserted(), 1);
    assert_eq!(report.accounts_created(), 1);
    assert_eq!(
        report.requests_left, 22,
        "90 days of history is two requests"
    );

    let requests = bridge.account_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].query("start-date"), Some(NOW - 90 * DAY));
    assert_eq!(requests[0].query("end-date"), Some(NOW - 45 * DAY));
    assert_eq!(requests[1].query("start-date"), Some(NOW - 45 * DAY));
    assert_eq!(requests[1].query("end-date"), Some(NOW));
    let request = &requests[0];
    assert_eq!(request.method, "GET");
    let expected = format!(
        "Basic {}",
        STANDARD.encode(format!("{USER}:{PASSWORD_DECODED}"))
    );
    assert_eq!(request.authorization.as_deref(), Some(expected.as_str()));
    assert!(request.target.starts_with("/simplefin/accounts?"));
    assert!(request.target.contains("pending=1") && request.target.contains("version=2"));

    // The URL never carried the credentials; the log has the path only.
    let log = guard.log();
    let entry = log.last().unwrap();
    assert_eq!(entry.path, "/simplefin/accounts");
    assert!(entry.query_omitted);
    assert_eq!(entry.purpose, PURPOSE_SYNC);
    assert!(matches!(
        entry.outcome,
        Outcome::Completed { status: 200, .. }
    ));
    secrets_absent(&log_text(&guard));

    let raw = u.raw_imports().unwrap();
    assert_eq!(raw.len(), 1, "both windows answered the same: stored once");
    assert_eq!(raw[0].source, ImportSource::SimpleFin);
    assert_eq!(
        raw[0].bytes(),
        accounts_body(&[TRN_COFFEE_PENDING]).as_bytes()
    );

    let accounts = u.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].external_id.as_deref(), Some("ACT-1"));
    assert_eq!(accounts[0].name, "Everyday Checking");
    assert_eq!(
        accounts[0].masked_number, None,
        "an opaque id has no last four digits"
    );
    let transactions = u.transactions().unwrap();
    assert_eq!(transactions.len(), 1);
    assert_eq!(transactions[0].status, TransactionStatus::Pending);
    assert_eq!(transactions[0].amount.minor(), -450);
}

#[test]
fn later_syncs_match_accounts_by_id_settle_pending_and_drop_repeats() {
    let (_dir, mut u, bridge, guard) = connected();
    sync(&mut u, &guard, at(NOW)).unwrap();
    let account: AccountId = u.accounts().unwrap()[0].id;

    // The same answer again: stored once, nothing new.
    let again = sync(&mut u, &guard, at(NOW + 3600)).unwrap();
    assert_eq!(again.inserted(), 0);
    assert_eq!(u.raw_imports().unwrap().len(), 1);

    // The coffee posts and rent appears; the overlap repeats nothing.
    bridge.answer(vec![(200, accounts_body(&[TRN_COFFEE_POSTED, TRN_RENT]))]);
    let later = sync(&mut u, &guard, at(NOW + 7200)).unwrap();
    assert_eq!(later.posted_from_pending(), 1);
    assert_eq!(later.inserted(), 1);
    assert_eq!(later.accounts_created(), 0);
    let transactions = u.transactions().unwrap();
    assert_eq!(transactions.len(), 2);
    assert!(transactions.iter().all(|t| t.account_id == account));
    assert!(
        transactions
            .iter()
            .all(|t| t.status == TransactionStatus::Posted)
    );
    // And once more: all duplicates.
    bridge.answer(vec![(200, accounts_body(&[TRN_RENT, TRN_COFFEE_POSTED]))]);
    let repeat = sync(&mut u, &guard, at(NOW + 9000)).unwrap();
    assert_eq!(repeat.inserted(), 0);
    assert_eq!(repeat.duplicates(), 2);
    assert_eq!(u.transactions().unwrap().len(), 2);
    assert_eq!(u.accounts().unwrap().len(), 1);
}

#[test]
fn windows_overlap_by_five_days_and_never_exceed_45() {
    let (_dir, mut u, bridge, guard) = connected();
    sync(&mut u, &guard, at(NOW)).unwrap();
    sync(&mut u, &guard, at(NOW + DAY)).unwrap();
    // A long gap: several windows in one sync, oldest first.
    let later = NOW + 200 * DAY;
    let report = sync(&mut u, &guard, at(later)).unwrap();
    assert!(report.complete);
    assert_eq!(report.synced_to, Some(later));

    let ranges: Vec<(i64, i64)> = bridge
        .account_requests()
        .iter()
        .map(|s| (s.query("start-date").unwrap(), s.query("end-date").unwrap()))
        .collect();
    assert_eq!(ranges[0], (NOW - 90 * DAY, NOW - 45 * DAY));
    assert_eq!(ranges[1], (NOW - 45 * DAY, NOW));
    assert_eq!(ranges[2], (NOW - 5 * DAY, NOW + DAY), "5 days of overlap");
    assert_eq!(ranges[3].0, NOW + DAY - 5 * DAY);
    assert_eq!(ranges.last().unwrap().1, later);
    assert_eq!(ranges.len(), 3 + 5, "204 days in 45-day windows");
    for w in ranges[3..].windows(2) {
        assert_eq!(w[0].1, w[1].0, "contiguous");
    }
    for (from, to) in ranges {
        assert!(from < to && to - from <= 45 * DAY, "{from}..{to}");
    }
}

#[test]
fn the_first_sync_is_two_requests_counted_before_either_is_sent() {
    let (_dir, u, bridge, guard) = connected();
    let mut run = SyncRun::start(&u, at(NOW)).unwrap();
    assert_eq!(
        status(&u, at(NOW)).unwrap().requests_left,
        24,
        "starting counts nothing"
    );
    let first = run.next(&u).unwrap().unwrap();
    assert_eq!((first.from, first.to), (NOW - 90 * DAY, NOW - 45 * DAY));
    // Counted and stored in the ledger, with nothing sent yet.
    assert!(bridge.account_requests().is_empty());
    assert_eq!(status(&u, at(NOW)).unwrap().requests_left, 23);
    let mut u = u.lock().unlock(PASSWORD.as_bytes()).unwrap();
    assert_eq!(
        status(&u, at(NOW)).unwrap().requests_left,
        23,
        "even across a restart"
    );

    let body = fetch(&guard, &run, first).unwrap();
    run.store(&mut u, first, body).unwrap();
    let second = run.next(&u).unwrap().unwrap();
    assert_eq!((second.from, second.to), (NOW - 45 * DAY, NOW));
    assert_eq!(
        bridge.account_requests().len(),
        1,
        "counted before it is sent"
    );
    assert_eq!(status(&u, at(NOW)).unwrap().requests_left, 22);
    let body = fetch(&guard, &run, second).unwrap();
    run.store(&mut u, second, body).unwrap();
    assert_eq!(run.next(&u).unwrap(), None);
    let report = run.finish(&u).unwrap();
    assert!(report.complete);
    assert_eq!(
        bridge.account_requests().len(),
        2,
        "90 days in two requests"
    );
    assert_eq!(report.requests_left, 22, "sending counted nothing more");
}

/// Review finding M2: one window at a time, each stored before the next is fetched,
/// so the ledger is held for one window's import at most.
#[test]
fn each_window_is_stored_before_the_next_is_fetched() {
    let (_dir, mut u, bridge, guard) = connected();
    let mut run = SyncRun::start(&u, at(NOW)).unwrap();
    let first = run.next(&u).unwrap().unwrap();
    let body = fetch(&guard, &run, first).unwrap();
    run.store(&mut u, first, body).unwrap();
    let shown = status(&u, at(NOW)).unwrap();
    assert_eq!(
        shown.synced_to,
        Some(NOW - 45 * DAY),
        "stored as it arrived"
    );
    assert_eq!(shown.last_sync, Some(NOW));
    assert_eq!(u.transactions().unwrap().len(), 1);
    assert_eq!(bridge.account_requests().len(), 1);
}

/// Review finding M2: a response over the limit isn't read, and a sync stops asking
/// once its byte budget is spent.
#[test]
fn responses_and_syncs_are_size_limited() {
    let (_dir, mut u, bridge, guard) = connected();
    let huge = format!(
        r#"{{"accounts": [], "padding": "{}"}}"#,
        "x".repeat(usize::try_from(MAX_RESPONSE_BYTES).unwrap())
    );
    bridge.answer(vec![(200, huge)]);
    let err = sync(&mut u, &guard, at(NOW)).unwrap_err();
    assert!(
        matches!(
            err,
            SimpleFinError::Network(fm_netguard::Error::BodyTooLarge { .. })
        ),
        "{err:?}"
    );
    assert!(u.raw_imports().unwrap().is_empty());

    bridge.answer(vec![(200, accounts_body(&[TRN_RENT]))]);
    let mut run = SyncRun::start(&u, at(NOW + 60)).unwrap();
    run.budget = 10;
    let window = run.next(&u).unwrap().unwrap();
    let body = fetch(&guard, &run, window).unwrap();
    run.store(&mut u, window, body).unwrap();
    assert_eq!(run.next(&u).unwrap(), None, "the budget is spent");
    let report = run.finish(&u).unwrap();
    assert!(!report.complete, "the next sync carries on");
    assert_eq!(report.synced_to, Some(window.to));
}

/// Review finding L3: a disconnect (or a new connection) while a sync is running
/// means the sync stores nothing more.
#[test]
fn a_sync_stores_nothing_after_a_disconnect() {
    let (_dir, mut u, bridge, guard) = connected();
    let mut run = SyncRun::start(&u, at(NOW)).unwrap();
    let window = run.next(&u).unwrap().unwrap();
    let body = fetch(&guard, &run, window).unwrap();
    disconnect(&u, &guard).unwrap();
    assert!(matches!(
        run.store(&mut u, window, body.clone()),
        Err(SimpleFinError::NotConnected)
    ));
    assert!(matches!(run.next(&u), Err(SimpleFinError::NotConnected)));
    assert!(u.raw_imports().unwrap().is_empty());
    for key in ALL_KEYS {
        assert_eq!(get(&u, key).unwrap(), None, "{key} written back");
    }

    // Reconnected with a different access URL meanwhile: also refused.
    let mut run = {
        connect(&u, &guard, &bridge.setup_token(), &bridge.host()).unwrap();
        SyncRun::start(&u, at(NOW)).unwrap()
    };
    let window = run.next(&u).unwrap().unwrap();
    disconnect(&u, &guard).unwrap();
    bridge.script.lock().unwrap().claim.1 = format!(
        "http://OTHER:{PASSWORD_IN_URL}@127.0.0.1:{}/simplefin",
        bridge.port
    );
    connect(&u, &guard, &bridge.setup_token(), &bridge.host()).unwrap();
    assert!(matches!(
        run.store(&mut u, window, body),
        Err(SimpleFinError::NotConnected)
    ));
    assert!(u.raw_imports().unwrap().is_empty());
}

/// Review finding M1: the user confirms the host before anything is sent.
#[test]
fn the_claim_needs_the_host_the_user_confirmed() {
    let (_dir, u) = ledger();
    let bridge = Bridge::start();
    let guard = NetGuard::new(Allowlist::new());
    let info = inspect_token(&bridge.setup_token()).unwrap();
    assert_eq!(info.host, bridge.host());
    assert!(!info.simplefin);
    for wrong in ["", "beta-bridge.simplefin.org", "127.0.0.1"] {
        assert!(matches!(
            connect(&u, &guard, &bridge.setup_token(), wrong),
            Err(SimpleFinError::HostNotConfirmed)
        ));
    }
    assert!(bridge.seen().is_empty(), "nothing sent");
    assert!(guard.allowlist().is_empty());
    connect(&u, &guard, &bridge.setup_token(), &info.host).unwrap();
}

/// Review finding M1: the bridge's answer must also name a public host.
#[test]
fn an_access_url_for_a_local_or_private_host_is_refused() {
    let (_dir, u) = ledger();
    let bridge = Bridge::start();
    let guard = NetGuard::new(Allowlist::new());
    for access in [
        "https://u:p@localhost/simplefin",
        "https://u:p@127.0.0.1:8443/simplefin",
        "https://u:p@10.0.0.7/simplefin",
        "https://u:p@169.254.169.254/latest",
        "https://u:p@[::1]/simplefin",
    ] {
        bridge.script.lock().unwrap().claim = (200, access.into());
        let err = connect(&u, &guard, &bridge.setup_token(), &bridge.host()).unwrap_err();
        assert!(
            matches!(err, SimpleFinError::BadAccessUrl),
            "{access}: {err:?}"
        );
        assert!(guard.allowlist().is_empty(), "{access}");
        assert!(!is_connected(&u).unwrap());
    }
}

/// Review finding L1: no part of the claim URL's path reaches the network log, even
/// with a trailing slash after the token.
#[test]
fn a_token_with_a_trailing_slash_stays_out_of_the_log() {
    let (_dir, u) = ledger();
    let bridge = Bridge::start();
    let guard = NetGuard::new(Allowlist::new());
    let token = STANDARD.encode(format!(
        "http://127.0.0.1:{}/simplefin/claim/{CLAIM_SECRET}/",
        bridge.port
    ));
    connect(&u, &guard, &token, &bridge.host()).unwrap();
    assert_eq!(guard.log()[0].path, "/<setup token>");
    secrets_absent(&log_text(&guard));
}

#[test]
fn the_daily_limit_is_enforced_before_sending_and_survives_a_restart() {
    let (dir, mut u, bridge, guard) = connected();
    // The first sync makes two requests, each later one one.
    for i in 0..23 {
        sync(&mut u, &guard, at(NOW + i * 60)).unwrap();
    }
    assert_eq!(bridge.account_requests().len(), 24);
    let err = sync(&mut u, &guard, at(NOW + 3600)).unwrap_err();
    let SimpleFinError::QuotaReached { next_allowed } = err else {
        panic!("{err:?}")
    };
    assert_eq!(next_allowed, NOW + DAY);
    assert_eq!(bridge.account_requests().len(), 24, "nothing was sent");

    // Lock and unlock (an app restart): still used up.
    let vault = u.lock();
    let mut u = vault.unlock(PASSWORD.as_bytes()).unwrap();
    assert!(matches!(
        sync(&mut u, &guard, at(NOW + 3600)),
        Err(SimpleFinError::QuotaReached { .. })
    ));
    // Setting the clock back frees nothing.
    assert!(matches!(
        sync(&mut u, &guard, at(NOW - 30 * DAY)),
        Err(SimpleFinError::QuotaReached { .. })
    ));
    let status = status(&u, at(NOW + 3600)).unwrap();
    assert_eq!(
        (status.requests_left, status.next_allowed),
        (0, Some(NOW + DAY))
    );
    assert_eq!(bridge.account_requests().len(), 24);

    // A day after the first, one is free again.
    sync(&mut u, &guard, at(NOW + DAY)).unwrap();
    assert_eq!(bridge.account_requests().len(), 25);
    drop(dir);
}

#[test]
fn a_sync_needing_more_requests_than_are_left_stops_and_continues_later() {
    let (_dir, mut u, bridge, guard) = connected();
    // 23 requests in the last hour, and 200 days since the last sync (3 windows).
    let mut used = Quota::default();
    for i in 0..23 {
        used.record(NOW - 3600 + i * 60);
    }
    set(&u, KEY_REQUESTS, Some(&used.encode())).unwrap();
    set(&u, KEY_SYNCED_TO, Some(&(NOW - 200 * DAY).to_string())).unwrap();

    let partial = sync(&mut u, &guard, at(NOW)).unwrap();
    assert!(!partial.complete);
    assert_eq!(partial.requests_left, 0);
    assert_eq!(partial.next_allowed, Some(NOW - 3600 + DAY));
    let fetched_to = NOW - 205 * DAY + 45 * DAY;
    assert_eq!(
        partial.synced_to,
        Some(fetched_to),
        "the oldest window only"
    );
    assert_eq!(bridge.account_requests().len(), 1);

    // The next day it carries on from there (with the usual overlap).
    let rest = sync(&mut u, &guard, at(NOW + DAY)).unwrap();
    assert!(rest.complete);
    assert_eq!(rest.synced_to, Some(NOW + DAY));
    let requests = bridge.account_requests();
    assert_eq!(requests[1].query("start-date"), Some(fetched_to - 5 * DAY));
}

#[test]
fn bridge_errors_are_reported_and_shown() {
    let (_dir, mut u, bridge, guard) = connected();
    let with_errors = r#"{"errlist": [{"code": "con.auth", "msg": "Demo Bank needs you to sign in again", "conn_id": "CON-1"}],
        "errors": ["Demo Bank needs you to sign in again"], "accounts": []}"#;
    bridge.answer(vec![(200, with_errors.into())]);
    let report = sync(&mut u, &guard, at(NOW)).unwrap();
    assert!(report.messages.iter().any(|m| m.contains("sign in again")));
    assert_eq!(
        u.raw_imports().unwrap().len(),
        1,
        "stored even with no accounts"
    );
    let shown = status(&u, at(NOW)).unwrap().messages;
    assert_eq!(
        report.messages.len(),
        1,
        "a message repeated by both windows shows once"
    );
    assert_eq!(shown, report.messages, "kept for the settings screen");

    for (code, expected) in [(403, "revoked"), (402, "payment"), (500, "bridge")] {
        bridge.answer(vec![(code, "secret body text".into())]);
        let err = sync(&mut u, &guard, at(NOW + 60)).unwrap_err();
        let ok = match expected {
            "revoked" => matches!(err, SimpleFinError::AccessRevoked),
            "payment" => matches!(err, SimpleFinError::PaymentRequired),
            _ => matches!(err, SimpleFinError::Bridge { status: 500 }),
        };
        assert!(ok, "{code}: {err:?}");
        assert!(!err.to_string().contains("secret body"));
        secrets_absent(&err.to_string());
    }
    bridge.answer(vec![(200, "{not json".into())]);
    let err = sync(&mut u, &guard, at(NOW + 120)).unwrap_err();
    assert!(matches!(err, SimpleFinError::Malformed));
    // Failed requests still count.
    assert_eq!(status(&u, at(NOW + 120)).unwrap().requests_left, 24 - 2 - 4);
}

#[test]
fn the_bridge_must_be_allowed_again_after_a_restart() {
    let (_dir, mut u, bridge, _guard) = connected();
    // A fresh guard, as after the app restarts: the bridge isn't allowed yet.
    let guard = NetGuard::new(Allowlist::new());
    let err = sync(&mut u, &guard, at(NOW)).unwrap_err();
    assert!(
        matches!(
            err,
            SimpleFinError::Network(fm_netguard::Error::Blocked { .. })
        ),
        "{err:?}"
    );
    assert!(bridge.account_requests().is_empty());
    guard.allow(bridge_origin(&u).unwrap().unwrap());
    sync(&mut u, &guard, at(NOW)).unwrap();
    assert_eq!(bridge.account_requests().len(), 2);
}

#[test]
fn disconnecting_forgets_the_credential_and_the_bridge() {
    let (_dir, mut u, _bridge, guard) = connected();
    sync(&mut u, &guard, at(NOW)).unwrap();
    disconnect(&u, &guard).unwrap();
    assert!(guard.allowlist().is_empty());
    for key in ALL_KEYS {
        assert_eq!(get(&u, key).unwrap(), None, "{key}");
    }
    assert!(!status(&u, at(NOW)).unwrap().connected());
    assert!(matches!(
        sync(&mut u, &guard, at(NOW)),
        Err(SimpleFinError::NotConnected)
    ));
    assert_eq!(u.transactions().unwrap().len(), 1, "imported data stays");
}

#[test]
fn errors_never_show_secrets() {
    let errors = [
        SimpleFinError::InvalidToken,
        SimpleFinError::HostNotConfirmed,
        SimpleFinError::BadAccessUrl,
        SimpleFinError::TokenAlreadyUsed,
        SimpleFinError::AlreadyConnected,
        SimpleFinError::NotConnected,
        SimpleFinError::AccessRevoked,
        SimpleFinError::PaymentRequired,
        SimpleFinError::Bridge { status: 418 },
        SimpleFinError::QuotaReached { next_allowed: NOW },
        SimpleFinError::Malformed,
    ];
    for e in errors {
        secrets_absent(&e.to_string());
        secrets_absent(&format!("{e:?}"));
    }
}

/// Run once by hand (approved: the public demo token, 1 claim and at most 2
/// `/accounts` calls):
/// `cargo test -p fm-core --lib connectors::simplefin::tests::simplefin_demo -- --ignored --nocapture`
#[test]
#[ignore = "calls SimpleFIN's public demo bridge"]
fn simplefin_demo() {
    const DEMO_TOKEN: &str = "aHR0cHM6Ly9iZXRhLWJyaWRnZS5zaW1wbGVmaW4ub3JnL3NpbXBsZWZpbi9jbGFpbS9ERU1PLXYyLTUyMjI1MEZCMzBBNDNGRUNEQjBD";
    let (_dir, mut u) = ledger();
    let guard = NetGuard::new(Allowlist::new());
    let host = inspect_token(DEMO_TOKEN).unwrap().host;
    let connected = connect(&u, &guard, DEMO_TOKEN, &host).unwrap();
    println!("bridge: {:?}", connected.bridge);
    let report = sync(&mut u, &guard, SystemTime::now()).unwrap();
    let accounts_calls = guard
        .log()
        .iter()
        .filter(|e| e.purpose == PURPOSE_SYNC)
        .count();
    assert!(accounts_calls <= 2);
    println!(
        "inserted {} into {} new accounts; messages: {:?}",
        report.inserted(),
        report.accounts_created(),
        report.messages
    );
    println!("network log:");
    for e in guard.log() {
        println!(
            "  {} {}{} [{}] query_omitted={} {:?}",
            e.method, e.origin, e.path, e.purpose, e.query_omitted, e.outcome
        );
    }
    // When in the UTC day do the bridge's timestamps fall? (see the README)
    let mut hours = std::collections::BTreeMap::<i64, usize>::new();
    for raw in u.raw_imports().unwrap() {
        let v: serde_json::Value = serde_json::from_slice(raw.bytes()).unwrap();
        for a in v["accounts"].as_array().into_iter().flatten() {
            for t in a["transactions"].as_array().into_iter().flatten() {
                for field in ["posted", "transacted_at"] {
                    if let Some(s) = t[field].as_i64().filter(|&s| s > 0) {
                        *hours.entry(s.rem_euclid(DAY) / 3600).or_default() += 1;
                    }
                }
            }
        }
    }
    println!("UTC hour of timestamps -> count: {hours:?}");
    assert!(report.inserted() > 0);
}
