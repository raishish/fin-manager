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
use fm_core::storage::Unlocked;
use fm_netguard::{NetGuard, Origin};
use serde::Serialize;

use crate::unlock::AppUnlock;

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
    /// The backup after the import failed (the import itself is kept).
    backup_error: Option<String>,
}

impl From<SyncReport> for SyncView {
    fn from(r: SyncReport) -> Self {
        let backup_error = r.imports.iter().find_map(|i| match &i.backup {
            Some(Err(e)) => Some(e.to_string()),
            _ => None,
        });
        Self {
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
        let mut run = self
            .with_ledger(|open| simplefin::SyncRun::start(open.ledger(), now))
            .ok_or_else(locked)??;
        while let Some(window) = self
            .with_ledger(|open| run.next(open.ledger()))
            .ok_or_else(locked)??
        {
            let body = simplefin::fetch(net, &run, window)?;
            self.write_ledger(|open| run.store(open.ledger_mut(), window, body))
                .ok_or_else(locked)??;
        }
        let report = self
            .with_ledger(|open| run.finish(open.ledger()))
            .ok_or_else(locked)??;
        Ok(report.into())
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

    #[test]
    fn nothing_is_allowed_without_a_connection() {
        let net = Arc::new(NetGuard::new(fm_netguard::Allowlist::new()));
        let bridge = BridgeAllowance::new(Arc::clone(&net));
        bridge.update(None);
        assert!(net.allowlist().is_empty());
    }
}
