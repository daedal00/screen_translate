//! Self-update from the signed `latest.json` attached to each GitHub release.
//! The tray shows one item whose text and click action follow `UpdateState`.

use std::sync::Mutex;
use std::time::Duration;

use tauri::menu::MenuItem;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_updater::UpdaterExt as _;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    Available(String),
    Installing,
    UpToDate,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Check,
    Install,
    Ignore,
}

impl UpdateState {
    pub fn label(&self) -> String {
        match self {
            UpdateState::Idle => "Check for Updates…".into(),
            UpdateState::Checking => "Checking for Updates…".into(),
            UpdateState::Available(v) => format!("Install Update v{v} and Restart"),
            UpdateState::Installing => "Installing Update…".into(),
            UpdateState::UpToDate => "Up to Date, Check Again".into(),
            UpdateState::Failed => "Update Failed, Try Again".into(),
        }
    }

    pub fn enabled(&self) -> bool {
        !matches!(self, UpdateState::Checking | UpdateState::Installing)
    }

    pub fn click(&self) -> Click {
        match self {
            UpdateState::Checking | UpdateState::Installing => Click::Ignore,
            UpdateState::Available(_) => Click::Install,
            _ => Click::Check,
        }
    }
}

/// First check shortly after launch, then periodically for a tray app that stays running.
const FIRST_CHECK: Duration = Duration::from_secs(10);
const RECHECK: Duration = Duration::from_secs(6 * 60 * 60);

pub struct Updates {
    item: MenuItem<Wry>,
    state: Mutex<UpdateState>,
}

impl Updates {
    pub fn new(item: MenuItem<Wry>) -> Self {
        Updates { item, state: Mutex::new(UpdateState::Idle) }
    }
}

fn set(app: &AppHandle, state: UpdateState) {
    let u = app.state::<Updates>();
    let r = u.item.set_text(state.label()).and(u.item.set_enabled(state.enabled()));
    if let Err(e) = r {
        log::warn!("updating tray item failed: {e}");
    }
    *u.state.lock().unwrap() = state;
}

fn current(app: &AppHandle) -> UpdateState {
    app.state::<Updates>().state.lock().unwrap().clone()
}

pub fn on_click(app: &AppHandle) {
    match current(app).click() {
        Click::Check => spawn_check(app),
        Click::Install => spawn_install(app),
        Click::Ignore => {}
    }
}

/// Background checks never overwrite a found update or an install in progress.
pub fn start_background_checks(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if matches!(current(&app), UpdateState::Idle | UpdateState::UpToDate | UpdateState::Failed) {
                check(&app).await;
            }
            tokio::time::sleep(RECHECK).await;
        }
    });
}

/// Debug builds can point at a local manifest (`ST_UPDATE_ENDPOINT=http://...`) to test an install.
fn updater(app: &AppHandle) -> tauri_plugin_updater::Result<tauri_plugin_updater::Updater> {
    let builder = app.updater_builder();
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("ST_UPDATE_ENDPOINT") {
        let url = tauri::Url::parse(&url).map_err(|e| tauri_plugin_updater::Error::Io(std::io::Error::other(e)))?;
        return builder.endpoints(vec![url])?.build();
    }
    builder.build()
}

fn spawn_check(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { check(&app).await });
}

async fn check(app: &AppHandle) {
    set(app, UpdateState::Checking);
    let found = match updater(app) {
        Ok(u) => u.check().await,
        Err(e) => Err(e),
    };
    match found {
        Ok(Some(update)) => {
            log::info!("update available: {}", update.version);
            set(app, UpdateState::Available(update.version));
            // Debug hook: install straight away (the tray menu can't be scripted).
            #[cfg(debug_assertions)]
            if std::env::var_os("ST_UPDATE_AUTOINSTALL").is_some() {
                spawn_install(app);
            }
        }
        Ok(None) => set(app, UpdateState::UpToDate),
        Err(e) => {
            log::warn!("update check failed: {e}");
            set(app, UpdateState::Failed);
        }
    }
}

fn spawn_install(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        set(&app, UpdateState::Installing);
        match install(&app).await {
            Ok(true) => {
                // The restart skips RunEvent::Exit, so stop the managed llama-server here.
                app.state::<crate::manager::ManagerState>().server.stop();
                app.restart();
            }
            Ok(false) => set(&app, UpdateState::UpToDate),
            Err(e) => {
                log::warn!("update install failed: {e}");
                set(&app, UpdateState::Failed);
            }
        }
    });
}

/// Re-checks so the install uses a fresh, signature-verified manifest.
async fn install(app: &AppHandle) -> tauri_plugin_updater::Result<bool> {
    let Some(update) = updater(app)?.check().await? else { return Ok(false) };
    update.download_and_install(|_, _| {}, || {}).await?;
    Ok(true)
}
