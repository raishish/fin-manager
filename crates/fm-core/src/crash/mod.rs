//! Crash reports (SPEC decision 21): off by default, scrubbed, previewed, never sent by the
//! app.
//!
//! - Nothing is captured until the user opts in ([`CrashStore::set_enabled`]). While
//!   disabled, the panic hook only checks the setting and returns.
//! - When enabled, a panic writes one [`CrashReport`] as JSON. Every free-text field goes
//!   through the [`Scrubber`], which removes transaction text, amounts, merchant names and
//!   anything that identifies the machine (see `scrub.rs` for the rules).
//! - The app shows the saved file's exact contents and its path. The user copies it or
//!   attaches the file themselves (SPEC decision 39); the app has no way to upload it.
//!
//! Scrubbing free text is a backstop, not a guarantee, so code must still keep financial
//! data out of panic and `expect` messages.

mod report;
mod scrub;
mod store;

use std::backtrace::Backtrace;
use std::io;
use std::panic::PanicHookInfo;
use std::sync::{Arc, PoisonError, RwLock};

pub use report::{
    CrashReport, MAX_FRAME_BYTES, MAX_FRAMES, MAX_MESSAGE_BYTES, PanicFacts, REPORT_SCHEMA,
    normalize_path,
};
pub use scrub::{MIN_TERM_CHARS, Scrubber};
pub use store::{CrashStore, InvalidReportId, ReportId, StoredReport};

use crate::model::Timestamp;

/// Shown when a panic's payload isn't a string.
const NON_STRING_PAYLOAD: &str = "<non-string panic payload>";

/// Captures panics into the [`CrashStore`]. Cheap to clone; clones share the store and the
/// known sensitive terms.
#[derive(Debug, Clone)]
pub struct CrashReporter {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    store: CrashStore,
    app_version: String,
    scrubber: RwLock<Scrubber>,
}

impl CrashReporter {
    pub fn new(store: CrashStore, app_version: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                app_version: app_version.into(),
                scrubber: RwLock::new(Scrubber::new()),
            }),
        }
    }

    pub fn store(&self) -> &CrashStore {
        &self.inner.store
    }

    /// Terms to remove from reports, e.g. the ledger's merchant and account names. Set them
    /// on unlock and clear them on lock.
    pub fn set_sensitive_terms<I, S>(&self, terms: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.inner
            .scrubber
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .set_terms(terms);
    }

    pub fn clear_sensitive_terms(&self) {
        self.inner
            .scrubber
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear_terms();
    }

    /// Builds and saves a report for a panic. Returns `None`, having written nothing, when
    /// crash reports are disabled.
    pub fn record(&self, facts: PanicFacts<'_>) -> io::Result<Option<StoredReport>> {
        if !self.inner.store.is_enabled() {
            return Ok(None);
        }
        let report = CrashReport::from_panic(
            facts,
            &self.inner.app_version,
            Timestamp::now(),
            &self
                .inner
                .scrubber
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        );
        self.inner.store.save(&report)
    }

    /// Installs a process-wide panic hook that records reports, after running the hook
    /// that was installed before (by default, printing the panic to stderr).
    pub fn install_panic_hook(&self) {
        let reporter = self.clone();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            previous(info);
            reporter.on_panic(info);
        }));
    }

    fn on_panic(&self, info: &PanicHookInfo<'_>) {
        // Check before capturing a backtrace, so a disabled reporter costs nothing.
        if !self.inner.store.is_enabled() {
            return;
        }
        let backtrace = Backtrace::force_capture().to_string();
        let thread = std::thread::current();
        let location = info.location();
        let facts = PanicFacts {
            message: info.payload_as_str().unwrap_or(NON_STRING_PAYLOAD),
            file: location.map(|l| l.file()),
            line: location.map_or(0, |l| l.line()),
            column: location.map_or(0, |l| l.column()),
            thread: thread.name(),
            backtrace: &backtrace,
        };
        if let Err(e) = self.record(facts) {
            // Nowhere else to report it; the panic itself was already printed.
            eprintln!("could not save the crash report: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::store::tests::TempDir;
    use super::*;

    /// The panic hook is process-wide: tests that install one take turns.
    static HOOK: Mutex<()> = Mutex::new(());

    /// Runs `f` on a new thread with `reporter`'s hook installed, then puts the default
    /// hook back.
    fn panic_with_hook(reporter: &CrashReporter, f: impl FnOnce() + Send + 'static) {
        let _turn = HOOK.lock().unwrap_or_else(PoisonError::into_inner);
        reporter.install_panic_hook();
        let joined = std::thread::Builder::new()
            .name("crash-test".into())
            .spawn(f)
            .unwrap()
            .join();
        drop(std::panic::take_hook());
        assert!(joined.is_err(), "the closure was expected to panic");
    }

    #[test]
    fn disabled_hook_writes_nothing() {
        let tmp = TempDir::new("crash-hook");
        let reporter = CrashReporter::new(CrashStore::new(&tmp.0), "0.0.0");
        panic_with_hook(&reporter, || panic!("STARBUCKS charged $4.50"));
        assert!(
            !tmp.0.exists(),
            "crash reports are off by default: nothing may be written"
        );
        assert!(reporter.store().list().unwrap().is_empty());
    }

    #[test]
    fn enabled_hook_saves_a_scrubbed_report() {
        let tmp = TempDir::new("crash-hook");
        let store = CrashStore::new(&tmp.0);
        store.set_enabled(true).unwrap();
        let reporter = CrashReporter::new(store, "1.2.3");
        reporter.set_sensitive_terms(["Blue Bottle"]);
        panic_with_hook(&reporter, || {
            let merchant = "Blue Bottle";
            let amount = "-4.50";
            panic!("hook-test: no category for {merchant} amount {amount} in \"Coffee Shop\"")
        });

        // Other tests in this binary may panic while the hook is installed; find ours.
        let reports: Vec<String> = reporter
            .store()
            .list()
            .unwrap()
            .iter()
            .map(|r| reporter.store().read(&r.id).unwrap())
            .filter(|json| json.contains("hook-test"))
            .collect();
        assert_eq!(reports.len(), 1);
        let json = &reports[0];
        let report: CrashReport = serde_json::from_str(json).unwrap();
        assert_eq!(
            report.message,
            "hook-test: no category for <redacted> amount <n> in \"<redacted>\""
        );
        assert_eq!(report.app_version, "1.2.3");
        assert_eq!(report.thread.as_deref(), Some("crash-test"));
        assert!(
            report
                .location
                .as_deref()
                .is_some_and(|l| l.starts_with("crates/fm-core/src/crash/mod.rs:")),
            "{:?}",
            report.location
        );
        assert!(
            report.backtrace[0]
                .starts_with("fm_core::crash::tests::enabled_hook_saves_a_scrubbed_report"),
            "the backtrace starts at the code that panicked: {:#?}",
            report.backtrace
        );
        for leaked in ["Blue Bottle", "4.50", "Coffee Shop", "/Users/", "/home/"] {
            assert!(!json.contains(leaked), "{leaked:?} leaked into {json}");
        }
    }

    #[test]
    fn known_terms_can_be_cleared() {
        let tmp = TempDir::new("crash-hook");
        let store = CrashStore::new(&tmp.0);
        store.set_enabled(true).unwrap();
        let reporter = CrashReporter::new(store, "0");
        reporter.set_sensitive_terms(["oatmilk"]);
        reporter.clear_sensitive_terms();
        let facts = PanicFacts {
            message: "oatmilk",
            file: None,
            line: 0,
            column: 0,
            thread: None,
            backtrace: "",
        };
        let saved = reporter.record(facts).unwrap().unwrap();
        assert!(
            reporter
                .store()
                .read(&saved.id)
                .unwrap()
                .contains("oatmilk")
        );
    }
}
