pub mod capture;
pub mod ocr;
pub mod popup;
pub mod settings;
pub mod translate;

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use image::{RgbaImage, codecs::png::{CompressionType, FilterType, PngEncoder}};
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
    ipc::Response,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use capture::LogicalRect;
use popup::place;
use translate::{Translation, TranslationCache, deepl::DeepL, ollama::Ollama};

struct Shot {
    image: RgbaImage,
    scale: f64,
    /// Monitor origin and size in physical pixels (virtual desktop), for placing the popup.
    pos: (i32, i32),
    size: (u32, u32),
}

#[derive(Default)]
struct Selection(Mutex<Option<Shot>>);

/// Set while a selector is being created; also swallows key auto-repeat.
#[derive(Default)]
struct Opening(AtomicBool);

struct Cache(Mutex<TranslationCache>);

struct Http {
    deepl: reqwest::Client,
    ollama: reqwest::Client,
}

/// Returned to the selector (which stays open) when the dragged rect maps to no pixels.
const SELECTION_EMPTY: &str = "selection is empty";
const NO_KEY: &str = "Add your DeepL API key in Settings";

#[derive(Clone, serde::Serialize)]
struct RegionResult {
    original: String,
    translation: Option<String>,
    detected_source: Option<String>,
    error: Option<String>,
    /// The popup offers a Settings button.
    needs_settings: bool,
}

impl RegionResult {
    fn failed(original: String, error: impl Into<String>) -> Self {
        Self { original, translation: None, detected_source: None, error: Some(error.into()), needs_settings: false }
    }
}

/// Latest result for the popup. The generation lives under the same lock as the result, so a slow
/// earlier request can't overwrite a newer one (bump+clear and compare+store are each atomic).
#[derive(Default)]
struct Latest(Mutex<LatestInner>);

#[derive(Default)]
struct LatestInner {
    generation: u64,
    result: Option<RegionResult>,
}

fn open_settings(app: &AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") {
        return w.set_focus().map_err(|e| e.to_string());
    }
    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Screen Translate Settings")
        .inner_size(420.0, 440.0)
        .resizable(false)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn clear_selection(app: &AppHandle) {
    if let Ok(mut shot) = app.state::<Selection>().0.lock() {
        *shot = None;
    }
}

fn open_selector(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window("selector").is_some() {
        return Ok(());
    }
    let cursor = app.cursor_position().map_err(|e| e.to_string())?;
    // tao on macOS reports the cursor in points x primary-monitor scale, while
    // `monitor_from_point` expects points, so undo the primary scale there.
    #[cfg(target_os = "macos")]
    let (cursor_x, cursor_y) = {
        let primary = app.primary_monitor().map_err(|e| e.to_string())?.map_or(1.0, |m| m.scale_factor());
        (cursor.x / primary, cursor.y / primary)
    };
    #[cfg(not(target_os = "macos"))]
    let (cursor_x, cursor_y) = (cursor.x, cursor.y);
    let monitor = app
        .monitor_from_point(cursor_x, cursor_y)
        .map_err(|e| e.to_string())?
        .or(app.primary_monitor().map_err(|e| e.to_string())?)
        .ok_or("no monitor found")?;
    let (pos, size, scale) = (*monitor.position(), *monitor.size(), monitor.scale_factor());

    // xcap origins: physical pixels on Windows, points (CGDisplayBounds) on macOS.
    #[cfg(target_os = "macos")]
    let origin = ((pos.x as f64 / scale).round() as i32, (pos.y as f64 / scale).round() as i32);
    #[cfg(not(target_os = "macos"))]
    let origin = (pos.x, pos.y);
    let image = capture::grab(origin)?;
    *app.state::<Selection>().0.lock().map_err(|e| e.to_string())? = Some(Shot { image, scale, pos: (pos.x, pos.y), size: (size.width, size.height) });

    // The page fetches the screenshot on load, so the state is set before the window exists.
    let window = match WebviewWindowBuilder::new(app, "selector", WebviewUrl::App("selector.html".into()))
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .visible_on_all_workspaces(true)
        .build()
    {
        Ok(w) => w,
        Err(e) => {
            clear_selection(app);
            return Err(e.to_string());
        }
    };
    let handle = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            clear_selection(&handle);
        }
    });
    // Position/size are applied before show; any failure must destroy the window, or an invisible
    // selector would block every later hotkey press.
    let placed = (|| {
        // Windows: tao converts Logical values with the window's *current* scale (the primary's,
        // since it is created there), so mixed-DPI setups land on the wrong monitor. The monitor
        // values are already physical virtual-desktop pixels, so pass them as-is.
        #[cfg(windows)]
        {
            window.set_position(tauri::PhysicalPosition::new(pos.x, pos.y))?;
            window.set_size(tauri::PhysicalSize::new(size.width, size.height))?;
        }
        #[cfg(not(windows))]
        {
            window.set_position(tauri::LogicalPosition::new(pos.x as f64 / scale, pos.y as f64 / scale))?;
            window.set_size(tauri::LogicalSize::new(size.width as f64 / scale, size.height as f64 / scale))?;
        }
        window.show()
    })();
    if let Err(e) = placed {
        let _ = window.destroy();
        return Err(e.to_string());
    }
    if let Err(e) = window.set_focus() {
        log::warn!("selector set_focus failed: {e}");
    }
    Ok(())
}

fn start_selection(app: &AppHandle) {
    log::info!("start_selection triggered");
    if let Some(popup) = app.get_webview_window("popup") {
        let _ = popup.close();
    }
    if app.state::<Opening>().0.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    let app = app.clone();
    // Window creation and capture must not block the main (event) thread.
    std::thread::spawn(move || {
        if let Err(e) = open_selector(&app) {
            log::error!("start_selection failed: {e}");
        }
        app.state::<Opening>().0.store(false, Ordering::Release);
    });
}

/// PNG bytes of the frozen screenshot. Sent as a raw IPC response (no base64 inflation).
#[tauri::command]
async fn get_selection_image(state: State<'_, Selection>) -> Result<Response, String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    let shot = guard.as_ref().ok_or("no screenshot")?;
    let mut png = Vec::new();
    shot.image
        .write_with_encoder(PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::NoFilter))
        .map_err(|e| e.to_string())?;
    Ok(Response::new(png))
}

/// Closes the calling window (selector or popup).
#[tauri::command]
async fn close_window(window: WebviewWindow) -> Result<(), String> {
    window.close().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_last_result(latest: State<'_, Latest>) -> Option<RegionResult> {
    latest.0.lock().ok()?.result.clone()
}

#[tauri::command]
fn open_settings_window(app: AppHandle) -> Result<(), String> {
    open_settings(&app)
}

/// Show the popup next to the selection in its "translating" state (no result yet).
fn show_popup(app: &AppHandle, shot: &Shot, rect: LogicalRect, generation: u64) -> Result<(), String> {
    const SIZE: (f64, f64) = (380.0, 220.0);
    if let Some(old) = app.get_webview_window("popup") {
        let _ = old.destroy();
    }
    // `rect` is relative to the monitor (the selector fills it), so place in monitor-local coords.
    let scale = shot.scale;
    let screen = LogicalRect { x: 0.0, y: 0.0, width: shot.size.0 as f64 / scale, height: shot.size.1 as f64 / scale };
    let (x, y) = place(rect, SIZE.0, SIZE.1, screen, 8.0);

    let window = WebviewWindowBuilder::new(app, "popup", WebviewUrl::App("popup.html".into()))
        .inner_size(SIZE.0, SIZE.1)
        .accept_first_mouse(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .visible_on_all_workspaces(true)
        .build()
        .map_err(|e| e.to_string())?;
    // Screen content is private: drop the result once the popup is gone (unless a newer one took over).
    let handle = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed)
            && let Ok(mut l) = handle.state::<Latest>().0.lock()
            && l.generation == generation
        {
            // Bump too, so an in-flight translation that finishes later is neither stored nor emitted.
            l.generation += 1;
            l.result = None;
        }
    });
    let placed = (|| {
        // Same rule as the selector: Physical on Windows (tao would use the wrong scale for
        // Logical), Logical on macOS where `shot.pos` is converted back to points.
        #[cfg(windows)]
        window.set_position(tauri::PhysicalPosition::new(
            shot.pos.0 + (x * scale).round() as i32,
            shot.pos.1 + (y * scale).round() as i32,
        ))?;
        #[cfg(not(windows))]
        window.set_position(tauri::LogicalPosition::new(shot.pos.0 as f64 / scale + x, shot.pos.1 as f64 / scale + y))?;
        window.show()
    })();
    if let Err(e) = placed {
        let _ = window.destroy();
        return Err(e.to_string());
    }
    if let Err(e) = window.set_focus() {
        log::warn!("popup set_focus failed: {e}");
    }
    Ok(())
}

async fn translate_crop(
    app: &AppHandle,
    cropped: RgbaImage,
    http: &Http,
    cache: &Mutex<TranslationCache>,
) -> RegionResult {
    let lines = match tauri::async_runtime::spawn_blocking(move || ocr::recognize(&cropped)).await {
        Ok(Ok(lines)) => lines,
        Ok(Err(e)) => return RegionResult::failed(String::new(), e),
        Err(e) => return RegionResult::failed(String::new(), e.to_string()),
    };
    let original = ocr::join_lines(&lines);
    log::debug!("recognized text: {original:?}");
    if original.trim().is_empty() {
        return RegionResult::failed(original, "No text found");
    }

    let target = settings::target_lang(app);
    let provider = settings::provider(app);
    let model = settings::ollama_model(app);
    // Provider and model are part of the cache key, so switching either never serves stale results.
    let cache_target = if provider == "ollama" { format!("ollama:{model}:{target}") } else { target.clone() };
    let cached = cache.lock().ok().and_then(|mut c| c.get(&cache_target, &original));
    let translation = match cached {
        Some(t) => t,
        None => {
            let result = if provider == "ollama" {
                Ollama::new(translate::ollama::DEFAULT_BASE.into(), model, http.ollama.clone())
                    .translate(&original, &target)
                    .await
            } else {
                let Some(key) = settings::api_key() else {
                    log::warn!("no DeepL API key set");
                    return RegionResult { needs_settings: true, ..RegionResult::failed(original, NO_KEY) };
                };
                let base = translate::deepl::base_for_key(&key).into();
                DeepL::with_client(key, base, http.deepl.clone()).translate(&original, &target).await
            };
            match result {
                Ok(t) => {
                    if let Ok(mut c) = cache.lock() {
                        c.put(&cache_target, &original, t.clone());
                    }
                    t
                }
                Err(e) => {
                    log::warn!("translation failed: {e}");
                    return RegionResult::failed(original, e);
                }
            }
        }
    };
    let Translation { text, detected_source } = translation;
    log::debug!("translation: {text:?} (from {})", detected_source.as_deref().unwrap_or("?"));
    RegionResult { original, translation: Some(text), detected_source, error: None, needs_settings: false }
}

#[tauri::command]
async fn submit_region(
    window: WebviewWindow,
    state: State<'_, Selection>,
    cache: State<'_, Cache>,
    http: State<'_, Http>,
    latest: State<'_, Latest>,
    rect: LogicalRect,
) -> Result<(), String> {
    let (shot, physical) = {
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        let shot = guard.take().ok_or("no screenshot")?;
        match capture::to_physical(rect, shot.scale, shot.image.width(), shot.image.height()) {
            Some(physical) => (shot, physical),
            None => {
                // Keep the selector usable: put the screenshot back and report to it.
                *guard = Some(shot);
                return Err(SELECTION_EMPTY.into());
            }
        }
    };
    let cropped = capture::crop(&shot.image, physical);

    let app = window.app_handle();
    let generation = {
        let mut l = latest.0.lock().map_err(|e| e.to_string())?;
        l.generation += 1;
        l.result = None;
        l.generation
    };
    // Hide the selector, focus the popup, and only then destroy the selector, so the OS doesn't
    // hand activation back to the user's app after the popup took focus.
    let _ = window.hide();
    let shown = show_popup(app, &shot, rect, generation);
    let _ = window.destroy();
    shown?;

    // Screen content is private: keep the debugging crop out of release builds.
    if cfg!(debug_assertions) {
        let saved = app.path().app_cache_dir().map_err(|e| e.to_string()).and_then(|dir| {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join("last_crop.png");
            cropped.save(&path).map_err(|e| e.to_string())?;
            Ok(path)
        });
        match saved {
            Ok(path) => log::debug!("saved crop {}x{} to {}", cropped.width(), cropped.height(), path.display()),
            Err(e) => log::warn!("saving debug crop failed: {e}"),
        }
    }

    let result = translate_crop(app, cropped, &http, &cache.0).await;
    let mut l = latest.0.lock().map_err(|e| e.to_string())?;
    if l.generation == generation {
        l.result = Some(result.clone());
        if let Err(e) = app.emit_to("popup", "result", &result) {
            log::warn!("emit result failed: {e}");
        }
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let hotkey = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyT);
    #[cfg(not(target_os = "macos"))]
    let hotkey = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyT);

    tauri::Builder::default()
        .manage(Selection::default())
        .manage(Opening::default())
        .manage(Latest::default())
        .manage(Http { deepl: translate::deepl::http_client(), ollama: translate::ollama::http_client() })
        .manage(Cache(Mutex::new(TranslationCache::new(256))))
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_log::Builder::default()
                .level(if cfg!(debug_assertions) { log::LevelFilter::Debug } else { log::LevelFilter::Info })
                .build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state() == ShortcutState::Pressed {
                        start_selection(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![get_selection_image, submit_region, close_window, get_last_result, open_settings_window, settings::get_settings, settings::save_settings, settings::clear_api_key])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            app.global_shortcut().register(hotkey)?;

            let translate = MenuItem::with_id(app, "translate", "Translate region", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&translate, &settings, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("missing default icon").clone())
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "translate" => start_selection(app),
                    "settings" => {
                        if let Err(e) = open_settings(app) {
                            log::error!("open settings failed: {e}");
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // Tray app: closing the selector must not exit. Explicit `exit(0)` still quits.
            if let RunEvent::ExitRequested { api, code: None, .. } = event {
                api.prevent_exit();
            }
        });
}
