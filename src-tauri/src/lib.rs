pub mod capture;
pub mod live;
pub mod ocr;
pub mod popup;
pub mod settings;
pub mod translate;

use std::collections::HashMap;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use image::RgbaImage;
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use capture::LogicalRect;
use live::{ChangeGate, Fingerprint, OverlayLine};
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

/// True when the open selector pins a live region instead of running the one-shot popup.
#[derive(Default)]
struct LiveSelect(AtomicBool);

/// The running live session, if any. Events for a session are only emitted under this lock while it
/// is still current, so a stopped or restarted session can never emit after `clear`.
#[derive(Default)]
struct Live(Mutex<LiveInner>);

#[derive(Default)]
struct LiveInner {
    generation: u64,
    running: bool,
    /// What the overlay currently shows, so a freshly loaded overlay page can fetch it.
    lines: Vec<OverlayLine>,
}

/// Outcome of one live fire. `error` is set when some lines (or all) failed; `lines` holds the rest.
struct Fire {
    lines: Vec<OverlayLine>,
    hits: usize,
    misses: usize,
    error: Option<String>,
}

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
const LIVE_TICK: Duration = Duration::from_millis(500);
/// Cells that must change (see `live::distance`) before a live frame counts as new; stable for 2 ticks first.
const LIVE_THRESHOLD: u32 = 4;
const LIVE_SETTLE: u32 = 2;
/// Pause after a failed fire, so a persistent failure doesn't hammer the provider.
const LIVE_BACKOFF: Duration = Duration::from_secs(2);
/// In-flight translations per fire (DeepL Free rate-limits bursts).
const LIVE_CONCURRENCY: usize = 4;
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
    // Live overlay: sits over the captured region, so it must be click-through and excluded from capture.
    let live = WebviewWindowBuilder::new(app, "live", WebviewUrl::App("live.html".into()))
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focusable(false)
        .visible(false)
        .visible_on_all_workspaces(true)
        .build()
        .map_err(|e| e.to_string())?;
    live.set_ignore_cursor_events(true).map_err(|e| e.to_string())?;
    if let Err(e) = exclude_from_capture(&live) {
        log::warn!("excluding the live overlay from capture failed (it may be translated back into itself): {e}");
    }

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

fn open_selector(app: &AppHandle, live: bool) -> Result<(), String> {
    let t = Instant::now();
    let selector = app.get_webview_window("selector").ok_or("selector window missing")?;
    // Also swallows key auto-repeat.
    if selector.is_visible().unwrap_or(false) {
        return Ok(());
    }
    app.state::<LiveSelect>().0.store(live, Ordering::Release);
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

fn start_selection(app: &AppHandle, live: bool) {
    log::info!("start_selection triggered (live: {live})");
    if app.state::<Opening>().0.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    let app = app.clone();
    // Keep window work off the main (event) thread.
    std::thread::spawn(move || {
        if let Err(e) = open_selector(&app, live) {
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

/// Translate `text` through the cache or the configured provider. The bool is true on a cache hit.
async fn translate_text(
    app: &AppHandle,
    text: &str,
    http: &Http,
    cache: &Mutex<TranslationCache>,
) -> Result<(Translation, bool), String> {
    let target = settings::target_lang(app);
    let provider = settings::provider(app);
    let (url, model) = (settings::local_url(app), settings::local_model(app));
    // Provider, server URL and model are part of the cache key, so switching any never serves stale results.
    let cache_target = if provider == "local" { format!("local:{url}:{model}:{target}") } else { target.clone() };
    if let Some(t) = cache.lock().ok().and_then(|mut c| c.get(&cache_target, text)) {
        return Ok((t, true));
    }
    let t = if provider == "local" {
        Local::new(url, model, http.local.clone()).translate(text, &target).await?
    } else {
        let Some(key) = settings::api_key() else {
            log::warn!("no DeepL API key set");
            return Err(NO_KEY.into());
        };
        let base = translate::deepl::base_for_key(&key).into();
        DeepL::with_client(key, base, http.deepl.clone()).translate(text, &target).await?
    };
    if let Ok(mut c) = cache.lock() {
        c.put(&cache_target, text, t.clone());
    }
    Ok((t, false))
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

    let translation = match translate_text(app, &original, http, cache).await {
        Ok((t, _)) => t,
        Err(e) => {
            log::warn!("translation failed: {e}");
            return RegionResult { needs_settings: e == NO_KEY, ..RegionResult::failed(original, e) };
        }
    };
    let Translation { text, detected_source } = translation;
    log::debug!("timing: ocr {ocr_ms}ms ({} chars), translate {}ms", original.chars().count(), t_ocr.elapsed().as_millis() - ocr_ms);
    log::debug!("translation: {text:?} (from {})", detected_source.as_deref().unwrap_or("?"));
    RegionResult { original, translation: Some(text), detected_source, error: None, needs_settings: false }
}

#[tauri::command]
async fn submit_region(app: AppHandle, rect: LogicalRect) -> Result<(), String> {
    if app.state::<LiveSelect>().0.load(Ordering::Acquire) {
        start_live_region(&app, rect).await
    } else {
        run_region(&app, rect).await
    }
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

/// The selector's monitor, or `SELECTION_EMPTY` (rejected while the selector is still up, so the user can drag again).
fn active_target(app: &AppHandle, rect: LogicalRect) -> Result<Target, String> {
    let target = (*app.state::<Active>().0.lock().map_err(|e| e.to_string())?).ok_or("no active selection")?;
    let (logical_w, logical_h) = (target.size.0 as f64 / target.scale, target.size.1 as f64 / target.scale);
    if capture::to_physical(rect, 1.0, logical_w.ceil() as u32, logical_h.ceil() as u32).is_none() {
        return Err(SELECTION_EMPTY.into());
    }
    Ok(target)
}

/// Hide the selector and wait for it to be gone: for the main thread to hide the window, then one
/// composited frame, in case exclusion from capture is unavailable (Windows < 2004).
fn hide_selector_blocking(app: &AppHandle) {
    let Some(w) = app.get_webview_window("selector") else { return };
    let (tx, rx) = std::sync::mpsc::channel();
    let queued = app.run_on_main_thread(move || {
        let _ = w.hide();
        let _ = tx.send(());
    });
    if queued.is_err() || rx.recv_timeout(Duration::from_millis(500)).is_err() {
        log::warn!("selector hide was not confirmed");
    }
    std::thread::sleep(HIDE_SETTLE);
}

/// Hide the selector, capture `rect` (selector-local logical px) from the screen, show the popup, OCR + translate.
async fn run_region(app: &AppHandle, rect: LogicalRect) -> Result<(), String> {
    let t0 = Instant::now();
    let target = active_target(app, rect)?;
    let cache = app.state::<Cache>();
    let http = app.state::<Http>();
    let generation = {
        let latest = app.state::<Latest>();
        let mut l = latest.0.lock().map_err(|e| e.to_string())?;
        l.generation += 1;
        l.result = None;
        l.generation
    };
    // Hide must be a real barrier before capturing.
    let (origin, scale) = (target.origin, target.scale);
    let hide_app = app.clone();
    let t_cap = Instant::now();
    let captured = tauri::async_runtime::spawn_blocking(move || {
        hide_selector_blocking(&hide_app);
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

fn live_running(app: &AppHandle) -> bool {
    app.state::<Live>().0.lock().is_ok_and(|l| l.running)
}

fn toggle_live(app: &AppHandle) {
    if live_running(app) {
        stop_live(app);
    } else {
        start_selection(app, true);
    }
}

/// End the session: no further events, overlay cleared and hidden.
fn stop_live(app: &AppHandle) {
    {
        let live = app.state::<Live>();
        let Ok(mut l) = live.0.lock() else { return };
        if !l.running {
            return;
        }
        l.running = false;
        l.generation += 1;
        l.lines.clear();
        let _ = app.emit_to("live", "clear", ());
    }
    if let Some(w) = app.get_webview_window("live") {
        let _ = w.hide();
    }
    log::info!("live stopped");
}

/// Replace what the overlay shows (and its error, if any) unless `session` was stopped or replaced.
fn live_show(app: &AppHandle, session: u64, lines: Vec<OverlayLine>, error: Option<&str>) {
    let live = app.state::<Live>();
    let Ok(mut l) = live.0.lock() else { return };
    if l.running && l.generation == session {
        let _ = app.emit_to("live", "lines", &lines);
        if let Some(e) = error {
            let _ = app.emit_to("live", "error", e);
        }
        l.lines = lines;
    }
}

/// The overlay page asks for the current lines when it loads, in case it missed an emit.
#[tauri::command]
fn get_live_lines(live: State<'_, Live>) -> Vec<OverlayLine> {
    live.0.lock().map(|l| l.lines.clone()).unwrap_or_default()
}

fn live_current(app: &AppHandle, session: u64) -> bool {
    app.state::<Live>().0.lock().is_ok_and(|l| l.running && l.generation == session)
}

/// Hide the selector and pin `rect` as the live region.
async fn start_live_region(app: &AppHandle, rect: LogicalRect) -> Result<(), String> {
    let target = active_target(app, rect)?;
    let hide_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || hide_selector_blocking(&hide_app)).await.map_err(|e| e.to_string())?;
    let overlay = app.get_webview_window("live").ok_or("live overlay missing")?;
    // Same per-platform units as the selector in `open_selector`. Size first: on macOS a resize
    // keeps the bottom-left corner fixed, which would move a position set earlier.
    let scale = target.scale;
    #[cfg(windows)]
    {
        overlay
            .set_size(tauri::PhysicalSize::new((rect.width * scale).round() as u32, (rect.height * scale).round() as u32))
            .map_err(|e| e.to_string())?;
        overlay
            .set_position(tauri::PhysicalPosition::new(
                target.pos.0 + (rect.x * scale).round() as i32,
                target.pos.1 + (rect.y * scale).round() as i32,
            ))
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    {
        overlay.set_size(tauri::LogicalSize::new(rect.width, rect.height)).map_err(|e| e.to_string())?;
        overlay
            .set_position(tauri::LogicalPosition::new(target.pos.0 as f64 / scale + rect.x, target.pos.1 as f64 / scale + rect.y))
            .map_err(|e| e.to_string())?;
    }
    overlay.show().map_err(|e| e.to_string())?;
    // Claim the session only once the window is up, so a failure above can't leave a phantom session.
    let session = {
        let live = app.state::<Live>();
        let mut l = live.0.lock().map_err(|e| e.to_string())?;
        l.generation += 1;
        l.running = true;
        l.lines.clear();
        l.generation
    };
    let _ = app.emit_to("live", "clear", ());
    log::info!("live started on {rect:?}");
    let app = app.clone();
    std::thread::spawn(move || live_loop(app, target, rect, session));
    Ok(())
}

fn live_loop(app: AppHandle, target: Target, rect: LogicalRect, session: u64) {
    let new_gate = || ChangeGate::new(LIVE_THRESHOLD, LIVE_SETTLE);
    let mut gate = new_gate();
    let mut last_fired: Option<Fingerprint> = None;
    let mut last_error = String::new();
    while live_current(&app, session) {
        std::thread::sleep(LIVE_TICK);
        let img = match capture::grab_region(target.origin, rect, target.scale) {
            Ok(Some(img)) => img,
            Ok(None) => continue,
            Err(e) => {
                // Re-fire on the next good settled frame, even if the content stays static.
                gate = new_gate();
                // The same failure every tick would flood the log.
                if e != last_error {
                    log::warn!("live capture failed: {e}");
                    live_show(&app, session, Vec::new(), Some(&e));
                    last_error = e;
                }
                continue;
            }
        };
        let fp = live::fingerprint(&img);
        if !gate.tick(fp.clone()) {
            continue;
        }
        let t = Instant::now();
        let dist = last_fired.as_ref().map(|p| live::distance(p, &fp));
        last_fired = Some(fp);
        let fire = match live_translate(&app, session, &img, rect) {
            Ok(Some(fire)) => fire,
            Ok(None) => continue,
            Err(e) => Fire { lines: Vec::new(), hits: 0, misses: 0, error: Some(e) },
        };
        // Screen text is private: per-fire detail stays at debug.
        log::debug!(
            "live fire: distance {dist:?}, {} lines, cache {} hit / {} miss, {}ms",
            fire.lines.len(),
            fire.hits,
            fire.misses,
            t.elapsed().as_millis()
        );
        for l in &fire.lines {
            log::debug!("live line: {:?}", l.text);
        }
        last_error = fire.error.clone().unwrap_or_default();
        live_show(&app, session, fire.lines, fire.error.as_deref());
        if let Some(e) = &fire.error {
            log::warn!("live translate failed: {e}");
            // Fire again on the next settled frame, after a pause.
            gate = new_gate();
            std::thread::sleep(LIVE_BACKOFF);
        }
    }
}

/// OCR `img` and translate each distinct line, a few at a time. Lines that fail are left out and
/// reported in `error`. `Ok(None)` when the session ended meanwhile.
fn live_translate(app: &AppHandle, session: u64, img: &RgbaImage, rect: LogicalRect) -> Result<Option<Fire>, String> {
    let lines = ocr::recognize(img)?;
    if !live_current(app, session) {
        return Ok(None);
    }
    let mut texts: Vec<String> = lines.iter().map(|l| l.text.trim().to_owned()).filter(|t| !t.is_empty()).collect();
    texts.sort();
    texts.dedup();
    let results = tauri::async_runtime::block_on(async {
        let mut out = Vec::new();
        for chunk in texts.chunks(LIVE_CONCURRENCY) {
            let tasks: Vec<_> = chunk
                .iter()
                .cloned()
                .map(|text| {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let (http, cache) = (app.state::<Http>(), app.state::<Cache>());
                        let r = translate_text(&app, &text, &http, &cache.0).await;
                        (text, r)
                    })
                })
                .collect();
            for task in tasks {
                out.push(task.await.map_err(|e| e.to_string())?);
            }
        }
        Ok::<_, String>(out)
    })?;
    if !live_current(app, session) {
        return Ok(None);
    }
    let (mut hits, mut misses, mut error) = (0, 0, None);
    let mut translated = HashMap::new();
    for (text, r) in results {
        match r {
            Ok((t, hit)) => {
                if hit { hits += 1 } else { misses += 1 }
                translated.insert(text, t.text);
            }
            Err(e) => {
                error.get_or_insert(e);
            }
        }
    }
    let pairs: Vec<_> = lines
        .iter()
        .filter_map(|l| Some((l.bbox, translated.get(l.text.trim())?.clone())))
        .collect();
    // The image may be physical pixels where the rect is logical (retina), so derive the scale from both.
    let scale = img.width() as f64 / rect.width;
    Ok(Some(Fire { lines: live::layout(&pairs, scale), hits, misses, error }))
}

/// Debug-only: env var `var` = "x,y,w,h" drives hotkey -> selector -> region submit without input
/// (`ST_SELFTEST`: one-shot popup, `ST_LIVE_SELFTEST`: live session).
#[cfg(debug_assertions)]
fn selftest(app: AppHandle, var: &str, live: bool) {
    let Ok(spec) = std::env::var(var) else { return };
    let v: Vec<f64> = spec.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    let [x, y, width, height] = v[..] else {
        log::error!("selftest: {var} must be x,y,w,h");
        return;
    };
    std::thread::spawn(move || {
        // Let the hidden selector and popup pages load.
        std::thread::sleep(std::time::Duration::from_secs(3));
        let t = Instant::now();
        start_selection(&app, live);
        let visible = || app.get_webview_window("selector").and_then(|w| w.is_visible().ok()).unwrap_or(false);
        while !visible() {
            if t.elapsed() > std::time::Duration::from_secs(15) {
                return log::error!("selftest: selector never visible");
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let shown = t.elapsed().as_millis();
        let rect = LogicalRect { x, y, width, height };
        let result = tauri::async_runtime::block_on(async {
            if live { start_live_region(&app, rect).await } else { run_region(&app, rect).await }
        });
        log::info!("selftest: hotkey->visible {shown}ms, region total {}ms, result {result:?}", (t.elapsed().as_millis() - shown));
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let hotkey = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyT);
    #[cfg(not(target_os = "macos"))]
    let hotkey = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyT);
    #[cfg(target_os = "macos")]
    let live_hotkey = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyL);
    #[cfg(not(target_os = "macos"))]
    let live_hotkey = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyL);

    tauri::Builder::default()
        .manage(Active::default())
        .manage(Armed::default())
        .manage(Opening::default())
        .manage(Latest::default())
        .manage(LiveSelect::default())
        .manage(Live::default())
        .manage(Http { deepl: translate::deepl::http_client(), local: translate::local::http_client() })
        .manage(Cache(Mutex::new(TranslationCache::new(256))))
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_log::Builder::default()
                .level(if cfg!(debug_assertions) { log::LevelFilter::Debug } else { log::LevelFilter::Info })
                .build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        if *shortcut == live_hotkey {
                            toggle_live(app);
                        } else {
                            start_selection(app, false);
                        }
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![submit_region, close_window, get_last_result, get_live_lines, open_settings_window, settings::get_settings, settings::save_settings, settings::clear_api_key])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            app.global_shortcut().register(hotkey)?;
            app.global_shortcut().register(live_hotkey)?;
            create_overlays(app.handle())?;

            let translate = MenuItem::with_id(app, "translate", "Translate region", true, None::<&str>)?;
            let live = MenuItem::with_id(app, "live", "Live mode", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&translate, &live, &settings, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("missing default icon").clone())
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "translate" => start_selection(app, false),
                    "live" => toggle_live(app),
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
            {
                selftest(app.handle().clone(), "ST_SELFTEST", false);
                selftest(app.handle().clone(), "ST_LIVE_SELFTEST", true);
            }
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
