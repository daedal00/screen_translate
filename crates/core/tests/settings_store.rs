use screen_translate_core::mode::Mode;
use screen_translate_core::settings::SettingsStore;
use std::fs;

// Keys as written by v0.3.x via tauri-plugin-store (flat object).
const OLD: &str = r#"{
  "target_lang": "DE",
  "provider": "local",
  "local_url": "http://127.0.0.1:8080",
  "local_model": "qwen",
  "mode": "live",
  "managed_model": "m.gguf",
  "managed_autostart": true,
  "some_future_key": {"a": 1}
}"#;

fn json(p: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn getters_read_old_plugin_store_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    fs::write(&p, OLD).unwrap();
    let s = SettingsStore::open_at(&p);
    assert_eq!(s.target_lang(), "DE");
    assert_eq!(s.provider(), "local");
    assert_eq!(s.local_url(), "http://127.0.0.1:8080");
    assert_eq!(s.local_model(), "qwen");
    assert_eq!(s.mode(), Mode::Live);
    assert_eq!(s.managed_model().as_deref(), Some("m.gguf"));
    assert!(s.managed_autostart());
}

#[test]
fn missing_file_gives_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let s = SettingsStore::open_at(dir.path().join("nope.json"));
    assert_eq!(s.target_lang(), "EN-US");
    assert_eq!(s.provider(), "deepl");
    assert_eq!(s.mode(), Mode::Popup);
    assert_eq!(s.managed_model(), None);
    assert!(!s.managed_autostart());
}

#[test]
fn legacy_ollama_file_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    fs::write(&p, r#"{"provider":"ollama","ollama_model":"llama3"}"#).unwrap();
    let s = SettingsStore::open_at(&p);
    assert_eq!(s.provider(), "local");
    assert_eq!(s.local_url(), "http://127.0.0.1:11434");
    assert_eq!(s.local_model(), "llama3");
}

#[test]
fn save_preserves_unrelated_keys_and_updates_form_keys() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    fs::write(&p, OLD).unwrap();
    let s = SettingsStore::open_at(&p);
    s.save(
        None,
        "FR".into(),
        "local".into(),
        " http://localhost:9000 ".into(),
        " m2 ".into(),
        "popup".into(),
    )
    .unwrap();
    let v = json(&p);
    assert_eq!(v["target_lang"], "FR");
    assert_eq!(v["mode"], "popup");
    assert_eq!(v["local_url"], "http://localhost:9000");
    assert_eq!(v["local_model"], "m2");
    assert_eq!(v["managed_model"], "m.gguf");
    assert_eq!(v["managed_autostart"], true);
    assert_eq!(v["some_future_key"], serde_json::json!({"a": 1}));
    let r = SettingsStore::open_at(&p);
    assert_eq!(r.target_lang(), "FR");
    assert_eq!(r.mode(), Mode::Popup);
}

#[test]
fn save_with_deepl_keeps_local_keys() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    fs::write(&p, OLD).unwrap();
    let s = SettingsStore::open_at(&p);
    s.save(
        None,
        "JA".into(),
        "deepl".into(),
        "garbage".into(),
        "x".into(),
        "live".into(),
    )
    .unwrap();
    let v = json(&p);
    assert_eq!(v["provider"], "deepl");
    assert_eq!(v["local_url"], "http://127.0.0.1:8080");
    assert_eq!(v["local_model"], "qwen");
}

#[test]
fn invalid_save_is_rejected_and_file_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    fs::write(&p, OLD).unwrap();
    let s = SettingsStore::open_at(&p);
    assert!(
        s.save(
            None,
            "XX".into(),
            "deepl".into(),
            "".into(),
            "".into(),
            "popup".into()
        )
        .is_err()
    );
    assert!(
        s.save(
            None,
            "DE".into(),
            "nope".into(),
            "".into(),
            "".into(),
            "popup".into()
        )
        .is_err()
    );
    assert!(
        s.save(
            None,
            "DE".into(),
            "deepl".into(),
            "".into(),
            "".into(),
            "bogus".into()
        )
        .is_err()
    );
    assert!(
        s.save(
            None,
            "DE".into(),
            "local".into(),
            "ftp://x".into(),
            "".into(),
            "popup".into()
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(&p).unwrap(), OLD);
}

#[test]
fn managed_setters_persist_and_preserve_others() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    fs::write(&p, OLD).unwrap();
    let s = SettingsStore::open_at(&p);
    s.set_managed_model(None).unwrap();
    s.set_managed_autostart(false).unwrap();
    let v = json(&p);
    assert!(v.get("managed_model").is_none());
    assert_eq!(v["managed_autostart"], false);
    assert_eq!(v["target_lang"], "DE");
    assert_eq!(v["some_future_key"], serde_json::json!({"a": 1}));
}
