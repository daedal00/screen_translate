//! Self-update glue: tray item state, background checks and the platform install step.
//! Network and verification live in `screen_translate_core::update`.

use std::path::PathBuf;
use std::time::Duration;

use gpui::App;
use screen_translate_core::update::{
    self, Click, UpdateState, current_platform_keys, should_background_check,
};

use crate::AppState;

/// First check shortly after launch, then periodically for a tray app that stays running.
const FIRST_CHECK: Duration = Duration::from_secs(10);
const RECHECK: Duration = Duration::from_secs(6 * 60 * 60);
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Debug builds can point at a local manifest and key (`ST_UPDATE_ENDPOINT`, `ST_UPDATE_PUBKEY`) to test an install.
fn endpoint() -> String {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("ST_UPDATE_ENDPOINT") {
        return url;
    }
    update::ENDPOINT.into()
}

fn pubkey() -> String {
    #[cfg(debug_assertions)]
    if let Ok(key) = std::env::var("ST_UPDATE_PUBKEY") {
        return key;
    }
    update::PUBKEY.into()
}

pub fn current(cx: &App) -> UpdateState {
    cx.global::<AppState>()
        .update
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn set(cx: &mut App, state: UpdateState) {
    let app = cx.global::<AppState>();
    if let Some((_, item)) = app.menu_items.iter().find(|(id, _)| *id == "update") {
        item.set_text(state.label());
        item.set_enabled(state.enabled());
    }
    *app.update.lock().unwrap_or_else(|e| e.into_inner()) = state;
}

pub fn on_click(cx: &mut App) {
    match current(cx).click() {
        Click::Check => spawn_check(cx),
        Click::Install => spawn_install(cx),
        Click::Ignore => {}
    }
}

pub fn start_background_checks(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(FIRST_CHECK).await;
        loop {
            cx.update(|cx| {
                if should_background_check(&current(cx)) {
                    spawn_check(cx);
                }
            });
            cx.background_executor().timer(RECHECK).await;
        }
    })
    .detach();
}

fn spawn_check(cx: &mut App) {
    set(cx, UpdateState::Checking);
    let rt = cx.global::<AppState>().runtime.handle().clone();
    cx.spawn(async move |cx| {
        let found = rt
            .spawn(async {
                let client = client();
                update::check(&client, &endpoint(), VERSION, &current_platform_keys()).await
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        cx.update(|cx| match found {
            Ok(Some(u)) => {
                log::info!("update available: {}", u.version);
                set(cx, UpdateState::Available(u.version));
                // Debug hook: install straight away (the tray menu can't be scripted).
                #[cfg(debug_assertions)]
                if std::env::var_os("ST_UPDATE_AUTOINSTALL").is_some() {
                    spawn_install(cx);
                }
            }
            Ok(None) => set(cx, UpdateState::UpToDate),
            Err(e) => {
                log::warn!("update check failed: {e}");
                set(cx, UpdateState::Failed);
            }
        });
    })
    .detach();
}

fn spawn_install(cx: &mut App) {
    set(cx, UpdateState::Installing);
    let rt = cx.global::<AppState>().runtime.handle().clone();
    cx.spawn(async move |cx| {
        let staged = rt
            .spawn(prepare())
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        cx.update(|cx| match staged {
            Ok(Some(path)) => finish(cx, path),
            Ok(None) => set(cx, UpdateState::UpToDate),
            Err(e) => {
                log::warn!("update install failed: {e}");
                set(cx, UpdateState::Failed);
            }
        });
    })
    .detach();
}

/// Re-checks so the install uses a fresh manifest, downloads and verifies, then stages it:
/// macOS replaces the running bundle and returns it; Windows returns the written installer.
async fn prepare() -> Result<Option<PathBuf>, String> {
    let client = client();
    let Some(found) =
        update::check(&client, &endpoint(), VERSION, &current_platform_keys()).await?
    else {
        return Ok(None);
    };
    let bytes = update::download(&client, &found, &pubkey(), |_, _| {}).await?;
    let version = found.version;
    tokio::task::spawn_blocking(move || stage(&version, &bytes))
        .await
        .map_err(|e| e.to_string())?
        .map(Some)
}

#[cfg(target_os = "macos")]
fn stage(_version: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::os::unix::fs::MetadataExt;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bundle = update::bundle_from_exe(&exe).ok_or("not running from an app bundle")?;
    let parent = bundle.parent().ok_or("the app bundle has no parent dir")?;
    // The swap needs both paths on one volume: the temp dir if it shares one with the bundle, else next to it.
    let temp = std::env::temp_dir();
    let same_volume = matches!(
        (std::fs::metadata(&temp), std::fs::metadata(parent)),
        (Ok(a), Ok(b)) if a.dev() == b.dev()
    );
    let base = if same_volume {
        temp
    } else {
        parent.to_path_buf()
    };
    // A fixed name: the pre-clean also removes what a crashed earlier run left behind.
    let staging = base.join(".screen-translate-update");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging)
        .map_err(|e| format!("cannot stage the update in {}: {e}", base.display()))?;
    let result = update::extract_bundle(bytes, &staging)
        .and_then(|()| update::replace_bundle(&staging, &bundle))
        .map_err(|e| format!("cannot replace {} (is it writable?): {e}", bundle.display()));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result.map(|()| bundle)
}

/// UNVERIFIED: not run on Windows.
#[cfg(windows)]
fn stage(version: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let path = update::windows_installer_path(&std::env::temp_dir(), version);
    std::fs::create_dir_all(path.parent().ok_or("no installer dir")?).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Stops live translation and llama-server (the restart skips nothing, but a new instance must find the port
/// free), starts the new app and quits. If the launch fails the app keeps running: the live overlay is already
/// closed and the managed server is started again when autostart is on.
fn finish(cx: &mut App, staged: PathBuf) {
    // `Overlays` exists only once a selection was opened; without it there is no overlay to close.
    if cx.has_global::<crate::flow::Overlays>() {
        crate::flow::stop_live(cx);
    } else {
        cx.global::<AppState>().live.stop();
    }
    cx.global::<AppState>().manager.server.stop();
    match launch(&staged) {
        Ok(()) => cx.quit(),
        Err(e) => {
            log::warn!("update install failed: {e}");
            crate::start_server_if_autostart(
                cx.global::<AppState>(),
                crate::models_host::events(cx),
            );
            set(cx, UpdateState::Failed);
        }
    }
}

#[cfg(target_os = "macos")]
fn launch(bundle: &std::path::Path) -> Result<(), String> {
    // `status`, not `spawn`: `open` exits once LaunchServices accepted (or refused) the bundle, and the old app
    // must only quit after that.
    let status = std::process::Command::new("open")
        .args(["-n", "-g"])
        .arg(bundle)
        .status()
        .map_err(|e| format!("relaunch failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("relaunch failed: open exited with {status}"))
    }
}

/// UNVERIFIED: not run on Windows. ShellExecuteW (not `Command`) so an installer that asks for admin still starts.
/// Microsoft recommends `CoInitializeEx` before `ShellExecute*`; a plain `.exe` with "open" works without, so
/// check this in the Windows manual test.
#[cfg(windows)]
fn launch(installer: &std::path::Path) -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    use windows::core::{HSTRING, w};

    let args = HSTRING::from(update::installer_args().join(" "));
    let file = HSTRING::from(installer.as_os_str());
    // SAFETY: all strings are NUL-terminated and outlive the call.
    let r = unsafe { ShellExecuteW(None, w!("open"), &file, &args, None, SW_SHOW) };
    if (r.0 as isize) <= 32 {
        return Err(format!(
            "starting the installer failed (code {})",
            r.0 as isize
        ));
    }
    Ok(())
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("screen-translate/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}
