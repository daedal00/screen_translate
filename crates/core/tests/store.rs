use screen_translate_core::store::Store;
use serde_json::json;
use std::fs;

#[test]
fn missing_file_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path().join("settings.json"));
    assert_eq!(s.get("target_lang"), None);
}

#[test]
fn corrupt_file_is_empty_and_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    fs::write(&p, "{ not json").unwrap();
    let s = Store::open(&p);
    assert_eq!(s.get("anything"), None);
    fs::write(&p, "[1,2,3]").unwrap();
    assert_eq!(Store::open(&p).get("anything"), None);
}

#[test]
fn reads_plugin_store_style_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    fs::write(
        &p,
        r#"{"target_lang":"en","provider":"deepl","n":3,"b":true}"#,
    )
    .unwrap();
    let s = Store::open(&p);
    assert_eq!(s.get("target_lang"), Some(json!("en")));
    assert_eq!(s.get("provider"), Some(json!("deepl")));
    assert_eq!(s.get("n"), Some(json!(3)));
    assert_eq!(s.get("b"), Some(json!(true)));
    assert_eq!(s.get("nope"), None);
}

#[test]
fn set_get_roundtrip_through_save_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    let mut s = Store::open(&p);
    s.set("a", json!("x"));
    s.set("obj", json!({"k": [1, 2]}));
    assert_eq!(s.get("a"), Some(json!("x")));
    s.save().unwrap();
    let r = Store::open(&p);
    assert_eq!(r.get("a"), Some(json!("x")));
    assert_eq!(r.get("obj"), Some(json!({"k": [1, 2]})));
}

#[test]
fn set_overwrites_existing_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(dir.path().join("s.json"));
    s.set("k", json!(1));
    s.set("k", json!(2));
    assert_eq!(s.get("k"), Some(json!(2)));
}

#[test]
fn delete_removes_key_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("s.json");
    let mut s = Store::open(&p);
    s.set("a", json!(1));
    s.set("b", json!(2));
    s.delete("a");
    s.delete("never_existed");
    assert_eq!(s.get("a"), None);
    s.save().unwrap();
    let r = Store::open(&p);
    assert_eq!(r.get("a"), None);
    assert_eq!(r.get("b"), Some(json!(2)));
}

#[test]
fn save_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a").join("b").join("settings.json");
    let mut s = Store::open(&p);
    s.set("k", json!("v"));
    s.save().unwrap();
    assert!(p.is_file());
}

#[test]
fn saved_file_is_flat_pretty_object_with_no_temp_leftovers() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    let mut s = Store::open(&p);
    s.set("target_lang", json!("en"));
    s.set("provider", json!("deepl"));
    s.save().unwrap();
    let text = fs::read_to_string(&p).unwrap();
    assert!(
        text.contains('\n'),
        "expected pretty-printed JSON: {text:?}"
    );
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v, json!({"target_lang": "en", "provider": "deepl"}));
    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["settings.json".to_string()],
        "temp file left behind"
    );
}

#[test]
fn save_replaces_existing_file_and_keeps_other_keys() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    fs::write(&p, r#"{"keep":"me","change":"old"}"#).unwrap();
    let mut s = Store::open(&p);
    s.set("change", json!("new"));
    s.save().unwrap();
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(v, json!({"keep": "me", "change": "new"}));
}

#[test]
fn save_error_is_returned_not_panicked() {
    let dir = tempfile::tempdir().unwrap();
    // Parent "directory" is actually a file, so dirs cannot be created.
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "x").unwrap();
    let mut s = Store::open(blocker.join("settings.json"));
    s.set("k", json!(1));
    assert!(s.save().is_err());
}
