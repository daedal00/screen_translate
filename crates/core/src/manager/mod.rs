//! In-app management of a local llama.cpp runtime, GGUF models and the llama-server process.

pub mod catalog;
pub mod download;
pub mod hardware;
pub mod models;
pub mod runtime;
pub mod server;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::paths::app_data_dir;
use crate::settings::SettingsStore;
use hardware::{Budget, Fit, Hardware, OVERHEAD};
use runtime::Backend;

/// Minimum gap between progress events sent to the Settings window.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// True when a progress event should be emitted: always for the first and the final one
/// (`finished`), otherwise only once `PROGRESS_INTERVAL` has passed since `last`.
pub fn should_emit(last: Option<Instant>, now: Instant, finished: bool) -> bool {
    finished || last.is_none_or(|l| now.saturating_duration_since(l) >= PROGRESS_INTERVAL)
}

/// Path of an installed model to launch: `file` must be a bare `.gguf` name (no separators or `..`)
/// naming an existing regular file in `models_dir`. Errors are user-readable.
pub fn model_path(models_dir: &Path, file: &str) -> Result<PathBuf, String> {
    models::valid_name(file)?;
    let path = models_dir.join(file);
    if !path.is_file() {
        return Err(format!("{file} is not installed"));
    }
    Ok(path)
}

/// Message for a Local-provider request when the stored URL is the managed server's and it isn't Ready;
/// None when the URL is something else (the user's own server) or the managed server is Ready.
/// The URL matches `server::base_url()` ignoring trailing `/` and a trailing `/v1`.
/// Loading -> `MANAGED_LOADING`; Stopped or Error -> `MANAGED_STOPPED`.
pub const MANAGED_LOADING: &str = "The local model is still loading. Try again in a moment.";
pub const MANAGED_STOPPED: &str = "The local model server is not running. Start it in Settings.";

/// True when `local_url` is `server::base_url()`, ignoring surrounding spaces, trailing `/` and a trailing `/v1`.
pub fn is_managed_url(local_url: &str) -> bool {
    let url = local_url.trim().trim_end_matches('/');
    url.strip_suffix("/v1").unwrap_or(url).trim_end_matches('/') == server::base_url()
}

pub fn managed_not_ready(local_url: &str, status: &server::Status) -> Option<&'static str> {
    if !is_managed_url(local_url) {
        return None;
    }
    match status {
        server::Status::Ready { .. } => None,
        server::Status::Loading { .. } => Some(MANAGED_LOADING),
        server::Status::Stopped | server::Status::Error { .. } => Some(MANAGED_STOPPED),
    }
}

const BUSY: &str = "Another download is in progress";

/// Where the manager reports to: server status changes and download progress. Called from any thread.
#[derive(Clone)]
pub struct Events {
    pub status: Arc<dyn Fn(server::Status) + Send + Sync>,
    pub progress: Arc<dyn Fn(ProgressEvent<'_>) + Send + Sync>,
}

pub struct ManagerState {
    pub server: server::Server,
    /// Cancel flag of the running download job and whether it is a runtime install; at most one at a time.
    job: Mutex<Option<(Arc<AtomicBool>, bool)>>,
    client: reqwest::Client,
    /// Detected on first use (it spawns a few commands), then kept.
    hardware: OnceLock<Hardware>,
    /// Set when a CUDA start failed and the Vulkan build took over; cleared by the next user-started server.
    fallback_note: Mutex<Option<String>>,
}

impl Default for ManagerState {
    fn default() -> Self {
        Self {
            server: server::Server::new(),
            job: Mutex::new(None),
            client: download::client(),
            hardware: OnceLock::new(),
            fallback_note: Mutex::new(None),
        }
    }
}

impl ManagerState {
    fn hardware(&self) -> &Hardware {
        self.hardware.get_or_init(hardware::detect)
    }
}

const FALLBACK_NOTE: &str = "The CUDA build failed to start, switched to Vulkan.";
const FALLBACK_RUNNING: &str = "The CUDA build failed to start. Installing the Vulkan build...";
const FALLBACK_BUSY: &str = "The CUDA build failed to start, and the Vulkan build could not be installed because another download is running.";

pub fn set_note(state: &ManagerState, note: Option<&str>) {
    if let Ok(mut n) = state.fallback_note.lock() {
        *n = note.map(String::from);
    }
}

/// Starts detecting the hardware in the background so Settings and installs find it ready.
pub fn warm_hardware(state: &Arc<ManagerState>) {
    let state = state.clone();
    std::thread::spawn(move || {
        state.hardware();
    });
}

/// How `file` (`size` bytes) fits `b`: a catalog file uses the catalog's own requirement, any other file its size plus `OVERHEAD`.
fn fit_of(file: &str, size: u64, b: &Budget) -> Fit {
    hardware::fit(
        catalog::entries()
            .iter()
            .find(|e| e.file == file)
            .map_or(size + OVERHEAD, catalog::required),
        b,
    )
}

/// `<runtime_dir>/backend.json`: which build was installed, and for which GPU.
#[derive(serde::Serialize, serde::Deserialize)]
struct InstalledBackend {
    backend: Backend,
    gpu: Option<String>,
}

fn backend_file() -> Result<PathBuf, String> {
    Ok(runtime::runtime_dir(&data_dir()?).join("backend.json"))
}

/// What `backend.json` says, only when `exe` is the llama-server this app installed (under `<runtime_dir>/bin`):
/// a llama-server from PATH, or a missing or unreadable file, gives None.
fn installed_backend(exe: &Path) -> Option<InstalledBackend> {
    let runtime = runtime::runtime_dir(&data_dir().ok()?);
    if !exe.starts_with(runtime.join("bin")) {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(runtime.join("backend.json")).ok()?).ok()
}

/// Holds the single job slot; frees it on drop.
struct Job<'a> {
    state: &'a ManagerState,
    cancel: Arc<AtomicBool>,
}

impl<'a> Job<'a> {
    fn begin(state: &'a ManagerState, runtime: bool) -> Result<Self, String> {
        let mut slot = state.job.lock().map_err(|e| e.to_string())?;
        if slot.is_some() {
            return Err(BUSY.into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *slot = Some((cancel.clone(), runtime));
        Ok(Self { state, cancel })
    }
}

impl Drop for Job<'_> {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.state.job.lock() {
            *slot = None;
        }
    }
}

#[derive(Clone, serde::Serialize)]
pub struct ProgressEvent<'a> {
    pub job: &'a str,
    pub file: &'a str,
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Overall progress across all files of one job, emitted throttled.
struct Tracker<'a> {
    events: &'a Events,
    job: &'static str,
    total: Option<u64>,
    per_file: HashMap<String, u64>,
    last: Option<Instant>,
}

impl<'a> Tracker<'a> {
    fn new(
        events: &'a Events,
        job: &'static str,
        sizes: impl Iterator<Item = Option<u64>>,
    ) -> Self {
        let total = sizes.sum::<Option<u64>>();
        Self {
            events,
            job,
            total,
            per_file: HashMap::new(),
            last: None,
        }
    }

    fn update(&mut self, file: &str, p: download::Progress) {
        self.per_file.insert(file.to_string(), p.downloaded);
        let downloaded = self.per_file.values().sum();
        let now = Instant::now();
        if should_emit(self.last, now, self.total == Some(downloaded)) {
            self.last = Some(now);
            self.emit(file, downloaded);
        }
    }

    fn emit(&self, file: &str, downloaded: u64) {
        (self.events.progress)(ProgressEvent {
            job: self.job,
            file,
            downloaded,
            total: self.total,
        });
    }

    /// The final event, so the UI always ends on 100%.
    fn finish(&self, file: &str) {
        self.emit(
            file,
            self.total.unwrap_or_else(|| self.per_file.values().sum()),
        );
    }
}

fn data_dir() -> Result<PathBuf, String> {
    app_data_dir().ok_or_else(|| "cannot find the app data directory".to_string())
}

fn find_runtime() -> Option<PathBuf> {
    let dir = runtime::runtime_dir(&data_dir().ok()?);
    runtime::find_server(
        &dir,
        std::env::var_os("PATH").as_deref(),
        std::env::consts::OS,
    )
}

fn running_model(state: &ManagerState) -> Option<String> {
    match state.server.status() {
        server::Status::Loading { model } | server::Status::Ready { model, .. } => Some(model),
        _ => None,
    }
}

#[derive(serde::Serialize)]
pub struct CatalogRow {
    #[serde(flatten)]
    pub entry: &'static catalog::Entry,
    pub fit: Fit,
}

#[derive(serde::Serialize)]
pub struct InstalledRow {
    #[serde(flatten)]
    pub model: models::InstalledModel,
    pub fit: Fit,
}

#[derive(serde::Serialize)]
pub struct BackendInfo {
    pub recommended: Backend,
    pub installed: Option<Backend>,
    /// GPU name recorded at install time.
    pub installed_gpu: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ManagerInfo {
    pub runtime: Option<String>,
    pub os: &'static str,
    pub hardware: Hardware,
    pub budget: Budget,
    pub recommended: Vec<catalog::Recommendation>,
    pub catalog: Vec<CatalogRow>,
    pub backend: BackendInfo,
    pub fallback_note: Option<String>,
    pub installed: Vec<InstalledRow>,
    pub server: server::Status,
    pub base_url: String,
    pub autostart: bool,
    pub selected_model: Option<String>,
    pub downloading: bool,
}

pub fn status(state: &ManagerState, settings: &SettingsStore) -> Result<ManagerInfo, String> {
    let dir = data_dir()?;
    let installed = models::list_installed(&models::models_dir(&dir));
    let hw = state.hardware();
    let budget = hardware::budget(hw);
    let exe = find_runtime();
    let installed_backend = exe.as_deref().and_then(installed_backend);
    Ok(ManagerInfo {
        runtime: exe.as_ref().map(|p| p.to_string_lossy().into_owned()),
        os: std::env::consts::OS,
        hardware: hw.clone(),
        budget,
        recommended: catalog::recommend(catalog::entries(), &budget, 3),
        catalog: catalog::entries()
            .iter()
            .map(|entry| CatalogRow {
                entry,
                fit: hardware::fit(catalog::required(entry), &budget),
            })
            .collect(),
        backend: BackendInfo {
            recommended: hardware::choose_backend(hw.gpu.as_ref()),
            installed: installed_backend.as_ref().map(|b| b.backend),
            installed_gpu: installed_backend.and_then(|b| b.gpu),
        },
        fallback_note: state.fallback_note.lock().ok().and_then(|n| n.clone()),
        installed: installed
            .iter()
            .map(|m| InstalledRow {
                fit: fit_of(&m.file, m.size, &budget),
                model: m.clone(),
            })
            .collect(),
        server: state.server.status(),
        base_url: server::base_url(),
        autostart: settings.managed_autostart(),
        selected_model: settings
            .managed_model()
            .filter(|m| installed.iter().any(|i| &i.file == m)),
        downloading: state.job.lock().is_ok_and(|j| j.is_some()),
    })
}

/// Installs llama.cpp for `backend` (None: the one `choose_backend` picks for this GPU) and records it in `backend.json`.
pub async fn install_backend(
    state: &ManagerState,
    events: &Events,
    backend: Option<Backend>,
) -> Result<String, String> {
    if running_model(state).is_some() {
        return Err("Stop the server before reinstalling llama.cpp".into());
    }
    let job = Job::begin(state, true)?;
    let dir = data_dir()?;
    let client = state.client.clone();
    let gpu = state.hardware().gpu.as_ref();
    let backend = backend.unwrap_or_else(|| hardware::choose_backend(gpu));
    let releases = runtime::fetch_release(&client, runtime::RELEASES_API).await?;
    let assets = runtime::pick_release(
        &releases,
        std::env::consts::OS,
        std::env::consts::ARCH,
        backend,
        gpu.and_then(|g| g.max_cuda),
    )?;
    let mut tracker = Tracker::new(events, "runtime", assets.iter().map(|a| Some(a.size)));
    let exe = runtime::install_assets(
        &client,
        &runtime::runtime_dir(&dir),
        &assets,
        &job.cancel,
        |name, p| tracker.update(name, p),
    )
    .await?;
    tracker.finish("llama.cpp");
    let record = InstalledBackend {
        backend,
        gpu: gpu.map(|g| g.name.clone()),
    };
    if let Err(e) = serde_json::to_string(&record)
        .map_err(|e| e.to_string())
        .and_then(|j| std::fs::write(backend_file()?, j).map_err(|e| e.to_string()))
    {
        log::warn!("Cannot record the installed backend: {e}");
    }
    Ok(exe.to_string_lossy().into_owned())
}

/// Downloads `files` into the models folder under `job`, reporting progress; returns the first file's name.
async fn download_to_models(
    state: &ManagerState,
    events: &Events,
    job: &Job<'_>,
    files: &[models::RemoteFile],
) -> Result<String, String> {
    let dir = models::models_dir(&data_dir()?);
    let client = state.client.clone();
    for f in files {
        download::require_https(&f.url)?;
    }
    let mut tracker = Tracker::new(events, "model", files.iter().map(|f| f.size));
    let name = models::download_files(&client, &dir, files, &job.cancel, |file, p| {
        tracker.update(file, p)
    })
    .await?;
    tracker.finish(&name);
    Ok(name)
}

pub async fn download_model(
    state: &ManagerState,
    events: &Events,
    source: &str,
) -> Result<String, String> {
    let job = Job::begin(state, false)?;
    let client = state.client.clone();
    let files = models::resolve(&client, models::HF_BASE, &models::parse_source(source)?).await?;
    download_to_models(state, events, &job, &files).await
}

/// Downloads catalog entry `id` (its pinned file and sha256 from `catalog.json`).
pub async fn download_catalog(
    state: &ManagerState,
    events: &Events,
    id: &str,
) -> Result<String, String> {
    let entry = catalog::entries()
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| format!("Unknown model {id}"))?;
    let job = Job::begin(state, false)?;
    let file = models::RemoteFile {
        name: entry.file.clone(),
        url: format!(
            "{}/{}/resolve/main/{}",
            models::HF_BASE,
            entry.repo,
            entry.file
        ),
        size: Some(entry.size),
        sha256: entry.sha256.clone(),
    };
    download_to_models(state, events, &job, &[file]).await
}

/// Searches Hugging Face for GGUF repos; an empty query gives an empty list.
pub async fn hf_search(state: &ManagerState, query: &str) -> Result<Vec<models::HfRepo>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let client = state.client.clone();
    let offline = |_| models::OFFLINE.to_string();
    let resp = client
        .get(models::search_url(models::HF_BASE, query))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(offline)?;
    if !resp.status().is_success() {
        return Err(format!(
            "Hugging Face returned HTTP {}",
            resp.status().as_u16()
        ));
    }
    models::parse_search(&resp.text().await.map_err(offline)?)
}

#[derive(serde::Serialize)]
pub struct QuantRow {
    #[serde(flatten)]
    pub file: models::QuantFile,
    pub fit: Fit,
}

/// The quantizations of `repo` (`owner/name`) with how well each fits this machine. Download a row with
/// `download_model("owner/name:<row.name>")`.
pub async fn hf_files(state: &ManagerState, repo: &str) -> Result<Vec<QuantRow>, String> {
    let invalid = || format!("Expected a repository like owner/name, got {repo:?}");
    let Ok(models::Source::Hf { repo, .. }) = models::parse_source(&format!("{repo}:x")) else {
        return Err(invalid());
    };
    let tree = models::fetch_tree(&state.client, models::HF_BASE, &repo).await?;
    let budget = hardware::budget(state.hardware());
    Ok(models::quant_files(&tree)?
        .into_iter()
        .map(|file| QuantRow {
            fit: fit_of(&file.name, file.size, &budget),
            file,
        })
        .collect())
}

pub fn cancel_download(state: &ManagerState) {
    if let Ok(job) = state.job.lock()
        && let Some((cancel, _)) = job.as_ref()
    {
        cancel.store(true, Ordering::SeqCst);
    }
}

pub fn delete_model(
    state: &ManagerState,
    settings: &SettingsStore,
    file: &str,
) -> Result<(), String> {
    if running_model(state).as_deref() == Some(file) {
        return Err("Stop the server before deleting the model it is running".into());
    }
    models::delete_model(&models::models_dir(&data_dir()?), file)?;
    if settings.managed_model().as_deref() == Some(file) {
        settings.set_managed_model(None)?;
    }
    Ok(())
}

/// Launches llama-server on installed model `file`. Must be called inside a tokio runtime (the Vulkan fallback runs on it).
/// Only a recorded CPU or CUDA install adds GPU flags (see `server::gpu_args`). A CUDA build that dies while loading
/// before it ever named a GPU device is replaced by the Vulkan build and the same model is started again.
/// The caller remembers `file` as the selected model.
pub fn start(state: &Arc<ManagerState>, file: &str, events: &Events) -> Result<(), String> {
    if state
        .job
        .lock()
        .is_ok_and(|j| j.as_ref().is_some_and(|(_, runtime)| *runtime))
    {
        return Err("Wait for the llama.cpp install to finish".into());
    }
    let exe = find_runtime().ok_or("Install llama.cpp first")?;
    let path = model_path(&models::models_dir(&data_dir()?), file)?;
    let backend = installed_backend(&exe).map(|b| b.backend);
    let mut extra = server::gpu_args(std::env::consts::OS, backend);
    extra.extend(server::model_args(file));
    let args = server::server_args(&path, &extra);
    log::info!("Starting llama-server with {args:?}");
    let health = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(1))
        .build()
        .map_err(|e| e.to_string())?;
    let (shared, events, runtime) = (
        state.clone(),
        events.clone(),
        tokio::runtime::Handle::try_current().map_err(|e| e.to_string())?,
    );
    let model = file.to_string();
    let previous = Mutex::new(server::Status::Stopped);
    state.server.start(
        &exe,
        args,
        file.to_string(),
        format!("{}/health", server::base_url()),
        health,
        Duration::from_millis(500),
        server::LOAD_TIMEOUT,
        move |status| {
            let before = std::mem::replace(&mut *previous.lock().unwrap(), status.clone());
            let failed = matches!(status, server::Status::Error { .. });
            if failed
                && server::should_fallback_to_vulkan(
                    std::env::consts::OS,
                    backend.unwrap_or(Backend::Cpu),
                    &before,
                    shared.server.last_exit().as_ref(),
                )
            {
                let (state, events, model, generation) = (
                    shared.clone(),
                    events.clone(),
                    model.clone(),
                    shared.server.generation(),
                );
                runtime.spawn(async move {
                    fall_back_to_vulkan(&state, &model, status, generation, &events).await
                });
            } else {
                (events.status)(status);
            }
        },
    )
}

/// Installs the Vulkan build and starts `file` again. `error` is the CUDA failure and `generation` the server's
/// counter when it happened: if the user stopped or started anything since, nothing is restarted. The UI sees
/// Loading plus `fallback_note` while this runs, and the real error if it cannot finish.
async fn fall_back_to_vulkan(
    state: &Arc<ManagerState>,
    file: &str,
    error: server::Status,
    generation: u64,
    events: &Events,
) {
    let report = |status: server::Status| (events.status)(status);
    set_note(state, Some(FALLBACK_RUNNING));
    report(server::Status::Loading {
        model: file.to_string(),
    });
    if let Err(e) = install_backend(state, events, Some(Backend::Vulkan)).await {
        log::warn!("Vulkan fallback install failed: {e}");
        set_note(state, (e == BUSY).then_some(FALLBACK_BUSY));
        return report(error);
    }
    if state.server.generation() != generation {
        return set_note(state, None);
    }
    match start(state, file, events) {
        Ok(()) => set_note(state, Some(FALLBACK_NOTE)),
        Err(e) => {
            log::warn!("Vulkan fallback start failed: {e}");
            set_note(state, None);
            report(state.server.status());
        }
    }
}
