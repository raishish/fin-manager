//! SimpleFIN in the app (T5.4). The connector is `fm_core::connectors::simplefin`;
//! `app/src/routes/connectors/api.ts` mirrors these types.
//!
//! The network parts (the claim, the `/accounts` requests) run without holding the
//! ledger, so locking never waits for a slow bridge. The bridge's origin is on the
//! app's allowlist only while the ledger is open and connected ([`BridgeAllowance`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use fm_core::connectors::simplefin::{self, MAX_REQUESTS_PER_DAY, SimpleFinError, SyncReport};
use fm_core::pipeline;
use fm_core::storage::Unlocked;
use fm_netguard::{NetGuard, Origin};
use serde::Serialize;

use crate::importing::AfterImport;
use crate::unlock::AppUnlock;

/// The unfinished-import mark a sync leaves until transfers, the rules and the backup
/// have run after it (see `importing.rs`).
const SYNC_MARK: &str = "simplefin-sync";

fn storage(e: fm_core::storage::StorageError) -> ConnectorErrorView {
    error("Storage", &e.to_string())
}

/// Keeps the bridge's origin on the allowlist exactly while the ledger is open and
/// connected. Called by the unlock observer after every change.
pub struct BridgeAllowance {
    net: Arc<NetGuard>,
    allowed: Mutex<Option<Origin>>,
}

impl BridgeAllowance {
    pub fn new(net: Arc<NetGuard>) -> Self {
        Self {
            net,
            allowed: Mutex::new(None),
        }
    }

    pub fn update(&self, open: Option<&Unlocked>) {
        let wanted = open.and_then(|l| simplefin::bridge_origin(l).ok().flatten());
        let mut allowed = self.allowed.lock().unwrap_or_else(PoisonError::into_inner);
        if *allowed == wanted {
            return;
        }
        if let Some(old) = allowed.take() {
            self.net.revoke(&old);
        }
        if let Some(new) = &wanted {
            self.net.allow(new.clone());
        }
        *allowed = wanted;
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusView {
    connected: bool,
    bridge: Option<String>,
    /// Unix seconds.
    synced_to: Option<i64>,
    last_sync: Option<i64>,
    requests_left: usize,
    requests_per_day: usize,
    next_allowed: Option<i64>,
    /// The bridge's own messages, shown as they are.
    messages: Vec<String>,
}

impl From<simplefin::Status> for StatusView {
    fn from(s: simplefin::Status) -> Self {
        Self {
            connected: s.connected(),
            bridge: s.bridge,
            synced_to: s.synced_to,
            last_sync: s.last_sync,
            requests_left: s.requests_left,
            requests_per_day: MAX_REQUESTS_PER_DAY,
            next_allowed: s.next_allowed,
            messages: s.messages,
        }
    }
}

/// Where a setup token points, for the user to confirm before connecting.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenView {
    /// `host[:port]`.
    host: String,
    /// The host is SimpleFIN's own (`simplefin.org`).
    simplefin: bool,
}

impl From<simplefin::TokenInfo> for TokenView {
    fn from(t: simplefin::TokenInfo) -> Self {
        Self {
            host: t.host,
            simplefin: t.simplefin,
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncView {
    inserted: usize,
    duplicates: usize,
    posted_from_pending: usize,
    accounts_created: usize,
    messages: Vec<String>,
    /// False when the day's requests ran out first; the next sync continues.
    complete: bool,
    synced_to: Option<i64>,
    requests_left: usize,
    next_allowed: Option<i64>,
    /// The backup after the update failed (what was fetched is kept).
    backup_error: Option<String>,
    /// Transfers between my accounts paired, and what the rules filed (T5.13).
    transfers_paired: usize,
    transfers_suggested: usize,
    rule_buckets: usize,
    cleaned_merchants: usize,
    /// What was fetched is kept, but this step after it didn't finish ("pairing
    /// transfers", "applying your rules", "the backup"); `finish_import` runs it.
    incomplete: Option<&'static str>,
}

impl SyncView {
    fn new(r: SyncReport, after: AfterImport) -> Self {
        let backup_error = match &after.backup {
            Some(b) if !b.ok => b.message.clone(),
            _ => None,
        };
        Self {
            transfers_paired: after.transfers_paired,
            transfers_suggested: after.transfers_suggested,
            rule_buckets: after.rule_buckets,
            cleaned_merchants: after.cleaned_merchants,
            incomplete: after.incomplete,
            inserted: r.inserted(),
            duplicates: r.duplicates(),
            posted_from_pending: r.posted_from_pending(),
            accounts_created: r.accounts_created(),
            complete: r.complete,
            synced_to: r.synced_to,
            requests_left: r.requests_left,
            next_allowed: r.next_allowed,
            messages: r.messages,
            backup_error,
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorErrorView {
    code: &'static str,
    message: String,
    /// For `QuotaReached`: Unix seconds.
    next_allowed: Option<i64>,
}

impl From<SimpleFinError> for ConnectorErrorView {
    fn from(e: SimpleFinError) -> Self {
        let mut next_allowed = None;
        let code = match &e {
            SimpleFinError::InvalidToken => "InvalidToken",
            SimpleFinError::HostNotConfirmed => "HostNotConfirmed",
            SimpleFinError::BadAccessUrl => "BadAccessUrl",
            SimpleFinError::BridgeHostChanged => "BridgeHostChanged",
            SimpleFinError::TokenAlreadyUsed => "TokenAlreadyUsed",
            SimpleFinError::AlreadyConnected => "AlreadyConnected",
            SimpleFinError::NotConnected => "NotConnected",
            SimpleFinError::AccessRevoked => "AccessRevoked",
            SimpleFinError::PaymentRequired => "PaymentRequired",
            SimpleFinError::Bridge { .. } => "Bridge",
            SimpleFinError::QuotaReached { next_allowed: t } => {
                next_allowed = Some(*t);
                "QuotaReached"
            }
            SimpleFinError::Malformed => "Malformed",
            SimpleFinError::Interrupted => "Interrupted",
            SimpleFinError::Network(_) => "Network",
            _ => "Connector",
        };
        Self {
            code,
            message: e.to_string(),
            next_allowed,
        }
    }
}

fn error(code: &'static str, message: &str) -> ConnectorErrorView {
    ConnectorErrorView {
        code,
        message: message.into(),
        next_allowed: None,
    }
}

fn locked() -> ConnectorErrorView {
    error("Locked", "unlock first")
}

type Result<T> = std::result::Result<T, ConnectorErrorView>;

/// One connect or sync at a time.
#[derive(Default)]
pub struct Busy(AtomicBool);

struct BusyGuard<'a>(&'a AtomicBool);

impl Busy {
    fn start(&self) -> Result<BusyGuard<'_>> {
        if self.0.swap(true, Ordering::AcqRel) {
            return Err(error("Busy", "SimpleFIN is already being updated"));
        }
        Ok(BusyGuard(&self.0))
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl AppUnlock {
    pub fn simplefin_status(&self) -> Result<StatusView> {
        self.with_ledger(|open| simplefin::status(open.ledger(), SystemTime::now()))
            .ok_or_else(locked)?
            .map(Into::into)
            .map_err(Into::into)
    }

    /// Where a setup token points, for the user to confirm. Contacts nothing.
    pub fn simplefin_inspect_token(&self, setup_token: &str) -> Result<TokenView> {
        // Gated like the rest, though it touches no data.
        self.with_ledger(|_| ()).ok_or_else(locked)?;
        Ok(simplefin::inspect_token(setup_token)?.into())
    }

    /// Claims the token once the user has confirmed its host (`confirmed_host`).
    pub fn simplefin_connect(
        &self,
        net: &NetGuard,
        busy: &Busy,
        setup_token: &str,
        confirmed_host: &str,
    ) -> Result<StatusView> {
        let _busy = busy.start()?;
        if self
            .with_ledger(|open| simplefin::is_connected(open.ledger()))
            .ok_or_else(locked)??
        {
            return Err(SimpleFinError::AlreadyConnected.into());
        }
        let claimed = simplefin::claim(net, setup_token, confirmed_host)?;
        let origin = claimed.origin().clone();
        let saved = self
            .write_ledger(|open| simplefin::save(open.ledger(), net, claimed))
            .ok_or_else(locked)
            .and_then(|r| r.map_err(Into::into));
        if saved.is_err() {
            // Locked meanwhile, or the save failed: don't leave the bridge allowed.
            net.revoke(&origin);
        }
        saved.map(Into::into)
    }

    /// One window at a time: the ledger is taken to count a request, released while
    /// the bridge answers, and taken again to store that one response. Locking waits
    /// for one window's import at most; if the ledger is locked meanwhile, the sync
    /// stops (what was stored stays, and the next sync carries on).
    pub fn simplefin_sync(&self, net: &NetGuard, busy: &Busy) -> Result<SyncView> {
        let _busy = busy.start()?;
        let now = SystemTime::now();
        let (mut run, earlier, generation) = self
            .with_ledger(|open| -> Result<_> {
                let run = simplefin::SyncRun::start(open.ledger(), now)?;
                let earlier = pipeline::unfinished(open.ledger()).map_err(storage)?;
                Ok((run, earlier, self.generation()))
            })
            .ok_or_else(locked)??;
        // One backup after the whole sync, with transfers and the rules (below).
        run.set_backup(false);
        let cancel = || self.interrupt.load(Ordering::SeqCst);
        let mut marked = false;
        let stopped = loop {
            let window = match self.with_ledger(|open| run.next(open.ledger(), SystemTime::now())) {
                None => break Some(locked()),
                Some(Err(e)) => break Some(e.into()),
                Some(Ok(None)) => break None,
                Some(Ok(Some(window))) => window,
            };
            let body = match simplefin::fetch(net, &run, window) {
                Ok(body) => body,
                Err(e) => break Some(e.into()),
            };
            let stored = self.write_ledger(|open| -> Result<()> {
                if self.generation() != generation {
                    return Err(locked());
                }
                // Marked before the first window commits, as a file import is: a lock
                // or crash later leaves its transfers and rules to finish.
                let first = !marked;
                if first {
                    pipeline::set_unfinished(open.ledger(), Some(SYNC_MARK)).map_err(storage)?;
                    marked = true;
                }
                let result = run.store_cancellable(open.ledger_mut(), window, body, &cancel);
                if result.is_err() && first {
                    // Nothing of this sync was stored: the earlier mark, if any, again.
                    pipeline::set_unfinished(open.ledger(), earlier.as_deref()).map_err(storage)?;
                    marked = false;
                }
                result.map_err(Into::into)
            });
            match stored {
                None => break Some(locked()),
                Some(Err(e)) => break Some(e),
                Some(Ok(())) => {}
            }
        };
        let report = self
            .with_ledger(|open| run.finish(open.ledger()))
            .ok_or_else(locked)??;
        // Transfers, the rules and the backup, once, for what this sync stored. Over
        // everything when another import was left unfinished: there is one mark, and
        // this sync's replaced it.
        let after = if marked {
            let scope = if earlier.is_some() {
                None
            } else {
                Some(report.changed())
            };
            self.after_import(scope, now, SYNC_MARK, generation)
        } else {
            AfterImport::default()
        };
        match stopped {
            Some(e) => Err(e),
            None => Ok(SyncView::new(report, after)),
        }
    }

    /// Waits for no sync: refused while one runs (a sync also stores nothing after a
    /// disconnect).
    pub fn simplefin_disconnect(&self, net: &NetGuard, busy: &Busy) -> Result<StatusView> {
        let _busy = busy.start()?;
        self.write_ledger(|open| {
            simplefin::disconnect(open.ledger(), net)?;
            simplefin::status(open.ledger(), SystemTime::now())
        })
        .ok_or_else(locked)?
        .map(Into::into)
        .map_err(Into::into)
    }
}

/// Tauri commands behind `app/src/routes/connectors/api.ts`. All need an open ledger
/// (the unlock gate refuses them otherwise).
pub mod commands {
    use tauri::State;
    use zeroize::Zeroizing;

    use super::{Busy, Result, StatusView, SyncView, TokenView};
    use crate::Net;
    use crate::unlock::AppUnlock;

    #[tauri::command]
    pub fn simplefin_status(unlock: State<'_, AppUnlock>) -> Result<StatusView> {
        unlock.simplefin_status()
    }

    /// `setup_token` is a secret until claimed; it is wiped after use.
    #[tauri::command]
    pub fn simplefin_inspect_token(
        unlock: State<'_, AppUnlock>,
        setup_token: String,
    ) -> Result<TokenView> {
        let setup_token = Zeroizing::new(setup_token);
        unlock.simplefin_inspect_token(&setup_token)
    }

    /// `confirmed_host` is the host the user confirmed after `simplefin_inspect_token`.
    #[tauri::command(async)]
    pub fn simplefin_connect(
        unlock: State<'_, AppUnlock>,
        net: State<'_, Net>,
        busy: State<'_, Busy>,
        setup_token: String,
        confirmed_host: String,
    ) -> Result<StatusView> {
        let setup_token = Zeroizing::new(setup_token);
        unlock.simplefin_connect(&net.0, &busy, &setup_token, &confirmed_host)
    }

    #[tauri::command(async)]
    pub fn simplefin_sync(
        unlock: State<'_, AppUnlock>,
        net: State<'_, Net>,
        busy: State<'_, Busy>,
    ) -> Result<SyncView> {
        unlock.simplefin_sync(&net.0, &busy)
    }

    #[tauri::command]
    pub fn simplefin_disconnect(
        unlock: State<'_, AppUnlock>,
        net: State<'_, Net>,
        busy: State<'_, Busy>,
    ) -> Result<StatusView> {
        unlock.simplefin_disconnect(&net.0, &busy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_a_code_and_the_quota_time() {
        let view = ConnectorErrorView::from(SimpleFinError::QuotaReached { next_allowed: 42 });
        assert_eq!(view.code, "QuotaReached");
        assert_eq!(view.next_allowed, Some(42));
        assert!(view.message.contains("24"));
        let view = ConnectorErrorView::from(SimpleFinError::TokenAlreadyUsed);
        assert_eq!(view.code, "TokenAlreadyUsed");
        assert!(view.message.contains("disable it in SimpleFIN"));
    }

    /// Review finding L3: a disconnect waits for no sync; it is refused while one runs.
    #[test]
    fn only_one_update_at_a_time() {
        let busy = Busy::default();
        let first = busy.start().unwrap();
        assert_eq!(busy.start().err().unwrap().code, "Busy");
        drop(first);
        assert!(busy.start().is_ok());
    }

    // ---- a real sync against a fake bridge on this machine (fm-core's `fake-bridge`) ----

    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use fm_core::model::{Assignment, TransactionId};
    use tauri::Manager;

    const DAY: i64 = 86_400;

    /// A SimpleFIN bridge on 127.0.0.1: the claim, then `/accounts` answered from a
    /// queue (the last answer repeats). An answer can wait for a go-ahead, so a test
    /// can lock the app between two windows of a sync.
    struct Bridge {
        port: u16,
    }

    struct Answer {
        body: String,
        wait: Option<mpsc::Receiver<()>>,
    }

    impl Bridge {
        fn start(answers: Vec<Answer>) -> Bridge {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let answers = Mutex::new(answers);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let mut length = 0;
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).unwrap();
                        let header = header.trim_end();
                        if header.is_empty() {
                            break;
                        }
                        if let Some((name, value)) = header.split_once(':')
                            && name.eq_ignore_ascii_case("content-length")
                        {
                            length = value.trim().parse().unwrap();
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let reply = if line.contains("/claim/") {
                        format!("http://AB12:secret@127.0.0.1:{port}/simplefin")
                    } else {
                        let mut queue = answers.lock().unwrap();
                        let answer = if queue.len() > 1 {
                            queue.remove(0)
                        } else {
                            Answer {
                                body: queue[0].body.clone(),
                                wait: queue[0].wait.take(),
                            }
                        };
                        drop(queue);
                        if let Some(go) = answer.wait {
                            let _ = go.recv_timeout(Duration::from_secs(20));
                        }
                        answer.body
                    };
                    let _ = stream.write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                            reply.len()
                        )
                        .as_bytes(),
                    );
                }
            });
            Bridge { port }
        }

        fn token(&self) -> String {
            base64(format!("http://127.0.0.1:{}/simplefin/claim/SECRET", self.port).as_bytes())
        }

        fn host(&self) -> String {
            format!("127.0.0.1:{}", self.port)
        }
    }

    /// Standard base64, for a setup token (the app has no base64 dependency of its own).
    fn base64(bytes: &[u8]) -> String {
        const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ABC[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn now() -> i64 {
        i64::try_from(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap()
    }

    /// Two own accounts at one bank: a transfer between them, and two purchases.
    fn accounts(tag: &str, days_ago: i64) -> String {
        let at = now() - days_ago * DAY;
        format!(
            r#"{{"connections": [{{"conn_id": "C", "name": "Example Bank"}}],
              "accounts": [
                {{"id": "ACT-CHK", "name": "Checking", "conn_id": "C", "currency": "USD",
                  "balance": "1000.00", "balance-date": {at}, "transactions": [
                   {{"id": "{tag}-1", "posted": {at}, "amount": "-500.00", "description": "ONLINE TRANSFER TO SAVINGS {tag}"}},
                   {{"id": "{tag}-2", "posted": {at}, "amount": "-12.40", "description": "SQ *BLUE BOTTLE COFFEE OAKLAND CA"}},
                   {{"id": "{tag}-3", "posted": {at}, "amount": "-63.10", "description": "TRADER JOE S #123 SAN FRANCISCO CA"}}]}},
                {{"id": "ACT-SAV", "name": "Savings", "conn_id": "C", "currency": "USD",
                  "balance": "5000.00", "balance-date": {at}, "transactions": [
                   {{"id": "{tag}-4", "posted": {at}, "amount": "500.00", "description": "ONLINE TRANSFER FROM CHECKING {tag}"}}]}}]}}"#
        )
    }

    fn answer(body: String) -> Answer {
        Answer { body, wait: None }
    }

    struct App {
        _dir: tempfile::TempDir,
        app: tauri::App<tauri::test::MockRuntime>,
    }

    impl App {
        fn new() -> App {
            let dir = tempfile::tempdir().unwrap();
            let (app, w) = crate::tests::mock_app(&std::fs::canonicalize(dir.path()).unwrap());
            crate::tests::create_confirmed(&w);
            App { _dir: dir, app }
        }

        fn unlock(&self) -> tauri::State<'_, AppUnlock> {
            self.app.state::<AppUnlock>()
        }

        fn net(&self) -> &NetGuard {
            &self.app.state::<crate::Net>().inner().0
        }

        fn busy(&self) -> tauri::State<'_, Busy> {
            self.app.state::<Busy>()
        }

        fn connect(&self, bridge: &Bridge) {
            self.unlock()
                .simplefin_connect(self.net(), &self.busy(), &bridge.token(), &bridge.host())
                .unwrap();
        }

        fn sync(&self) -> Result<SyncView> {
            self.unlock().simplefin_sync(self.net(), &self.busy())
        }

        fn ledger(&self) -> fm_core::model::Ledger {
            self.unlock()
                .with_ledger(|o| o.ledger().load_ledger().unwrap())
                .unwrap()
        }

        fn mark(&self) -> Option<String> {
            self.unlock()
                .with_ledger(|o| pipeline::unfinished(o.ledger()).unwrap())
                .unwrap()
        }

        fn reopen(&self) {
            self.unlock()
                .unlock_with_password(zeroize::Zeroizing::new(
                    "velvet anchor mosaic tundra".to_string(),
                ))
                .unwrap();
        }
    }

    /// Transactions that have a merchant decision: the rules (the merchant clean-up)
    /// have run over them.
    fn with_merchant(l: &fm_core::model::Ledger) -> std::collections::BTreeSet<TransactionId> {
        l.decisions
            .iter()
            .filter(|d| matches!(d.assignment, Assignment::Merchant(_)))
            .map(|d| d.transaction_id)
            .collect()
    }

    /// T5.13 done-when: a synced window's transfer pair is detected and the rules run
    /// over its rows, then one backup; no mark is left.
    #[test]
    fn a_sync_pairs_transfers_and_runs_the_rules() {
        const { assert!(fm_core::FAKE_BRIDGE, "the tests' fake bridge is on") };
        let app = App::new();
        let bridge = Bridge::start(vec![answer(accounts("A", 10))]);
        app.connect(&bridge);
        let view = app.sync().unwrap();
        assert_eq!(view.incomplete, None);
        assert_eq!(view.inserted, 4);
        assert_eq!(view.transfers_paired, 1, "{view:?}");
        assert!(view.cleaned_merchants >= 2, "{view:?}");
        assert_eq!(app.mark(), None);
        let l = app.ledger();
        assert_eq!(
            with_merchant(&l).len(),
            4,
            "every synced row went through the rules"
        );
    }

    /// T5.13 done-when: a lock between two windows leaves the first window kept and the
    /// sync's mark set; finishing it runs transfers and the rules over what was kept.
    #[test]
    fn a_lock_mid_sync_leaves_it_to_finish() {
        let app = App::new();
        let (go, wait) = mpsc::channel();
        let bridge = Bridge::start(vec![
            answer(accounts("W1", 60)),
            Answer {
                body: accounts("W2", 10),
                wait: Some(wait),
            },
        ]);
        app.connect(&bridge);
        let (unlock, net, busy) = (app.unlock(), app.net(), app.busy());
        let result = thread::scope(|s| {
            let sync = s.spawn(|| unlock.simplefin_sync(net, &busy));
            // The second window is being fetched: the first is stored. Lock now.
            thread::sleep(Duration::from_millis(500));
            unlock.lock();
            go.send(()).unwrap();
            sync.join().unwrap()
        });
        assert_eq!(result.unwrap_err().code, "Locked");
        app.reopen();
        assert_eq!(app.mark().as_deref(), Some(SYNC_MARK), "left to finish");
        let l = app.ledger();
        assert_eq!(l.transactions.len(), 4, "the first window is kept");
        assert!(with_merchant(&l).is_empty(), "its rules haven't run yet");

        let finished = app.unlock().finish_import(None).unwrap();
        assert_eq!(finished.incomplete, None);
        assert_eq!(app.mark(), None);
        let l = app.ledger();
        assert_eq!(
            with_merchant(&l).len(),
            4,
            "the rules ran over the kept window"
        );
    }

    /// The single mark, order (a): a file import is left unfinished, then a sync runs.
    /// The sync's pass covers both, and no mark is left.
    #[test]
    fn a_sync_after_an_unfinished_file_import_covers_both() {
        let app = App::new();
        let file = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../testdata/bank-exports/data/chase/checking_2025.csv"),
        )
        .unwrap();
        // The file imported, but the app locked before transfers and the rules.
        let file_rows: Vec<TransactionId> = app
            .unlock()
            .write_ledger(|o| {
                let u = o.ledger_mut();
                pipeline::set_unfinished(u, Some("pending")).unwrap();
                let profiles = fm_import_csv::ProfileSet::with_builtin();
                let report = pipeline::import_file(
                    u,
                    &pipeline::ImportRequest {
                        bytes: &file,
                        file_name: None,
                        target: pipeline::Target::NewAccount {
                            name: "Card".into(),
                            kind: fm_core::model::AccountKind::Checking,
                            currency: fm_core::model::Currency::USD,
                        },
                        csv_currency: fm_core::model::Currency::USD,
                        csv_profiles: &profiles,
                        now: SystemTime::now(),
                        backup: false,
                        cancel: &pipeline::never,
                    },
                )
                .unwrap();
                pipeline::set_unfinished(u, Some(&report.import_id.0.to_string())).unwrap();
                report.statements[0]
                    .records
                    .iter()
                    .map(|(_, o)| o.transaction())
                    .collect()
            })
            .unwrap();
        assert!(with_merchant(&app.ledger()).is_empty());

        let bridge = Bridge::start(vec![answer(accounts("A", 10))]);
        app.connect(&bridge);
        let view = app.sync().unwrap();
        assert_eq!(view.incomplete, None);
        assert_eq!(app.mark(), None, "no mark left");
        let named = with_merchant(&app.ledger());
        let named_file_rows = file_rows.iter().filter(|t| named.contains(t)).count();
        assert!(
            named_file_rows > file_rows.len() / 2,
            "the file's rows got the rules too: {named_file_rows} of {}",
            file_rows.len()
        );
        assert_eq!(view.transfers_paired, 1);
    }

    /// The single mark, order (b): a sync is left unfinished, then a file import runs.
    /// The import's pass covers the sync's rows too.
    #[test]
    fn a_file_import_after_an_unfinished_sync_covers_both() {
        let app = App::new();
        let (go, wait) = mpsc::channel();
        let bridge = Bridge::start(vec![
            answer(accounts("W1", 60)),
            Answer {
                body: accounts("W2", 10),
                wait: Some(wait),
            },
        ]);
        app.connect(&bridge);
        let (unlock, net, busy) = (app.unlock(), app.net(), app.busy());
        thread::scope(|s| {
            let sync = s.spawn(|| unlock.simplefin_sync(net, &busy));
            thread::sleep(Duration::from_millis(500));
            unlock.lock();
            go.send(()).unwrap();
            let _ = sync.join().unwrap();
        });
        app.reopen();
        assert_eq!(app.mark().as_deref(), Some(SYNC_MARK));
        let synced: Vec<TransactionId> = app.ledger().transactions.iter().map(|t| t.id).collect();

        let file = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../testdata/bank-exports/data/chase/checking_2025.csv"),
        )
        .unwrap();
        let report = app
            .unlock()
            .import_file(
                &file,
                crate::importing::ImportArgs {
                    file_name: None,
                    target: crate::importing::TargetArg::NewAccount {
                        name: "Card".into(),
                        account_kind: fm_core::model::AccountKind::Checking,
                        currency: "USD".into(),
                    },
                    csv_currency: None,
                },
            )
            .unwrap();
        assert_eq!(report.incomplete, None);
        assert_eq!(app.mark(), None, "no mark left");
        let l = app.ledger();
        let named = with_merchant(&l);
        assert!(
            synced.iter().filter(|t| named.contains(t)).count() == 4,
            "the sync's rows got the rules"
        );
        assert_eq!(
            l.transactions
                .iter()
                .filter(|t| synced.contains(&t.id))
                .count(),
            4
        );
        let paired = l
            .decisions
            .iter()
            .filter(|d| {
                synced.contains(&d.transaction_id)
                    && matches!(
                        d.assignment,
                        Assignment::Bucket(fm_core::model::Bucket::Transfer {
                            counterpart: Some(_)
                        })
                    )
            })
            .count();
        assert_eq!(paired, 2, "the sync's transfer pair was detected");
    }

    #[test]
    fn nothing_is_allowed_without_a_connection() {
        let net = Arc::new(NetGuard::new(fm_netguard::Allowlist::new()));
        let bridge = BridgeAllowance::new(Arc::clone(&net));
        bridge.update(None);
        assert!(net.allowlist().is_empty());
    }
}
