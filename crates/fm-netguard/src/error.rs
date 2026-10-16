use std::fmt;

use crate::allowlist::Origin;

/// A bad allowlist entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    InvalidOrigin {
        entry: String,
        reason: String,
    },
    InvalidHost {
        host: String,
        reason: String,
    },
    /// Plain `http` to a host that isn't loopback.
    PlainHttp(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOrigin { entry, reason } => {
                write!(f, "invalid allowlist entry {entry:?}: {reason}")
            }
            Self::InvalidHost { host, reason } => write!(f, "invalid host {host:?}: {reason}"),
            Self::PlainHttp(host) => {
                write!(
                    f,
                    "plain http is only allowed for loopback hosts, not {host:?}"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Why a request didn't produce a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The URL couldn't be parsed or isn't acceptable (for example it
    /// carries credentials). Nothing was sent and nothing was logged.
    InvalidUrl { url: String, reason: String },
    /// A header the guard never sends (for example `Cookie`).
    ForbiddenHeader(String),
    /// A header name or value that isn't valid HTTP.
    InvalidHeader(String),
    /// The destination isn't on the allowlist. Nothing was sent; the attempt
    /// is in the network log.
    Blocked { origin: Origin },
    /// More redirects than the guard follows.
    TooManyRedirects,
    /// The response body was larger than the caller's limit.
    BodyTooLarge { limit: u64 },
    /// Connection, TLS, protocol or I/O failure.
    Transport(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl { url, reason } => write!(f, "invalid URL {url:?}: {reason}"),
            Self::ForbiddenHeader(name) => write!(f, "header {name:?} is never sent"),
            Self::InvalidHeader(name) => write!(f, "invalid header {name:?}"),
            Self::Blocked { origin } => write!(f, "blocked: {origin} is not on the allowlist"),
            Self::TooManyRedirects => f.write_str("too many redirects"),
            Self::BodyTooLarge { limit } => write!(f, "response body exceeds {limit} bytes"),
            Self::Transport(why) => write!(f, "network error: {why}"),
        }
    }
}

impl std::error::Error for Error {}
