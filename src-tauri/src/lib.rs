pub mod capture;
pub mod ocr;
pub mod popup;
pub mod settings;
pub mod translate;

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use image::RgbaImage;
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use capture::LogicalRect;
use popup::place;
use translate::{Translation, TranslationCache, deepl::DeepL, local::Local};

/// The monitor the selector currently covers.
#[derive(Clone, Copy)]
struct Target {
    scale: f64,
    /// Monitor origin and size in physical pixels (virtual desktop), for placing the popup.
    pos: (i32, i32),
    size: (u32, u32),
    /// Monitor origin in xcap's units: physical pixels on Windows, points on macOS.
    origin: (i32, i32),
}

#[derive(Default)]
struct Active(Mutex<Option<Target>>);

/// Set while a selector is being opened; also swallows key auto-repeat and tray+hotkey double triggers.
#[derive(Default)]
struct Opening(AtomicBool);

/// True once the popup has had focus since it was shown; only then may losing focus hide it.
#[derive(Default)]
struct Armed(AtomicBool);

struct Cache(Mutex<TranslationCache>);

struct Http {
    deepl: reqwest::Client,
    local: reqwest::Client,
}

/// Returned to the selector (which stays open) when the dragged rect maps to no pixels.
const SELECTION_EMPTY: &str = "selection is empty";
/// One composited frame, so a hidden selector is really gone before capture.
const HIDE_SETTLE: std::time::Duration = std::time::Duration::from_millis(30);
const POPUP_SIZE: (f64, f64) = (380.0, 220.0);
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

/// Keep the selector out of screenshots, so the overlay never lands in the capture. Best effort:
/// `run_region` also waits for the hide to land before capturing.
fn exclude_from_capture(window: &WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSWindow, NSWindowSharingType};
        let ptr = window.ns_window().map_err(|e| e.to_string())?;
        // SAFETY: tauri returns the live NSWindow backing this window; only a setter is called, on the main thread (setup).
        let ns = unsafe { &*(ptr as *const NSWindow) };
        ns.setSharingType(NSWindowSharingType::None);
    }
    #[cfg(windows)]
    {
        use windows::Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE},
        };
        let hwnd = window.hwnd().map_err(|e| e.to_string())?;
        unsafe { SetWindowDisplayAffinity(HWND(hwnd.0 as _), WDA_EXCLUDEFROMCAPTURE) }.map_err(|e| e.to_string())?;
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = window;
    Ok(())
}

/// Create the selector and popup once, hidden. Showing a live window is far cheaper than building a webview per use.
fn create_overlays(app: &AppHandle) -> Result<(), String> {
    let selector = WebviewWindowBuilder::new(app, "selector", WebviewUrl::App("selector.html".into()))
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .visible_on_all_workspaces(true)
        .accept_first_mouse(true)
        .build()
        .map_err(|e| e.to_string())?;
    if let Err(e) = exclude_from_capture(&selector) {
        log::warn!("excluding the selector from capture failed (relying on hide-before-capture): {e}");
    }

    let popup = WebviewWindowBuilder::new(app, "popup", WebviewUrl::App("popup.html".into()))
        .inner_size(POPUP_SIZE.0, POPUP_SIZE.1)
        .accept_first_mouse(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .visible_on_all_workspaces(true)
        .build()
        .map_err(|e| e.to_string())?;
    // Close on blur, but only after the popup has actually had focus since it was shown.
    let handle = app.clone();
    popup.on_window_event(move |event| {
        if let WindowEvent::Focused(focused) = event {
            let armed = &handle.state::<Armed>().0;
            if *focused {
                armed.store(true, Ordering::Release);
            } else if armed.load(Ordering::Acquire) {
                hide_popup(&handle);
            }
        }
    });
    Ok(())
}

/// Hide the popup and drop its content. Screen content is private; the bump also stops an in-flight
/// translation that finishes later from being stored or emitted.
fn hide_popup(app: &AppHandle) {
    app.state::<Armed>().0.store(false, Ordering::Release);
    if let Ok(mut l) = app.state::<Latest>().0.lock() {
        l.generation += 1;
        l.result = None;
    }
    if let Some(w) = app.get_webview_window("popup")
        && w.is_visible().unwrap_or(false)
    {
        let _ = w.hide();
        let _ = app.emit_to("popup", "pending", ());
    }
}

fn open_selector(app: &AppHandle) -> Result<(), String> {
    let t = Instant::now();
    let selector = app.get_webview_window("selector").ok_or("selector window missing")?;
    // Also swallows key auto-repeat.
    if selector.is_visible().unwrap_or(false) {
        return Ok(());
    }
    hide_popup(app);
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
    *app.state::<Active>().0.lock().map_err(|e| e.to_string())? =
        Some(Target { scale, pos: (pos.x, pos.y), size: (size.width, size.height), origin });

    // Windows: tao converts Logical values with the window's *current* scale, so mixed-DPI setups
    // land on the wrong monitor. The monitor values are already physical virtual-desktop pixels,
    // so pass them as-is.
    #[cfg(windows)]
    {
        selector.set_position(tauri::PhysicalPosition::new(pos.x, pos.y)).map_err(|e| e.to_string())?;
        selector.set_size(tauri::PhysicalSize::new(size.width, size.height)).map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    {
        selector.set_position(tauri::LogicalPosition::new(pos.x as f64 / scale, pos.y as f64 / scale)).map_err(|e| e.to_string())?;
        selector.set_size(tauri::LogicalSize::new(size.width as f64 / scale, size.height as f64 / scale)).map_err(|e| e.to_string())?;
    }
    let _ = app.emit_to("selector", "reset", ());
    selector.show().map_err(|e| e.to_string())?;
    if let Err(e) = selector.set_focus() {
        log::warn!("selector set_focus failed: {e}");
    }
    log::debug!("timing: selector shown in {}ms", t.elapsed().as_millis());
    Ok(())
}

fn start_selection(app: &AppHandle) {
    log::info!("start_selection triggered");
    if app.state::<Opening>().0.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    let app = app.clone();
    // Keep window work off the main (event) thread.
    std::thread::spawn(move || {
        if let Err(e) = open_selector(&app) {
            log::error!("start_selection failed: {e}");
        }
        app.state::<Opening>().0.store(false, Ordering::Release);
    });
}

/// Hides the calling window and, for the popup, drops its content.
#[tauri::command]
async fn close_window(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    match window.label() {
        "popup" => {
            hide_popup(&app);
            Ok(())
        }
        "selector" => window.hide().map_err(|e| e.to_string()),
        _ => window.close().map_err(|e| e.to_string()),
    }
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
fn show_popup(app: &AppHandle, target: &Target, rect: LogicalRect) -> Result<(), String> {
    let popup = app.get_webview_window("popup").ok_or("popup window missing")?;
    // `rect` is relative to the monitor (the selector fills it), so place in monitor-local coords.
    let scale = target.scale;
    let screen = LogicalRect { x: 0.0, y: 0.0, width: target.size.0 as f64 / scale, height: target.size.1 as f64 / scale };
    let (x, y) = place(rect, POPUP_SIZE.0, POPUP_SIZE.1, screen, 8.0);
    app.state::<Armed>().0.store(false, Ordering::Release);
    // Physical on Windows (tao would use the wrong scale for Logical), Logical on macOS.
    #[cfg(windows)]
    popup
        .set_position(tauri::PhysicalPosition::new(
            target.pos.0 + (x * scale).round() as i32,
            target.pos.1 + (y * scale).round() as i32,
        ))
        .map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    popup
        .set_position(tauri::LogicalPosition::new(target.pos.0 as f64 / scale + x, target.pos.1 as f64 / scale + y))
        .map_err(|e| e.to_string())?;
    let _ = app.emit_to("popup", "pending", ());
    popup.show().map_err(|e| e.to_string())?;
    if let Err(e) = popup.set_focus() {
        // Arm anyway so a later blur can still dismiss it.
        log::warn!("popup set_focus failed: {e}");
        app.state::<Armed>().0.store(true, Ordering::Release);
    }
    Ok(())
}

async fn translate_crop(
    app: &AppHandle,
    cropped: RgbaImage,
    http: &Http,
    cache: &Mutex<TranslationCache>,
) -> RegionResult {
    let t_ocr = std::time::Instant::now();
    let lines = match tauri::async_runtime::spawn_blocking(move || ocr::recognize(&cropped)).await {
        Ok(Ok(lines)) => lines,
        Ok(Err(e)) => return RegionResult::failed(String::new(), e),
        Err(e) => return RegionResult::failed(String::new(), e.to_string()),
    };
    let original = ocr::join_lines(&lines);
    log::debug!("recognized text: {original:?}");
    let ocr_ms = t_ocr.elapsed().as_millis();
    if original.trim().is_empty() {
        return RegionResult::failed(original, "No text found");
    }

    let target = settings::target_lang(app);
    let provider = settings::provider(app);
    let (url, model) = (settings::local_url(app), settings::local_model(app));
    // Provider, server URL and model are part of the cache key, so switching any never serves stale results.
    let cache_target = if provider == "local" { format!("local:{url}:{model}:{target}") } else { target.clone() };
    let cached = cache.lock().ok().and_then(|mut c| c.get(&cache_target, &original));
    let translation = match cached {
        Some(t) => t,
        None => {
            let result = if provider == "local" {
                Local::new(url, model, http.local.clone())
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
    log::debug!("timing: ocr {ocr_ms}ms ({} chars), translate {}ms", original.chars().count(), t_ocr.elapsed().as_millis() - ocr_ms);
    log::debug!("translation: {text:?} (from {})", detected_source.as_deref().unwrap_or("?"));
    RegionResult { original, translation: Some(text), detected_source, error: None, needs_settings: false }
}

#[tauri::command]
async fn submit_region(app: AppHandle, rect: LogicalRect) -> Result<(), String> {
    run_region(&app, rect).await
}

/// Store and emit `result` unless a newer request (or a hide) superseded `generation`.
fn finish(app: &AppHandle, generation: u64, result: RegionResult) {
    let latest = app.state::<Latest>();
    let Ok(mut l) = latest.0.lock() else { return };
    if l.generation == generation {
        l.result = Some(result.clone());
        if let Err(e) = app.emit_to("popup", "result", &result) {
            log::warn!("emit result failed: {e}");
        }
    }
}

/// Hide the selector, capture `rect` (selector-local logical px) from the screen, show the popup, OCR + translate.
async fn run_region(app: &AppHandle, rect: LogicalRect) -> Result<(), String> {
    let t0 = Instant::now();
    let target = (*app.state::<Active>().0.lock().map_err(|e| e.to_string())?).ok_or("no active selection")?;
    let (logical_w, logical_h) = (target.size.0 as f64 / target.scale, target.size.1 as f64 / target.scale);
    // Reject an empty rect while the selector is still up, so the user can drag again.
    if capture::to_physical(rect, 1.0, logical_w.ceil() as u32, logical_h.ceil() as u32).is_none() {
        return Err(SELECTION_EMPTY.into());
    }
    let cache = app.state::<Cache>();
    let http = app.state::<Http>();
    let generation = {
        let latest = app.state::<Latest>();
        let mut l = latest.0.lock().map_err(|e| e.to_string())?;
        l.generation += 1;
        l.result = None;
        l.generation
    };
    // Hide must be a real barrier before capturing: wait for the main thread to hide the window,
    // then one composited frame, in case exclusion from capture is unavailable (Windows < 2004).
    let selector = app.get_webview_window("selector");
    let (origin, scale) = (target.origin, target.scale);
    let hide_app = app.clone();
    let t_cap = Instant::now();
    let captured = tauri::async_runtime::spawn_blocking(move || {
        if let Some(w) = selector {
            let (tx, rx) = std::sync::mpsc::channel();
            let queued = hide_app.run_on_main_thread(move || {
                let _ = w.hide();
                let _ = tx.send(());
            });
            if queued.is_err() || rx.recv_timeout(std::time::Duration::from_millis(500)).is_err() {
                log::warn!("selector hide was not confirmed");
            }
            std::thread::sleep(HIDE_SETTLE);
        }
        let hidden = t_cap.elapsed().as_millis();
        (hidden, capture::grab_region(origin, rect, scale))
    })
    .await
    .map_err(|e| e.to_string())?;
    let (hidden_ms, captured) = captured;
    log::debug!("timing: hide barrier {hidden_ms}ms, capture {}ms", t_cap.elapsed().as_millis() - hidden_ms);
    // A newer hotkey press or a hide superseded this request while capturing.
    let current = app.state::<Latest>().0.lock().map_err(|e| e.to_string())?.generation == generation;
    if !current {
        return Ok(());
    }
    let cropped = match captured {
        Ok(Some(img)) => img,
        other => {
            let msg = other.err().unwrap_or_else(|| "Selection is empty".into());
            show_popup(app, &target, rect)?;
            finish(app, generation, RegionResult::failed(String::new(), msg));
            return Ok(());
        }
    };
    show_popup(app, &target, rect)?;
    log::debug!("timing: mouseup->popup visible {}ms", t0.elapsed().as_millis());

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
    finish(app, generation, result);
    Ok(())
}

/// Debug-only: `ST_SELFTEST="x,y,w,h"` drives hotkey -> selector -> region submit without input.
#[cfg(debug_assertions)]
fn selftest(app: AppHandle) {
    let Ok(spec) = std::env::var("ST_SELFTEST") else { return };
    let v: Vec<f64> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    let [x, y, width, height] = v[..] else {
        log::error!("selftest: ST_SELFTEST must be x,y,w,h");
        return;
    };
    std::thread::spawn(move || {
        // Let the hidden selector and popup pages load.
        std::thread::sleep(std::time::Duration::from_secs(3));
        let t = Instant::now();
        start_selection(&app);
        let visible = || app.get_webview_window("selector").and_then(|w| w.is_visible().ok()).unwrap_or(false);
        while !visible() {
            if t.elapsed() > std::time::Duration::from_secs(15) {
                return log::error!("selftest: selector never visible");
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let shown = t.elapsed().as_millis();
        let result = tauri::async_runtime::block_on(run_region(&app, LogicalRect { x, y, width, height }));
        log::info!("selftest: hotkey->visible {shown}ms, region total {}ms, result {result:?}", (t.elapsed().as_millis() - shown));
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let hotkey = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyT);
    #[cfg(not(target_os = "macos"))]
    let hotkey = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyT);

    tauri::Builder::default()
        .manage(Active::default())
        .manage(Armed::default())
        .manage(Opening::default())
        .manage(Latest::default())
        .manage(Http { deepl: translate::deepl::http_client(), local: translate::local::http_client() })
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
        .invoke_handler(tauri::generate_handler![submit_region, close_window, get_last_result, open_settings_window, settings::get_settings, settings::save_settings, settings::clear_api_key])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            app.global_shortcut().register(hotkey)?;
            create_overlays(app.handle())?;

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
            #[cfg(debug_assertions)]
            selftest(app.handle().clone());
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
