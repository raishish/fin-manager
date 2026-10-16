//! Allowlist of origins (scheme, host, port) the app may connect to.
//!
//! Matching is exact: no wildcards, no subdomains, no paths. Plain `http` is
//! accepted only for loopback hosts, so a misconfigured entry can't send data
//! unencrypted over the network.

use std::collections::BTreeSet;
use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::str::FromStr;

use crate::error::ConfigError;

/// URL scheme of an origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scheme {
    Https,
    /// Loopback only (local test servers, a local SimpleFIN bridge).
    Http,
}

impl Scheme {
    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "https" => Some(Self::Https),
            "http" => Some(Self::Http),
            _ => None,
        }
    }

    fn default_port(self) -> u16 {
        match self {
            Self::Https => 443,
            Self::Http => 80,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Https => "https",
            Self::Http => "http",
        }
    }
}

/// One allowed destination. Two URLs reach the same origin only if scheme,
/// host and port are all equal (default ports are filled in).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Origin {
    scheme: Scheme,
    /// Lowercase ASCII; IPv6 literals keep their brackets.
    host: String,
    port: u16,
}

impl Origin {
    /// Builds an origin, applying the same checks as parsing.
    pub fn new(scheme: Scheme, host: &str, port: Option<u16>) -> Result<Self, ConfigError> {
        let host = normalize_host(host)?;
        if scheme == Scheme::Http && !is_loopback(&host) {
            return Err(ConfigError::PlainHttp(host));
        }
        Ok(Self {
            scheme,
            port: port.unwrap_or(scheme.default_port()),
            host,
        })
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Origin of a parsed request URL. Unlike [`Origin::new`], plain `http`
    /// to a non-loopback host is allowed here: such an origin can never be on
    /// the allowlist, so the request is blocked and logged rather than
    /// rejected as malformed.
    pub(crate) fn of_uri(uri: &ureq::http::Uri) -> Result<Self, String> {
        let scheme = uri
            .scheme_str()
            .and_then(Scheme::parse)
            .ok_or("scheme must be http or https")?;
        let authority = uri.authority().ok_or("URL has no host")?;
        if authority.as_str().contains('@') {
            return Err("credentials in the URL are not allowed; send them in a header".into());
        }
        let host = normalize_host(authority.host()).map_err(|e| e.to_string())?;
        Ok(Self {
            scheme,
            port: authority.port_u16().unwrap_or(scheme.default_port()),
            host,
        })
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme.as_str(), self.host)?;
        if self.port != self.scheme.default_port() {
            write!(f, ":{}", self.port)?;
        }
        Ok(())
    }
}

impl FromStr for Origin {
    type Err = ConfigError;

    /// Parses `scheme://host[:port]`. A trailing `/` is tolerated; any other
    /// path, query, fragment or credentials are rejected so an entry can't
    /// look narrower than it is.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = |why: &str| ConfigError::InvalidOrigin {
            entry: s.to_string(),
            reason: why.to_string(),
        };
        let (scheme, rest) = s
            .split_once("://")
            .ok_or_else(|| invalid("missing scheme"))?;
        let scheme =
            Scheme::parse(scheme).ok_or_else(|| invalid("scheme must be http or https"))?;
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        if authority.contains(['/', '?', '#']) {
            return Err(invalid("an origin has no path, query or fragment"));
        }
        if authority.contains('@') {
            return Err(invalid("an origin has no credentials"));
        }
        let (host, port) = split_host_port(authority).map_err(invalid)?;
        Origin::new(scheme, host, port)
    }
}

/// The set of origins the guard lets requests through to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Allowlist {
    origins: BTreeSet<Origin>,
}

impl Allowlist {
    /// An empty allowlist: every request is blocked.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses the config format: one origin per line, `#` starts a comment,
    /// blank lines are ignored.
    ///
    /// ```text
    /// # Model downloads
    /// https://huggingface.co
    /// ```
    pub fn parse(config: &str) -> Result<Self, ConfigError> {
        let mut list = Self::new();
        for line in config.lines() {
            let entry = line.split('#').next().unwrap_or("").trim();
            if !entry.is_empty() {
                list.allow(entry.parse()?);
            }
        }
        Ok(list)
    }

    /// Adds an origin. Returns `false` if it was already present.
    pub fn allow(&mut self, origin: Origin) -> bool {
        self.origins.insert(origin)
    }

    /// Removes an origin. Returns `false` if it wasn't present.
    pub fn revoke(&mut self, origin: &Origin) -> bool {
        self.origins.remove(origin)
    }

    pub fn contains(&self, origin: &Origin) -> bool {
        self.origins.contains(origin)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Origin> {
        self.origins.iter()
    }

    pub fn len(&self) -> usize {
        self.origins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.origins.is_empty()
    }
}

impl fmt::Display for Allowlist {
    /// Writes the config format accepted by [`Allowlist::parse`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for origin in &self.origins {
            writeln!(f, "{origin}")?;
        }
        Ok(())
    }
}

fn split_host_port(authority: &str) -> Result<(&str, Option<u16>), &'static str> {
    let (host, port) = if authority.starts_with('[') {
        let end = authority.find(']').ok_or("unclosed IPv6 bracket")?;
        let (host, rest) = authority.split_at(end + 1);
        match rest {
            "" => (host, None),
            _ => (
                host,
                Some(rest.strip_prefix(':').ok_or("junk after IPv6 address")?),
            ),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = match port {
        Some(p) => Some(
            p.parse::<u16>()
                .ok()
                .filter(|&p| p != 0)
                .ok_or("invalid port")?,
        ),
        None => None,
    };
    Ok((host, port))
}

fn normalize_host(host: &str) -> Result<String, ConfigError> {
    let invalid = |why: &str| ConfigError::InvalidHost {
        host: host.to_string(),
        reason: why.to_string(),
    };
    if host.is_empty() {
        return Err(invalid("empty host"));
    }
    if let Some(inner) = host.strip_prefix('[') {
        let inner = inner
            .strip_suffix(']')
            .ok_or_else(|| invalid("unclosed IPv6 bracket"))?;
        let ip: Ipv6Addr = inner.parse().map_err(|_| invalid("invalid IPv6 address"))?;
        return Ok(format!("[{ip}]"));
    }
    // Non-ASCII (IDN) hosts must be given in punycode, so what is listed is
    // exactly what is resolved.
    let valid = host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    if !valid || host.starts_with('.') || host.ends_with('.') || host.contains("..") {
        return Err(invalid(
            "hosts are ASCII letters, digits, '-' and '.' (use punycode)",
        ));
    }
    Ok(host.to_ascii_lowercase())
}

fn is_loopback(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(s: &str) -> Origin {
        s.parse().unwrap()
    }

    #[test]
    fn default_ports_and_case_are_normalized() {
        assert_eq!(
            origin("https://Example.COM"),
            origin("https://example.com:443/")
        );
        assert_ne!(
            origin("https://example.com"),
            origin("https://example.com:8443")
        );
        assert_eq!(
            origin("https://example.com:8443").to_string(),
            "https://example.com:8443"
        );
        assert_eq!(origin("http://[::1]:8080").host(), "[::1]");
    }

    #[test]
    fn plain_http_only_for_loopback() {
        assert!(matches!(
            "http://example.com".parse::<Origin>(),
            Err(ConfigError::PlainHttp(_))
        ));
        for ok in [
            "http://127.0.0.1:9000",
            "http://localhost",
            "http://[::1]:1",
        ] {
            assert_eq!(origin(ok).scheme(), Scheme::Http, "{ok}");
        }
    }

    #[test]
    fn entries_cannot_carry_more_than_an_origin() {
        for bad in [
            "example.com",
            "ftp://example.com",
            "https://example.com/path",
            "https://example.com?q",
            "https://user:pw@example.com",
            "https://*.example.com",
            "https://exämple.com",
            "https://example.com:0",
            "https://example.com:99999",
            "https://",
            "https://.example.com",
        ] {
            assert!(bad.parse::<Origin>().is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn config_round_trips() {
        let list = Allowlist::parse(
            "# models\nhttps://huggingface.co\n\n  https://bridge.simplefin.org  # bridge\n",
        )
        .unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&origin("https://huggingface.co")));
        assert_eq!(Allowlist::parse(&list.to_string()).unwrap(), list);
    }

    #[test]
    fn config_errors_name_the_entry() {
        let err = Allowlist::parse("https://ok.example\nhttps://bad.example/x\n").unwrap_err();
        assert!(err.to_string().contains("https://bad.example/x"), "{err}");
    }
}
