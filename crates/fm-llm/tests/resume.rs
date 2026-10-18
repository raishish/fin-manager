//! Resumable model downloads ([`fm_llm::download_with`]) against throwaway servers on
//! loopback, through a real fm-netguard. The final size and SHA-256 check decides
//! acceptance whatever a server answers to a `Range` request.

use std::borrow::Cow;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use fm_llm::{DownloadError, ModelSpec, download_with, ensure_model_with, sha256_file};
use fm_netguard::{Allowlist, NetGuard, Origin};
use sha2::{Digest, Sha256};

/// 64 KiB of patterned bytes, so a byte out of place changes the hash.
fn body() -> Vec<u8> {
    (0..65_536u32).map(|i| (i % 251) as u8).collect()
}

/// The `Range` header of each request, `None` when absent.
type Seen = Arc<Mutex<Vec<Option<String>>>>;

struct Server {
    port: u16,
    seen: Seen,
}

impl Server {
    /// `respond(range)` builds the whole HTTP response for a request.
    fn start(respond: impl Fn(Option<&str>) -> Vec<u8> + Send + 'static) -> Self {
        Self::start_paused(respond, None)
    }

    /// Like [`Server::start`], but pauses after the first `pause_after` bytes of each
    /// response, so the client sees the body arrive in (at least) two pieces.
    fn start_paused(
        respond: impl Fn(Option<&str>) -> Vec<u8> + Send + 'static,
        pause_after: Option<usize>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Seen = Arc::default();
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut range = None;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    let header = header.trim_end();
                    if header.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':')
                        && name.eq_ignore_ascii_case("range")
                    {
                        range = Some(value.trim().to_string());
                    }
                }
                log.lock().unwrap().push(range.clone());
                let response = respond(range.as_deref());
                match pause_after {
                    Some(n) if n < response.len() => {
                        let _ = stream.write_all(&response[..n]);
                        let _ = stream.flush();
                        thread::sleep(std::time::Duration::from_millis(200));
                        let _ = stream.write_all(&response[n..]);
                    }
                    _ => {
                        let _ = stream.write_all(&response);
                    }
                }
            }
        });
        Self { port, seen }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/model.gguf", self.port)
    }

    fn guard(&self) -> NetGuard {
        let mut allowlist = Allowlist::new();
        let origin: Origin = format!("http://127.0.0.1:{}", self.port).parse().unwrap();
        allowlist.allow(origin);
        NetGuard::new(allowlist)
    }

    fn ranges(&self) -> Vec<Option<String>> {
        self.seen.lock().unwrap().clone()
    }
}

fn http(status: u16, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
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

/// A server that honours `Range: bytes=<n>-` correctly.
fn honest(full: Vec<u8>) -> Server {
    honest_paused(full, None)
}

/// [`honest`], pausing after the first `pause_after` bytes of each response.
fn honest_paused(full: Vec<u8>, pause_after: Option<usize>) -> Server {
    Server::start_paused(
        move |range| match range.and_then(|r| r.strip_prefix("bytes=")) {
            Some(r) => {
                let start: usize = r.trim_end_matches('-').parse().unwrap();
                let content_range = format!("bytes {start}-{}/{}", full.len() - 1, full.len());
                http(206, &[("Content-Range", content_range)], &full[start..])
            }
            None => http(200, &[], &full),
        },
        pause_after,
    )
}

fn spec(url: String, full: &[u8]) -> ModelSpec {
    ModelSpec {
        id: Cow::Borrowed("test-model"),
        url: Cow::Owned(url),
        file_name: Cow::Borrowed("test.gguf"),
        size: full.len() as u64,
        sha256: Sha256::digest(full).into(),
    }
}

fn temp_dir() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "fm-llm-resume-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn keep_going(_: u64) -> bool {
    true
}

#[test]
fn a_cancelled_download_resumes_with_a_range_request() {
    let full = body();
    // The first response arrives in two pieces, so there is a second write to stop at,
    // whatever the network stack's buffering.
    let server = honest_paused(full.clone(), Some(20_000));
    let (guard, spec, dir) = (server.guard(), spec(server.url(), &full), temp_dir());

    // Stop once some bytes are on disk.
    let mut stop_after_first = |written: u64| written == 0;
    let err = download_with(&guard, &spec, &dir, &mut stop_after_first).unwrap_err();
    assert!(matches!(err, DownloadError::Cancelled), "{err}");
    let partial = dir.join("test.gguf.partial");
    let kept = fs::metadata(&partial).unwrap().len();
    assert!(kept > 0 && kept < full.len() as u64, "kept {kept} bytes");
    assert!(!dir.join("test.gguf").exists(), "nothing at the final path");

    let mut seen = Vec::new();
    let path = download_with(&guard, &spec, &dir, &mut |n| {
        seen.push(n);
        true
    })
    .unwrap();
    assert_eq!(fs::read(&path).unwrap(), full);
    assert_eq!(sha256_file(&path).unwrap(), spec.sha256);
    assert_eq!(server.ranges(), [None, Some(format!("bytes={kept}-"))]);
    assert_eq!(
        seen.first(),
        Some(&kept),
        "progress starts from the kept bytes"
    );
    assert_eq!(seen.last(), Some(&(full.len() as u64)));
    assert!(!partial.exists());
}

#[test]
fn a_server_that_ignores_the_range_means_starting_again() {
    let full = body();
    // Always the whole file with 200, Range or not.
    let whole = full.clone();
    let server = Server::start(move |_| http(200, &[], &whole));
    let (guard, spec, dir) = (server.guard(), spec(server.url(), &full), temp_dir());
    fs::write(dir.join("test.gguf.partial"), &full[..1000]).unwrap();

    let path = download_with(&guard, &spec, &dir, &mut keep_going).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        full,
        "restarted from zero, not appended"
    );
    assert_eq!(server.ranges(), [Some("bytes=1000-".into()), None]);
}

#[test]
fn a_206_with_the_wrong_content_range_means_starting_again() {
    let full = body();
    let copy = full.clone();
    // Claims a different range than the one asked for.
    let server = Server::start(move |range| match range {
        Some(_) => http(
            206,
            &[(
                "Content-Range",
                format!("bytes 0-{}/{}", copy.len() - 1, copy.len()),
            )],
            &copy[1000..],
        ),
        None => http(200, &[], &copy),
    });
    let (guard, spec, dir) = (server.guard(), spec(server.url(), &full), temp_dir());
    fs::write(dir.join("test.gguf.partial"), &full[..1000]).unwrap();

    let path = download_with(&guard, &spec, &dir, &mut keep_going).unwrap();
    assert_eq!(fs::read(&path).unwrap(), full);
    assert_eq!(
        server.ranges().len(),
        2,
        "the range was refused, then a full download"
    );
}

/// A range answer with the right headers but the wrong bytes can't get past the final
/// hash: the file is refused and nothing lands at the final path.
#[test]
fn tampered_range_bytes_fail_the_final_hash() {
    let full = body();
    let copy = full.clone();
    let server = Server::start(move |range| {
        let start: usize = range
            .and_then(|r| r.strip_prefix("bytes="))
            .map_or(0, |r| r.trim_end_matches('-').parse().unwrap());
        let mut rest = copy[start..].to_vec();
        rest[10] ^= 0xff;
        let content_range = format!("bytes {start}-{}/{}", copy.len() - 1, copy.len());
        http(206, &[("Content-Range", content_range)], &rest)
    });
    let (guard, spec, dir) = (server.guard(), spec(server.url(), &full), temp_dir());
    fs::write(dir.join("test.gguf.partial"), &full[..1000]).unwrap();

    let err = download_with(&guard, &spec, &dir, &mut keep_going).unwrap_err();
    assert!(matches!(err, DownloadError::WrongHash { .. }), "{err}");
    assert!(!dir.join("test.gguf").exists());
    assert!(
        !dir.join("test.gguf.partial").exists(),
        "a partial file that failed the check is deleted"
    );
}

/// Corrupted bytes already in the partial file are caught the same way.
#[test]
fn a_corrupt_partial_file_fails_the_final_hash() {
    let full = body();
    let server = honest(full.clone());
    let (guard, spec, dir) = (server.guard(), spec(server.url(), &full), temp_dir());
    let mut bad = full[..1000].to_vec();
    bad[3] ^= 0x01;
    fs::write(dir.join("test.gguf.partial"), &bad).unwrap();

    let err = download_with(&guard, &spec, &dir, &mut keep_going).unwrap_err();
    assert!(matches!(err, DownloadError::WrongHash { .. }), "{err}");
    assert!(!dir.join("test.gguf.partial").exists());
    // The next attempt starts clean and succeeds.
    let path = download_with(&guard, &spec, &dir, &mut keep_going).unwrap();
    assert_eq!(fs::read(&path).unwrap(), full);
}

#[test]
fn a_complete_partial_file_is_verified_without_a_request() {
    let full = body();
    let server = honest(full.clone());
    let (guard, spec, dir) = (server.guard(), spec(server.url(), &full), temp_dir());
    fs::write(dir.join("test.gguf.partial"), &full).unwrap();
    let path = download_with(&guard, &spec, &dir, &mut keep_going).unwrap();
    assert_eq!(fs::read(&path).unwrap(), full);
    assert!(server.ranges().is_empty());
}

#[test]
fn a_partial_file_longer_than_the_model_is_discarded() {
    let full = body();
    let server = honest(full.clone());
    let (guard, spec, dir) = (server.guard(), spec(server.url(), &full), temp_dir());
    let mut long = full.clone();
    long.extend_from_slice(b"extra");
    fs::write(dir.join("test.gguf.partial"), &long).unwrap();
    let path = download_with(&guard, &spec, &dir, &mut keep_going).unwrap();
    assert_eq!(fs::read(&path).unwrap(), full);
    assert_eq!(server.ranges(), [None], "a fresh download, no range");
}

#[test]
fn ensure_model_with_keeps_a_verified_file() {
    let full = body();
    let server = honest(full.clone());
    let (guard, spec, dir) = (server.guard(), spec(server.url(), &full), temp_dir());
    let first = ensure_model_with(&guard, &spec, &dir, &mut keep_going).unwrap();
    let again = ensure_model_with(&guard, &spec, &dir, &mut keep_going).unwrap();
    assert_eq!(first, again);
    assert_eq!(
        server.ranges().len(),
        1,
        "the second call found it verified"
    );
}

/// Review T5.5 #3: a `.partial` that is a symbolic link is never followed: its target is
/// neither read nor written, and the link stays where it is.
#[cfg(unix)]
#[test]
fn a_symlinked_partial_file_is_refused() {
    let full = body();
    let server = honest(full.clone());
    let (guard, spec, dir) = (server.guard(), spec(server.url(), &full), temp_dir());
    let target = dir.join("elsewhere.bin");
    fs::write(&target, b"not yours").unwrap();
    std::os::unix::fs::symlink(&target, dir.join("test.gguf.partial")).unwrap();

    let err = download_with(&guard, &spec, &dir, &mut keep_going).unwrap_err();
    assert!(matches!(err, DownloadError::SymlinkedPartial), "{err}");
    assert_eq!(
        fs::read(&target).unwrap(),
        b"not yours",
        "the target is untouched"
    );
    assert!(
        fs::symlink_metadata(dir.join("test.gguf.partial"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(server.ranges().is_empty(), "nothing was requested");
    assert!(!dir.join("test.gguf").exists());
    // The old API refuses it the same way.
    let err = fm_llm::download(&guard, &spec, &dir).unwrap_err();
    assert!(matches!(err, DownloadError::SymlinkedPartial), "{err}");
}

/// Review T5.5 #2: while another download (another app instance) holds the partial
/// file, a second one reports Busy and touches nothing.
#[test]
fn a_second_download_of_the_same_file_is_busy() {
    let full = body();
    let server = honest(full.clone());
    let (guard, spec, dir) = (server.guard(), spec(server.url(), &full), temp_dir());
    let partial = dir.join("test.gguf.partial");
    fs::write(&partial, &full[..1000]).unwrap();
    let other = fs::OpenOptions::new().write(true).open(&partial).unwrap();
    other.try_lock().unwrap();

    let err = download_with(&guard, &spec, &dir, &mut keep_going).unwrap_err();
    assert!(matches!(err, DownloadError::Busy), "{err}");
    assert_eq!(
        fs::read(&partial).unwrap(),
        &full[..1000],
        "the other's bytes are untouched"
    );
    assert!(server.ranges().is_empty());

    // Once the other lets go, the download resumes from its bytes.
    drop(other);
    let path = download_with(&guard, &spec, &dir, &mut keep_going).unwrap();
    assert_eq!(fs::read(&path).unwrap(), full);
    assert_eq!(server.ranges(), [Some("bytes=1000-".into())]);
}
