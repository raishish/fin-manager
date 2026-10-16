//! The only crate allowed to make network connections: allowlist and network log.
//!
//! Every request goes through a [`NetGuard`]. Before anything is sent, the
//! destination's origin (scheme, host, port) is checked against the
//! [`Allowlist`]; anything not on it is refused with [`Error::Blocked`]. Every
//! attempt (allowed, blocked or rejected as malformed) is recorded in the
//! network log (SPEC decision 21), which the app shows to the user.
//!
//! Hardening on top of the allowlist:
//! - Redirects are followed by the guard itself, one hop at a time, and every
//!   hop is checked and logged. Only `GET` and `HEAD` follow redirects, only
//!   for 301/302/303/307/308. A redirect may not go from `https` to `http` or
//!   from a network host to this machine, and when it changes origin only a
//!   few harmless headers are carried over.
//! - Proxy settings from the environment are ignored, so traffic can't be
//!   routed somewhere the allowlist doesn't cover.
//! - No cookies are stored or sent, and the user agent carries no version or
//!   other identifier. Callers can't set headers that control framing,
//!   routing or identity.
//! - Response bodies are size-limited, and revoking an origin stops a body
//!   that is still being read from it.
//! - The log never holds credentials, query strings or (on request) paths.

mod allowlist;
mod error;
mod log;

use std::io::{self, Read, Write};
use std::sync::{Mutex, PoisonError, RwLock};
use std::time::Duration;

use ureq::config::AutoHeaderValue;
use ureq::http::{self, Uri};

pub use allowlist::{Allowlist, Origin, Scheme};
pub use error::{BlockReason, ConfigError, Error};
pub use log::{LogEntry, MAX_FIELD_BYTES, Outcome};

use log::{NetLog, now_unix_ms};

/// Crate name, used by the workspace smoke test.
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Sent on every request instead of a client name and version.
const USER_AGENT: &str = "fin-manager";

/// Headers callers may not set: cookies, routing, framing, connection
/// management, proxy credentials and the client's identity are the guard's
/// business.
const FORBIDDEN_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "cookie",
    "expect",
    "host",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "transfer-encoding",
    "upgrade",
    "user-agent",
];

/// The only caller headers carried over when a redirect changes origin.
/// Anything else (`Authorization`, API keys...) stays with the origin the
/// caller addressed.
const CROSS_ORIGIN_HEADERS: &[&str] = &[
    "accept",
    "accept-language",
    "if-modified-since",
    "if-none-match",
    "range",
];

/// Logged in place of the path of a redirect hop when the caller redacted
/// the request's path.
const REDACTED: &str = "[redacted]";

/// Receives each log entry as it is recorded, e.g. to persist the log.
pub type LogSink = Box<dyn Fn(&LogEntry) + Send + Sync>;

/// Limits and timeouts. The defaults suit API calls; raise the body limit per
/// request (see [`Request::max_body_bytes`]) for downloads.
#[derive(Debug, Clone)]
pub struct Options {
    pub timeout_resolve: Duration,
    pub timeout_connect: Duration,
    /// Time to wait for response headers after the request is sent.
    pub timeout_response: Duration,
    /// Redirect hops followed for `GET` and `HEAD`.
    pub max_redirects: u32,
    /// Entries kept in memory; older ones are dropped first.
    pub log_capacity: usize,
    /// Default response body limit.
    pub max_body_bytes: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout_resolve: Duration::from_secs(15),
            timeout_connect: Duration::from_secs(15),
            timeout_response: Duration::from_secs(60),
            max_redirects: 5,
            log_capacity: 10_000,
            max_body_bytes: 16 * 1024 * 1024,
        }
    }
}

/// HTTP methods the app needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
}

impl Method {
    fn as_http(self) -> http::Method {
        match self {
            Self::Get => http::Method::GET,
            Self::Head => http::Method::HEAD,
            Self::Post => http::Method::POST,
        }
    }

    fn follows_redirects(self) -> bool {
        matches!(self, Self::Get | Self::Head)
    }
}

/// An outgoing request.
#[derive(Debug, Clone)]
pub struct Request {
    method: Method,
    url: String,
    purpose: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    max_body_bytes: Option<u64>,
    log_path: Option<String>,
}

impl Request {
    /// `purpose` is a short label shown in the network log, e.g.
    /// `"model-download"` or `"simplefin-sync"`.
    pub fn new(method: Method, url: impl Into<String>, purpose: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            purpose: purpose.into(),
            headers: Vec::new(),
            body: Vec::new(),
            max_body_bytes: None,
            log_path: None,
        }
    }

    pub fn get(url: impl Into<String>, purpose: impl Into<String>) -> Self {
        Self::new(Method::Get, url, purpose)
    }

    pub fn post(url: impl Into<String>, purpose: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            body,
            ..Self::new(Method::Post, url, purpose)
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Overrides [`Options::max_body_bytes`] for this request.
    pub fn max_body_bytes(mut self, limit: u64) -> Self {
        self.max_body_bytes = Some(limit);
        self
    }

    /// For URLs whose path carries a secret (e.g. a SimpleFIN claim token):
    /// `shown` replaces the path in the network log and in errors, e.g.
    /// `"/claim/<token>"`. Redirect hops of this request are logged as
    /// `[redacted]`.
    pub fn log_path(mut self, shown: impl Into<String>) -> Self {
        self.log_path = Some(shown.into());
        self
    }
}

/// A response whose body was read into memory.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The URL that answered, after redirects.
    pub final_url: String,
}

/// Status and headers of a response whose body was streamed to a writer.
#[derive(Debug, Clone)]
pub struct Download {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub bytes: u64,
    pub final_url: String,
}

/// What `exchange` hands back besides the body it wrote.
struct Received {
    status: u16,
    headers: Vec<(String, String)>,
    bytes: u64,
    final_url: String,
}

/// The parts of a request that every log entry for it shares.
struct Attempt<'a> {
    method: Method,
    purpose: &'a str,
    log_path: Option<&'a str>,
}

impl Attempt<'_> {
    fn shown_path(&self, hop: u32, real: &str) -> String {
        match self.log_path {
            None => real.to_string(),
            Some(shown) if hop == 0 => shown.to_string(),
            Some(_) => REDACTED.to_string(),
        }
    }

    fn target(&self, uri: &Uri, origin: &Origin, hop: u32) -> Target {
        Target {
            origin: origin.to_string(),
            path: self.shown_path(hop, uri.path()),
            query_omitted: uri.query().is_some(),
        }
    }

    fn target_raw(&self, raw: &str, hop: u32) -> Target {
        let mut target = Target::of_raw(raw);
        target.path = self.shown_path(hop, &target.path);
        target
    }
}

/// Where an attempt was headed, safe to log: no credentials, query or
/// fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    origin: String,
    path: String,
    query_omitted: bool,
}

impl Target {
    /// Best effort for a URL that couldn't be parsed or accepted.
    fn of_raw(raw: &str) -> Self {
        let raw = raw.split('#').next().unwrap_or_default();
        let (raw, query_omitted) = match raw.split_once('?') {
            Some((before, _)) => (before, true),
            None => (raw, false),
        };
        let (origin, path) = match raw.split_once("://") {
            Some((scheme, rest)) if is_scheme(scheme) => {
                let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
                let host = authority
                    .rsplit_once('@')
                    .map_or(authority, |(_, host)| host);
                (format!("{scheme}://{host}"), path.to_string())
            }
            // `mailto:...`, `user:pw@host`: without a clean `scheme://` the
            // scheme can't be told apart from a user name, so nothing is kept.
            _ => (String::new(), String::new()),
        };
        Self {
            origin,
            path,
            query_omitted,
        }
    }

    fn url(&self) -> String {
        format!("{}{}", self.origin, self.path)
    }
}

fn is_scheme(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

/// Why reading a body stopped early.
enum Stop {
    Revoked,
    TooLarge,
    Io(io::Error),
}

/// The single HTTP client. Share one per process (it is `Send + Sync`).
pub struct NetGuard {
    agent: ureq::Agent,
    allowlist: RwLock<Allowlist>,
    log: Mutex<NetLog>,
    sink: Option<LogSink>,
    options: Options,
}

impl NetGuard {
    pub fn new(allowlist: Allowlist) -> Self {
        Self::with_options(allowlist, Options::default())
    }

    pub fn with_options(allowlist: Allowlist, options: Options) -> Self {
        let config = ureq::Agent::config_builder()
            // Never pick up HTTP(S)_PROXY / ALL_PROXY from the environment.
            .proxy(None)
            // Redirects are followed in `exchange` so each hop is checked.
            .max_redirects(0)
            .save_redirect_history(false)
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .accept_encoding(AutoHeaderValue::None)
            .timeout_resolve(Some(options.timeout_resolve))
            .timeout_connect(Some(options.timeout_connect))
            .timeout_recv_response(Some(options.timeout_response))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            allowlist: RwLock::new(allowlist),
            log: Mutex::new(NetLog::new(options.log_capacity)),
            sink: None,
            options,
        }
    }

    /// Calls `sink` with every entry as it is recorded.
    pub fn set_log_sink(&mut self, sink: LogSink) {
        self.sink = Some(sink);
    }

    /// A copy of the current allowlist.
    pub fn allowlist(&self) -> Allowlist {
        self.allowlist
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Adds an origin, e.g. the user's own SimpleFIN bridge.
    pub fn allow(&self, origin: Origin) -> bool {
        self.allowlist
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .allow(origin)
    }

    /// Removes an origin. Requests to it are blocked from now on, and a
    /// response body still being read from it stops at its next chunk with
    /// [`BlockReason::Revoked`].
    pub fn revoke(&self, origin: &Origin) -> bool {
        self.allowlist
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .revoke(origin)
    }

    /// The most recent log entries, oldest first.
    pub fn log(&self) -> Vec<LogEntry> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .snapshot()
    }

    /// Sends a request and reads the body into memory.
    pub fn send(&self, request: Request) -> Result<Response, Error> {
        let mut body = Vec::new();
        let received = self.exchange(request, &mut body)?;
        Ok(Response {
            status: received.status,
            headers: received.headers,
            body,
            final_url: received.final_url,
        })
    }

    /// `GET` with the body read into memory.
    pub fn get(&self, url: &str, purpose: &str) -> Result<Response, Error> {
        self.send(Request::get(url, purpose))
    }

    /// Sends a request and streams the body to `out`. On an error after the
    /// body started (too large, revoked, connection lost) `out` holds a
    /// partial body and must be discarded.
    pub fn download(&self, request: Request, out: &mut dyn Write) -> Result<Download, Error> {
        let received = self.exchange(request, out)?;
        Ok(Download {
            status: received.status,
            headers: received.headers,
            bytes: received.bytes,
            final_url: received.final_url,
        })
    }

    fn exchange(&self, request: Request, out: &mut dyn Write) -> Result<Received, Error> {
        let Request {
            method,
            url,
            purpose,
            headers,
            body,
            max_body_bytes,
            log_path,
        } = request;
        let limit = max_body_bytes.unwrap_or(self.options.max_body_bytes);
        let attempt = Attempt {
            method,
            purpose: &purpose,
            log_path: log_path.as_deref(),
        };
        let started = now_unix_ms();

        let headers = match check_headers(headers) {
            Ok(headers) => headers,
            Err(e) => {
                return Err(self.reject(&attempt, started, attempt.target_raw(&url, 0), None, e));
            }
        };
        let mut uri = match url.parse::<Uri>() {
            Ok(uri) => uri,
            Err(e) => {
                let target = attempt.target_raw(&url, 0);
                return Err(self.reject_url(&attempt, started, target, None, e.to_string()));
            }
        };
        let mut origin = match Origin::of_uri(&uri) {
            Ok(origin) => origin,
            Err(reason) => {
                let target = attempt.target_raw(&url, 0);
                return Err(self.reject_url(&attempt, started, target, None, reason));
            }
        };
        let first_origin = origin.clone();
        let mut redirect_of = None;
        let mut hop = 0;

        loop {
            let started = now_unix_ms();
            let target = attempt.target(&uri, &origin, hop);
            if !self.is_allowed(&origin) {
                let reason = BlockReason::NotAllowed;
                self.record(
                    &attempt,
                    started,
                    target,
                    redirect_of,
                    Outcome::Blocked { reason },
                );
                return Err(Error::Blocked { origin, reason });
            }

            let mut builder = http::Request::builder()
                .method(method.as_http())
                .uri(uri.clone());
            for (name, value) in &headers {
                if origin == first_origin || CROSS_ORIGIN_HEADERS.contains(&name.as_str()) {
                    builder = builder.header(name, value);
                }
            }
            let outgoing = match builder.body(body.as_slice()) {
                Ok(outgoing) => outgoing,
                Err(e) => {
                    return Err(self.reject_url(
                        &attempt,
                        started,
                        target,
                        redirect_of,
                        e.to_string(),
                    ));
                }
            };

            let mut response = match self.agent.run(outgoing) {
                Ok(response) => response,
                Err(e) => {
                    let error = scrub(&e.to_string(), &uri, &target);
                    self.record(
                        &attempt,
                        started,
                        target,
                        redirect_of,
                        Outcome::Failed {
                            error: error.clone(),
                        },
                    );
                    return Err(Error::Transport(error));
                }
            };
            let status = response.status().as_u16();

            let location = response
                .headers()
                .get(http::header::LOCATION)
                .and_then(|v| v.to_str().ok());
            if is_followed_redirect(status)
                && method.follows_redirects()
                && let Some(location) = location
            {
                let id = self.record(
                    &attempt,
                    started,
                    target,
                    redirect_of,
                    Outcome::Completed { status, bytes: 0 },
                );
                if hop == self.options.max_redirects {
                    return Err(Error::TooManyRedirects);
                }
                hop += 1;
                redirect_of = Some(id);
                let started = now_unix_ms();

                let next_uri = match resolve_location(&uri, location) {
                    Ok(next) => next,
                    Err(reason) => {
                        let target = attempt.target_raw(location, hop);
                        return Err(self.reject_url(
                            &attempt,
                            started,
                            target,
                            redirect_of,
                            reason,
                        ));
                    }
                };
                let next_origin = match Origin::of_uri(&next_uri) {
                    Ok(next) => next,
                    Err(reason) => {
                        let target = attempt.target_raw(&next_uri.to_string(), hop);
                        return Err(self.reject_url(
                            &attempt,
                            started,
                            target,
                            redirect_of,
                            reason,
                        ));
                    }
                };
                if let Some(reason) = redirect_block(&origin, &next_origin) {
                    let target = attempt.target(&next_uri, &next_origin, hop);
                    self.record(
                        &attempt,
                        started,
                        target,
                        redirect_of,
                        Outcome::Blocked { reason },
                    );
                    return Err(Error::Blocked {
                        origin: next_origin,
                        reason,
                    });
                }
                uri = next_uri;
                origin = next_origin;
                continue;
            }

            let response_headers = header_pairs(response.headers());
            let mut reader = response.body_mut().as_reader();
            let copied = copy_limited(&mut reader, out, limit, || self.is_allowed(&origin));
            let (outcome, result) = match copied {
                Ok(bytes) => (
                    Outcome::Completed { status, bytes },
                    Ok(Received {
                        status,
                        headers: response_headers,
                        bytes,
                        final_url: uri.to_string(),
                    }),
                ),
                Err(Stop::Revoked) => {
                    let reason = BlockReason::Revoked;
                    (
                        Outcome::Blocked { reason },
                        Err(Error::Blocked { origin, reason }),
                    )
                }
                Err(Stop::TooLarge) => {
                    let e = Error::BodyTooLarge { limit };
                    (
                        Outcome::Failed {
                            error: e.to_string(),
                        },
                        Err(e),
                    )
                }
                Err(Stop::Io(e)) => {
                    let error = scrub(&e.to_string(), &uri, &target);
                    (
                        Outcome::Failed {
                            error: error.clone(),
                        },
                        Err(Error::Transport(error)),
                    )
                }
            };
            self.record(&attempt, started, target, redirect_of, outcome);
            return result;
        }
    }

    fn is_allowed(&self, origin: &Origin) -> bool {
        self.allowlist
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(origin)
    }

    /// Logs a request refused before sending and returns its error.
    fn reject(
        &self,
        attempt: &Attempt<'_>,
        started: u64,
        target: Target,
        redirect_of: Option<u64>,
        error: Error,
    ) -> Error {
        let reason = error.to_string();
        self.record(
            attempt,
            started,
            target,
            redirect_of,
            Outcome::Rejected { reason },
        );
        error
    }

    fn reject_url(
        &self,
        attempt: &Attempt<'_>,
        started: u64,
        target: Target,
        redirect_of: Option<u64>,
        reason: String,
    ) -> Error {
        let error = Error::InvalidUrl {
            url: target.url(),
            reason,
        };
        self.reject(attempt, started, target, redirect_of, error)
    }

    fn record(
        &self,
        attempt: &Attempt<'_>,
        at_unix_ms: u64,
        target: Target,
        redirect_of: Option<u64>,
        outcome: Outcome,
    ) -> u64 {
        let entry = {
            let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
            let entry = LogEntry {
                id: log.next_id(),
                at_unix_ms,
                method: attempt.method.as_http().to_string(),
                origin: target.origin,
                path: target.path,
                query_omitted: target.query_omitted,
                purpose: attempt.purpose.to_string(),
                redirect_of,
                outcome,
            };
            log.push(entry)
        };
        if let Some(sink) = &self.sink {
            sink(&entry);
        }
        entry.id
    }
}

fn check_headers(headers: Vec<(String, String)>) -> Result<Vec<(String, String)>, Error> {
    headers
        .into_iter()
        .map(|(name, value)| {
            let name = name.to_ascii_lowercase();
            if FORBIDDEN_HEADERS.contains(&name.as_str()) {
                return Err(Error::ForbiddenHeader(name));
            }
            if http::HeaderName::from_bytes(name.as_bytes()).is_err()
                || http::HeaderValue::from_str(&value).is_err()
            {
                return Err(Error::InvalidHeader(name));
            }
            Ok((name, value))
        })
        .collect()
}

fn is_followed_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Redirects the guard refuses even between allowlisted origins.
fn redirect_block(from: &Origin, to: &Origin) -> Option<BlockReason> {
    if from.scheme() == Scheme::Https && to.scheme() == Scheme::Http {
        Some(BlockReason::HttpsToHttp)
    } else if !from.is_loopback() && to.is_loopback() {
        Some(BlockReason::NetworkToLoopback)
    } else {
        None
    }
}

/// Resolves a `Location` header against the URL that returned it.
fn resolve_location(base: &Uri, location: &str) -> Result<Uri, String> {
    let scheme = base.scheme_str().unwrap_or("https");
    let authority = base.authority().map_or("", |a| a.as_str());
    let path = base.path();
    let absolute = if location.starts_with("//") {
        format!("{scheme}:{location}")
    } else if location.starts_with('/') {
        format!("{scheme}://{authority}{location}")
    } else if location.starts_with('?') {
        format!("{scheme}://{authority}{path}{location}")
    } else if location
        .split(['/', '?', '#'])
        .next()
        .is_some_and(|s| s.contains(':'))
    {
        location.to_string()
    } else {
        let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
        format!("{scheme}://{authority}{dir}{location}")
    };
    absolute.parse::<Uri>().map_err(|e| e.to_string())
}

/// Removes the query and any redacted path from an error message.
fn scrub(message: &str, uri: &Uri, target: &Target) -> String {
    let mut message = message.replace(&uri.to_string(), &target.url());
    if let Some(query) = uri.query() {
        message = message.replace(query, "");
    }
    if target.path != uri.path() && uri.path().len() > 1 {
        message = message.replace(uri.path(), &target.path);
    }
    message
}

fn header_pairs(headers: &http::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
            (name.as_str().to_string(), value)
        })
        .collect()
}

/// Copies the body to `out`, at most `limit` bytes, checking before each
/// chunk that the origin is still allowed.
fn copy_limited(
    reader: &mut impl Read,
    out: &mut dyn Write,
    limit: u64,
    still_allowed: impl Fn() -> bool,
) -> Result<u64, Stop> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Stop::Io(e)),
        };
        if !still_allowed() {
            return Err(Stop::Revoked);
        }
        let n64 = n as u64;
        if total + n64 > limit {
            return Err(Stop::TooLarge);
        }
        out.write_all(&buf[..n]).map_err(Stop::Io)?;
        total += n64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(s: &str) -> Origin {
        s.parse().unwrap()
    }

    #[test]
    fn crate_name_is_set() {
        assert_eq!(super::NAME, "fm-netguard");
    }

    #[test]
    fn agent_ignores_environment_proxies_and_redirects() {
        let guard = NetGuard::new(Allowlist::new());
        let config = guard.agent.config();
        assert!(config.proxy().is_none());
        assert_eq!(config.max_redirects(), 0);
        assert!(!config.http_status_as_error());
    }

    #[test]
    fn locations_resolve_against_the_base() {
        let base: Uri = "https://a.example/dir/file?x=1".parse().unwrap();
        for (location, expected) in [
            ("https://b.example/x", "https://b.example/x"),
            ("//b.example/x", "https://b.example/x"),
            ("/root", "https://a.example/root"),
            ("sibling?y=2", "https://a.example/dir/sibling?y=2"),
            ("?y=2", "https://a.example/dir/file?y=2"),
        ] {
            assert_eq!(
                resolve_location(&base, location).unwrap().to_string(),
                expected
            );
        }
    }

    #[test]
    fn only_real_redirect_statuses_are_followed() {
        for status in [301, 302, 303, 307, 308] {
            assert!(is_followed_redirect(status), "{status}");
        }
        for status in [200, 300, 304, 305, 306, 309, 399] {
            assert!(!is_followed_redirect(status), "{status}");
        }
    }

    #[test]
    fn unsafe_redirects_are_refused() {
        let cases = [
            (
                "https://a.example",
                "http://127.0.0.1:8080",
                Some(BlockReason::HttpsToHttp),
            ),
            (
                "https://127.0.0.1",
                "http://127.0.0.1",
                Some(BlockReason::HttpsToHttp),
            ),
            (
                "https://a.example",
                "https://localhost",
                Some(BlockReason::NetworkToLoopback),
            ),
            (
                "https://a.example",
                "https://[::1]",
                Some(BlockReason::NetworkToLoopback),
            ),
            ("https://a.example", "https://b.example", None),
            ("http://127.0.0.1:1", "http://localhost:2", None),
            ("http://127.0.0.1:1", "https://a.example", None),
        ];
        for (from, to, expected) in cases {
            assert_eq!(
                redirect_block(&origin(from), &origin(to)),
                expected,
                "{from} -> {to}"
            );
        }
    }

    #[test]
    fn raw_targets_drop_credentials_and_queries() {
        let t = Target::of_raw("https://user:pw@bank.example/claim/abc?token=secret#frag");
        assert_eq!(t.url(), "https://bank.example/claim/abc");
        assert!(t.query_omitted);
        assert_eq!(
            Target::of_raw("file:///etc/passwd").url(),
            "file:///etc/passwd"
        );
        assert_eq!(Target::of_raw("mailto:user:pw@x").url(), "");
        assert_eq!(Target::of_raw("user:pw@host/x").url(), "");
        assert_eq!(Target::of_raw("user:pw@host://x/y").url(), "");
    }

    #[test]
    fn copy_limited_enforces_the_limit() {
        let mut out = Vec::new();
        assert!(matches!(
            copy_limited(&mut &b"12345"[..], &mut out, 5, || true),
            Ok(5)
        ));
        assert!(matches!(
            copy_limited(&mut &b"123456"[..], &mut Vec::new(), 5, || true),
            Err(Stop::TooLarge)
        ));
        assert!(matches!(
            copy_limited(&mut &b"1"[..], &mut Vec::new(), 5, || false),
            Err(Stop::Revoked)
        ));
    }

    #[test]
    fn guard_is_shareable_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NetGuard>();
    }
}
