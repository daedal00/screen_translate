use screen_translate_core::mode::{DEFAULT_MODE, Hotkey, Mode, mode_for, tray_labels};

const BOTH: [Mode; 2] = [Mode::Popup, Mode::Live];

#[test]
fn default_is_popup() {
    assert_eq!(Mode::default(), Mode::Popup);
    assert_eq!(DEFAULT_MODE, Mode::Popup);
    assert_eq!(Mode::from_stored(None), Mode::Popup);
    assert_eq!(Mode::from_stored(Some("bogus")), Mode::Popup);
    assert_eq!(Mode::from_stored(Some("")), Mode::Popup);
}

#[test]
fn parse_accepts_exact_values_only() {
    assert_eq!(Mode::parse("popup"), Some(Mode::Popup));
    assert_eq!(Mode::parse("live"), Some(Mode::Live));
    for bad in ["Live", "POPUP", " live", "live ", "", "ollama"] {
        assert_eq!(Mode::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn as_str_values() {
    assert_eq!(Mode::Popup.as_str(), "popup");
    assert_eq!(Mode::Live.as_str(), "live");
}

#[test]
fn persist_round_trip() {
    for m in BOTH {
        assert_eq!(Mode::parse(m.as_str()), Some(m));
        assert_eq!(Mode::from_stored(Some(m.as_str())), m);
    }
}

#[test]
fn other_swaps_and_is_involution() {
    assert_eq!(Mode::Popup.other(), Mode::Live);
    assert_eq!(Mode::Live.other(), Mode::Popup);
    for m in BOTH {
        assert_eq!(m.other().other(), m);
        assert_ne!(m.other(), m);
    }
}

#[test]
fn labels() {
    assert_eq!(Mode::Popup.label(), "Popup (one-shot)");
    assert_eq!(Mode::Live.label(), "Live (pinned region)");
}

#[test]
fn mode_for_all_combos() {
    assert_eq!(mode_for(Hotkey::Main, Mode::Popup), Mode::Popup);
    assert_eq!(mode_for(Hotkey::Main, Mode::Live), Mode::Live);
    assert_eq!(mode_for(Hotkey::Secondary, Mode::Popup), Mode::Live);
    assert_eq!(mode_for(Hotkey::Secondary, Mode::Live), Mode::Popup);
}

#[test]
fn tray_labels_exact() {
    assert_eq!(
        tray_labels(Mode::Popup),
        (
            "Translate region: Popup (one-shot)".to_string(),
            "Live (pinned region)".to_string()
        )
    );
    assert_eq!(
        tray_labels(Mode::Live),
        (
            "Translate region: Live (pinned region)".to_string(),
            "Popup (one-shot)".to_string()
        )
    );
}

#[test]
fn secondary_label_names_secondary_hotkey_mode() {
    for setting in BOTH {
        let (_, secondary) = tray_labels(setting);
        assert_eq!(secondary, mode_for(Hotkey::Secondary, setting).label());
    }
}
