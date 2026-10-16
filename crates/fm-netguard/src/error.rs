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

/// Why the guard refused to contact an origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// The origin isn't on the allowlist.
    NotAllowed,
    /// The origin was revoked while a response body was being read.
    Revoked,
    /// A redirect from `https` to plain `http`.
    HttpsToHttp,
    /// A redirect from a network host to this machine.
    NetworkToLoopback,
}

impl fmt::Display for BlockReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotAllowed => "not on the allowlist",
            Self::Revoked => "removed from the allowlist during the transfer",
            Self::HttpsToHttp => "redirect from https to http",
            Self::NetworkToLoopback => "redirect from a network host to this machine",
        })
    }
}

/// Why a request didn't produce a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The URL couldn't be parsed or isn't acceptable (for example it
    /// carries credentials). Nothing was sent; the attempt is in the network
    /// log. `url` has credentials, query and fragment removed.
    InvalidUrl { url: String, reason: String },
    /// A header the guard never lets callers set (for example `Cookie`).
    /// Nothing was sent; the attempt is in the network log.
    ForbiddenHeader(String),
    /// A header name or value that isn't valid HTTP. Nothing was sent; the
    /// attempt is in the network log.
    InvalidHeader(String),
    /// The guard refused to contact `origin`. Nothing was sent to it (or, for
    /// [`BlockReason::Revoked`], reading stopped); the attempt is in the
    /// network log.
    Blocked { origin: Origin, reason: BlockReason },
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
            Self::ForbiddenHeader(name) => write!(f, "header {name:?} can't be set by callers"),
            Self::InvalidHeader(name) => write!(f, "invalid header {name:?}"),
            Self::Blocked { origin, reason } => write!(f, "blocked {origin}: {reason}"),
            Self::TooManyRedirects => f.write_str("too many redirects"),
            Self::BodyTooLarge { limit } => write!(f, "response body exceeds {limit} bytes"),
            Self::Transport(why) => write!(f, "network error: {why}"),
        }
    }
}

impl std::error::Error for Error {}
