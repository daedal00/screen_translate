//! Pure shell logic: tray menu, global hotkeys, log location. No gpui, so it is unit-testable.

use std::path::PathBuf;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use screen_translate_core::mode::{Hotkey, Mode, tray_labels};
use screen_translate_core::update::UpdateState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Main,
    Secondary,
    Settings,
    Update,
    Quit,
}

pub fn action_for_menu_id(id: &str) -> Option<Action> {
    match id {
        "main" => Some(Action::Main),
        "secondary" => Some(Action::Secondary),
        "settings" => Some(Action::Settings),
        "update" => Some(Action::Update),
        "quit" => Some(Action::Quit),
        _ => None,
    }
}

/// Tray menu entries as (id, label), in display order.
pub fn menu_items(mode: Mode, update: &UpdateState) -> Vec<(&'static str, String)> {
    let (main, secondary) = tray_labels(mode);
    vec![
        ("main", main),
        ("secondary", secondary),
        ("settings", "Settings…".into()),
        ("update", update.label()),
        ("quit", "Quit".into()),
    ]
}

#[cfg(target_os = "macos")]
const MOD: Modifiers = Modifiers::SUPER;
#[cfg(not(target_os = "macos"))]
const MOD: Modifiers = Modifiers::CONTROL;

#[cfg(target_os = "macos")]
const MOD_NAME: &str = "Cmd";
#[cfg(not(target_os = "macos"))]
const MOD_NAME: &str = "Ctrl";

pub fn hotkeys() -> [(Hotkey, HotKey); 2] {
    let mods = Some(MOD | Modifiers::SHIFT);
    [
        (Hotkey::Main, HotKey::new(mods, Code::KeyT)),
        (Hotkey::Secondary, HotKey::new(mods, Code::KeyL)),
    ]
}

pub fn hotkey_for_id(id: u32) -> Option<Hotkey> {
    hotkeys()
        .into_iter()
        .find(|(_, k)| k.id() == id)
        .map(|(which, _)| which)
}

/// Tries every hotkey; a failure never stops the others. Returns one warning per failure.
pub fn register_hotkeys<E: std::fmt::Display>(
    mut register: impl FnMut(HotKey) -> Result<(), E>,
) -> Vec<String> {
    let names = [(Hotkey::Main, "T"), (Hotkey::Secondary, "L")];
    hotkeys()
        .into_iter()
        .zip(names)
        .filter_map(|((_, key), (_, letter))| {
            register(key)
                .err()
                .map(|e| format!("{MOD_NAME}+Shift+{letter} is in use by another app: {e}"))
        })
        .collect()
}

/// Same directory tauri-plugin-log used for the app identifier.
pub fn log_dir() -> Option<PathBuf> {
    const ID: &str = "com.screentranslate.app";
    #[cfg(target_os = "macos")]
    return std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Library").join("Logs").join(ID));
    #[cfg(windows)]
    return std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(ID).join("logs"));
    #[cfg(not(any(target_os = "macos", windows)))]
    return std::env::var_os("HOME").map(|h| {
        PathBuf::from(h)
            .join(".local")
            .join("share")
            .join(ID)
            .join("logs")
    });
}
