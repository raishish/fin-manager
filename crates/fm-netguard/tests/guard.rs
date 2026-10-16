//! End-to-end tests against throwaway HTTP servers on loopback. Nothing here
//! touches the real network: blocked requests never resolve or connect.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use fm_netguard::{Allowlist, Error, LogEntry, NetGuard, Options, Origin, Outcome, Request};

/// One request as the test server saw it.
#[derive(Debug, Clone)]
struct Seen {
    request_line: String,
    /// Lowercased names.
    headers: Vec<(String, String)>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A loopback HTTP/1.1 server answering every request with `respond`.
struct TestServer {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl TestServer {
    fn start(respond: impl Fn(&Seen) -> String + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    let (name, value) = line.split_once(':').unwrap();
                    headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
                }
                let seen = Seen {
                    request_line: request_line.trim_end().to_string(),
                    headers,
                };
                if let Some(len) = seen.header("content-length") {
                    let mut body = vec![0; len.parse().unwrap()];
                    reader.read_exact(&mut body).unwrap();
                }
                let response = respond(&seen);
                log.lock().unwrap().push(seen);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { port, seen }
    }

    fn ok(body: &'static str) -> Self {
        Self::start(move |_| response(200, &[], body))
    }

    fn redirect_to(location: String) -> Self {
        Self::start(move |_| response(302, &[("Location", &location)], ""))
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn origin(&self) -> Origin {
        format!("http://127.0.0.1:{}", self.port).parse().unwrap()
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn response(status: u16, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(body);
    out
}

fn guard_allowing(servers: &[&TestServer]) -> NetGuard {
    let mut list = Allowlist::new();
    for server in servers {
        list.allow(server.origin());
    }
    NetGuard::new(list)
}

/// Gives a server time to accept a connection that shouldn't happen.
fn settle() {
    thread::sleep(Duration::from_millis(100));
}

#[test]
fn allowed_host_succeeds_and_is_logged() {
    let server = TestServer::ok("hello");
    let guard = guard_allowing(&[&server]);

    let response = guard
        .get(&server.url("/data?token=secret"), "test-fetch")
        .unwrap();

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"hello");
    assert_eq!(server.seen().len(), 1);
    let log = guard.log();
    assert_eq!(log.len(), 1);
    let entry = &log[0];
    assert_eq!(entry.method, "GET");
    assert_eq!(entry.origin, server.origin().to_string());
    assert_eq!(entry.path, "/data");
    assert!(entry.query_omitted);
    assert_eq!(entry.purpose, "test-fetch");
    assert_eq!(entry.redirect_of, None);
    assert_eq!(
        entry.outcome,
        Outcome::Completed {
            status: 200,
            bytes: 5
        }
    );
    assert!(entry.at_unix_ms > 0);
}

#[test]
fn unlisted_host_is_blocked_and_logged() {
    let listed = TestServer::ok("listed");
    let unlisted = TestServer::ok("unlisted");
    let guard = guard_allowing(&[&listed]);

    let err = guard.get(&unlisted.url("/x"), "test-fetch").unwrap_err();

    assert_eq!(
        err,
        Error::Blocked {
            origin: unlisted.origin()
        }
    );
    settle();
    assert!(
        unlisted.seen().is_empty(),
        "a blocked request reached the server"
    );
    let log = guard.log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].outcome, Outcome::Blocked);
    assert_eq!(log[0].origin, unlisted.origin().to_string());
}

#[test]
fn unlisted_public_hosts_are_blocked_before_dns() {
    let guard = NetGuard::new(Allowlist::new());
    for url in [
        "https://example.com/",
        "http://example.com/",
        "https://example.com:8443/",
        "https://93.184.215.14/",
    ] {
        assert!(
            matches!(guard.get(url, "test"), Err(Error::Blocked { .. })),
            "{url}"
        );
    }
    assert!(guard.log().iter().all(|e| e.outcome == Outcome::Blocked));
    assert_eq!(guard.log().len(), 4);
}

#[test]
fn same_host_on_another_port_or_scheme_is_blocked() {
    let server = TestServer::ok("x");
    let guard = guard_allowing(&[&server]);
    let other_port = format!("http://127.0.0.1:{}/", server.port.wrapping_add(1).max(1));
    let other_scheme = format!("https://127.0.0.1:{}/", server.port);
    let other_name = format!("http://localhost:{}/", server.port);
    for url in [other_port, other_scheme, other_name] {
        assert!(
            matches!(guard.get(&url, "test"), Err(Error::Blocked { .. })),
            "{url}"
        );
    }
    settle();
    assert!(server.seen().is_empty());
}

#[test]
fn allow_and_revoke_take_effect_immediately() {
    let server = TestServer::ok("x");
    let guard = NetGuard::new(Allowlist::new());
    let url = server.url("/");

    assert!(matches!(guard.get(&url, "t"), Err(Error::Blocked { .. })));
    assert!(guard.allow(server.origin()));
    assert_eq!(guard.get(&url, "t").unwrap().status, 200);
    assert!(guard.revoke(&server.origin()));
    assert!(matches!(guard.get(&url, "t"), Err(Error::Blocked { .. })));

    let outcomes: Vec<_> = guard.log().into_iter().map(|e| e.outcome).collect();
    assert_eq!(
        outcomes,
        [
            Outcome::Blocked,
            Outcome::Completed {
                status: 200,
                bytes: 1
            },
            Outcome::Blocked
        ]
    );
}

#[test]
fn redirect_to_unlisted_host_is_blocked_and_logged() {
    let target = TestServer::ok("should not be reached");
    let redirector = TestServer::redirect_to(target.url("/landing"));
    let guard = guard_allowing(&[&redirector]);

    let err = guard.get(&redirector.url("/start"), "test").unwrap_err();

    assert_eq!(
        err,
        Error::Blocked {
            origin: target.origin()
        }
    );
    settle();
    assert!(target.seen().is_empty());
    let log = guard.log();
    assert_eq!(log.len(), 2);
    assert_eq!(
        log[0].outcome,
        Outcome::Completed {
            status: 302,
            bytes: 0
        }
    );
    assert_eq!(log[1].outcome, Outcome::Blocked);
    assert_eq!(log[1].path, "/landing");
    assert_eq!(log[1].redirect_of, Some(log[0].id));
}

#[test]
fn redirect_to_allowed_host_is_followed_without_credentials() {
    let target = TestServer::ok("landed");
    let redirector = TestServer::redirect_to(target.url("/landing"));
    let guard = guard_allowing(&[&redirector, &target]);

    let response = guard
        .send(Request::get(redirector.url("/start"), "test").header("Authorization", "Basic abc"))
        .unwrap();

    assert_eq!(response.body, b"landed");
    assert_eq!(response.final_url, target.url("/landing"));
    assert_eq!(
        redirector.seen()[0].header("authorization"),
        Some("Basic abc")
    );
    assert_eq!(target.seen()[0].header("authorization"), None);
    let log = guard.log();
    assert_eq!(log.len(), 2);
    assert_eq!(log[1].redirect_of, Some(log[0].id));
}

#[test]
fn post_does_not_follow_redirects() {
    let target = TestServer::ok("x");
    let redirector = TestServer::redirect_to(target.url("/"));
    let guard = guard_allowing(&[&redirector, &target]);

    let response = guard
        .send(Request::post(
            redirector.url("/claim"),
            "test",
            b"{}".to_vec(),
        ))
        .unwrap();

    assert_eq!(response.status, 302);
    settle();
    assert!(target.seen().is_empty());
}

#[test]
fn redirect_loops_stop() {
    let looping = TestServer::start(|seen| {
        let path = seen.request_line.split(' ').nth(1).unwrap().to_string();
        response(302, &[("Location", &path)], "")
    });
    let mut list = Allowlist::new();
    list.allow(looping.origin());
    let guard = NetGuard::with_options(
        list,
        Options {
            max_redirects: 3,
            ..Options::default()
        },
    );

    assert_eq!(
        guard.get(&looping.url("/loop"), "t").unwrap_err(),
        Error::TooManyRedirects
    );
    assert_eq!(looping.seen().len(), 4);
    assert_eq!(guard.log().len(), 4);
}

#[test]
fn requests_carry_no_identifiers() {
    let server = TestServer::ok("x");
    let guard = guard_allowing(&[&server]);

    guard.get(&server.url("/"), "t").unwrap();

    let seen = &server.seen()[0];
    assert_eq!(seen.header("user-agent"), Some("fin-manager"));
    assert_eq!(seen.header("cookie"), None);
    assert_eq!(seen.header("accept-encoding"), None);
}

#[test]
fn forbidden_requests_are_rejected_before_anything_is_sent() {
    let server = TestServer::ok("x");
    let guard = guard_allowing(&[&server]);

    for name in ["Cookie", "Host", "Proxy-Authorization"] {
        let request = Request::get(server.url("/"), "t").header(name, "v");
        assert!(
            matches!(guard.send(request), Err(Error::ForbiddenHeader(_))),
            "{name}"
        );
    }
    let with_credentials = format!("http://user:pw@127.0.0.1:{}/", server.port);
    assert!(matches!(
        guard.get(&with_credentials, "t"),
        Err(Error::InvalidUrl { .. })
    ));
    assert!(matches!(
        guard.get("ftp://127.0.0.1/", "t"),
        Err(Error::InvalidUrl { .. })
    ));

    settle();
    assert!(server.seen().is_empty());
}

#[test]
fn oversized_bodies_fail_and_are_logged() {
    let server = TestServer::ok("0123456789");
    let guard = guard_allowing(&[&server]);

    let err = guard
        .send(Request::get(server.url("/"), "t").max_body_bytes(4))
        .unwrap_err();

    assert_eq!(err, Error::BodyTooLarge { limit: 4 });
    assert!(matches!(guard.log()[0].outcome, Outcome::Failed { .. }));
}

#[test]
fn downloads_stream_to_a_writer() {
    let server = TestServer::ok("model-bytes");
    let guard = guard_allowing(&[&server]);
    let mut file = Vec::new();

    let download = guard
        .download(
            Request::get(server.url("/model.gguf"), "model-download"),
            &mut file,
        )
        .unwrap();

    assert_eq!(file, b"model-bytes");
    assert_eq!(download.bytes, 11);
    assert_eq!(download.status, 200);
}

#[test]
fn connection_failures_are_logged() {
    // Bind then drop, so nothing listens on the port.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let origin: Origin = format!("http://127.0.0.1:{port}").parse().unwrap();
    let mut list = Allowlist::new();
    list.allow(origin);
    let guard = NetGuard::new(list);

    let err = guard
        .get(&format!("http://127.0.0.1:{port}/"), "t")
        .unwrap_err();

    assert!(matches!(err, Error::Transport(_)), "{err:?}");
    assert!(matches!(guard.log()[0].outcome, Outcome::Failed { .. }));
}

#[test]
fn log_sink_sees_every_entry() {
    let server = TestServer::ok("x");
    let mut guard = guard_allowing(&[&server]);
    let sunk: Arc<Mutex<Vec<LogEntry>>> = Arc::default();
    let sink = Arc::clone(&sunk);
    guard.set_log_sink(Box::new(move |entry| {
        sink.lock().unwrap().push(entry.clone())
    }));

    guard.get(&server.url("/"), "t").unwrap();
    let _ = guard.get("https://example.com/", "t");

    assert_eq!(*sunk.lock().unwrap(), guard.log());
    assert_eq!(guard.log().len(), 2);
}
