use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use screen_translate_app::shell::{
    Action, action_for_menu_id, hotkey_for_id, hotkeys, log_dir, menu_items, register_hotkeys,
};
use screen_translate_core::mode::{Hotkey, Mode, tray_labels};
use screen_translate_core::update::UpdateState;

const MOD: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::SUPER
} else {
    Modifiers::CONTROL
};

#[test]
fn menu_ids_map_to_actions() {
    assert_eq!(action_for_menu_id("main"), Some(Action::Main));
    assert_eq!(action_for_menu_id("secondary"), Some(Action::Secondary));
    assert_eq!(action_for_menu_id("settings"), Some(Action::Settings));
    assert_eq!(action_for_menu_id("update"), Some(Action::Update));
    assert_eq!(action_for_menu_id("quit"), Some(Action::Quit));
}

#[test]
fn unknown_menu_id_is_none() {
    assert_eq!(action_for_menu_id("nope"), None);
    assert_eq!(action_for_menu_id(""), None);
    assert_eq!(action_for_menu_id("Quit"), None);
}

#[test]
fn menu_items_order_ids_and_labels() {
    let update = UpdateState::Idle;
    let items = menu_items(Mode::Popup, &update);
    let ids: Vec<_> = items.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, ["main", "secondary", "settings", "update", "quit"]);
    let (main, secondary) = tray_labels(Mode::Popup);
    assert_eq!(items[0].1, main);
    assert_eq!(items[1].1, secondary);
    assert_eq!(items[2].1, "Settings…");
    assert_eq!(items[3].1, update.label());
    assert_eq!(items[4].1, "Quit");
}

#[test]
fn menu_labels_change_with_mode() {
    let update = UpdateState::Idle;
    let popup = menu_items(Mode::Popup, &update);
    let live = menu_items(Mode::Live, &update);
    assert_ne!(popup[0].1, live[0].1);
    assert_ne!(popup[1].1, live[1].1);
    assert_eq!(live[0].1, tray_labels(Mode::Live).0);
    assert_eq!(live[1].1, tray_labels(Mode::Live).1);
}

#[test]
fn update_label_follows_state() {
    let state = UpdateState::Available("1.2.3".into());
    let items = menu_items(Mode::Popup, &state);
    assert_eq!(items[3].0, "update");
    assert_eq!(items[3].1, state.label());
}

#[test]
fn hotkeys_are_cmd_or_ctrl_shift_t_and_l() {
    let keys = hotkeys();
    assert_eq!(keys[0].0, Hotkey::Main);
    assert_eq!(keys[1].0, Hotkey::Secondary);
    assert_eq!(
        keys[0].1,
        HotKey::new(Some(MOD | Modifiers::SHIFT), Code::KeyT)
    );
    assert_eq!(
        keys[1].1,
        HotKey::new(Some(MOD | Modifiers::SHIFT), Code::KeyL)
    );
}

#[test]
fn hotkey_ids_round_trip() {
    for (which, key) in hotkeys() {
        assert_eq!(hotkey_for_id(key.id()), Some(which));
    }
    let ids: Vec<_> = hotkeys().iter().map(|(_, k)| k.id()).collect();
    assert_ne!(ids[0], ids[1]);
    let unknown = HotKey::new(Some(Modifiers::ALT), Code::KeyZ).id();
    assert_eq!(hotkey_for_id(unknown), None);
}

#[test]
fn register_all_ok_returns_no_warnings() {
    let mut seen = Vec::new();
    let warnings = register_hotkeys(|k| {
        seen.push(k);
        Ok::<(), String>(())
    });
    assert!(warnings.is_empty());
    assert_eq!(seen, hotkeys().map(|(_, k)| k).to_vec());
}

#[test]
fn register_first_failure_still_tries_second() {
    let keys = hotkeys();
    let mut attempts = 0;
    let warnings = register_hotkeys(|k| {
        attempts += 1;
        if k == keys[0].1 {
            Err("already registered")
        } else {
            Ok(())
        }
    });
    assert_eq!(attempts, 2);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("Shift+T"), "{}", warnings[0]);
    assert!(
        warnings[0].contains("already registered"),
        "{}",
        warnings[0]
    );
}

#[test]
fn register_second_failure_warns_once() {
    let keys = hotkeys();
    let warnings = register_hotkeys(|k| if k == keys[1].1 { Err("taken") } else { Ok(()) });
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("Shift+L"), "{}", warnings[0]);
    assert!(warnings[0].contains("taken"), "{}", warnings[0]);
}

#[test]
fn register_both_failing_gives_two_distinct_warnings() {
    let warnings = register_hotkeys(|_| Err("boom"));
    assert_eq!(warnings.len(), 2);
    assert!(warnings[0].contains("Shift+T"));
    assert!(warnings[1].contains("Shift+L"));
    assert!(warnings.iter().all(|w| w.contains("boom")));
}

#[test]
fn log_dir_matches_tauri_plugin_log_location() {
    let Some(dir) = log_dir() else {
        panic!("log_dir should resolve when HOME / LOCALAPPDATA is set");
    };
    let s = dir.to_string_lossy().replace('\\', "/");
    if cfg!(target_os = "macos") {
        assert!(s.ends_with("Library/Logs/com.screentranslate.app"), "{s}");
    } else if cfg!(target_os = "windows") {
        assert!(s.ends_with("com.screentranslate.app/logs"), "{s}");
    }
}
