use screen_translate_lib::settings::{DEFAULT_TARGET, TARGETS, is_valid_target};

#[test]
fn default_target_in_targets_and_valid() {
    assert!(TARGETS.contains(&DEFAULT_TARGET));
    assert!(is_valid_target(DEFAULT_TARGET));
}

#[test]
fn targets_include_expected_codes() {
    for c in ["EN-US", "EN-GB", "DE", "FR", "ES", "JA", "ZH-HANS", "KO"] {
        assert!(TARGETS.contains(&c), "missing {c}");
    }
}

#[test]
fn every_target_is_valid() {
    for c in TARGETS {
        assert!(is_valid_target(c), "{c}");
    }
}

#[test]
fn invalid_targets_rejected() {
    for c in ["", "en-us", "XX", " EN-US"] {
        assert!(!is_valid_target(c), "{c:?}");
    }
}
