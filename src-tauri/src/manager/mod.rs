//! In-app management of a local llama.cpp runtime, GGUF models and the llama-server process.

pub mod download;
pub mod runtime;
pub mod models;
pub mod server;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::settings;

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
}

impl Default for ManagerState {
    fn default() -> Self {
        Self { server: server::Server::new(), job: Mutex::new(None), client: download::client() }
    }
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
        server::Status::Loading { model } | server::Status::Ready { model } => Some(model),
        _ => None,
    }
}

#[derive(serde::Serialize)]
pub struct ManagerInfo {
    runtime: Option<String>,
    os: &'static str,
    catalog: &'static [models::CatalogModel],
    installed: Vec<models::InstalledModel>,
    server: server::Status,
    base_url: String,
    autostart: bool,
    selected_model: Option<String>,
    downloading: bool,
}

#[tauri::command]
pub fn manager_status(app: AppHandle) -> Result<ManagerInfo, String> {
    let state = app.state::<ManagerState>();
    let dir = data_dir(&app)?;
    let installed = models::list_installed(&models::models_dir(&dir));
    Ok(ManagerInfo {
        runtime: find_runtime(&app).map(|p| p.to_string_lossy().into_owned()),
        os: std::env::consts::OS,
        catalog: models::CATALOG,
        installed: installed.clone(),
        server: state.server.status(),
        base_url: server::base_url(),
        autostart: settings::managed_autostart(&app),
        selected_model: settings::managed_model(&app).filter(|m| installed.iter().any(|i| &i.file == m)),
        downloading: state.job.lock().is_ok_and(|j| j.is_some()),
    })
}

#[tauri::command]
pub async fn install_runtime(app: AppHandle, backend: runtime::Backend) -> Result<String, String> {
    if running_model(&app).is_some() {
        return Err("Stop the server before reinstalling llama.cpp".into());
    }
    let job = Job::begin(&app, true)?;
    let dir = data_dir(&app)?;
    let client = app.state::<ManagerState>().client.clone();
    let releases = runtime::fetch_release(&client, runtime::RELEASES_API).await?;
    let assets = runtime::pick_release(&releases, std::env::consts::OS, std::env::consts::ARCH, backend)?;
    let mut tracker = Tracker::new(&app, "runtime", assets.iter().map(|a| Some(a.size)));
    let exe = runtime::install_assets(&client, &runtime::runtime_dir(&dir), &assets, &job.cancel, |name, p| tracker.update(name, p)).await?;
    tracker.finish("llama.cpp");
    Ok(exe.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn download_model(app: AppHandle, source: String) -> Result<String, String> {
    let job = Job::begin(&app, false)?;
    let dir = models::models_dir(&data_dir(&app)?);
    let client = app.state::<ManagerState>().client.clone();
    let files = models::resolve(&client, models::HF_BASE, &models::parse_source(&source)?).await?;
    for f in &files {
        download::require_https(&f.url)?;
    }
    let mut tracker = Tracker::new(&app, "model", files.iter().map(|f| f.size));
    let name = models::download_files(&client, &dir, &files, &job.cancel, |file, p| tracker.update(file, p)).await?;
    tracker.finish(&name);
    Ok(name)
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

/// Launches llama-server on installed model `file` (remembered as the selected model).
pub fn start(app: &AppHandle, file: &str) -> Result<(), String> {
    if app.state::<ManagerState>().job.lock().is_ok_and(|j| j.as_ref().is_some_and(|(_, runtime)| *runtime)) {
        return Err("Wait for the llama.cpp install to finish".into());
    }
    let exe = find_runtime(app).ok_or("Install llama.cpp first")?;
    let path = model_path(&models::models_dir(&data_dir(app)?), file)?;
    let health = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(1))
        .build()
        .map_err(|e| e.to_string())?;
    let handle = app.clone();
    app.state::<ManagerState>().server.start(
        &exe,
        server::server_args(&path),
        file.to_string(),
        format!("{}/health", server::base_url()),
        health,
        Duration::from_millis(500),
        server::LOAD_TIMEOUT,
        move |status| {
            let _ = handle.emit_to("settings", STATUS_EVENT, status);
        },
    )?;
    settings::set_managed_model(app, Some(file))
}

#[tauri::command]
pub fn start_server(app: AppHandle, file: String) -> Result<(), String> {
    start(&app, &file)?;
    settings::use_managed_server(&app)
}

#[tauri::command]
pub fn stop_server(state: State<'_, ManagerState>) {
    state.server.stop();
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::set_managed_autostart(&app, enabled)
}
