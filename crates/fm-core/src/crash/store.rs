//! Crash reports on disk: the opt-in setting and the saved reports.
//!
//! Everything lives in one directory (the app passes `<app data>/crash-reports`):
//! - `settings.json`, `{"enabled": true}` once the user opts in. Missing, unreadable or
//!   invalid means disabled, so crash reports are off by default.
//! - `crash-<unix ms>-<n>.json`, one per report, holding exactly the bytes the preview
//!   shows.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::report::CrashReport;

const SETTINGS_FILE: &str = "settings.json";
const REPORT_PREFIX: &str = "crash-";
const REPORT_SUFFIX: &str = ".json";

/// Keeps two reports written in the same millisecond apart.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    enabled: bool,
}

/// Names one saved report: `crash-<unix ms>-<n>`. Parsing checks the shape, so an id from
/// the UI can't name any other file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReportId {
    unix_ms: u64,
    seq: u64,
}

impl ReportId {
    fn file_name(&self) -> String {
        format!("{self}{REPORT_SUFFIX}")
    }
}

impl fmt::Display for ReportId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{REPORT_PREFIX}{}-{}", self.unix_ms, self.seq)
    }
}

/// The id isn't `crash-<digits>-<digits>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidReportId;

impl fmt::Display for InvalidReportId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a crash report id")
    }
}

impl std::error::Error for InvalidReportId {}

impl FromStr for ReportId {
    type Err = InvalidReportId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rest = s.strip_prefix(REPORT_PREFIX).ok_or(InvalidReportId)?;
        let (ms, seq) = rest.split_once('-').ok_or(InvalidReportId)?;
        let number = |part: &str| {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(InvalidReportId);
            }
            part.parse::<u64>().map_err(|_| InvalidReportId)
        };
        Ok(Self {
            unix_ms: number(ms)?,
            seq: number(seq)?,
        })
    }
}

/// A report saved on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReport {
    pub id: ReportId,
    /// Where the file is, so the user can attach it to an issue or email themselves.
    pub path: PathBuf,
}

/// The crash-report directory.
#[derive(Debug, Clone)]
pub struct CrashStore {
    dir: PathBuf,
}

impl CrashStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether the user has opted in. Anything but a valid settings file saying so is `false`.
    pub fn is_enabled(&self) -> bool {
        fs::read(self.dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
            .is_some_and(|s| s.enabled)
    }

    pub fn set_enabled(&self, enabled: bool) -> io::Result<()> {
        let json = serde_json::to_vec(&Settings { enabled }).map_err(io::Error::other)?;
        write_atomically(&self.dir, SETTINGS_FILE, &json)
    }

    /// Saves a report if crash reports are enabled; returns `None` (and writes nothing)
    /// when they are not.
    pub fn save(&self, report: &CrashReport) -> io::Result<Option<StoredReport>> {
        if !self.is_enabled() {
            return Ok(None);
        }
        let id = ReportId {
            unix_ms: now_unix_ms(),
            seq: SEQUENCE.fetch_add(1, Ordering::Relaxed),
        };
        write_atomically(&self.dir, &id.file_name(), report.to_json().as_bytes())?;
        Ok(Some(self.stored(id)))
    }

    /// Saved reports, newest first. A missing directory means none.
    pub fn list(&self) -> io::Result<Vec<StoredReport>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut ids = Vec::new();
        for entry in entries {
            let name = entry?.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(id) = name
                .strip_suffix(REPORT_SUFFIX)
                .and_then(|stem| stem.parse::<ReportId>().ok())
            {
                ids.push(id);
            }
        }
        ids.sort_unstable_by(|a, b| b.cmp(a));
        Ok(ids.into_iter().map(|id| self.stored(id)).collect())
    }

    /// The report's exact contents: what the preview shows and what the user sends.
    pub fn read(&self, id: &ReportId) -> io::Result<String> {
        fs::read_to_string(self.dir.join(id.file_name()))
    }

    pub fn delete(&self, id: &ReportId) -> io::Result<()> {
        fs::remove_file(self.dir.join(id.file_name()))
    }

    fn stored(&self, id: ReportId) -> StoredReport {
        StoredReport {
            path: self.dir.join(id.file_name()),
            id,
        }
    }
}

/// Writes `name` in `dir` through a temporary file and a rename, so a crash mid-write never
/// leaves a half-written file. Files are readable by the owner only.
fn write_atomically(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, dir.join(name))
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::crash::report::PanicFacts;
    use crate::crash::scrub::Scrubber;
    use crate::model::Timestamp;

    /// A fresh directory under the system temp dir, removed on drop.
    pub(crate) struct TempDir(pub PathBuf);

    impl TempDir {
        pub(crate) fn new(label: &str) -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "fm-core-{label}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn report(message: &str) -> CrashReport {
        CrashReport::from_panic(
            PanicFacts {
                message,
                file: None,
                line: 0,
                column: 0,
                thread: None,
                backtrace: "",
            },
            "0.0.0",
            Timestamp::from_unix_seconds(0),
            &Scrubber::new(),
        )
    }

    #[test]
    fn disabled_by_default_and_nothing_is_written() {
        let tmp = TempDir::new("crash-store");
        let store = CrashStore::new(&tmp.0);
        assert!(!store.is_enabled());
        assert_eq!(store.save(&report("boom")).unwrap(), None);
        assert!(!tmp.0.exists(), "a disabled store must not create anything");
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn invalid_settings_mean_disabled() {
        let tmp = TempDir::new("crash-store");
        fs::create_dir_all(&tmp.0).unwrap();
        for bad in [
            "",
            "{",
            "{\"enabled\": \"yes\"}",
            "true",
            "{\"enabled\": false}",
        ] {
            fs::write(tmp.0.join(SETTINGS_FILE), bad).unwrap();
            assert!(!CrashStore::new(&tmp.0).is_enabled(), "{bad:?}");
        }
    }

    #[test]
    fn enabled_store_saves_lists_reads_and_deletes() {
        let tmp = TempDir::new("crash-store");
        let store = CrashStore::new(&tmp.0);
        store.set_enabled(true).unwrap();
        assert!(store.is_enabled());

        let first = report("first");
        let a = store.save(&first).unwrap().unwrap();
        let b = store.save(&report("second")).unwrap().unwrap();
        assert_eq!(store.list().unwrap(), [b.clone(), a.clone()]);
        // The file holds exactly the report's JSON: what the preview shows is what is sent.
        assert_eq!(store.read(&a.id).unwrap(), first.to_json());
        assert_eq!(fs::read_to_string(&a.path).unwrap(), first.to_json());

        store.delete(&a.id).unwrap();
        assert_eq!(store.list().unwrap(), [b]);

        store.set_enabled(false).unwrap();
        assert!(!store.is_enabled());
        assert_eq!(store.save(&report("third")).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("crash-store");
        let store = CrashStore::new(&tmp.0);
        store.set_enabled(true).unwrap();
        let saved = store.save(&report("x")).unwrap().unwrap();
        let mode = fs::metadata(&saved.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn report_ids_only_name_report_files() {
        let id: ReportId = "crash-1792000000000-3".parse().unwrap();
        assert_eq!(id.to_string(), "crash-1792000000000-3");
        for bad in [
            "",
            "crash-",
            "crash-1",
            "crash-1-",
            "crash--1",
            "crash-1-2-3",
            "crash-1-x",
            "crash-+1-2",
            "settings",
            "../crash-1-2",
            "crash-1-2/../../etc",
        ] {
            assert_eq!(bad.parse::<ReportId>(), Err(InvalidReportId), "{bad:?}");
        }
    }

    #[test]
    fn unrelated_files_are_not_listed() {
        let tmp = TempDir::new("crash-store");
        let store = CrashStore::new(&tmp.0);
        store.set_enabled(true).unwrap();
        fs::write(tmp.0.join("notes.json"), "{}").unwrap();
        fs::write(tmp.0.join("crash-x-1.json"), "{}").unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
