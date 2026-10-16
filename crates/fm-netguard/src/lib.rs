//! The only crate allowed to make network connections: allowlist and network log.
//!
//! Every request goes through a [`NetGuard`]. Before anything is sent, the
//! destination's origin (scheme, host, port) is checked against the
//! [`Allowlist`]; anything not on it is refused with [`Error::Blocked`]. Every
//! attempt, allowed or blocked, is recorded in the network log (SPEC decision
//! 21), which the app shows to the user.
//!
//! Hardening on top of the allowlist:
//! - Redirects are followed by the guard itself, one hop at a time, and every
//!   hop is checked and logged. Only `GET` and `HEAD` follow redirects, and
//!   `Authorization` is dropped when a redirect changes origin.
//! - Proxy settings from the environment are ignored, so traffic can't be
//!   routed somewhere the allowlist doesn't cover.
//! - No cookies are stored or sent, and the user agent carries no version or
//!   other identifier.
//! - Response bodies are size-limited.

mod allowlist;
mod error;
mod log;

use std::io::{self, Read, Write};
use std::sync::{Mutex, PoisonError, RwLock};
use std::time::Duration;

use ureq::config::AutoHeaderValue;
use ureq::http::{self, Uri};

pub use allowlist::{Allowlist, Origin, Scheme};
pub use error::{ConfigError, Error};
pub use log::{LogEntry, Outcome};

use log::{NetLog, now_unix_ms};

/// Crate name, used by the workspace smoke test.
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Sent on every request instead of a client name and version.
const USER_AGENT: &str = "fin-manager";

/// Headers callers may not set: state, routing and proxy credentials are the
/// guard's business.
const FORBIDDEN_HEADERS: &[&str] = &["cookie", "host", "proxy-authorization", "proxy-connection"];

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

    /// Sends a request and streams the body to `out`. On
    /// [`Error::BodyTooLarge`] `out` holds a truncated body and must be
    /// discarded.
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
        } = request;
        let limit = max_body_bytes.unwrap_or(self.options.max_body_bytes);
        let headers = check_headers(headers)?;
        let mut uri = parse_url(&url)?;
        let first_origin = origin_of(&uri)?;
        let mut redirect_of = None;

        for hop in 0..=self.options.max_redirects {
            let origin = origin_of(&uri)?;
            let started = now_unix_ms();
            let record = |outcome: Outcome| {
                self.record(
                    started,
                    method,
                    &origin,
                    &uri,
                    &purpose,
                    redirect_of,
                    outcome,
                )
            };

            if !self.is_allowed(&origin) {
                record(Outcome::Blocked);
                return Err(Error::Blocked { origin });
            }

            let mut builder = http::Request::builder()
                .method(method.as_http())
                .uri(uri.clone());
            for (name, value) in &headers {
                if origin != first_origin && name == "authorization" {
                    continue;
                }
                builder = builder.header(name, value);
            }
            let outgoing = builder
                .body(body.as_slice())
                .map_err(|e| Error::InvalidUrl {
                    url: uri.to_string(),
                    reason: e.to_string(),
                })?;

            let mut response = match self.agent.run(outgoing) {
                Ok(response) => response,
                Err(e) => {
                    record(Outcome::Failed {
                        error: e.to_string(),
                    });
                    return Err(Error::Transport(e.to_string()));
                }
            };
            let status = response.status().as_u16();

            let location = response
                .headers()
                .get(http::header::LOCATION)
                .and_then(|v| v.to_str().ok());
            if response.status().is_redirection()
                && method.follows_redirects()
                && let Some(location) = location
            {
                let next = resolve_location(&uri, location);
                let id = record(Outcome::Completed { status, bytes: 0 });
                if hop == self.options.max_redirects {
                    return Err(Error::TooManyRedirects);
                }
                uri = next?;
                redirect_of = Some(id);
                continue;
            }

            let response_headers = header_pairs(response.headers());
            let mut reader = response.body_mut().as_reader();
            return match copy_limited(&mut reader, out, limit) {
                Ok(bytes) => {
                    record(Outcome::Completed { status, bytes });
                    Ok(Received {
                        status,
                        headers: response_headers,
                        bytes,
                        final_url: uri.to_string(),
                    })
                }
                Err(e) => {
                    record(Outcome::Failed {
                        error: e.to_string(),
                    });
                    Err(e)
                }
            };
        }
        unreachable!("the last hop always returns")
    }

    fn is_allowed(&self, origin: &Origin) -> bool {
        self.allowlist
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(origin)
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &self,
        at_unix_ms: u64,
        method: Method,
        origin: &Origin,
        uri: &Uri,
        purpose: &str,
        redirect_of: Option<u64>,
        outcome: Outcome,
    ) -> u64 {
        let entry = {
            let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
            let entry = LogEntry {
                id: log.next_id(),
                at_unix_ms,
                method: method.as_http().to_string(),
                origin: origin.to_string(),
                path: uri.path().to_string(),
                query_omitted: uri.query().is_some(),
                purpose: purpose.to_string(),
                redirect_of,
                outcome,
            };
            log.push(entry.clone());
            entry
        };
        if let Some(sink) = &self.sink {
            sink(&entry);
        }
        entry.id
    }
}

fn parse_url(url: &str) -> Result<Uri, Error> {
    url.parse::<Uri>().map_err(|e| Error::InvalidUrl {
        url: url.to_string(),
        reason: e.to_string(),
    })
}

fn origin_of(uri: &Uri) -> Result<Origin, Error> {
    Origin::of_uri(uri).map_err(|reason| Error::InvalidUrl {
        url: uri.to_string(),
        reason,
    })
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

/// Resolves a `Location` header against the URL that returned it.
fn resolve_location(base: &Uri, location: &str) -> Result<Uri, Error> {
    let scheme = base.scheme_str().unwrap_or("https");
    let authority = base.authority().map_or("", |a| a.as_str());
    let absolute = if location.starts_with("//") {
        format!("{scheme}:{location}")
    } else if location.starts_with('/') {
        format!("{scheme}://{authority}{location}")
    } else if location
        .split(['/', '?', '#'])
        .next()
        .is_some_and(|s| s.contains(':'))
    {
        location.to_string()
    } else {
        let path = base.path();
        let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
        format!("{scheme}://{authority}{dir}{location}")
    };
    parse_url(&absolute)
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

/// Copies at most `limit` bytes; errors if the body is longer.
fn copy_limited(reader: &mut impl Read, out: &mut dyn Write, limit: u64) -> Result<u64, Error> {
    let transport = |e: io::Error| Error::Transport(e.to_string());
    let copied = io::copy(&mut reader.take(limit), out).map_err(transport)?;
    if copied == limit && reader.read(&mut [0u8; 1]).map_err(transport)? > 0 {
        return Err(Error::BodyTooLarge { limit });
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        ] {
            assert_eq!(
                resolve_location(&base, location).unwrap().to_string(),
                expected
            );
        }
    }

    #[test]
    fn copy_limited_enforces_the_limit() {
        let mut out = Vec::new();
        assert_eq!(copy_limited(&mut &b"12345"[..], &mut out, 5), Ok(5));
        assert_eq!(
            copy_limited(&mut &b"123456"[..], &mut Vec::new(), 5),
            Err(Error::BodyTooLarge { limit: 5 })
        );
    }

    #[test]
    fn guard_is_shareable_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NetGuard>();
    }
}
