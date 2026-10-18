//! The model file, downloaded in the background during first run (T5.5, SPEC
//! decision 34).
//!
//! - **Which model:** [`model_spec`], SPEC decision 48's Qwen3.5-4B, the one place to
//!   change it. [`ANSWER_LEAD`] goes with it.
//! - **How:** a background thread runs [`fm_llm::ensure_model_with`] through the app's
//!   one [`NetGuard`]. A verified file already there is kept; an interrupted download
//!   resumes; size and SHA-256 are checked before the file is used (fm-llm).
//! - **Network:** the download origins ([`fm_llm::DOWNLOAD_ORIGINS`]) are on the
//!   allowlist only while the download runs, and taken off again afterwards, whatever
//!   the outcome, so the network log shows exactly that window (SPEC decision 40).
//! - **No ledger data:** it reads and writes only `<app data>/models`, so it may run
//!   before a ledger exists and while one is locked.
//! - **Quitting** stops it; the partial file is kept and the next start resumes.
//! - **Loading (T4.5):** once the file is verified, [`ModelHost`] loads it, once, and
//!   shares it: the Ask screen and the categoriser (T4.6) each open their own
//!   [`fm_llm::Session`] on it. The status says `ready` only when it is loaded. It is
//!   never kept in a static: ggml's Metal backend aborts at exit while a model is loaded,
//!   so quitting drops it ([`ModelHost::shut_down`]).
//! - **When the model is ready**, [`on_model_ready`] runs. Model categorisation (T4.6)
//!   plugs in there.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use fm_llm::{DownloadError, LlmError, LoadOptions, Model, ModelSpec};
use fm_netguard::{NetGuard, Origin};
use serde::Serialize;

/// The model the app downloads: SPEC decision 48, Qwen3.5-4B Q4_K_M from unsloth's GGUF
/// at a fixed revision, checked by size and SHA-256 (the same file T2.4 measured, in
/// fm-cli's candidates). Change it here only, together with [`ANSWER_LEAD`].
pub fn model_spec() -> ModelSpec {
    ModelSpec {
        id: Cow::Borrowed("qwen3.5-4b-q4_k_m"),
        url: Cow::Borrowed(
            "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf",
        ),
        file_name: Cow::Borrowed("Qwen3.5-4B-Q4_K_M.gguf"),
        size: 2_740_937_888,
        sha256: fm_llm::sha256_hex(
            "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4",
        ),
    }
}

/// What [`model_spec`]'s model writes before its answer: Qwen3.5 thinks first unless its
/// turn starts with an empty thinking block, which is how its chat template renders
/// "thinking off". Every caller of the app's model puts it after the prompt.
pub const ANSWER_LEAD: &str = "<think>\n\n</think>\n\n";

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
    /// Loads the verified file. Without one, `ready` means verified (the download tests).
    host: Option<Arc<ModelHost>>,
}

impl ModelDownload {
    /// The app's download: [`model_spec`] into `models_dir`, from Hugging Face, loaded
    /// into `host` once verified.
    pub fn for_app(models_dir: &Path, net: Arc<NetGuard>, host: Arc<ModelHost>) -> Arc<Self> {
        let origins = fm_llm::DOWNLOAD_ORIGINS
            .iter()
            .map(|o| o.parse().expect("fm-llm's download origins are valid"))
            .collect();
        Self::with_host(model_spec(), models_dir, net, origins, Some(host))
    }

    /// Without a host: `ready` means verified (the download tests, the mock app).
    #[cfg(test)]
    pub fn new(spec: ModelSpec, dir: &Path, net: Arc<NetGuard>, origins: Vec<Origin>) -> Arc<Self> {
        Self::with_host(spec, dir, net, origins, None)
    }

    pub fn with_host(
        spec: ModelSpec,
        dir: &Path,
        net: Arc<NetGuard>,
        origins: Vec<Origin>,
        host: Option<Arc<ModelHost>>,
    ) -> Arc<Self> {
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
            host,
        })
    }

    /// At launch: if a download was started in an earlier run (the file or its partial
    /// is there), check it and load it again, or resume it. A first run waits for the
    /// onboarding screen to start it.
    pub fn resume_if_started(self: &Arc<Self>) {
        let file = self.dir.join(self.spec.file_name.as_ref());
        let partial = self.dir.join(format!("{}.partial", self.spec.file_name));
        if file.exists() || partial.exists() {
            self.start();
        }
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
        let model_id = self.spec.id.to_string();
        // A file already here (a later launch) is loaded straight away: the load checks
        // it (one hash), and nothing goes on the allowlist. Only a file that fails the
        // check is downloaded again.
        let file = self.dir.join(self.spec.file_name.as_ref());
        if let Some(host) = &self.host
            && file.exists()
        {
            self.set(ModelStatus::Downloading {
                model_id: model_id.clone(),
                bytes: self.spec.size,
                total: self.spec.size,
            });
            match host.load(&file, &self.spec) {
                Err(LoadError::BadFile) => {}
                done => return self.loaded(host, done),
            }
        }
        // Only the origins this download added come off again afterwards.
        let added: Vec<Origin> = self
            .origins
            .iter()
            .filter(|o| self.net.allow((*o).clone()))
            .cloned()
            .collect();
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
        let path = match result {
            Ok(path) => path,
            Err(e) => {
                return self.set(ModelStatus::Failed {
                    model_id,
                    code: error_code(&e),
                    message: e.to_string(),
                });
            }
        };
        let Some(host) = &self.host else {
            return self.set(ModelStatus::Ready { model_id });
        };
        // Still `downloading` (at 100%) while it loads: `ready` means usable.
        let done = host.load(&path, &self.spec);
        self.loaded(host, done);
    }

    fn loaded(&self, host: &Arc<ModelHost>, done: Result<(), LoadError>) {
        let model_id = self.spec.id.to_string();
        match done {
            Ok(()) => {
                self.set(ModelStatus::Ready { model_id });
                on_model_ready(host);
            }
            Err(e) => self.set(ModelStatus::Failed {
                model_id,
                code: "load",
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

/// Runs once the model is loaded. Model categorisation (T4.6) of what is still in Needs
/// review starts here.
fn on_model_ready(_host: &Arc<ModelHost>) {}

/// Before the model file is checked and mapped: its folder only the user can open or
/// change (0700), and the file read-only (0444), so nothing writes to it by accident
/// between the check and the map. A download replaces it by rename, which the folder's
/// mode allows.
fn lock_down(file: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(dir) = file.parent() {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o444))?;
    }
    Ok(())
}

/// Why the model didn't load. No ledger data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// The file isn't the model's (size or SHA-256): download it again.
    BadFile,
    /// The app is quitting.
    Quitting,
    /// Already loading.
    Busy,
    /// llama.cpp's reason, or the file couldn't be read.
    Failed(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadFile => f.write_str("the model file doesn't match its checksum"),
            Self::Quitting => f.write_str("the app is quitting"),
            Self::Busy => f.write_str("the model is already loading"),
            Self::Failed(why) => f.write_str(why),
        }
    }
}

/// The loaded model, shared by everything that uses it (T4.5).
///
/// - **Loaded once**, after the download is verified ([`ModelDownload`]); [`Self::model`]
///   hands out `Arc` clones.
/// - **Sessions:** each user opens its own [`fm_llm::Session`] on its clone. llama.cpp
///   allows several contexts on one model at the same time (fm-llm serialises only
///   their creation), but on an 8 GB machine two would share memory and cores, so the
///   Ask screen goes first: it holds [`Self::interactive`] while it runs, and background
///   work checks [`Self::interactive_pending`] between items and waits with
///   [`Self::wait_for_interactive`], keeping its own session.
/// - **CPU-only machines** load without GPU layers and fm-llm turns flash attention off
///   for them (T2.4); `options` only says whether to try the GPU.
/// - **Quitting:** [`Self::shut_down`] after every user has dropped its clone.
pub struct ModelHost {
    options: LoadOptions,
    /// Set on quitting: a load in progress stops (llama.cpp's progress callback).
    abort: Arc<AtomicBool>,
    state: Mutex<Hosted>,
    /// Signalled when `state` changes or an interactive call ends.
    changed: Condvar,
    interactive: AtomicUsize,
}

#[derive(Default)]
struct Hosted {
    model: Option<Arc<Model>>,
    loading: bool,
    closed: bool,
}

/// Held while an interactive call (a question) uses the model.
pub struct Interactive {
    host: Arc<ModelHost>,
}

impl Drop for Interactive {
    fn drop(&mut self) {
        self.host.interactive.fetch_sub(1, Ordering::SeqCst);
        let _state = self.host.lock();
        self.host.changed.notify_all();
    }
}

impl ModelHost {
    /// The app's host: the GPU when there is one.
    pub fn for_app() -> Arc<Self> {
        Self::new(LoadOptions::default())
    }

    pub fn new(options: LoadOptions) -> Arc<Self> {
        Arc::new(Self {
            options,
            abort: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(Hosted::default()),
            changed: Condvar::new(),
            interactive: AtomicUsize::new(0),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Hosted> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Loads the file at `path` after checking it against `spec` (size and SHA-256),
    /// unless that model is already loaded; quitting stops it
    /// ([`fm_llm::Model::load_cancellable`]).
    pub fn load(&self, path: &Path, spec: &ModelSpec) -> Result<(), LoadError> {
        {
            let mut state = self.lock();
            if state.closed {
                return Err(LoadError::Quitting);
            }
            if state.model.as_ref().is_some_and(|m| m.id() == spec.id) {
                return Ok(());
            }
            if state.loading {
                return Err(LoadError::Busy);
            }
            state.loading = true;
        }
        // Mapped, not read in: reading the weights in costs ~3.8 GB that can't be released
        // (over decision 48's 8 GB budget); what mapping leaves is a window from the check
        // to the map, which needs local write access, narrowed by the modes set here.
        let loaded = lock_down(path)
            .map_err(|e| LlmError::Runtime(e.to_string()))
            .and_then(|()| {
                let abort = Arc::clone(&self.abort);
                let cancel = fm_llm::Cancel::new(move || abort.load(Ordering::SeqCst));
                Model::load_cancellable(path, spec, &self.options, &cancel)
            });
        let mut state = self.lock();
        state.loading = false;
        let out = match loaded {
            // Quitting while it loaded: dropped here, before the process exits.
            Ok(_) | Err(LlmError::Cancelled) if state.closed => Err(LoadError::Quitting),
            Ok(model) => {
                state.model = Some(Arc::new(model));
                Ok(())
            }
            Err(LlmError::File(
                DownloadError::WrongSize { .. } | DownloadError::WrongHash { .. },
            )) => Err(LoadError::BadFile),
            Err(e) => Err(LoadError::Failed(e.to_string())),
        };
        self.changed.notify_all();
        out
    }

    /// The loaded model, or `None` before it is loaded and after quitting.
    pub fn model(&self) -> Option<Arc<Model>> {
        self.lock().model.clone()
    }

    pub fn is_ready(&self) -> bool {
        self.lock().model.is_some()
    }

    /// Marks an interactive call until the guard drops (see the type's docs).
    pub fn interactive(self: &Arc<Self>) -> Interactive {
        self.interactive.fetch_add(1, Ordering::SeqCst);
        Interactive {
            host: Arc::clone(self),
        }
    }

    /// Whether an interactive call is running or waiting: background work pauses.
    #[allow(
        dead_code,
        reason = "T4.6's categoriser checks it between transactions"
    )]
    pub fn interactive_pending(&self) -> bool {
        self.interactive.load(Ordering::SeqCst) > 0
    }

    /// Waits until no interactive call is pending, or `stop` is set.
    #[allow(dead_code, reason = "T4.6's categoriser waits for Ask")]
    pub fn wait_for_interactive(&self, stop: &AtomicBool) {
        let mut state = self.lock();
        while self.interactive_pending() && !stop.load(Ordering::SeqCst) {
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Quitting: no new loads, a load in progress is stopped and waited for (never left
    /// running into the process's exit), and the host's model is dropped. Callers stop
    /// their own users first, so this drops the last clone.
    pub fn shut_down(&self) {
        self.abort.store(true, Ordering::SeqCst);
        let mut state = self.lock();
        state.closed = true;
        while state.loading {
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        let model = state.model.take();
        drop(state);
        drop(model);
    }
}

/// Emitted each time the model is ready and the ledger is unlocked: once after loading
/// if the ledger is open, and once after each unlock. Background model work (T4.6)
/// starts from it.
pub const READY_EVENT: &str = "model://ready";

/// How often [`watch_ready`] looks.
const READY_TICK: Duration = Duration::from_secs(1);

/// Decides when [`READY_EVENT`] fires: once per unlocked session of the ledger (its
/// generation) while the model is ready.
#[derive(Debug, Default)]
pub struct ReadyEdge {
    fired_for: Option<u64>,
}

impl ReadyEdge {
    /// Whether to fire, given whether the model is ready and the ledger's generation if
    /// it is unlocked (`None` while locked).
    pub fn step(&mut self, ready: bool, unlocked: Option<u64>) -> bool {
        match unlocked {
            Some(generation) if ready && self.fired_for != Some(generation) => {
                self.fired_for = Some(generation);
                true
            }
            _ => false,
        }
    }

    /// Whether it has already fired for this generation (nothing to ask the ledger).
    fn fired_for(&self, generation: u64) -> bool {
        self.fired_for == Some(generation)
    }
}

/// Watches for the model being ready while the ledger is unlocked, and emits
/// [`READY_EVENT`] (see [`ReadyEdge`]). Stops when the host shuts down. It holds no
/// model between looks.
pub fn watch_ready<R: tauri::Runtime>(app: tauri::AppHandle<R>, host: Arc<ModelHost>) {
    use tauri::{Emitter, Manager};
    let watch = move || {
        let mut edge = ReadyEdge::default();
        while !host.lock().closed {
            let unlock = app.state::<crate::unlock::AppUnlock>();
            let before = unlock.generation();
            let ready = host.is_ready();
            // The session lock is taken only when the answer could fire the event.
            let unlocked = (ready && !edge.fired_for(before) && unlock.is_unlocked())
                .then(|| unlock.generation())
                .filter(|after| *after == before);
            if edge.step(ready, unlocked) {
                let _ = app.emit(READY_EVENT, ());
            }
            std::thread::sleep(READY_TICK);
        }
    };
    // Without the watcher only the event is missing; the Ask screen asks the host.
    let _ = std::thread::Builder::new()
        .name("model-ready".into())
        .spawn(watch);
}

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

    /// The status once the download, its check and the load are over (all of them
    /// report `downloading`). Generous: on CI's macOS VM llama.cpp's backend takes
    /// ~23 s to start (its Metal shaders compile on a virtual GPU) before a load can
    /// fail.
    fn wait_until_settled(d: &ModelDownload) -> ModelStatus {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let s = d.status();
            if !matches!(s, ModelStatus::Downloading { .. }) {
                return s;
            }
            assert!(
                Instant::now() < deadline,
                "still downloading or loading after 120 s"
            );
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

    /// Decision 48's file, pinned to a revision on an allowed origin.
    #[test]
    fn the_app_model_comes_from_one_place() {
        let spec = model_spec();
        assert_eq!(spec.id, "qwen3.5-4b-q4_k_m");
        assert!(
            spec.url
                .starts_with("https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/")
        );
        assert!(
            spec.url
                .contains("/e87f176479d0855a907a41277aca2f8ee7a09523/")
        );
        assert!(spec.url.ends_with(spec.file_name.as_ref()));
        assert_eq!(spec.size, 2_740_937_888);
        assert_eq!(
            fm_llm::hex(&spec.sha256),
            "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4"
        );
        assert!(ANSWER_LEAD.starts_with("<think>") && ANSWER_LEAD.ends_with("\n\n"));
        let d = ModelDownload::for_app(
            Path::new("/nonexistent"),
            Arc::new(NetGuard::new(Allowlist::new())),
            ModelHost::for_app(),
        );
        assert_eq!(d.origins.len(), fm_llm::DOWNLOAD_ORIGINS.len());
    }

    /// A download with a host is `ready` only once loaded; a file that isn't a model
    /// fails as `load`, and nothing is loaded.
    #[test]
    fn ready_means_loaded() {
        let body = vec![3u8; 5_000];
        let origin = serve(body.clone());
        let net = Arc::new(NetGuard::new(Allowlist::new()));
        let dir = tempfile::tempdir().unwrap();
        let host = ModelHost::new(LoadOptions { gpu: false });
        let d = ModelDownload::with_host(
            spec(&origin, &body, Sha256::digest(&body).into()),
            dir.path(),
            net,
            vec![origin.clone()],
            Some(Arc::clone(&host)),
        );
        d.start();
        let ModelStatus::Failed { code, .. } = wait_until_settled(&d) else {
            panic!("a file that isn't a model can't be ready")
        };
        assert_eq!(code, "load");
        assert!(!host.is_ready() && host.model().is_none());
        // Resuming at the next launch finds the file and tries again.
        // A file already here is loaded without the network: no request, nothing
        // allowed.
        let quiet = Arc::new(NetGuard::new(Allowlist::new()));
        let again = ModelDownload::with_host(
            d.spec.clone(),
            dir.path(),
            Arc::clone(&quiet),
            vec![serve(body.clone())],
            Some(Arc::clone(&host)),
        );
        again.resume_if_started();
        assert!(matches!(
            wait_until_settled(&again),
            ModelStatus::Failed { code: "load", .. }
        ));
        assert!(quiet.log().is_empty() && quiet.allowlist().is_empty());
        // Checked and loaded with the folder 0700 and the file 0444.
        let mode = |p: &Path| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(p).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode(dir.path()), 0o700);
        assert_eq!(mode(&dir.path().join("model.gguf")), 0o444);
        // A file that fails the check is downloaded again (and then loaded).
        std::fs::remove_file(dir.path().join("model.gguf")).unwrap();
        std::fs::write(dir.path().join("model.gguf"), b"not the model").unwrap();
        let net = Arc::new(NetGuard::new(Allowlist::new()));
        let fixed = ModelDownload::with_host(
            d.spec.clone(),
            dir.path(),
            Arc::clone(&net),
            vec![origin],
            Some(Arc::clone(&host)),
        );
        fixed.resume_if_started();
        assert!(matches!(
            wait_until_settled(&fixed),
            ModelStatus::Failed { code: "load", .. }
        ));
        assert_eq!(std::fs::read(dir.path().join("model.gguf")).unwrap(), body);
        assert!(!net.log().is_empty(), "it was downloaded again");
        // Nothing started before: nothing resumes.
        let fresh = tempfile::tempdir().unwrap();
        let none = ModelDownload::new(
            d.spec.clone(),
            fresh.path(),
            Arc::new(NetGuard::new(Allowlist::new())),
            vec![],
        );
        none.resume_if_started();
        assert!(matches!(none.status(), ModelStatus::Missing { .. }));
    }

    #[test]
    fn ask_goes_first_and_background_work_waits_for_it() {
        let host = ModelHost::new(LoadOptions { gpu: false });
        let stop = AtomicBool::new(false);
        assert!(!host.interactive_pending());
        host.wait_for_interactive(&stop); // returns at once
        let turn = host.interactive();
        assert!(host.interactive_pending());
        let waiter = {
            let host = Arc::clone(&host);
            std::thread::spawn(move || {
                let stop = AtomicBool::new(false);
                host.wait_for_interactive(&stop);
                Instant::now()
            })
        };
        std::thread::sleep(Duration::from_millis(150));
        let released = Instant::now();
        drop(turn);
        assert!(
            waiter.join().unwrap() >= released,
            "it waited for the question"
        );
        assert!(!host.interactive_pending());
        // `stop` ends the wait even while a question runs.
        let _turn = host.interactive();
        stop.store(true, Ordering::SeqCst);
        host.wait_for_interactive(&stop);
    }

    #[test]
    fn after_shut_down_nothing_loads() {
        let host = ModelHost::new(LoadOptions { gpu: false });
        host.shut_down();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.gguf");
        std::fs::write(&path, b"x").unwrap();
        assert_eq!(
            host.load(&path, &fm_llm::CI_TEST_MODEL).unwrap_err(),
            LoadError::Quitting
        );
        assert!(host.model().is_none());
    }

    #[test]
    fn ready_fires_once_per_unlocked_session() {
        let mut edge = ReadyEdge::default();
        assert!(!edge.step(false, Some(1)), "not loaded yet");
        assert!(!edge.step(true, None), "locked");
        assert!(edge.step(true, Some(1)), "loaded and unlocked");
        assert!(!edge.step(true, Some(1)), "only once");
        assert!(!edge.step(true, None), "locked again");
        assert!(edge.step(true, Some(3)), "the next unlock");
        assert!(edge.fired_for(3) && !edge.fired_for(1));
    }
}
