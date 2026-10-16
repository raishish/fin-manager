//! End-to-end tests against throwaway HTTP servers on loopback. Nothing here
//! touches the real network: blocked requests never resolve or connect.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use fm_netguard::{
    Allowlist, BlockReason, Error, LogEntry, MAX_FIELD_BYTES, NetGuard, Options, Origin, Outcome,
    Request,
};

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

const NOT_ALLOWED: Outcome = Outcome::Blocked {
    reason: BlockReason::NotAllowed,
};

fn not_allowed(origin: Origin) -> Error {
    Error::Blocked {
        origin,
        reason: BlockReason::NotAllowed,
    }
}

fn is_rejected(entry: &LogEntry) -> bool {
    matches!(entry.outcome, Outcome::Rejected { .. })
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

    assert_eq!(err, not_allowed(unlisted.origin()));
    settle();
    assert!(
        unlisted.seen().is_empty(),
        "a blocked request reached the server"
    );
    let log = guard.log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].outcome, NOT_ALLOWED);
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
    assert!(guard.log().iter().all(|e| e.outcome == NOT_ALLOWED));
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
            NOT_ALLOWED,
            Outcome::Completed {
                status: 200,
                bytes: 1
            },
            NOT_ALLOWED
        ]
    );
}

#[test]
fn redirect_to_unlisted_host_is_blocked_and_logged() {
    let target = TestServer::ok("should not be reached");
    let redirector = TestServer::redirect_to(target.url("/landing"));
    let guard = guard_allowing(&[&redirector]);

    let err = guard.get(&redirector.url("/start"), "test").unwrap_err();

    assert_eq!(err, not_allowed(target.origin()));
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
    assert_eq!(log[1].outcome, NOT_ALLOWED);
    assert_eq!(log[1].path, "/landing");
    assert_eq!(log[1].redirect_of, Some(log[0].id));
}

#[test]
fn cross_origin_redirects_keep_only_safe_headers() {
    let target = TestServer::ok("landed");
    let redirector = TestServer::redirect_to(target.url("/landing"));
    let guard = guard_allowing(&[&redirector, &target]);

    let response = guard
        .send(
            Request::get(redirector.url("/start"), "test")
                .header("Authorization", "Basic abc")
                .header("X-Api-Key", "key")
                .header("Accept", "application/json")
                .header("Range", "bytes=0-9")
                .header("If-None-Match", "\"v1\""),
        )
        .unwrap();

    assert_eq!(response.body, b"landed");
    assert_eq!(response.final_url, target.url("/landing"));
    let first = &redirector.seen()[0];
    assert_eq!(first.header("authorization"), Some("Basic abc"));
    assert_eq!(first.header("x-api-key"), Some("key"));
    let second = &target.seen()[0];
    assert_eq!(second.header("authorization"), None);
    assert_eq!(second.header("x-api-key"), None);
    assert_eq!(second.header("accept"), Some("application/json"));
    assert_eq!(second.header("range"), Some("bytes=0-9"));
    assert_eq!(second.header("if-none-match"), Some("\"v1\""));
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

    let forbidden = [
        "Cookie",
        "Host",
        "Proxy-Authorization",
        "Proxy-Connection",
        "User-Agent",
        "Content-Length",
        "Transfer-Encoding",
        "Connection",
        "Upgrade",
        "TE",
        "Expect",
    ];
    for name in forbidden {
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
    let log = guard.log();
    assert_eq!(log.len(), forbidden.len() + 2);
    assert!(log.iter().all(is_rejected));
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

#[test]
fn same_origin_redirects_keep_caller_headers() {
    let server = TestServer::start(|seen| {
        if seen.request_line.contains("/start") {
            response(302, &[("Location", "/landing")], "")
        } else {
            response(200, &[], "landed")
        }
    });
    let guard = guard_allowing(&[&server]);

    guard
        .send(Request::get(server.url("/start"), "t").header("X-Api-Key", "key"))
        .unwrap();

    let seen = server.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].header("x-api-key"), Some("key"));
}

#[test]
fn secret_paths_can_be_redacted_in_the_log() {
    let target = TestServer::ok("claimed");
    let redirector = TestServer::redirect_to(target.url("/also/SECRET-TOKEN"));
    let guard = guard_allowing(&[&redirector, &target]);

    guard
        .send(
            Request::post(
                redirector.url("/claim/SECRET-TOKEN?x=1"),
                "simplefin-claim",
                vec![],
            )
            .log_path("/claim/<token>"),
        )
        .unwrap();
    guard
        .send(Request::get(redirector.url("/claim/SECRET-TOKEN"), "t").log_path("/claim/<token>"))
        .unwrap();
    let blocked = guard.send(
        Request::get("https://bridge.example/claim/SECRET-TOKEN", "t").log_path("/claim/<token>"),
    );

    assert!(matches!(blocked, Err(Error::Blocked { .. })));
    let paths: Vec<String> = guard.log().into_iter().map(|e| e.path).collect();
    assert_eq!(
        paths,
        [
            "/claim/<token>",
            "/claim/<token>",
            "[redacted]",
            "/claim/<token>"
        ]
    );
    assert!(!format!("{:?}", guard.log()).contains("SECRET"));
}

#[test]
fn errors_and_log_never_hold_credentials_or_queries() {
    let server = TestServer::ok("x");
    let guard = guard_allowing(&[&server]);
    let url = format!(
        "http://user:hunter2@127.0.0.1:{}/x?token=SECRET#frag",
        server.port
    );

    let err = guard.get(&url, "t").unwrap_err();
    let Error::InvalidUrl { url: shown, .. } = &err else {
        panic!("{err:?}")
    };
    assert_eq!(shown, &format!("http://127.0.0.1:{}/x", server.port));

    let redacted = guard
        .send(Request::get(&url, "t").log_path("/<hidden>"))
        .unwrap_err();

    for text in [
        err.to_string(),
        redacted.to_string(),
        format!("{:?}", guard.log()),
    ] {
        for secret in ["hunter2", "SECRET", "frag", "user:"] {
            assert!(!text.contains(secret), "{secret} leaked into {text}");
        }
    }
    assert!(guard.log()[0].query_omitted);
}

#[test]
fn rejected_requests_are_logged() {
    let file_redirect = TestServer::redirect_to("file:///etc/passwd".to_string());
    let guard = guard_allowing(&[&file_redirect]);

    let urls = [
        "http://user:pw@127.0.0.1:1/".to_string(),
        "ftp://127.0.0.1/".to_string(),
        "http://localhost./".to_string(),
        "http://3232235777/".to_string(),
        "http://0xc0.0xa8.1.1/".to_string(),
        "not a url".to_string(),
    ];
    for url in &urls {
        assert!(
            matches!(guard.get(url, "t"), Err(Error::InvalidUrl { .. })),
            "{url}"
        );
    }
    assert!(matches!(
        guard.get(&file_redirect.url("/start"), "t"),
        Err(Error::InvalidUrl { .. })
    ));

    let log = guard.log();
    assert_eq!(log.len(), urls.len() + 2);
    assert!(log[..urls.len()].iter().all(is_rejected), "{log:#?}");
    let (hop, file) = (&log[urls.len()], &log[urls.len() + 1]);
    assert_eq!(
        hop.outcome,
        Outcome::Completed {
            status: 302,
            bytes: 0
        }
    );
    assert!(is_rejected(file));
    assert_eq!(file.origin, "file://");
    assert_eq!(file.redirect_of, Some(hop.id));
}

#[test]
fn logged_paths_are_capped() {
    let long = format!("/{}", "a".repeat(5000));
    let server = TestServer::ok("x");
    let redirector = TestServer::redirect_to(format!("https://unlisted.example{long}"));
    let guard = guard_allowing(&[&server, &redirector]);

    guard.get(&server.url(&long), "t").unwrap();
    let _ = guard.get(&redirector.url("/"), "t");
    let _ = guard.get(
        &format!("https://unlisted.example{long}"),
        &"p".repeat(5000),
    );

    let log = guard.log();
    assert_eq!(log.len(), 4);
    for entry in &log {
        assert!(entry.path.len() <= MAX_FIELD_BYTES, "{}", entry.path.len());
        assert!(entry.purpose.len() <= MAX_FIELD_BYTES);
    }
    assert!(log[0].path.starts_with("/aaa"));
}

#[test]
fn non_redirect_3xx_responses_are_returned() {
    let target = TestServer::ok("x");
    let multiple = target.url("/");
    let server = TestServer::start(move |seen| {
        let status = if seen.request_line.contains("/300") {
            300
        } else {
            304
        };
        response(status, &[("Location", &multiple)], "")
    });
    let guard = guard_allowing(&[&server, &target]);

    assert_eq!(guard.get(&server.url("/300"), "t").unwrap().status, 300);
    assert_eq!(guard.get(&server.url("/304"), "t").unwrap().status, 304);
    settle();
    assert!(target.seen().is_empty());
}

#[test]
fn relative_query_redirects_keep_the_path() {
    let server = TestServer::start(|seen| {
        if seen.request_line.contains("step=2") {
            response(200, &[], "done")
        } else {
            response(302, &[("Location", "?step=2")], "")
        }
    });
    let guard = guard_allowing(&[&server]);

    let response = guard.get(&server.url("/dir/page?step=1"), "t").unwrap();

    assert_eq!(response.final_url, server.url("/dir/page?step=2"));
    assert_eq!(
        server.seen()[1].request_line,
        "GET /dir/page?step=2 HTTP/1.1"
    );
}

/// Hands each write to the test and signals after the first one.
struct SignalWriter {
    bytes: usize,
    first_write: Option<mpsc::Sender<()>>,
}

impl Write for SignalWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes += buf.len();
        if let Some(tx) = self.first_write.take() {
            tx.send(()).unwrap();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn revoke_stops_a_download_in_progress() {
    const HALF: usize = 1000;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (send_rest, rest_requested) = mpsc::channel::<()>();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() > 2 {
            line.clear();
        }
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            2 * HALF
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&[b'a'; HALF]).unwrap();
        stream.flush().unwrap();
        let _ = rest_requested.recv();
        let _ = stream.write_all(&[b'b'; HALF]);
    });
    let origin: Origin = format!("http://127.0.0.1:{port}").parse().unwrap();
    let mut list = Allowlist::new();
    list.allow(origin.clone());
    let guard = Arc::new(NetGuard::new(list));
    let (first_write, first_written) = mpsc::channel();

    let downloader = {
        let guard = Arc::clone(&guard);
        thread::spawn(move || {
            let mut out = SignalWriter {
                bytes: 0,
                first_write: Some(first_write),
            };
            let result = guard.download(
                Request::get(
                    format!("http://127.0.0.1:{port}/model.gguf"),
                    "model-download",
                ),
                &mut out,
            );
            (result, out.bytes)
        })
    };
    first_written.recv().unwrap();
    assert!(guard.revoke(&origin));
    send_rest.send(()).unwrap();
    let (result, bytes) = downloader.join().unwrap();

    assert_eq!(
        result.unwrap_err(),
        Error::Blocked {
            origin,
            reason: BlockReason::Revoked
        }
    );
    assert!(bytes < 2 * HALF, "kept writing after revoke: {bytes}");
    assert_eq!(
        guard.log()[0].outcome,
        Outcome::Blocked {
            reason: BlockReason::Revoked
        }
    );
}
