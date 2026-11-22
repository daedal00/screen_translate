//! In-app management of a local llama.cpp runtime, GGUF models and the llama-server process.

pub mod catalog;
pub mod download;
pub mod hardware;
pub mod runtime;
pub mod models;
pub mod server;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::settings;
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

pub fn managed_not_ready(local_url: &str, status: &server::Status) -> Option<&'static str> {
    let url = local_url.trim().trim_end_matches('/');
    let url = url.strip_suffix("/v1").unwrap_or(url).trim_end_matches('/');
    if url != server::base_url() {
        return None;
    }
    match status {
        server::Status::Ready { .. } => None,
        server::Status::Loading { .. } => Some(MANAGED_LOADING),
        server::Status::Stopped | server::Status::Error { .. } => Some(MANAGED_STOPPED),
    }
}

const STATUS_EVENT: &str = "manager://status";
const PROGRESS_EVENT: &str = "manager://progress";
const BUSY: &str = "Another download is in progress";

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

fn set_note(app: &AppHandle, note: Option<&str>) {
    if let Ok(mut n) = app.state::<ManagerState>().fallback_note.lock() {
        *n = note.map(String::from);
    }
}

/// Starts detecting the hardware in the background so Settings and installs find it ready.
pub fn warm_hardware(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        app.state::<ManagerState>().hardware();
    });
}

/// How `file` (`size` bytes) fits `b`: a catalog file uses the catalog's own requirement, any other file its size plus `OVERHEAD`.
fn fit_of(file: &str, size: u64, b: &Budget) -> Fit {
    hardware::fit(catalog::entries().iter().find(|e| e.file == file).map_or(size + OVERHEAD, catalog::required), b)
}

/// `<runtime_dir>/backend.json`: which build was installed, and for which GPU.
#[derive(serde::Serialize, serde::Deserialize)]
struct InstalledBackend {
    backend: Backend,
    gpu: Option<String>,
}

fn backend_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(runtime::runtime_dir(&data_dir(app)?).join("backend.json"))
}

/// What `backend.json` says, only when `exe` is the llama-server this app installed (under `<runtime_dir>/bin`):
/// a llama-server from PATH, or a missing or unreadable file, gives None.
fn installed_backend(app: &AppHandle, exe: &Path) -> Option<InstalledBackend> {
    let runtime = runtime::runtime_dir(&data_dir(app).ok()?);
    if !exe.starts_with(runtime.join("bin")) {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(runtime.join("backend.json")).ok()?).ok()
}

/// Holds the single job slot; frees it on drop.
struct Job {
    app: AppHandle,
    cancel: Arc<AtomicBool>,
}

impl Job {
    fn begin(app: &AppHandle, runtime: bool) -> Result<Self, String> {
        let state = app.state::<ManagerState>();
        let mut slot = state.job.lock().map_err(|e| e.to_string())?;
        if slot.is_some() {
            return Err(BUSY.into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *slot = Some((cancel.clone(), runtime));
        Ok(Self { app: app.clone(), cancel })
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.app.state::<ManagerState>().job.lock() {
            *slot = None;
        }
    }
}

#[derive(Clone, serde::Serialize)]
struct ProgressEvent<'a> {
    job: &'a str,
    file: &'a str,
    downloaded: u64,
    total: Option<u64>,
}

/// Overall progress across all files of one job, emitted throttled.
struct Tracker<'a> {
    app: &'a AppHandle,
    job: &'static str,
    total: Option<u64>,
    per_file: HashMap<String, u64>,
    last: Option<Instant>,
}

impl<'a> Tracker<'a> {
    fn new(app: &'a AppHandle, job: &'static str, sizes: impl Iterator<Item = Option<u64>>) -> Self {
        let total = sizes.sum::<Option<u64>>();
        Self { app, job, total, per_file: HashMap::new(), last: None }
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
        let _ = self.app.emit_to("settings", PROGRESS_EVENT, ProgressEvent { job: self.job, file, downloaded, total: self.total });
    }

    /// The final event, so the UI always ends on 100%.
    fn finish(&self, file: &str) {
        self.emit(file, self.total.unwrap_or_else(|| self.per_file.values().sum()));
    }
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

fn find_runtime(app: &AppHandle) -> Option<PathBuf> {
    let dir = runtime::runtime_dir(&data_dir(app).ok()?);
    runtime::find_server(&dir, std::env::var_os("PATH").as_deref(), std::env::consts::OS)
}

fn running_model(app: &AppHandle) -> Option<String> {
    match app.state::<ManagerState>().server.status() {
        server::Status::Loading { model } | server::Status::Ready { model, .. } => Some(model),
        _ => None,
    }
}

#[derive(serde::Serialize)]
struct CatalogRow {
    #[serde(flatten)]
    entry: &'static catalog::Entry,
    fit: Fit,
}

#[derive(serde::Serialize)]
struct InstalledRow {
    #[serde(flatten)]
    model: models::InstalledModel,
    fit: Fit,
}

#[derive(serde::Serialize)]
struct BackendInfo {
    recommended: Backend,
    installed: Option<Backend>,
    /// GPU name recorded at install time.
    installed_gpu: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ManagerInfo {
    runtime: Option<String>,
    os: &'static str,
    hardware: Hardware,
    budget: Budget,
    recommended: Vec<catalog::Recommendation>,
    catalog: Vec<CatalogRow>,
    backend: BackendInfo,
    fallback_note: Option<String>,
    installed: Vec<InstalledRow>,
    server: server::Status,
    base_url: String,
    autostart: bool,
    selected_model: Option<String>,
    downloading: bool,
}

#[tauri::command]
pub async fn manager_status(app: AppHandle) -> Result<ManagerInfo, String> {
    let state = app.state::<ManagerState>();
    let dir = data_dir(&app)?;
    let installed = models::list_installed(&models::models_dir(&dir));
    let hw = state.hardware();
    let budget = hardware::budget(hw);
    let exe = find_runtime(&app);
    let installed_backend = exe.as_deref().and_then(|e| installed_backend(&app, e));
    Ok(ManagerInfo {
        runtime: exe.as_ref().map(|p| p.to_string_lossy().into_owned()),
        os: std::env::consts::OS,
        hardware: hw.clone(),
        budget,
        recommended: catalog::recommend(catalog::entries(), &budget, 3),
        catalog: catalog::entries().iter().map(|entry| CatalogRow { entry, fit: hardware::fit(catalog::required(entry), &budget) }).collect(),
        backend: BackendInfo {
            recommended: hardware::choose_backend(hw.gpu.as_ref()),
            installed: installed_backend.as_ref().map(|b| b.backend),
            installed_gpu: installed_backend.and_then(|b| b.gpu),
        },
        fallback_note: state.fallback_note.lock().ok().and_then(|n| n.clone()),
        installed: installed.iter().map(|m| InstalledRow { fit: fit_of(&m.file, m.size, &budget), model: m.clone() }).collect(),
        server: state.server.status(),
        base_url: server::base_url(),
        autostart: settings::managed_autostart(&app),
        selected_model: settings::managed_model(&app).filter(|m| installed.iter().any(|i| &i.file == m)),
        downloading: state.job.lock().is_ok_and(|j| j.is_some()),
    })
}

/// Installs llama.cpp for `backend` (None: the one `choose_backend` picks for this GPU) and records it in `backend.json`.
async fn install_backend(app: &AppHandle, backend: Option<Backend>) -> Result<String, String> {
    if running_model(app).is_some() {
        return Err("Stop the server before reinstalling llama.cpp".into());
    }
    let job = Job::begin(app, true)?;
    let dir = data_dir(app)?;
    let state = app.state::<ManagerState>();
    let client = state.client.clone();
    let gpu = state.hardware().gpu.as_ref();
    let backend = backend.unwrap_or_else(|| hardware::choose_backend(gpu));
    let releases = runtime::fetch_release(&client, runtime::RELEASES_API).await?;
    let assets = runtime::pick_release(&releases, std::env::consts::OS, std::env::consts::ARCH, backend, gpu.and_then(|g| g.max_cuda))?;
    let mut tracker = Tracker::new(app, "runtime", assets.iter().map(|a| Some(a.size)));
    let exe = runtime::install_assets(&client, &runtime::runtime_dir(&dir), &assets, &job.cancel, |name, p| tracker.update(name, p)).await?;
    tracker.finish("llama.cpp");
    let record = InstalledBackend { backend, gpu: gpu.map(|g| g.name.clone()) };
    if let Err(e) = serde_json::to_string(&record).map_err(|e| e.to_string()).and_then(|j| std::fs::write(backend_file(app)?, j).map_err(|e| e.to_string())) {
        log::warn!("Cannot record the installed backend: {e}");
    }
    Ok(exe.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn install_runtime(app: AppHandle, backend: Option<Backend>) -> Result<String, String> {
    install_backend(&app, backend).await
}

/// Downloads `files` into the models folder under `job`, reporting progress; returns the first file's name.
async fn download_to_models(app: &AppHandle, job: &Job, files: &[models::RemoteFile]) -> Result<String, String> {
    let dir = models::models_dir(&data_dir(app)?);
    let client = app.state::<ManagerState>().client.clone();
    for f in files {
        download::require_https(&f.url)?;
    }
    let mut tracker = Tracker::new(app, "model", files.iter().map(|f| f.size));
    let name = models::download_files(&client, &dir, files, &job.cancel, |file, p| tracker.update(file, p)).await?;
    tracker.finish(&name);
    Ok(name)
}

#[tauri::command]
pub async fn download_model(app: AppHandle, source: String) -> Result<String, String> {
    let job = Job::begin(&app, false)?;
    let client = app.state::<ManagerState>().client.clone();
    let files = models::resolve(&client, models::HF_BASE, &models::parse_source(&source)?).await?;
    download_to_models(&app, &job, &files).await
}

/// Downloads catalog entry `id` (its pinned file and sha256 from `catalog.json`).
#[tauri::command]
pub async fn download_catalog(app: AppHandle, id: String) -> Result<String, String> {
    let entry = catalog::entries().iter().find(|e| e.id == id).ok_or_else(|| format!("Unknown model {id}"))?;
    let job = Job::begin(&app, false)?;
    let file = models::RemoteFile {
        name: entry.file.clone(),
        url: format!("{}/{}/resolve/main/{}", models::HF_BASE, entry.repo, entry.file),
        size: Some(entry.size),
        sha256: entry.sha256.clone(),
    };
    download_to_models(&app, &job, &[file]).await
}

/// Searches Hugging Face for GGUF repos; an empty query gives an empty list.
#[tauri::command]
pub async fn hf_search(app: AppHandle, query: String) -> Result<Vec<models::HfRepo>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let client = app.state::<ManagerState>().client.clone();
    let offline = |_| models::OFFLINE.to_string();
    let resp = client.get(models::search_url(models::HF_BASE, query)).timeout(Duration::from_secs(10)).send().await.map_err(offline)?;
    if !resp.status().is_success() {
        return Err(format!("Hugging Face returned HTTP {}", resp.status().as_u16()));
    }
    models::parse_search(&resp.text().await.map_err(offline)?)
}

#[derive(serde::Serialize)]
pub struct QuantRow {
    #[serde(flatten)]
    file: models::QuantFile,
    fit: Fit,
}

/// The quantizations of `repo` (`owner/name`) with how well each fits this machine. Download a row with
/// `download_model("owner/name:<row.name>")`.
#[tauri::command]
pub async fn hf_files(app: AppHandle, repo: String) -> Result<Vec<QuantRow>, String> {
    let invalid = || format!("Expected a repository like owner/name, got {repo:?}");
    let Ok(models::Source::Hf { repo, .. }) = models::parse_source(&format!("{repo}:x")) else { return Err(invalid()) };
    let state = app.state::<ManagerState>();
    let tree = models::fetch_tree(&state.client, models::HF_BASE, &repo).await?;
    let budget = hardware::budget(state.hardware());
    Ok(models::quant_files(&tree)?
        .into_iter()
        .map(|file| QuantRow { fit: fit_of(&file.name, file.size, &budget), file })
        .collect())
}

#[tauri::command]
pub fn cancel_download(state: State<'_, ManagerState>) {
    if let Ok(job) = state.job.lock()
        && let Some((cancel, _)) = job.as_ref()
    {
        cancel.store(true, Ordering::SeqCst);
    }
}

#[tauri::command]
pub fn delete_model(app: AppHandle, file: String) -> Result<(), String> {
    if running_model(&app).as_deref() == Some(file.as_str()) {
        return Err("Stop the server before deleting the model it is running".into());
    }
    models::delete_model(&models::models_dir(&data_dir(&app)?), &file)?;
    if settings::managed_model(&app).as_deref() == Some(file.as_str()) {
        settings::set_managed_model(&app, None)?;
    }
    Ok(())
}

/// Launches llama-server on installed model `file` (remembered as the selected model). Only a recorded CPU
/// or CUDA install adds GPU flags (see `server::gpu_args`). A CUDA build that dies while loading before it
/// ever named a GPU device is replaced by the Vulkan build and the same model is started again.
pub fn start(app: &AppHandle, file: &str) -> Result<(), String> {
    let state = app.state::<ManagerState>();
    if state.job.lock().is_ok_and(|j| j.as_ref().is_some_and(|(_, runtime)| *runtime)) {
        return Err("Wait for the llama.cpp install to finish".into());
    }
    let exe = find_runtime(app).ok_or("Install llama.cpp first")?;
    let path = model_path(&models::models_dir(&data_dir(app)?), file)?;
    let backend = installed_backend(app, &exe).map(|b| b.backend);
    let args = server::server_args(&path, &server::gpu_args(std::env::consts::OS, backend));
    log::info!("Starting llama-server with {args:?}");
    let health = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(1))
        .build()
        .map_err(|e| e.to_string())?;
    let handle = app.clone();
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
            let server = &handle.state::<ManagerState>().server;
            let failed = matches!(status, server::Status::Error { .. });
            if failed && server::should_fallback_to_vulkan(std::env::consts::OS, backend.unwrap_or(Backend::Cpu), &before, server.last_exit().as_ref()) {
                let (handle, model, generation) = (handle.clone(), model.clone(), server.generation());
                tauri::async_runtime::spawn(async move { fall_back_to_vulkan(&handle, &model, status, generation).await });
            } else {
                let _ = handle.emit_to("settings", STATUS_EVENT, status);
            }
        },
    )?;
    settings::set_managed_model(app, Some(file))
}

/// Installs the Vulkan build and starts `file` again. `error` is the CUDA failure and `generation` the server's
/// counter when it happened: if the user stopped or started anything since, nothing is restarted. The UI sees
/// Loading plus `fallback_note` while this runs, and the real error if it cannot finish.
async fn fall_back_to_vulkan(app: &AppHandle, file: &str, error: server::Status, generation: u64) {
    let report = |status: server::Status| {
        let _ = app.emit_to("settings", STATUS_EVENT, status);
    };
    set_note(app, Some(FALLBACK_RUNNING));
    report(server::Status::Loading { model: file.to_string() });
    if let Err(e) = install_backend(app, Some(Backend::Vulkan)).await {
        log::warn!("Vulkan fallback install failed: {e}");
        set_note(app, (e == BUSY).then_some(FALLBACK_BUSY));
        return report(error);
    }
    let server = &app.state::<ManagerState>().server;
    if server.generation() != generation {
        return set_note(app, None);
    }
    match start(app, file) {
        Ok(()) => set_note(app, Some(FALLBACK_NOTE)),
        Err(e) => {
            log::warn!("Vulkan fallback start failed: {e}");
            set_note(app, None);
            report(server.status());
        }
    }
}

#[tauri::command]
pub fn start_server(app: AppHandle, file: String) -> Result<(), String> {
    set_note(&app, None);
    start(&app, &file)?;
    settings::use_managed_server(&app)
}

#[tauri::command]
pub fn stop_server(app: AppHandle, state: State<'_, ManagerState>) {
    set_note(&app, None);
    state.server.stop();
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::set_managed_autostart(&app, enabled)
}
