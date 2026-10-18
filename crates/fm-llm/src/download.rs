//! Model files: pinned specs, download through fm-netguard, and verification.
//!
//! A model is fetched from a URL pinned to a fixed revision and accepted only
//! if its size and SHA-256 match the spec. The body is hashed while it streams
//! into `<file>.partial`, which is synced and renamed into place only after
//! both checks pass, so a file at the final path is always a verified one.
//!
//! [`download_with`] can also resume. An interrupted or cancelled download keeps
//! its `.partial` file; the next call hashes the bytes already there, asks for
//! the rest with `Range: bytes=<n>-`, and continues the same hash over the new
//! bytes. Only a `206` whose `Content-Range` is exactly `bytes <n>-<size−1>/<size>`
//! is appended; any other answer truncates the file and starts again from zero.
//! Resuming changes nothing about acceptance: the whole file's size and SHA-256
//! must still match the spec, so a wrong or tampered range can only fail.

use std::borrow::Cow;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fm_netguard::{NetGuard, Request};
use sha2::{Digest, Sha256};

/// Label for model downloads in the network log.
pub const DOWNLOAD_PURPOSE: &str = "model-download";

/// Every origin a model download may touch (SPEC decision 40): Hugging Face
/// and the CDN hosts its `resolve` URLs redirect to. The app puts exactly
/// these on the allowlist, as exact origins.
///
/// The CDN host depends on region and storage backend. Listed are the ones
/// actually seen: on 2026-10-16, from the US, every repo tried (SmolLM2,
/// Qwen2.5, Qwen3, Llama 3.2, Phi-3, TinyLlama, BERT) redirected to
/// `us.aws.cdn.hf.co`.
///
/// A download redirected to any other host fails with
/// [`DownloadError::Blocked`], whose `host` names it. To add one: confirm it
/// is Hugging Face's (`curl -sI <resolve URL>` shows it in `Location`), add
/// it here with a note of where it was seen, and record the change with the
/// coordinator. The SHA-256 pin is checked whichever host served the file.
pub const DOWNLOAD_ORIGINS: &[&str] = &["https://huggingface.co", "https://us.aws.cdn.hf.co"];

/// The small model the tests run against. Not the product model (T2.4 picks
/// that): SmolLM2-135M-Instruct, Q8_0, Apache-2.0.
pub const CI_TEST_MODEL: ModelSpec = ModelSpec {
    id: Cow::Borrowed("smollm2-135m-instruct-q8_0"),
    url: Cow::Borrowed(
        "https://huggingface.co/bartowski/SmolLM2-135M-Instruct-GGUF/resolve/09816acd5d99df7be770d85ea30822623dab342c/SmolLM2-135M-Instruct-Q8_0.gguf",
    ),
    file_name: Cow::Borrowed("SmolLM2-135M-Instruct-Q8_0.gguf"),
    size: 144_811_360,
    sha256: sha256_hex("5a1395716f7913741cc51d98581b9b1228d80987a9f7d3664106742eb06bba83"),
};

/// One exact model file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    /// Name and quantisation, recorded with every decision, e.g.
    /// `"smollm2-135m-instruct-q8_0"`.
    pub id: Cow<'static, str>,
    /// Where to fetch it, pinned to a fixed revision.
    pub url: Cow<'static, str>,
    /// File name in the models directory. A plain name, no path separators.
    pub file_name: Cow<'static, str>,
    pub size: u64,
    pub sha256: [u8; 32],
}

/// Why a model file couldn't be fetched or isn't the expected one.
#[derive(Debug)]
pub enum DownloadError {
    /// The network guard refused a host on the way, e.g. a CDN the download
    /// was redirected to that isn't on the allowlist.
    Blocked {
        host: String,
        reason: String,
    },
    /// Any other refusal or failure from the network guard.
    Network(fm_netguard::Error),
    /// The server answered with something other than 200.
    Status(u16),
    /// The file is not the size the spec says.
    WrongSize {
        expected: u64,
        actual: u64,
    },
    /// The file's SHA-256 is not the one the spec says.
    WrongHash {
        expected: String,
        actual: String,
    },
    /// The spec's file name is empty or contains a path separator.
    BadFileName,
    /// The caller's progress callback asked to stop. The `.partial` file is kept, so
    /// the next [`download_with`] resumes from it.
    Cancelled,
    /// Another download (another app instance) holds the `.partial` file's lock. Nothing
    /// was touched; try again once it finishes.
    Busy,
    /// The `.partial` path is a symbolic link; it is never followed or removed.
    SymlinkedPartial,
    Io(io::Error),
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked { host, reason } => write!(
                f,
                "model download blocked at {host} ({reason}); that host must be on the allowlist"
            ),
            Self::Network(e) => write!(f, "model download failed: {e}"),
            Self::Status(status) => write!(f, "model download failed: HTTP status {status}"),
            Self::WrongSize { expected, actual } => {
                write!(f, "model file is {actual} bytes, expected {expected}")
            }
            Self::WrongHash { expected, actual } => {
                write!(f, "model file SHA-256 is {actual}, expected {expected}")
            }
            Self::BadFileName => f.write_str("model file name must be a plain file name"),
            Self::Cancelled => f.write_str("model download stopped; it resumes next time"),
            Self::Busy => f.write_str("another download of this model is running"),
            Self::SymlinkedPartial => {
                f.write_str("the partial model file is a symbolic link; remove it and try again")
            }
            Self::Io(e) => write!(f, "model file I/O error: {e}"),
        }
    }
}

impl std::error::Error for DownloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Network(e) => Some(e),
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for DownloadError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<fm_netguard::Error> for DownloadError {
    fn from(e: fm_netguard::Error) -> Self {
        match e {
            fm_netguard::Error::Blocked { origin, reason } => Self::Blocked {
                host: origin.to_string(),
                reason: reason.to_string(),
            },
            e => Self::Network(e),
        }
    }
}

/// Makes sure `dir` holds the verified model file and returns its path.
///
/// A file already there is checked first and kept if it matches; one that
/// doesn't match is replaced. Otherwise the file is downloaded through
/// `guard`. Either way the returned path holds exactly the spec's bytes.
pub fn ensure_model(
    guard: &NetGuard,
    spec: &ModelSpec,
    dir: &Path,
) -> Result<PathBuf, DownloadError> {
    let path = model_path(spec, dir)?;
    if path.exists() {
        match verify_file(&path, spec) {
            Ok(()) => return Ok(path),
            Err(DownloadError::WrongSize { .. } | DownloadError::WrongHash { .. }) => {
                fs::remove_file(&path)?;
            }
            Err(e) => return Err(e),
        }
    }
    download(guard, spec, dir)
}

/// Downloads the model into `dir`, replacing any file at its path, and
/// returns the path. Nothing is left at the final path unless it verified.
pub fn download(guard: &NetGuard, spec: &ModelSpec, dir: &Path) -> Result<PathBuf, DownloadError> {
    let path = model_path(spec, dir)?;
    fs::create_dir_all(dir)?;
    let partial = dir.join(format!("{}.partial", spec.file_name));
    let result = fetch_into(guard, spec, &partial).and_then(|lock| {
        fs::rename(&partial, &path)?;
        sync_dir(dir)?;
        drop(lock);
        Ok(())
    });
    if result.is_err()
        && !matches!(
            result,
            Err(DownloadError::Busy | DownloadError::SymlinkedPartial)
        )
    {
        let _ = fs::remove_file(&partial);
    }
    result.map(|()| path)
}

/// [`ensure_model`], downloading with [`download_with`]: resumable, with progress.
pub fn ensure_model_with(
    guard: &NetGuard,
    spec: &ModelSpec,
    dir: &Path,
    on_progress: &mut dyn FnMut(u64) -> bool,
) -> Result<PathBuf, DownloadError> {
    let path = model_path(spec, dir)?;
    if path.exists() {
        match verify_file(&path, spec) {
            Ok(()) => return Ok(path),
            Err(DownloadError::WrongSize { .. } | DownloadError::WrongHash { .. }) => {
                fs::remove_file(&path)?;
            }
            Err(e) => return Err(e),
        }
    }
    download_with(guard, spec, dir, on_progress)
}

/// Downloads the model into `dir` like [`download`], but resumes from a `.partial`
/// file left by an earlier call, and reports progress: `on_progress` gets the bytes
/// on disk so far and returns `false` to stop (then [`DownloadError::Cancelled`], with
/// the partial file kept). A partial file that fails the final check is deleted.
pub fn download_with(
    guard: &NetGuard,
    spec: &ModelSpec,
    dir: &Path,
    on_progress: &mut dyn FnMut(u64) -> bool,
) -> Result<PathBuf, DownloadError> {
    let path = model_path(spec, dir)?;
    fs::create_dir_all(dir)?;
    let partial = dir.join(format!("{}.partial", spec.file_name));
    let result = resume_into(guard, spec, &partial, on_progress).and_then(|lock| {
        // The lock is held across the rename, so no other download touches the file
        // between the hash check and here.
        fs::rename(&partial, &path)?;
        sync_dir(dir)?;
        drop(lock);
        // The file now at the final path is checked once more, whole.
        if let Err(e) = verify_file(&path, spec) {
            let _ = fs::remove_file(&path);
            return Err(e);
        }
        Ok(())
    });
    match &result {
        // Kept for the next attempt: a stop, or the network dropping mid-way. Whatever
        // the partial file holds is re-hashed from disk next time and checked in full.
        Err(DownloadError::Cancelled | DownloadError::Network(_)) => {}
        // Not ours to delete: another instance's download, or a link we won't follow.
        Err(DownloadError::Busy | DownloadError::SymlinkedPartial) => {}
        // A wrong size or hash, a refused host or status, a bad file: start clean.
        Err(_) => {
            let _ = fs::remove_file(&partial);
        }
        Ok(()) => {}
    }
    result.map(|()| path)
}

/// Opens `partial` for this download alone: never through a symbolic link, and with an
/// exclusive lock, so two app instances can't write the same file. The returned handle
/// holds the lock until it is dropped.
fn open_partial(partial: &Path) -> Result<File, DownloadError> {
    if fs::symlink_metadata(partial).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(DownloadError::SymlinkedPartial);
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(partial)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(DownloadError::Busy),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// The resumable fetch: picks up from what `partial` holds, else from zero. Returns the
/// locked handle, to be held until the file is renamed into place.
fn resume_into(
    guard: &NetGuard,
    spec: &ModelSpec,
    partial: &Path,
    on_progress: &mut dyn FnMut(u64) -> bool,
) -> Result<File, DownloadError> {
    let mut file = open_partial(partial)?;
    let have = file.metadata()?.len();
    if have > 0 && have <= spec.size {
        // Hash what is already there; the new bytes continue the same hash.
        let mut hasher = Sha256::new();
        file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::with_capacity(1 << 20, (&file).take(have));
        io::copy(&mut reader, &mut HashOnly(&mut hasher))?;
        if have == spec.size {
            check_hash(spec, &hasher.finalize().into())?;
            file.sync_all()?;
            return Ok(file);
        }
        file.seek(SeekFrom::End(0))?;
        match fetch_range(guard, spec, file.try_clone()?, hasher, have, on_progress)? {
            Ranged::Done => return Ok(file),
            // The server didn't answer with exactly the rest: start again from zero.
            Ranged::Refused => {}
        }
    }
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    fetch_full(guard, spec, file.try_clone()?, on_progress)?;
    Ok(file)
}

enum Ranged {
    Done,
    /// The answer wasn't `206` with the exact `Content-Range` asked for.
    Refused,
}

/// Asks for `start..size`, appending to `file`. Accepts only `206` with
/// `Content-Range: bytes <start>-<size-1>/<size>` and exactly that many bytes.
fn fetch_range(
    guard: &NetGuard,
    spec: &ModelSpec,
    file: File,
    hasher: Sha256,
    start: u64,
    on_progress: &mut dyn FnMut(u64) -> bool,
) -> Result<Ranged, DownloadError> {
    let remaining = spec.size - start;
    let mut out = ProgressWriter {
        inner: io::BufWriter::with_capacity(1 << 20, file),
        hasher,
        written: start,
        on_progress,
        cancelled: false,
    };
    let request = Request::get(spec.url.as_ref(), DOWNLOAD_PURPOSE)
        .header("Range", format!("bytes={start}-"))
        .max_body_bytes(remaining);
    let response = guard.download(request, &mut out);
    if out.cancelled {
        out.flush_kept()?;
        return Err(DownloadError::Cancelled);
    }
    let download = match response {
        Ok(d) => d,
        // A body longer than the rest: the server ignored the range.
        Err(fm_netguard::Error::BodyTooLarge { .. }) => return Ok(Ranged::Refused),
        Err(e) => {
            out.flush_kept()?;
            return Err(e.into());
        }
    };
    let expected_range = format!("bytes {start}-{}/{}", spec.size - 1, spec.size);
    let range_ok = download.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-range") && value.trim() == expected_range
    });
    if download.status != 206 || !range_ok || download.bytes != remaining {
        return Ok(Ranged::Refused);
    }
    out.finish(spec)?;
    Ok(Ranged::Done)
}

/// The whole file from zero into `file`.
fn fetch_full(
    guard: &NetGuard,
    spec: &ModelSpec,
    file: File,
    on_progress: &mut dyn FnMut(u64) -> bool,
) -> Result<(), DownloadError> {
    let mut out = ProgressWriter {
        inner: io::BufWriter::with_capacity(1 << 20, file),
        hasher: Sha256::new(),
        written: 0,
        on_progress,
        cancelled: false,
    };
    let request = Request::get(spec.url.as_ref(), DOWNLOAD_PURPOSE).max_body_bytes(spec.size);
    let response = guard.download(request, &mut out);
    if out.cancelled {
        out.flush_kept()?;
        return Err(DownloadError::Cancelled);
    }
    let download = match response {
        Ok(d) => d,
        Err(e) => {
            out.flush_kept()?;
            return Err(e.into());
        }
    };
    if download.status != 200 {
        return Err(DownloadError::Status(download.status));
    }
    if download.bytes != spec.size {
        return Err(DownloadError::WrongSize {
            expected: spec.size,
            actual: download.bytes,
        });
    }
    out.finish(spec)
}

/// Writes and hashes the body, reporting progress; stops the transfer (with an I/O
/// error the guard passes on) when the callback says so.
struct ProgressWriter<'a, W: Write> {
    inner: W,
    hasher: Sha256,
    /// Bytes in the file so far, including any resumed from.
    written: u64,
    on_progress: &'a mut dyn FnMut(u64) -> bool,
    cancelled: bool,
}

impl<W: Write> Write for ProgressWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.cancelled || !(self.on_progress)(self.written) {
            self.cancelled = true;
            return Err(io::Error::other("model download cancelled"));
        }
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl ProgressWriter<'_, io::BufWriter<File>> {
    /// Keeps what was written for a later resume.
    fn flush_kept(&mut self) -> Result<(), DownloadError> {
        self.inner.flush()?;
        Ok(())
    }

    /// Checks the whole file's hash and syncs it.
    fn finish(self, spec: &ModelSpec) -> Result<(), DownloadError> {
        (self.on_progress)(self.written);
        check_hash(spec, &self.hasher.finalize().into())?;
        let file = self
            .inner
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?;
        file.sync_all()?;
        Ok(())
    }
}

/// Feeds bytes to a hasher, for re-hashing a partial file.
struct HashOnly<'a>(&'a mut Sha256);

impl Write for HashOnly<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Checks the file's size and SHA-256 against the spec.
pub fn verify_file(path: &Path, spec: &ModelSpec) -> Result<(), DownloadError> {
    let actual = fs::metadata(path)?.len();
    if actual != spec.size {
        return Err(DownloadError::WrongSize {
            expected: spec.size,
            actual,
        });
    }
    let digest = sha256_file(path)?;
    check_hash(spec, &digest)
}

/// SHA-256 of a file, read in chunks.
pub fn sha256_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut reader = BufReader::with_capacity(1 << 20, File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(hasher.finalize().into())
}

/// Lowercase hex of a digest.
pub fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses 64 hex digits at compile time; a bad constant fails the build.
pub const fn sha256_hex(s: &str) -> [u8; 32] {
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("not a hex digit"),
        }
    }
    let s = s.as_bytes();
    assert!(s.len() == 64, "a SHA-256 is 64 hex digits");
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nibble(s[2 * i]) << 4) | nibble(s[2 * i + 1]);
        i += 1;
    }
    out
}

fn model_path(spec: &ModelSpec, dir: &Path) -> Result<PathBuf, DownloadError> {
    let name = spec.file_name.as_ref();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(DownloadError::BadFileName);
    }
    Ok(dir.join(name))
}

fn check_hash(spec: &ModelSpec, digest: &[u8; 32]) -> Result<(), DownloadError> {
    if *digest == spec.sha256 {
        Ok(())
    } else {
        Err(DownloadError::WrongHash {
            expected: hex(&spec.sha256),
            actual: hex(digest),
        })
    }
}

/// Streams the body into `partial`, hashing as it goes, then checks it.
/// Returns the locked handle, to be held until the file is renamed into place.
fn fetch_into(guard: &NetGuard, spec: &ModelSpec, partial: &Path) -> Result<File, DownloadError> {
    // The same protections as the resumable path: no symbolic link, one writer.
    let file = open_partial(partial)?;
    file.set_len(0)?;
    let mut out = HashingWriter {
        inner: io::BufWriter::with_capacity(1 << 20, file),
        hasher: Sha256::new(),
    };
    let request = Request::get(spec.url.as_ref(), DOWNLOAD_PURPOSE).max_body_bytes(spec.size);
    let download = guard.download(request, &mut out)?;
    if download.status != 200 {
        return Err(DownloadError::Status(download.status));
    }
    if download.bytes != spec.size {
        return Err(DownloadError::WrongSize {
            expected: spec.size,
            actual: download.bytes,
        });
    }
    let HashingWriter { inner, hasher } = out;
    check_hash(spec, &hasher.finalize().into())?;
    let file = inner.into_inner().map_err(io::IntoInnerError::into_error)?;
    file.sync_all()?;
    Ok(file)
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<(), DownloadError> {
    File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> Result<(), DownloadError> {
    Ok(())
}

struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let digest = sha256_hex("00ff10ABcdef00000000000000000000000000000000000000000000000000ee");
        assert_eq!(
            hex(&digest),
            "00ff10abcdef00000000000000000000000000000000000000000000000000ee"
        );
    }

    #[test]
    fn file_names_must_be_plain() {
        let dir = Path::new("/models");
        for bad in ["", ".", "..", "a/b", r"a\b", "../x"] {
            let spec = ModelSpec {
                file_name: Cow::Owned(bad.to_string()),
                ..CI_TEST_MODEL
            };
            assert!(
                matches!(model_path(&spec, dir), Err(DownloadError::BadFileName)),
                "{bad:?}"
            );
        }
        assert_eq!(
            model_path(&CI_TEST_MODEL, dir).unwrap(),
            dir.join("SmolLM2-135M-Instruct-Q8_0.gguf")
        );
    }

    #[test]
    fn the_test_model_is_pinned_to_a_revision_on_an_allowed_origin() {
        let url = CI_TEST_MODEL.url.as_ref();
        assert!(url.starts_with(&format!("{}/", DOWNLOAD_ORIGINS[0])));
        assert!(url.contains("/resolve/09816acd5d99df7be770d85ea30822623dab342c/"));
        for origin in DOWNLOAD_ORIGINS {
            origin.parse::<fm_netguard::Origin>().unwrap();
        }
    }

    #[test]
    fn hashing_writer_hashes_what_it_writes() {
        let mut w = HashingWriter {
            inner: Vec::new(),
            hasher: Sha256::new(),
        };
        w.write_all(b"ab").unwrap();
        w.write_all(b"c").unwrap();
        let digest: [u8; 32] = w.hasher.finalize().into();
        assert_eq!(
            hex(&digest),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(w.inner, b"abc");
    }
}
