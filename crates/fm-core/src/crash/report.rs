//! What a crash report contains. Only the fields below are ever written, and every free-text
//! field has been through the [`Scrubber`].

use serde::{Deserialize, Serialize};

use super::scrub::{PATH, Scrubber};
use crate::model::Timestamp;

/// Bumped when the report's fields change.
pub const REPORT_SCHEMA: u32 = 1;

/// Longest panic message kept, in bytes, after scrubbing.
pub const MAX_MESSAGE_BYTES: usize = 2048;
/// Most backtrace frames kept.
pub const MAX_FRAMES: usize = 64;
/// Longest single frame kept, in bytes.
pub const MAX_FRAME_BYTES: usize = 300;

/// A scrubbed crash report. Serialised as JSON, it is exactly what the user previews and
/// may send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashReport {
    pub schema: u32,
    pub app_version: String,
    /// `std::env::consts::OS`, e.g. `"macos"`. No OS version, host name or user name.
    pub os: String,
    /// `std::env::consts::ARCH`, e.g. `"aarch64"`.
    pub arch: String,
    pub at: Timestamp,
    /// The panicking thread's name, scrubbed. Threads the app doesn't name have none.
    pub thread: Option<String>,
    /// The panic message, scrubbed.
    pub message: String,
    /// Where the panic happened, `file:line:column`, with the path shortened (see
    /// [`normalize_path`]).
    pub location: Option<String>,
    /// Function names with shortened source locations, innermost first, starting below the
    /// panic machinery.
    pub backtrace: Vec<String>,
}

/// The raw facts about a panic, before scrubbing.
#[derive(Debug, Clone, Copy)]
pub struct PanicFacts<'a> {
    pub message: &'a str,
    pub file: Option<&'a str>,
    pub line: u32,
    pub column: u32,
    pub thread: Option<&'a str>,
    /// `std::backtrace::Backtrace`'s `Display` output.
    pub backtrace: &'a str,
}

impl CrashReport {
    /// Builds a report from a panic, scrubbing every free-text field.
    pub fn from_panic(
        facts: PanicFacts<'_>,
        app_version: &str,
        at: Timestamp,
        scrubber: &Scrubber,
    ) -> Self {
        let mut message = scrubber.scrub(facts.message);
        truncate(&mut message, MAX_MESSAGE_BYTES);
        Self {
            schema: REPORT_SCHEMA,
            app_version: app_version.to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            at,
            thread: facts.thread.map(|t| scrubber.scrub(t)),
            message,
            location: facts
                .file
                .map(|f| format!("{}:{}:{}", normalize_path(f), facts.line, facts.column)),
            backtrace: frames(facts.backtrace, scrubber),
        }
    }

    /// The exact text shown in the preview and written to disk.
    pub fn to_json(&self) -> String {
        // A struct of strings, numbers and options always serialises.
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }
}

/// Shortens a source path so it names the code, not the machine:
/// - workspace files keep their path from `crates/` or `app/src-tauri/` on, and paths
///   relative to a crate (`src/...`, as backtraces print them when run from the crate)
///   stay as they are;
/// - crates.io dependencies become `<registry>/<crate>-<version>/...`;
/// - the standard library becomes `<rust>/library/...`;
/// - anything else keeps only its file name, as `<path>/<file>`.
pub fn normalize_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    // `<index>-<hash>/<crate>-<version>/src/...`: drop the index directory.
    if let Some((_, rest)) = path.split_once("/registry/src/")
        && let Some((_, in_crate)) = rest.split_once('/')
    {
        return format!("<registry>/{in_crate}");
    }
    if let Some((_, rest)) = path.split_once("/rustc/")
        && let Some((_, in_rust)) = rest.split_once('/')
    {
        return format!("<rust>/{in_rust}");
    }
    let relative = path.strip_prefix("./").unwrap_or(&path);
    if relative.starts_with("src/") && !relative.split('/').any(|part| part == "..") {
        return relative.to_string();
    }
    for root in ["crates/", "app/src-tauri/"] {
        if path.starts_with(root) {
            return path;
        }
        if let Some(at) = path.find(&format!("/{root}")) {
            return path[at + 1..].to_string();
        }
    }
    let file = path.rsplit('/').next().unwrap_or_default();
    format!("{PATH}/{file}")
}

/// Turns `Backtrace` display output into `function (location)` lines.
///
/// The format is
/// ```text
///    0: some::function
///              at /path/to/file.rs:12:5
/// ```
/// Function names keep their form; only known terms are removed from them (see
/// [`scrub_symbol`]).
fn frames(backtrace: &str, scrubber: &Scrubber) -> Vec<String> {
    let mut frames: Vec<(String, Option<String>)> = Vec::new();
    for line in backtrace.lines() {
        let line = line.trim();
        if let Some(at) = line.strip_prefix("at ") {
            if let Some(last) = frames.last_mut() {
                last.1 = Some(normalize_location(at));
            }
        } else if let Some((index, symbol)) = line.split_once(": ")
            && !index.is_empty()
            && index.chars().all(|c| c.is_ascii_digit())
        {
            frames.push((symbol.to_string(), None));
        }
    }
    // Skip the backtrace capture and panic machinery above the code that panicked: the
    // first run of machinery frames. Later ones (a `catch_unwind` further down the stack)
    // are kept.
    let start = frames
        .iter()
        .position(|(symbol, _)| is_panic_machinery(symbol))
        .map_or(0, |first| {
            first
                + frames[first..]
                    .iter()
                    .take_while(|(symbol, _)| is_panic_machinery(symbol))
                    .count()
        });
    frames
        .into_iter()
        .skip(start)
        .take(MAX_FRAMES)
        .map(|(symbol, location)| {
            let symbol = scrub_symbol(&symbol, scrubber);
            let mut frame = match location {
                Some(location) => format!("{symbol} ({location})"),
                None => symbol,
            };
            truncate(&mut frame, MAX_FRAME_BYTES);
            frame
        })
        .collect()
}

fn is_panic_machinery(symbol: &str) -> bool {
    [
        "std::panicking::",
        "core::panicking::",
        "std::panic::",
        "rust_begin_unwind",
        "__rustc::rust_begin_unwind",
        "std::backtrace::",
        "std::sys::backtrace::",
        "fm_core::crash::CrashReporter::",
        "<alloc::boxed::Box<F,A> as core::ops::function::Fn<Args>>::call",
    ]
    .iter()
    .any(|p| symbol.starts_with(p) || symbol.contains(&format!(" {p}")))
}

/// Function names are code, so the pattern rules (which would eat `<T>` generics and
/// `::h1234...` hashes) don't apply; only the known terms are removed, in case a closure or
/// type name ever matches one.
fn scrub_symbol(symbol: &str, scrubber: &Scrubber) -> String {
    let without_hash = match symbol.rsplit_once("::h") {
        Some((name, hash)) if hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()) => {
            name
        }
        _ => symbol,
    };
    scrubber.scrub_terms_only(without_hash)
}

/// `/path/to/file.rs:12:5` with the path shortened.
fn normalize_location(location: &str) -> String {
    // Split off `:line:col` from the right; Windows paths have a `:` after the drive.
    let mut parts = location.rsplitn(3, ':');
    let (col, line, path) = (parts.next(), parts.next(), parts.next());
    match (path, line, col) {
        (Some(path), Some(line), Some(col))
            if line.chars().all(|c| c.is_ascii_digit())
                && col.chars().all(|c| c.is_ascii_digit()) =>
        {
            format!("{}:{line}:{col}", normalize_path(path))
        }
        _ => normalize_path(location),
    }
}

/// Cuts `text` to at most `max` bytes on a character boundary, marking the cut with `…`.
fn truncate(text: &mut String, max: usize) {
    const MARK: &str = "…";
    if text.len() <= max {
        return;
    }
    let mut end = max - MARK.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(MARK);
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACKTRACE: &str = "   0: std::backtrace_rs::backtrace::libunwind::trace
             at /rustc/58cf8f9c3abc/library/std/src/../../backtrace/src/backtrace/libunwind.rs:117:9
   1: std::backtrace::Backtrace::create
             at /rustc/58cf8f9c3abc/library/std/src/backtrace.rs:331:13
   2: fm_core::crash::CrashReporter::record
             at ./crates/fm-core/src/crash/mod.rs:80:9
   3: std::panicking::rust_panic_with_hook
             at /rustc/58cf8f9c3abc/library/std/src/panicking.rs:841:13
   4: core::panicking::panic_fmt
             at /rustc/58cf8f9c3abc/library/core/src/panicking.rs:75:14
   5: fm_core::pipeline::import::h0123456789abcdef
             at /Users/jane/src/fin-manager/crates/fm-core/src/pipeline/mod.rs:42:5
   6: serde_json::de::from_str
             at /Users/jane/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde_json-1.0.154/src/de.rs:2699:5
   7: std::panic::catch_unwind
             at /rustc/58cf8f9c3abc/library/std/src/panic.rs:358:14
   8: main
";

    fn facts(message: &str) -> PanicFacts<'_> {
        PanicFacts {
            message,
            file: Some("crates/fm-core/src/pipeline/mod.rs"),
            line: 42,
            column: 5,
            thread: Some("main"),
            backtrace: BACKTRACE,
        }
    }

    #[test]
    fn report_keeps_only_scrubbed_fields() {
        let report = CrashReport::from_panic(
            facts(r#"no category for "STARBUCKS #1234" amount -4.50"#),
            "0.1.0",
            Timestamp::from_unix_seconds(1_792_000_000),
            &Scrubber::new(),
        );
        assert_eq!(report.schema, REPORT_SCHEMA);
        assert_eq!(report.app_version, "0.1.0");
        assert_eq!(report.os, std::env::consts::OS);
        assert_eq!(report.arch, std::env::consts::ARCH);
        assert_eq!(report.thread.as_deref(), Some("main"));
        assert_eq!(report.message, r#"no category for "<redacted>" amount <n>"#);
        assert_eq!(
            report.location.as_deref(),
            Some("crates/fm-core/src/pipeline/mod.rs:42:5")
        );
        let json = report.to_json();
        assert!(!json.contains("STARBUCKS"));
        assert!(!json.contains("4.50"));
        assert!(!json.contains("jane"));
    }

    #[test]
    fn backtrace_starts_below_the_panic_machinery_and_hides_the_machine() {
        let report = CrashReport::from_panic(
            facts("boom"),
            "0.1.0",
            Timestamp::from_unix_seconds(0),
            &Scrubber::new(),
        );
        assert_eq!(
            report.backtrace,
            [
                "fm_core::pipeline::import (crates/fm-core/src/pipeline/mod.rs:42:5)",
                "serde_json::de::from_str (<registry>/serde_json-1.0.154/src/de.rs:2699:5)",
                "std::panic::catch_unwind (<rust>/library/std/src/panic.rs:358:14)",
                "main",
            ]
        );
    }

    #[test]
    fn known_terms_are_removed_from_frames() {
        let mut scrubber = Scrubber::new();
        scrubber.set_terms(["pipeline"]);
        let report =
            CrashReport::from_panic(facts("x"), "0", Timestamp::from_unix_seconds(0), &scrubber);
        assert!(report.backtrace[0].starts_with("fm_core::<redacted>::import ("));
    }

    #[test]
    fn paths_are_shortened() {
        assert_eq!(
            normalize_path("crates/fm-core/src/a.rs"),
            "crates/fm-core/src/a.rs"
        );
        assert_eq!(
            normalize_path("/home/jo/fm/app/src-tauri/src/lib.rs"),
            "app/src-tauri/src/lib.rs"
        );
        assert_eq!(
            normalize_path(
                "/Users/jo/.cargo/registry/src/index.crates.io-abc/tauri-2.12.4/src/app.rs"
            ),
            "<registry>/tauri-2.12.4/src/app.rs"
        );
        assert_eq!(
            normalize_path("/rustc/58cf8f9c3/library/core/src/option.rs"),
            "<rust>/library/core/src/option.rs"
        );
        assert_eq!(normalize_path("./src/crash/mod.rs"), "src/crash/mod.rs");
        assert_eq!(normalize_path("src/../../x/y.rs"), "<path>/y.rs");
        assert_eq!(normalize_path("./Documents/x/y.rs"), "<path>/y.rs");
        assert_eq!(
            normalize_path("/Users/jo/secret/thing.rs"),
            "<path>/thing.rs"
        );
        assert_eq!(normalize_path(r"C:\Users\jo\x\y.rs"), "<path>/y.rs");
    }

    #[test]
    fn long_messages_are_capped() {
        let long = "x".repeat(10_000);
        let report = CrashReport::from_panic(
            facts(&long),
            "0",
            Timestamp::from_unix_seconds(0),
            &Scrubber::new(),
        );
        assert!(report.message.len() <= MAX_MESSAGE_BYTES);
        assert!(report.message.ends_with('…'));
    }

    #[test]
    fn json_round_trips() {
        let report = CrashReport::from_panic(
            facts("boom"),
            "0.1.0",
            Timestamp::from_unix_seconds(5),
            &Scrubber::new(),
        );
        let back: CrashReport = serde_json::from_str(&report.to_json()).unwrap();
        assert_eq!(back, report);
    }
}
