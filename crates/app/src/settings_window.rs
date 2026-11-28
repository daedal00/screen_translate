//! Opens the Settings window (at most one) and keeps the tray labels in step with what it saves.

use gpui::{
    App, AppContext, Bounds, Global, TitlebarOptions, WindowBounds, WindowHandle, WindowOptions,
    px, size,
};
use gpui_component::{Root, Theme};
use screen_translate_app::settings::{SettingsEvent, SettingsView};
use screen_translate_app::shell;
use screen_translate_core::settings::api_key;

use crate::{AppState, models_host, updater};

/// Size of the Tauri settings window.
const SIZE: (f32, f32) = (720.0, 680.0);
const MIN_SIZE: (f32, f32) = (560.0, 520.0);

struct Open(Option<WindowHandle<Root>>);

impl Global for Open {}

/// Opens Settings, or brings the open one to the front. The app is an Accessory app, so it must activate itself.
pub fn open(cx: &mut App) {
    open_tab(cx, 0);
}

/// `open`, showing tab `tab` when the window is created (an open one keeps its tab).
pub fn open_tab(cx: &mut App, tab: usize) {
    if let Some(handle) = cx.try_global::<Open>().and_then(|o| o.0) {
        // Errors once the user has closed the window.
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            cx.activate(true);
            return;
        }
    }
    match open_window(cx, tab) {
        Ok(handle) => {
            cx.set_global(Open(Some(handle)));
            cx.activate(true);
        }
        Err(e) => log::error!("opening settings failed: {e}"),
    }
}

fn open_window(cx: &mut App, tab: usize) -> Result<WindowHandle<Root>, String> {
    let store = cx.global::<AppState>().settings.clone();
    let has_key = matches!(api_key(), Ok(Some(_)));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(SIZE.0), px(SIZE.1)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Screen Translate Settings".into()),
            ..Default::default()
        }),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        focus: true,
        show: true,
        ..Default::default()
    };
    let handle = cx
        .open_window(options, |window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
            let view = cx.new(|cx| SettingsView::new(store, window, cx));
            view.update(cx, |v, cx| {
                v.set_has_key(has_key, window, cx);
                v.select_tab(tab, cx);
            });
            models_host::attach(cx, window.window_handle(), &view);
            cx.subscribe(&view, |_, event, cx| match event {
                SettingsEvent::Saved => refresh_tray(cx),
                SettingsEvent::KeyChanged(key) => {
                    cx.global::<AppState>().translator.set_api_key(key.clone())
                }
            })
            .detach();
            window.activate_window();
            // The system can switch between light and dark while Settings is open.
            window
                .observe_window_appearance(|window, cx| {
                    Theme::sync_system_appearance(Some(window), cx)
                })
                .detach();
            cx.new(|cx| Root::new(view, window, cx))
        })
        .map_err(|e| e.to_string())?;
    Ok(handle)
}

/// The tray's main/secondary labels follow the mode, which Settings can change.
fn refresh_tray(cx: &mut App) {
    let update = updater::current(cx);
    let state = cx.global::<AppState>();
    let labels = shell::menu_items(state.settings.mode(), &update);
    for (id, item) in &state.menu_items {
        if matches!(*id, "main" | "secondary")
            && let Some((_, label)) = labels.iter().find(|(i, _)| i == id)
        {
            item.set_text(label);
        }
    }
}
