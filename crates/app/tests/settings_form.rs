//! Milestone 3c RED: Settings General tab. Needs `screen_translate_app::settings`.
//! Exact API under test:
//!   SettingsForm { pub mode: Mode, pub target_lang: String, pub provider: String, pub key: String,
//!                  pub has_key: bool, pub local_url: String, pub local_model: String }
//!   SettingsForm::load(&SettingsStore, has_key: bool) -> Self
//!   deepl_fields_visible / local_fields_visible / remote_hint_visible(&self) -> bool, key_placeholder(&self) -> &str
//!   save(&mut self, &SettingsStore, impl FnOnce(&str) -> Result<(), String>) -> Result<String, String>
//!   remove_key(&mut self, impl FnOnce() -> Result<(), String>) -> Result<String, String>
//!   reload_local(&mut self, &SettingsStore)
//!   SettingsView::new(Arc<SettingsStore>, &mut Window, &mut Context<SettingsView>) -> SettingsView
//! `save` validates BEFORE calling save_key (an invalid form never touches the keyring), then calls save_key(trimmed key)
//! only when the key is non-empty, then persists. If save_key fails nothing is persisted. Errors are "Error: <msg>".
use std::{cell::Cell, sync::Arc};

use gpui::{AppContext, TestAppContext};
use screen_translate_app::settings::{SettingsForm, SettingsView};
use screen_translate_core::{mode::Mode, settings::SettingsStore};
use tempfile::TempDir;

fn store_with(json: &str) -> (TempDir, SettingsStore) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    if !json.is_empty() {
        std::fs::write(&path, json).unwrap();
    }
    let store = SettingsStore::open_at(&path);
    (dir, store)
}

fn form_with_url(url: &str) -> SettingsForm {
    let (_d, s) = store_with("");
    let mut f = SettingsForm::load(&s, false);
    f.local_url = url.into();
    f
}

fn never_key(_: &str) -> Result<(), String> {
    panic!("save_key must not be called");
}

#[test]
fn load_from_old_format_file() {
    let (_d, s) =
        store_with(r#"{"provider":"ollama","ollama_model":"qwen2.5:7b","target_lang":"DE"}"#);
    let f = SettingsForm::load(&s, true);
    assert_eq!(f.mode, Mode::Popup);
    assert_eq!(f.target_lang, "DE");
    assert_eq!(f.provider, "local");
    assert_eq!(f.local_url, "http://127.0.0.1:11434");
    assert_eq!(f.local_model, "qwen2.5:7b");
    assert_eq!(f.key, "");
    assert!(f.has_key);
}

#[test]
fn load_defaults_and_stored_mode() {
    let (_d, s) = store_with("");
    let f = SettingsForm::load(&s, false);
    assert_eq!(
        (f.mode, f.target_lang.as_str(), f.provider.as_str()),
        (Mode::Popup, "EN-US", "deepl")
    );
    assert!(!f.has_key);
    let (_d, s) = store_with(r#"{"mode":"live"}"#);
    assert_eq!(SettingsForm::load(&s, false).mode, Mode::Live);
}

#[test]
fn visibility_follows_provider() {
    let (_d, s) = store_with("");
    let mut f = SettingsForm::load(&s, false);
    assert!(f.deepl_fields_visible() && !f.local_fields_visible());
    f.provider = "local".into();
    assert!(!f.deepl_fields_visible() && f.local_fields_visible());
}

#[test]
fn key_placeholder_reflects_has_key() {
    let (_d, s) = store_with("");
    assert_eq!(SettingsForm::load(&s, true).key_placeholder(), "saved");
    assert_eq!(SettingsForm::load(&s, false).key_placeholder(), "");
}

#[test]
fn remote_hint_cases() {
    for local in [
        "http://127.0.0.1:8080",
        "http://localhost:11434",
        "http://[::1]:8080",
        "garbage",
        "",
    ] {
        assert!(
            !form_with_url(local).remote_hint_visible(),
            "{local} should count as local"
        );
    }
    assert!(form_with_url("http://192.168.1.5:8080").remote_hint_visible());
    assert!(form_with_url("https://example.com/v1").remote_hint_visible());
}

#[test]
fn save_persists_and_clears_key() {
    let (_d, s) = store_with("");
    let mut f = SettingsForm::load(&s, false);
    f.mode = Mode::Live;
    f.target_lang = "JA".into();
    f.provider = "local".into();
    f.local_url = "http://localhost:9999".into();
    f.local_model = " m1 ".into();
    f.key = " secret ".into();
    let got = Cell::new(None);
    let status = f
        .save(&s, |k| {
            got.set(Some(k.to_owned()));
            Ok(())
        })
        .unwrap();
    assert_eq!(status, "Saved.");
    assert_eq!(got.take().as_deref(), Some("secret"));
    assert_eq!(f.key, "");
    assert!(f.has_key);
    assert_eq!(s.mode(), Mode::Live);
    assert_eq!(s.target_lang(), "JA");
    assert_eq!(s.provider(), "local");
    assert_eq!(s.local_url(), "http://localhost:9999");
    assert_eq!(s.local_model(), "m1");
}

#[test]
fn save_without_key_does_not_call_save_key_and_keeps_has_key() {
    let (_d, s) = store_with("");
    let mut f = SettingsForm::load(&s, true);
    f.target_lang = "FR".into();
    assert_eq!(f.save(&s, never_key).unwrap(), "Saved.");
    assert!(f.has_key);
    assert_eq!(s.target_lang(), "FR");
    f.key = "   ".into();
    assert_eq!(f.save(&s, never_key).unwrap(), "Saved.");
}

#[test]
fn save_invalid_url_errors_and_leaves_store_unchanged() {
    let (d, s) = store_with(r#"{"target_lang":"DE"}"#);
    let before = std::fs::read_to_string(d.path().join("settings.json")).unwrap();
    let mut f = SettingsForm::load(&s, false);
    f.provider = "local".into();
    f.local_url = "ftp://nope".into();
    f.key = "k".into();
    let err = f.save(&s, never_key).unwrap_err();
    assert!(err.starts_with("Error: "), "{err}");
    assert_eq!(
        std::fs::read_to_string(d.path().join("settings.json")).unwrap(),
        before
    );
    assert_eq!(f.key, "k");
    assert!(!f.has_key);
    assert_eq!(s.provider(), "deepl");
}

#[test]
fn save_invalid_target_errors_and_leaves_store_unchanged() {
    let (_d, s) = store_with(r#"{"target_lang":"DE"}"#);
    let mut f = SettingsForm::load(&s, false);
    f.target_lang = "XX".into();
    let err = f.save(&s, never_key).unwrap_err();
    assert!(err.starts_with("Error: "), "{err}");
    assert_eq!(s.target_lang(), "DE");
}

#[test]
fn save_key_failure_is_reported_and_nothing_persisted() {
    let (_d, s) = store_with(r#"{"target_lang":"DE"}"#);
    let mut f = SettingsForm::load(&s, false);
    f.target_lang = "FR".into();
    f.key = "k".into();
    let err = f.save(&s, |_| Err("keychain locked".into())).unwrap_err();
    assert_eq!(err, "Error: keychain locked");
    assert_eq!(s.target_lang(), "DE");
    assert!(!f.has_key);
}

#[test]
fn remove_key_success_and_failure() {
    let (_d, s) = store_with("");
    let mut f = SettingsForm::load(&s, true);
    assert_eq!(f.remove_key(|| Ok(())).unwrap(), "Key removed.");
    assert!(!f.has_key);
    assert_eq!(f.key_placeholder(), "");

    let mut f = SettingsForm::load(&s, true);
    let err = f.remove_key(|| Err("boom".into())).unwrap_err();
    assert_eq!(err, "Error: boom");
    assert!(f.has_key);
}

#[test]
fn reload_local_refreshes_only_url_and_model() {
    let (_d, s) =
        store_with(r#"{"provider":"local","local_url":"http://127.0.0.1:1111","local_model":"a"}"#);
    let mut f = SettingsForm::load(&s, false);
    f.target_lang = "KO".into();
    f.key = "typed".into();
    f.provider = "deepl".into();
    s.use_managed_server().unwrap();
    f.reload_local(&s);
    assert_eq!(f.local_url, s.local_url());
    assert_eq!(f.local_model, "");
    assert_eq!(f.target_lang, "KO");
    assert_eq!(f.key, "typed");
    assert_eq!(f.provider, "deepl");
}

#[gpui::test]
fn settings_view_opens_in_root(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let (_d, store) = store_with("");
    let store = Arc::new(store);
    let window = cx.add_window(|window, cx| {
        let view = cx.new(|cx| SettingsView::new(store.clone(), window, cx));
        gpui_component::Root::new(view, window, cx)
    });
    cx.run_until_parked();
    window.update(cx, |_, _, _| ()).unwrap();
}
