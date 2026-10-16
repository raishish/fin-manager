//! The network log: every request attempt, allowed or blocked.
//!
//! Entries record when, where (origin and path) and why (the caller's
//! purpose label), never headers, bodies or query strings, which can hold
//! tokens and signatures.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

/// What happened to one request attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Not on the allowlist; nothing was sent.
    Blocked,
    /// A response arrived. `bytes` is the body size actually read.
    Completed { status: u16, bytes: u64 },
    /// Sent (or attempted) but failed: connection, TLS, timeout, size limit.
    Failed { error: String },
}

/// One request attempt. A redirect is a separate entry pointing back at the
/// entry that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// Increases by one per entry for the life of the guard.
    pub id: u64,
    /// Milliseconds since the Unix epoch, when the attempt started.
    pub at_unix_ms: u64,
    pub method: String,
    /// `scheme://host[:port]`.
    pub origin: String,
    /// Path only. `query_omitted` says whether a query string was dropped.
    pub path: String,
    pub query_omitted: bool,
    /// Caller-supplied label shown in the network log, e.g. `"model-download"`.
    pub purpose: String,
    /// The entry whose redirect led here.
    pub redirect_of: Option<u64>,
    pub outcome: Outcome,
}

/// In-memory ring buffer of the most recent entries.
#[derive(Debug)]
pub(crate) struct NetLog {
    entries: VecDeque<LogEntry>,
    capacity: usize,
    next_id: u64,
}

impl NetLog {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
            next_id: 1,
        }
    }

    pub(crate) fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub(crate) fn push(&mut self, entry: LogEntry) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    pub(crate) fn snapshot(&self) -> Vec<LogEntry> {
        self.entries.iter().cloned().collect()
    }
}

pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64) -> LogEntry {
        LogEntry {
            id,
            at_unix_ms: 0,
            method: "GET".into(),
            origin: "https://example.com".into(),
            path: "/".into(),
            query_omitted: false,
            purpose: "test".into(),
            redirect_of: None,
            outcome: Outcome::Blocked,
        }
    }

    #[test]
    fn ring_buffer_keeps_the_newest_entries() {
        let mut log = NetLog::new(2);
        for _ in 0..3 {
            let id = log.next_id();
            log.push(entry(id));
        }
        let ids: Vec<u64> = log.snapshot().iter().map(|e| e.id).collect();
        assert_eq!(ids, [2, 3]);
    }
}
