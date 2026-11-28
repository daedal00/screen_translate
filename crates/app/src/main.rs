#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::{App, Global, QuitMode, WindowingRequest};
use screen_translate_app::shell::{self, Action};
use screen_translate_core::manager::{self, Events, ManagerState};
use screen_translate_core::mode::{Hotkey, Mode, mode_for};
use screen_translate_core::pipeline::{Latest, Live, Translator};
use screen_translate_core::settings::SettingsStore;
use screen_translate_core::update::UpdateState;

mod flow;
mod models_host;
mod native;
mod settings_window;
mod updater;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Shared app state. The tray, hotkey manager and menu handles must stay alive for the whole run.
#[allow(dead_code)] // tray and hotkeys are only held to stay alive
struct AppState {
    settings: Arc<SettingsStore>,
    manager: Arc<ManagerState>,
    translator: Arc<Translator>,
    latest: Arc<Latest>,
    live: Arc<Live>,
    /// Owns the async core work (server start, translation).
    runtime: tokio::runtime::Runtime,
    tray: TrayIcon,
    menu_items: MenuItems,
    /// The managed-server autostart task; quit waits for it so a late start can't outlive `stop`.
    autostart: Mutex<Option<tokio::task::JoinHandle<()>>>,
    hotkeys: GlobalHotKeyManager,
    update: Mutex<UpdateState>,
}

impl Global for AppState {}

type MenuItems = Vec<(&'static str, MenuItem)>;

/// Rotate the log at startup once it passes this size.
const MAX_LOG_BYTES: u64 = 1_000_000;
/// Longest quit waits for an in-flight autostart.
const AUTOSTART_JOIN: Duration = Duration::from_secs(2);

enum Event {
    Hotkey(Hotkey),
    Menu(Action),
}

fn init_logging() {
    let mut dispatch = fern::Dispatch::new()
        .format(|out, message, record| {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            out.finish(format_args!(
                "[{secs}][{}][{}] {message}",
                record.level(),
                record.target()
            ))
        })
        .level(log::LevelFilter::Info)
        .chain(std::io::stderr());
    if cfg!(debug_assertions) {
        dispatch = dispatch
            .level_for("screen_translate_core", log::LevelFilter::Debug)
            .level_for("screen_translate_app", log::LevelFilter::Debug)
            .level_for("screen_translate", log::LevelFilter::Debug);
    }
    if let Some(dir) = shell::log_dir()
        && std::fs::create_dir_all(&dir).is_ok()
        && let Ok(file) = {
            let path = dir.join("screen-translate.log");
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
                let _ = std::fs::rename(&path, dir.join("screen-translate.log.old"));
            }
            fern::log_file(path)
        }
    {
        dispatch = dispatch.chain(file);
    }
    if let Err(e) = dispatch.apply() {
        eprintln!("logging init failed: {e}");
    }
}

fn tray_icon() -> Result<Icon, String> {
    let img = image::load_from_memory(include_bytes!("../../../packaging/icons/32x32.png"))
        .map_err(|e| e.to_string())?
        .into_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).map_err(|e| e.to_string())
}

fn build_tray(settings: &SettingsStore) -> Result<(TrayIcon, MenuItems), String> {
    let items: Vec<_> = shell::menu_items(settings.mode(), &UpdateState::Idle)
        .into_iter()
        .map(|(id, label)| (id, MenuItem::with_id(id, label, true, None)))
        .collect();
    let menu = Menu::new();
    for (_, item) in &items {
        menu.append(item).map_err(|e| e.to_string())?;
    }
    let tray = TrayIconBuilder::new()
        .with_icon(tray_icon()?)
        .with_menu(Box::new(menu))
        .build()
        .map_err(|e| e.to_string())?;
    Ok((tray, items))
}

fn start_server_if_autostart(state: &AppState, events: Events) {
    let (settings, manager) = (state.settings.clone(), state.manager.clone());
    if !settings.managed_autostart() {
        return;
    }
    let Some(file) = settings.managed_model() else {
        return;
    };
    // `start` needs a tokio runtime context.
    let task = state.runtime.spawn_blocking(move || {
        if let Err(e) = manager::start(&manager, &file, &events) {
            log::warn!("managed server autostart failed: {e}");
        }
    });
    *state.autostart.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
}

fn run_hotkey(which: Hotkey, cx: &mut App) {
    match mode_for(which, cx.global::<AppState>().settings.mode()) {
        Mode::Popup => flow::start_selection(cx, false),
        Mode::Live => flow::toggle_live(cx),
    }
}

fn handle(event: Event, cx: &mut App) {
    match event {
        Event::Hotkey(which) => run_hotkey(which, cx),
        Event::Menu(Action::Main) => run_hotkey(Hotkey::Main, cx),
        Event::Menu(Action::Secondary) => run_hotkey(Hotkey::Secondary, cx),
        Event::Menu(Action::Settings) => settings_window::open(cx),
        Event::Menu(Action::Update) => updater::on_click(cx),
        Event::Menu(Action::Quit) => {
            cx.quit();
        }
    }
}

fn setup(cx: &mut App) -> Result<(), String> {
    let settings = Arc::new(SettingsStore::open()?);
    let manager = Arc::new(ManagerState::default());
    let translator = Arc::new(Translator::new(settings.clone(), manager.clone()));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    // Events arrive on the libraries' own threads; forward them to the gpui main thread.
    let (tx, mut rx) = unbounded();
    let hotkey_tx = tx.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if e.state == HotKeyState::Pressed
            && let Some(which) = shell::hotkey_for_id(e.id)
        {
            let _ = hotkey_tx.unbounded_send(Event::Hotkey(which));
        }
    }));
    MenuEvent::set_event_handler(Some(std::sync::Arc::new(move |e: MenuEvent| {
        if let Some(action) = shell::action_for_menu_id(e.id.as_ref()) {
            let _ = tx.unbounded_send(Event::Menu(action));
        }
    })));

    let hotkeys = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
    let warnings = shell::register_hotkeys(|k| hotkeys.register(k));
    for warning in &warnings {
        log::warn!("{warning}");
    }
    log::info!("hotkeys registered ({} warnings)", warnings.len());
    let (tray, menu_items) = build_tray(&settings)?;
    log::info!("tray created");

    manager::warm_hardware(&manager);
    models_host::init(cx);
    let state = AppState {
        settings,
        manager,
        translator,
        latest: Arc::default(),
        live: Arc::default(),
        runtime,
        tray,
        menu_items,
        autostart: Mutex::default(),
        hotkeys,
        update: Mutex::default(),
    };
    start_server_if_autostart(&state, models_host::events(cx));
    cx.set_global(state);
    // Runs for every exit path (tray Quit, logout, `terminate:`), so llama-server is never orphaned.
    cx.on_app_quit(|cx| {
        let state = cx.global::<AppState>();
        let task = state
            .autostart
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(task) = task
            && state
                .runtime
                .block_on(tokio::time::timeout(AUTOSTART_JOIN, task))
                .is_err()
        {
            log::warn!("autostart still running at quit");
        }
        state.live.stop();
        state.manager.server.stop();
        log::info!("screen-translate exited");
        async {}
    })
    .detach();
    updater::start_background_checks(cx);
    #[cfg(target_os = "macos")]
    warm_up_ocr();
    #[cfg(debug_assertions)]
    flow::selftest(cx);

    #[cfg(debug_assertions)]
    if let Some(which) = std::env::var_os("ST_OPEN_SETTINGS") {
        // ST_OPEN_SETTINGS=models opens the Models tab.
        let tab = usize::from(which == "models");
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            cx.update(|cx| settings_window::open_tab(cx, tab));
        })
        .detach();
    }

    cx.spawn(async move |cx| {
        while let Some(event) = rx.next().await {
            cx.update(|cx| handle(event, cx));
        }
    })
    .detach();
    Ok(())
}

/// The first Vision OCR call on macOS can take ~50s (model setup), so run one on a synthetic image shortly after
/// startup instead of on the user's first capture.
#[cfg(target_os = "macos")]
fn warm_up_ocr() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(3));
        let img = image::RgbaImage::from_fn(320, 96, |x, y| {
            let ink = (24..40).contains(&(y % 48)) && x % 12 < 8 && x > 16 && x < 300;
            image::Rgba(if ink {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            })
        });
        let start = std::time::Instant::now();
        let result = screen_translate_core::ocr::recognize(&img);
        log::debug!(
            "OCR warm-up took {:?} (ok: {})",
            start.elapsed(),
            result.is_ok()
        );
    });
}

fn main() {
    init_logging();
    log::info!("screen-translate starting");
    // Headless windowing on macOS only means "Accessory" (no Dock icon), set before launch; windows still render.
    gpui_platform::application()
        .with_windowing(WindowingRequest::Headless)
        .with_quit_mode(QuitMode::Explicit)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            // Tray app: no windows is the normal state, so quit only via `cx.quit()` (see QuitMode::Explicit).
            if let Err(e) = setup(cx) {
                log::error!("startup failed: {e}");
                cx.quit();
            }
        });
}
