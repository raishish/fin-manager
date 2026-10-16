//! The network log as the UI sees it (SPEC decision 21: every request, when and where).

use fm_netguard::{LogEntry, NetGuard, Outcome};
use serde::Serialize;

/// One request attempt, shaped for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkLogEntry {
    pub id: u64,
    pub at_unix_ms: u64,
    pub method: String,
    pub origin: String,
    pub path: String,
    pub query_omitted: bool,
    pub purpose: String,
    pub redirect_of: Option<u64>,
    pub outcome: NetworkOutcome,
}

/// [`Outcome`] with every variant, tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NetworkOutcome {
    Completed { status: u16, bytes: u64 },
    Blocked { reason: String },
    Rejected { reason: String },
    Failed { error: String },
}

impl From<&LogEntry> for NetworkLogEntry {
    fn from(entry: &LogEntry) -> Self {
        Self {
            id: entry.id,
            at_unix_ms: entry.at_unix_ms,
            method: entry.method.clone(),
            origin: entry.origin.clone(),
            path: entry.path.clone(),
            query_omitted: entry.query_omitted,
            purpose: entry.purpose.clone(),
            redirect_of: entry.redirect_of,
            outcome: match &entry.outcome {
                Outcome::Completed { status, bytes } => NetworkOutcome::Completed {
                    status: *status,
                    bytes: *bytes,
                },
                Outcome::Blocked { reason } => NetworkOutcome::Blocked {
                    reason: reason.to_string(),
                },
                Outcome::Rejected { reason } => NetworkOutcome::Rejected {
                    reason: reason.clone(),
                },
                Outcome::Failed { error } => NetworkOutcome::Failed {
                    error: error.clone(),
                },
            },
        }
    }
}

/// Entries newer than `since_id` (all of them for `None`), oldest first. The UI polls with
/// the last id it has, so it sees every entry the guard still holds exactly once.
pub fn entries_since(guard: &NetGuard, since_id: Option<u64>) -> Vec<NetworkLogEntry> {
    guard
        .log()
        .iter()
        .filter(|e| since_id.is_none_or(|since| e.id > since))
        .map(NetworkLogEntry::from)
        .collect()
}

/// The origins the app may contact, as `scheme://host[:port]`.
pub fn allowed_origins(guard: &NetGuard) -> Vec<String> {
    guard.allowlist().iter().map(ToString::to_string).collect()
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    use fm_netguard::{Allowlist, Origin, Request};

    use super::*;

    /// A loopback server answering `count` requests with `200 OK` and a 2-byte body.
    fn serve(count: usize) -> (Origin, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            for stream in listener.incoming().take(count) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                    line.clear();
                }
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .unwrap();
            }
        });
        let origin = format!("http://127.0.0.1:{port}").parse().unwrap();
        (origin, handle)
    }

    #[test]
    fn every_attempt_is_visible_once() {
        let (origin, server) = serve(2);
        let mut allowlist = Allowlist::new();
        allowlist.allow(origin.clone());
        let guard = NetGuard::new(allowlist);

        // Allowed, blocked (with a query that must not show), rejected, allowed.
        guard.get(&format!("{origin}/one"), "test-allowed").unwrap();
        assert!(
            guard
                .get("https://tracker.example/pixel?user=jane", "test-blocked")
                .is_err()
        );
        assert!(
            guard
                .send(Request::get(format!("{origin}/x"), "test-rejected").header("Cookie", "a=b"))
                .is_err()
        );
        let all = entries_since(&guard, None);
        assert_eq!(all.len(), 3);

        guard.get(&format!("{origin}/two"), "test-allowed").unwrap();
        server.join().unwrap();

        let all = entries_since(&guard, None);
        assert_eq!(all.len(), 4, "{all:#?}");
        let ids: Vec<u64> = all.iter().map(|e| e.id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");

        assert_eq!(all[0].path, "/one");
        assert_eq!(all[0].purpose, "test-allowed");
        assert_eq!(
            all[0].outcome,
            NetworkOutcome::Completed {
                status: 200,
                bytes: 2
            }
        );

        assert_eq!(all[1].origin, "https://tracker.example");
        assert_eq!(all[1].path, "/pixel");
        assert!(all[1].query_omitted);
        assert!(matches!(all[1].outcome, NetworkOutcome::Blocked { .. }));
        let json = serde_json::to_string(&all).unwrap();
        assert!(
            !json.contains("jane"),
            "the query string must never be shown"
        );

        assert!(matches!(all[2].outcome, NetworkOutcome::Rejected { .. }));

        // Polling from the last id seen returns only what is new.
        let newer = entries_since(&guard, Some(all[2].id));
        assert_eq!(newer, all[3..]);
        assert!(entries_since(&guard, Some(all[3].id)).is_empty());
    }

    #[test]
    fn failures_are_shown() {
        // Bind and drop a listener, so the port is (almost certainly) closed.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let origin: Origin = format!("http://127.0.0.1:{port}").parse().unwrap();
        let mut allowlist = Allowlist::new();
        allowlist.allow(origin.clone());
        let guard = NetGuard::new(allowlist);
        assert!(guard.get(&format!("{origin}/"), "test-failed").is_err());
        let all = entries_since(&guard, None);
        assert!(
            matches!(all[0].outcome, NetworkOutcome::Failed { .. }),
            "{all:#?}"
        );
    }

    #[test]
    fn outcomes_serialise_with_a_kind_tag() {
        let json = serde_json::to_value(NetworkOutcome::Blocked {
            reason: "not on the allowlist".into(),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "blocked", "reason": "not on the allowlist"})
        );
        let json = serde_json::to_value(NetworkOutcome::Completed {
            status: 200,
            bytes: 5,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "completed", "status": 200, "bytes": 5})
        );
    }

    #[test]
    fn the_app_starts_with_nothing_allowed() {
        let guard = NetGuard::new(Allowlist::new());
        assert!(allowed_origins(&guard).is_empty());
    }
}
