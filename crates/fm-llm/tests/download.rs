//! Model downloads against throwaway HTTP servers on loopback, through a real
//! fm-netguard. Nothing here touches the real network.

use std::borrow::Cow;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use fm_llm::{DOWNLOAD_PURPOSE, DownloadError, ModelSpec, download, ensure_model, sha256_file};
use fm_netguard::{Allowlist, NetGuard, Origin, Outcome};
use sha2::{Digest, Sha256};

const BODY: &[u8] = b"GGUF not really a model, but bytes all the same";

/// A loopback server answering every request with `respond(path)`, counting
/// requests.
struct TestServer {
    port: u16,
    requests: Arc<AtomicUsize>,
}

impl TestServer {
    fn start(respond: impl Fn(&str) -> Vec<u8> + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim_end().is_empty() {
                        break;
                    }
                }
                count.fetch_add(1, Ordering::SeqCst);
                let path = request_line.split(' ').nth(1).unwrap_or("/").to_string();
                let _ = stream.write_all(&respond(&path));
            }
        });
        Self { port, requests }
    }

    fn serving(status: u16, body: &'static [u8]) -> Self {
        Self::start(move |_| response(status, &[], body))
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn origin(&self) -> Origin {
        format!("http://127.0.0.1:{}", self.port).parse().unwrap()
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

fn guard_for(servers: &[&TestServer]) -> NetGuard {
    let mut allowlist = Allowlist::new();
    for server in servers {
        allowlist.allow(server.origin());
    }
    NetGuard::new(allowlist)
}

fn spec_for(url: String, body: &[u8]) -> ModelSpec {
    ModelSpec {
        id: Cow::Borrowed("test-model"),
        url: Cow::Owned(url),
        file_name: Cow::Borrowed("test.gguf"),
        size: body.len() as u64,
        sha256: Sha256::digest(body).into(),
    }
}

/// A fresh empty directory under the system temp dir.
fn temp_dir() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "fm-llm-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Files in `dir`, sorted.
fn listing(dir: &PathBuf) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_matching_download_lands_verified_and_is_logged() {
    let server = TestServer::serving(200, BODY);
    let guard = guard_for(&[&server]);
    let dir = temp_dir();
    let spec = spec_for(server.url("/model.gguf"), BODY);

    let path = download(&guard, &spec, &dir).unwrap();
    assert_eq!(path, dir.join("test.gguf"));
    assert_eq!(fs::read(&path).unwrap(), BODY);
    assert_eq!(sha256_file(&path).unwrap(), spec.sha256);
    assert_eq!(listing(&dir), ["test.gguf"]);

    let log = guard.log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].purpose, DOWNLOAD_PURPOSE);
    assert!(matches!(
        log[0].outcome,
        Outcome::Completed { status: 200, .. }
    ));
}

#[test]
fn a_wrong_hash_leaves_nothing_behind() {
    let server = TestServer::serving(200, BODY);
    let guard = guard_for(&[&server]);
    let dir = temp_dir();
    let mut spec = spec_for(server.url("/model.gguf"), BODY);
    spec.sha256[0] ^= 1;

    let err = download(&guard, &spec, &dir).unwrap_err();
    assert!(matches!(err, DownloadError::WrongHash { .. }), "{err}");
    assert!(listing(&dir).is_empty());
}

#[test]
fn a_short_or_long_body_is_refused() {
    let short = TestServer::serving(200, &BODY[..10]);
    let guard = guard_for(&[&short]);
    let dir = temp_dir();
    let err = download(&guard, &spec_for(short.url("/m"), BODY), &dir).unwrap_err();
    assert!(
        matches!(err, DownloadError::WrongSize { expected, actual: 10 } if expected == BODY.len() as u64),
        "{err}"
    );
    assert!(listing(&dir).is_empty());

    // More bytes than the spec's size: the guard stops reading at the limit.
    let long = TestServer::serving(200, BODY);
    let guard = guard_for(&[&long]);
    let err = download(&guard, &spec_for(long.url("/m"), &BODY[..10]), &dir).unwrap_err();
    assert!(
        matches!(
            err,
            DownloadError::Network(fm_netguard::Error::BodyTooLarge { limit: 10 })
        ),
        "{err}"
    );
    assert!(listing(&dir).is_empty());
}

#[test]
fn an_error_status_is_refused() {
    let server = TestServer::serving(404, b"");
    let guard = guard_for(&[&server]);
    let dir = temp_dir();
    let err = download(&guard, &spec_for(server.url("/m"), BODY), &dir).unwrap_err();
    assert!(matches!(err, DownloadError::Status(404)), "{err}");
    assert!(listing(&dir).is_empty());
}

#[test]
fn a_redirect_to_a_host_not_on_the_allowlist_names_the_host() {
    let cdn = TestServer::serving(200, BODY);
    let cdn_url = cdn.url("/blob");
    let hub = TestServer::start(move |_| response(302, &[("Location", &cdn_url)], b""));
    // Only the hub is allowed, as if the CDN host were missing from the list.
    let guard = guard_for(&[&hub]);
    let dir = temp_dir();

    let err = download(&guard, &spec_for(hub.url("/resolve/x"), BODY), &dir).unwrap_err();
    let DownloadError::Blocked { host, .. } = &err else {
        panic!("expected Blocked, got {err}");
    };
    assert_eq!(host, &cdn.origin().to_string());
    assert!(err.to_string().contains(&cdn.origin().to_string()));
    assert_eq!(cdn.requests(), 0);
    assert!(listing(&dir).is_empty());

    // With the CDN allowed too, the redirect is followed and the file lands.
    guard.allow(cdn.origin());
    download(&guard, &spec_for(hub.url("/resolve/x"), BODY), &dir).unwrap();
    assert_eq!(listing(&dir), ["test.gguf"]);
}

#[test]
fn a_host_not_on_the_allowlist_is_never_contacted() {
    let server = TestServer::serving(200, BODY);
    let guard = NetGuard::new(Allowlist::new());
    let dir = temp_dir();
    let err = download(&guard, &spec_for(server.url("/m"), BODY), &dir).unwrap_err();
    assert!(matches!(err, DownloadError::Blocked { .. }), "{err}");
    assert_eq!(server.requests(), 0);
}

#[test]
fn ensure_model_keeps_a_verified_file_and_replaces_a_bad_one() {
    let server = TestServer::serving(200, BODY);
    let guard = guard_for(&[&server]);
    let dir = temp_dir();
    let spec = spec_for(server.url("/m"), BODY);

    ensure_model(&guard, &spec, &dir).unwrap();
    assert_eq!(server.requests(), 1);
    // Already there and verified: no request.
    ensure_model(&guard, &spec, &dir).unwrap();
    assert_eq!(server.requests(), 1);

    // Corrupted on disk (same size): replaced by a fresh download.
    let path = dir.join("test.gguf");
    let mut bytes = BODY.to_vec();
    bytes[0] ^= 0xff;
    fs::write(&path, &bytes).unwrap();
    ensure_model(&guard, &spec, &dir).unwrap();
    assert_eq!(server.requests(), 2);
    assert_eq!(fs::read(&path).unwrap(), BODY);
}

#[test]
fn a_stale_partial_file_is_overwritten() {
    let server = TestServer::serving(200, BODY);
    let guard = guard_for(&[&server]);
    let dir = temp_dir();
    fs::write(
        dir.join("test.gguf.partial"),
        b"left over from a crash, and longer than the body",
    )
    .unwrap();
    download(&guard, &spec_for(server.url("/m"), BODY), &dir).unwrap();
    assert_eq!(listing(&dir), ["test.gguf"]);
}
