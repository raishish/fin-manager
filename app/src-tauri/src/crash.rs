//! Crash-report settings and preview for the UI (SPEC decisions 21 and 39). The app only
//! reads, shows and deletes reports; it never sends them.

use fm_core::crash::{CrashStore, ReportId};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashSettings {
    pub enabled: bool,
    /// Where reports are saved.
    pub dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedReport {
    pub id: String,
    /// Shown so the user can attach the file themselves.
    pub path: String,
}

pub fn settings(store: &CrashStore) -> CrashSettings {
    CrashSettings {
        enabled: store.is_enabled(),
        dir: store.dir().display().to_string(),
    }
}

pub fn set_enabled(store: &CrashStore, enabled: bool) -> Result<CrashSettings, String> {
    store.set_enabled(enabled).map_err(|e| e.to_string())?;
    Ok(settings(store))
}

pub fn list(store: &CrashStore) -> Result<Vec<SavedReport>, String> {
    Ok(store
        .list()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|r| SavedReport {
            id: r.id.to_string(),
            path: r.path.display().to_string(),
        })
        .collect())
}

/// The report file's exact contents: the preview, and what the user copies.
pub fn read(store: &CrashStore, id: &str) -> Result<String, String> {
    let id: ReportId = id
        .parse()
        .map_err(|e: fm_core::crash::InvalidReportId| e.to_string())?;
    store.read(&id).map_err(|e| e.to_string())
}

pub fn delete(store: &CrashStore, id: &str) -> Result<(), String> {
    let id: ReportId = id
        .parse()
        .map_err(|e: fm_core::crash::InvalidReportId| e.to_string())?;
    store.delete(&id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use fm_core::crash::{CrashReporter, PanicFacts};

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fm-app-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn preview_is_the_saved_file_byte_for_byte() {
        let tmp = TempDir::new("crash-preview");
        let store = CrashStore::new(&tmp.0);
        assert!(!settings(&store).enabled, "off by default");
        assert!(set_enabled(&store, true).unwrap().enabled);

        let reporter = CrashReporter::new(store.clone(), "0.0.0");
        let saved = reporter
            .record(PanicFacts {
                message: "paid $12.00 at \"Corner Deli\"",
                file: Some("app/src-tauri/src/lib.rs"),
                line: 1,
                column: 1,
                thread: Some("main"),
                backtrace: "",
            })
            .unwrap()
            .unwrap();

        let listed = list(&store).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, saved.id.to_string());
        let preview = read(&store, &listed[0].id).unwrap();
        assert_eq!(preview.as_bytes(), std::fs::read(&saved.path).unwrap());
        assert!(!preview.contains("Corner Deli") && !preview.contains("12.00"));

        delete(&store, &listed[0].id).unwrap();
        assert!(list(&store).unwrap().is_empty());
    }

    #[test]
    fn ids_from_the_ui_cannot_name_other_files() {
        let tmp = TempDir::new("crash-ids");
        let store = CrashStore::new(&tmp.0);
        for bad in ["settings", "../secret", "crash-1-2/../../x"] {
            assert!(read(&store, bad).is_err(), "{bad:?}");
            assert!(delete(&store, bad).is_err(), "{bad:?}");
        }
    }
}
