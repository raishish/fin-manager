//! The setup token and the access URL.
//!
//! A setup token is the base64 of a one-time claim URL. Claiming it returns the access
//! URL, `https://user:password@bridge/path`: a long-lived credential. It is stored only in
//! the encrypted ledger. Requests go to the URL without the credentials, which travel
//! in an `Authorization: Basic` header instead (fm-netguard refuses credentials in
//! URLs and never logs headers).
//!
//! Both URLs come from outside (a pasted token, the bridge's answer), so both must name
//! a public host over https: never this machine, the local network or a link-local
//! address ([`is_public_host`]). A crafted token can't point the app at a local
//! service. The tests' fake bridge (plain http to 127.0.0.1 or localhost) is accepted
//! only in this crate's own tests.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use zeroize::Zeroizing;

use super::SimpleFinError;

/// The path the network log shows for a claim. The claim URL's path carries the
/// one-time token, so none of it is shown.
pub(super) const CLAIM_LOG_PATH: &str = "/<setup token>";

/// The claim URL inside a setup token, and where it points.
pub(super) struct Claim {
    pub url: Zeroizing<String>,
    /// `scheme://host[:port]`.
    pub origin: String,
    /// `host[:port]`, lowercase, for the user to confirm before claiming.
    pub host: String,
}

impl fmt::Debug for Claim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Claim")
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

/// Reads a setup token. Refuses anything that isn't an https URL to a public host.
pub(super) fn claim_url(setup_token: &str) -> Result<Claim, SimpleFinError> {
    let invalid = || SimpleFinError::InvalidToken;
    let compact = Zeroizing::new(setup_token.split_whitespace().collect::<String>());
    let bytes = Zeroizing::new(STANDARD.decode(compact.as_bytes()).map_err(|_| invalid())?);
    let url = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid());
    }
    let parts = split_url(url).ok_or_else(invalid)?;
    if parts.userinfo.is_some() || !allowed_endpoint(parts.scheme, &parts.host) {
        return Err(invalid());
    }
    Ok(Claim {
        url: Zeroizing::new(url.to_string()),
        origin: format!("{}://{}", parts.scheme, parts.authority),
        host: parts.authority,
    })
}

/// True for a bridge run by SimpleFIN itself, for the confirmation screen.
pub(super) fn is_simplefin_host(host: &str) -> bool {
    let name = host.split(':').next().unwrap_or(host);
    name == "simplefin.org" || name.ends_with(".simplefin.org")
}

/// An access URL split into where to send requests and how to authenticate.
pub(super) struct Credentials {
    /// `scheme://host[:port]/path`, no credentials, no trailing slash.
    pub base: String,
    /// `scheme://host[:port]`, for the network allowlist.
    pub origin: String,
    user: Zeroizing<String>,
    password: Zeroizing<String>,
}

impl Credentials {
    /// Refuses anything that isn't an https URL to a public host with credentials.
    pub(super) fn parse(access_url: &str) -> Result<Self, SimpleFinError> {
        let bad = || SimpleFinError::BadAccessUrl;
        let parts = split_url(access_url.trim()).ok_or_else(bad)?;
        let (user, password) = parts
            .userinfo
            .and_then(|u| u.split_once(':'))
            .ok_or_else(bad)?;
        if user.is_empty()
            || parts.rest.contains(['?', '#'])
            || !allowed_endpoint(parts.scheme, &parts.host)
        {
            return Err(bad());
        }
        let origin = format!("{}://{}", parts.scheme, parts.authority);
        Ok(Self {
            base: format!("{origin}{}", parts.rest.trim_end_matches('/')),
            origin,
            user: percent_decode(user).ok_or_else(bad)?,
            password: percent_decode(password).ok_or_else(bad)?,
        })
    }

    /// The `Authorization` header value.
    pub(super) fn authorization(&self) -> Zeroizing<String> {
        let pair = Zeroizing::new(format!("{}:{}", *self.user, *self.password));
        Zeroizing::new(format!("Basic {}", STANDARD.encode(pair.as_bytes())))
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

/// A URL's parts, as far as the checks here need them.
struct UrlParts<'a> {
    scheme: &'a str,
    userinfo: Option<&'a str>,
    /// `host[:port]`, lowercase.
    authority: String,
    /// The host alone, lowercase; IPv6 keeps its brackets.
    host: String,
    /// Path, query and fragment.
    rest: &'a str,
}

fn split_url(url: &str) -> Option<UrlParts<'_>> {
    let (scheme, after) = url.split_once("://")?;
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (full_authority, rest) = after.split_at(end);
    let (userinfo, authority) = match full_authority.rsplit_once('@') {
        Some((u, a)) => (Some(u), a),
        None => (None, full_authority),
    };
    let authority = authority.to_ascii_lowercase();
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (inside, after) = v6.split_once(']')?;
        if !after.is_empty() && !after.starts_with(':') {
            return None;
        }
        (format!("[{inside}]"), after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (authority.clone(), None),
        }
    };
    if host.is_empty() || port.is_some_and(|p| p.parse::<u16>().is_err()) {
        return None;
    }
    Some(UrlParts {
        scheme,
        userinfo,
        authority,
        host,
        rest,
    })
}

/// https to a public host; or, in this crate's tests only, the fake bridge.
fn allowed_endpoint(scheme: &str, host: &str) -> bool {
    match scheme {
        "https" => is_public_host(host),
        "http" => cfg!(test) && (host == "127.0.0.1" || host == "localhost"),
        _ => false,
    }
}

/// A host on the public internet: a public IP address, or a DNS name that isn't
/// local by its form. Refuses this machine (`localhost`, loopback), private and
/// shared ranges (RFC 1918, RFC 6598, RFC 4193), link-local, and numeric forms a
/// resolver would read as an address (`2130706433`, `0x7f.1`, `127.1`).
///
/// A public name that resolves to a private address (DNS rebinding) isn't caught
/// here; the user confirms the bridge's host before the claim.
pub(super) fn is_public_host(host: &str) -> bool {
    if let Some(v6) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return v6.parse::<Ipv6Addr>().is_ok_and(public_v6);
    }
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        return public_v4(v4);
    }
    let name = host.strip_suffix('.').unwrap_or(host);
    let labels: Vec<&str> = name.split('.').collect();
    let well_formed = labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    let last = labels.last().copied().unwrap_or_default();
    // Top-level domains are letters (or punycode): a numeric last label is an address.
    let named_tld = last.bytes().all(|b| b.is_ascii_alphabetic()) || last.starts_with("xn--");
    let local = [
        "localhost",
        "local",
        "internal",
        "lan",
        "home.arpa",
        "intranet",
    ]
    .iter()
    .any(|suffix| name == *suffix || name.ends_with(&format!(".{suffix}")));
    well_formed && named_tld && !local
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b)) // shared address space (CGNAT)
        || (a == 198 && (18..20).contains(&b)) // benchmarking
        || (a == 192 && b == 0 && c == 0)) // IETF protocol assignments
}

fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let s = ip.segments();
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00 // unique local
        || (s[0] & 0xffc0) == 0xfe80 // link-local
        || (s[0] & 0xffc0) == 0xfec0 // site-local (deprecated)
        || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
        || (s[0] == 0x0064 && s[1] == 0xff9b) // NAT64: an IPv4 address inside
        || (s[..4].iter().all(|&x| x == 0) && s[4] == 0xffff && s[5] == 0) // SIIT ::ffff:0:0:0/96
        || s[0] == 0x2002 // 6to4: an IPv4 address inside
        || s[..6].iter().all(|&x| x == 0)) // IPv4-compatible (deprecated)
}

/// Decodes `%XX` escapes into a wiped string.
fn percent_decode(s: &str) -> Option<Zeroizing<String>> {
    let bytes = s.as_bytes();
    // Sized up front so it never reallocates and leaves no unwiped copy.
    let mut out = Zeroizing::new(Vec::with_capacity(bytes.len()));
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let text = std::str::from_utf8(&out).ok()?;
    Some(Zeroizing::new(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(s: &str) -> String {
        STANDARD.encode(s)
    }

    #[test]
    fn the_demo_token_decodes_to_its_claim_url() {
        let demo = "aHR0cHM6Ly9iZXRhLWJyaWRnZS5zaW1wbGVmaW4ub3JnL3NpbXBsZWZpbi9jbGFpbS9ERU1PLXYyLTUyMjI1MEZCMzBBNDNGRUNEQjBD";
        let claim = claim_url(demo).unwrap();
        assert_eq!(
            *claim.url,
            "https://beta-bridge.simplefin.org/simplefin/claim/DEMO-v2-522250FB30A43FECDB0C"
        );
        assert_eq!(claim.host, "beta-bridge.simplefin.org");
        assert_eq!(claim.origin, "https://beta-bridge.simplefin.org");
        assert!(is_simplefin_host(&claim.host));
        assert!(!format!("{claim:?}").contains("DEMO-v2"));
    }

    #[test]
    fn bad_tokens_are_refused() {
        for bad in [
            "not base64!".to_string(),
            encode("http://bridge.example.com/claim/x"),
            encode("ftp://bridge.example.com/claim/x"),
            encode("https://user:pw@bridge.example.com/claim/x"),
            encode("https://bridge.example.com/claim/x y"),
            encode("https://bridge.example.com:99999/claim/x"),
            encode("https://[2606:4700::1111]x/claim/x"),
        ] {
            assert!(
                matches!(claim_url(&bad), Err(SimpleFinError::InvalidToken)),
                "{bad}"
            );
        }
        assert!(
            claim_url(&encode("http://127.0.0.1:8080/claim/x")).is_ok(),
            "tests"
        );
    }

    /// Review finding M1: a crafted token must not point the app at this machine or the
    /// local network, over https either.
    #[test]
    fn tokens_for_local_or_private_hosts_are_refused() {
        for host in [
            "localhost",
            "LocalHost",
            "app.localhost",
            "127.0.0.1",
            "127.1",
            "2130706433",
            "0x7f000001",
            "0x7f.1",
            "0.0.0.0",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "[::1]",
            "[::]",
            "[fe80::1]",
            "[fd12:3456::1]",
            "[::ffff:192.168.1.1]",
            "[::ffff:7f00:1]",
            "[64:ff9b::a00:1]",
            "[::ffff:0:a00:1]",
            "[::ffff:0:5db8:d822]",
            "[2002:c000:204::1]",
            "[2002:5db8:d822::1]",
            "printer.local",
            "nas.lan",
            "router.home.arpa",
            "intranet",
            "bridge",
            "-bad-.example.com",
        ] {
            for port in ["", ":443", ":8443"] {
                let url = format!("https://{host}{port}/simplefin/claim/x");
                assert!(
                    matches!(claim_url(&encode(&url)), Err(SimpleFinError::InvalidToken)),
                    "{url}"
                );
                let access = format!("https://u:p@{host}{port}/simplefin");
                assert!(Credentials::parse(&access).is_err(), "{access}");
            }
        }
        for host in [
            "beta-bridge.simplefin.org",
            "bridge.example.com",
            "xn--bcher-kva.example",
            "93.184.216.34",
            "[2606:4700::1111]",
        ] {
            let url = format!("https://{host}/simplefin/claim/x");
            assert!(claim_url(&encode(&url)).is_ok(), "{url}");
            assert!(Credentials::parse(&format!("https://u:p@{host}/s")).is_ok());
        }
        assert!(!is_simplefin_host("simplefin.org.evil.example"));
        assert!(!is_simplefin_host("evilsimplefin.org"));
    }

    #[test]
    fn access_urls_split_into_base_and_basic_auth() {
        let c = Credentials::parse("https://AB12:s%3Acret@Bridge.Example.com/simplefin/").unwrap();
        assert_eq!(c.base, "https://bridge.example.com/simplefin");
        assert_eq!(c.origin, "https://bridge.example.com");
        // base64("AB12:s:cret")
        assert_eq!(*c.authorization(), "Basic QUIxMjpzOmNyZXQ=");
        let debug = format!("{c:?}");
        assert!(
            !debug.contains("AB12") && !debug.contains("cret"),
            "{debug}"
        );
        for bad in [
            "bridge.example.com/simplefin",
            "https://bridge.example.com/simplefin",
            "https://user@bridge.example.com/x",
            "https://:pw@bridge.example.com/x",
            "ftp://u:p@bridge.example.com/x",
            "http://u:p@bridge.example.com/x",
            "https://u:p@bridge.example.com/x?y=1",
        ] {
            assert!(Credentials::parse(bad).is_err(), "{bad}");
        }
    }
}
