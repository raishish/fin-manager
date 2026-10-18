//! The model file, downloaded in the background during first run (T5.5, SPEC
//! decision 34).
//!
//! - **Which model:** [`model_spec`], the one place to change when T2.4 picks the
//!   product model. Until then it is fm-llm's small CI test model.
//! - **How:** a background thread runs [`fm_llm::ensure_model_with`] through the app's
//!   one [`NetGuard`]. A verified file already there is kept; an interrupted download
//!   resumes; size and SHA-256 are checked before the file is used (fm-llm).
//! - **Network:** the download origins ([`fm_llm::DOWNLOAD_ORIGINS`]) are on the
//!   allowlist only while the download runs, and taken off again afterwards, whatever
//!   the outcome, so the network log shows exactly that window (SPEC decision 40).
//! - **No ledger data:** it reads and writes only `<app data>/models`, so it may run
//!   before a ledger exists and while one is locked.
//! - **Quitting** stops it; the partial file is kept and the next start resumes.
//! - **When the model is ready**, [`on_model_ready`] runs. Model categorisation (T2.5)
//!   plugs in there once it lands.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use fm_llm::{DownloadError, ModelSpec};
use fm_netguard::{NetGuard, Origin};
use serde::Serialize;

/// The model the app downloads. T2.4 picks the product model; until then the small CI
/// test model stands in. Change it here only.
pub fn model_spec() -> ModelSpec {
    fm_llm::CI_TEST_MODEL
}

/// What the UI shows about the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ModelStatus {
    /// Not started in this run of the app.
    Missing {
        model_id: String,
        total: u64,
    },
    /// Downloading, or checking a file already there: `bytes` of `total` so far.
    Downloading {
        model_id: String,
        bytes: u64,
        total: u64,
    },
    Ready {
        model_id: String,
    },
    Failed {
        model_id: String,
        /// `blocked`, `network`, `status`, `verify`, `stopped`, `busy` (another instance is
        /// downloading it) or `io`.
        code: &'static str,
        message: String,
    },
}

/// Runs and tracks the download. Shared by the commands and the download thread.
pub struct ModelDownload {
    spec: ModelSpec,
    dir: PathBuf,
    net: Arc<NetGuard>,
    /// Allowlisted only while the download runs.
    origins: Vec<Origin>,
    status: Mutex<ModelStatus>,
    stop: AtomicBool,
}

impl ModelDownload {
    /// The app's download: [`model_spec`] into `models_dir`, from Hugging Face.
    pub fn for_app(models_dir: &Path, net: Arc<NetGuard>) -> Arc<Self> {
        let origins = fm_llm::DOWNLOAD_ORIGINS
            .iter()
            .map(|o| o.parse().expect("fm-llm's download origins are valid"))
            .collect();
        Self::new(model_spec(), models_dir, net, origins)
    }

    pub fn new(spec: ModelSpec, dir: &Path, net: Arc<NetGuard>, origins: Vec<Origin>) -> Arc<Self> {
        let status = ModelStatus::Missing {
            model_id: spec.id.to_string(),
            total: spec.size,
        };
        Arc::new(Self {
            spec,
            dir: dir.to_path_buf(),
            net,
            origins,
            status: Mutex::new(status),
            stop: AtomicBool::new(false),
        })
    }

    pub fn status(&self) -> ModelStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set(&self, status: ModelStatus) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status;
    }

    /// Starts the download in the background unless it is running or done. Returns the
    /// status right after.
    pub fn start(self: &Arc<Self>) -> ModelStatus {
        {
            let mut status = self.status.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(
                *status,
                ModelStatus::Downloading { .. } | ModelStatus::Ready { .. }
            ) {
                return status.clone();
            }
            *status = ModelStatus::Downloading {
                model_id: self.spec.id.to_string(),
                bytes: 0,
                total: self.spec.size,
            };
        }
        self.stop.store(false, Ordering::SeqCst);
        let this = Arc::clone(self);
        let started = std::thread::Builder::new()
            .name("model-download".into())
            .spawn(move || this.run());
        if let Err(e) = started {
            self.set(ModelStatus::Failed {
                model_id: self.spec.id.to_string(),
                code: "io",
                message: e.to_string(),
            });
        }
        self.status()
    }

    /// Stops a running download at its next chunk; the partial file is kept.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    fn run(&self) {
        // Only the origins this download added come off again afterwards.
        let added: Vec<Origin> = self
            .origins
            .iter()
            .filter(|o| self.net.allow((*o).clone()))
            .cloned()
            .collect();
        let model_id = self.spec.id.to_string();
        let total = self.spec.size;
        let result = fm_llm::ensure_model_with(&self.net, &self.spec, &self.dir, &mut |bytes| {
            self.set(ModelStatus::Downloading {
                model_id: model_id.clone(),
                bytes,
                total,
            });
            !self.stop.load(Ordering::SeqCst)
        });
        for origin in &added {
            self.net.revoke(origin);
        }
        match result {
            Ok(path) => {
                self.set(ModelStatus::Ready {
                    model_id: model_id.clone(),
                });
                on_model_ready(&path);
            }
            Err(e) => self.set(ModelStatus::Failed {
                model_id,
                code: error_code(&e),
                message: e.to_string(),
            }),
        }
    }
}

fn error_code(e: &DownloadError) -> &'static str {
    match e {
        DownloadError::Blocked { .. } => "blocked",
        DownloadError::Network(_) => "network",
        DownloadError::Status(_) => "status",
        DownloadError::WrongSize { .. } | DownloadError::WrongHash { .. } => "verify",
        DownloadError::Cancelled => "stopped",
        DownloadError::Busy => "busy",
        DownloadError::SymlinkedPartial | DownloadError::BadFileName | DownloadError::Io(_) => "io",
    }
}

/// Runs once the verified model file is in place. Model categorisation (T2.5) of what
/// is still in Needs review starts here when it lands; until then the UI only says the
/// model is ready.
fn on_model_ready(_path: &Path) {}

pub mod commands {
    use std::sync::Arc;

    use tauri::State;

    use super::{ModelDownload, ModelStatus};

    #[tauri::command]
    pub fn model_status(model: State<'_, Arc<ModelDownload>>) -> ModelStatus {
        model.status()
    }

    /// Idempotent: a running or finished download is left alone.
    #[tauri::command]
    pub fn start_model_download(model: State<'_, Arc<ModelDownload>>) -> ModelStatus {
        model.start()
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    use fm_netguard::Allowlist;
    use sha2::{Digest, Sha256};

    use super::*;

    /// A loopback server sending `body` with 200 to every request.
    fn serve(body: Vec<u8>) -> Origin {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                    line.clear();
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}").parse().unwrap()
    }

    fn spec(origin: &Origin, body: &[u8], sha256: [u8; 32]) -> ModelSpec {
        ModelSpec {
            id: Cow::Borrowed("test-model"),
            url: Cow::Owned(format!("{origin}/model.gguf")),
            file_name: Cow::Borrowed("model.gguf"),
            size: body.len() as u64,
            sha256,
        }
    }

    fn wait_until_settled(d: &ModelDownload) -> ModelStatus {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let s = d.status();
            if !matches!(s, ModelStatus::Downloading { .. }) || Instant::now() > deadline {
                return s;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The download origin is on the allowlist only while the download runs, and the
    /// request shows in the network log.
    #[test]
    fn downloads_in_the_background_and_closes_the_allowlist_after() {
        let body = vec![7u8; 200_000];
        let origin = serve(body.clone());
        let net = Arc::new(NetGuard::new(Allowlist::new()));
        let dir = tempfile::tempdir().unwrap();
        let d = ModelDownload::new(
            spec(&origin, &body, Sha256::digest(&body).into()),
            dir.path(),
            Arc::clone(&net),
            vec![origin.clone()],
        );
        assert!(matches!(
            d.status(),
            ModelStatus::Missing { total: 200_000, .. }
        ));
        assert!(matches!(d.start(), ModelStatus::Downloading { .. }));
        assert!(
            matches!(
                d.start(),
                ModelStatus::Downloading { .. } | ModelStatus::Ready { .. }
            ),
            "starting again while it runs changes nothing"
        );
        assert_eq!(
            wait_until_settled(&d),
            ModelStatus::Ready {
                model_id: "test-model".into()
            }
        );
        assert_eq!(std::fs::read(dir.path().join("model.gguf")).unwrap(), body);
        assert!(
            net.allowlist().is_empty(),
            "the origin is off the allowlist again"
        );
        let log = net.log();
        assert!(log.iter().all(|e| e.purpose == fm_llm::DOWNLOAD_PURPOSE));
        assert!(!log.is_empty());
        // Ready stays ready, without another request.
        assert!(matches!(d.start(), ModelStatus::Ready { .. }));
        assert_eq!(net.log().len(), log.len());
    }

    #[test]
    fn a_wrong_hash_is_a_failure_and_the_allowlist_still_closes() {
        let body = vec![1u8; 10_000];
        let origin = serve(body.clone());
        let net = Arc::new(NetGuard::new(Allowlist::new()));
        let dir = tempfile::tempdir().unwrap();
        let d = ModelDownload::new(
            spec(&origin, &body, [0; 32]),
            dir.path(),
            Arc::clone(&net),
            vec![origin],
        );
        d.start();
        let ModelStatus::Failed { code, .. } = wait_until_settled(&d) else {
            panic!("expected a failure")
        };
        assert_eq!(code, "verify");
        assert!(!dir.path().join("model.gguf").exists());
        assert!(net.allowlist().is_empty());
    }

    /// An origin the user (or another feature) already allowed stays allowed.
    #[test]
    fn an_origin_allowed_before_stays_allowed() {
        let body = vec![2u8; 1_000];
        let origin = serve(body.clone());
        let mut allowlist = Allowlist::new();
        allowlist.allow(origin.clone());
        let net = Arc::new(NetGuard::new(allowlist));
        let dir = tempfile::tempdir().unwrap();
        let d = ModelDownload::new(
            spec(&origin, &body, Sha256::digest(&body).into()),
            dir.path(),
            Arc::clone(&net),
            vec![origin.clone()],
        );
        d.start();
        assert!(matches!(wait_until_settled(&d), ModelStatus::Ready { .. }));
        assert!(net.allowlist().contains(&origin));
    }

    #[test]
    fn the_app_model_comes_from_one_place() {
        assert_eq!(model_spec(), fm_llm::CI_TEST_MODEL);
        let d = ModelDownload::for_app(
            Path::new("/nonexistent"),
            Arc::new(NetGuard::new(Allowlist::new())),
        );
        assert_eq!(d.origins.len(), fm_llm::DOWNLOAD_ORIGINS.len());
    }
}
