use screen_translate_core::paths::app_data_dir;
use std::path::PathBuf;
use std::sync::Mutex;

// Env vars are process-global; serialize tests that mutate them.
static ENV: Mutex<()> = Mutex::new(());

const ID: &str = "com.screentranslate.app";

#[allow(unused_unsafe)]
fn set(k: &str, v: &str) {
    unsafe { std::env::set_var(k, v) }
}
#[allow(unused_unsafe)]
fn unset(k: &str) {
    unsafe { std::env::remove_var(k) }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_uses_application_support() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    set("HOME", "/Users/tester");
    assert_eq!(
        app_data_dir(),
        Some(PathBuf::from("/Users/tester/Library/Application Support").join(ID))
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_without_home_is_none() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    unset("HOME");
    assert_eq!(app_data_dir(), None);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_uses_appdata() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    set("APPDATA", r"C:\Users\tester\AppData\Roaming");
    assert_eq!(
        app_data_dir(),
        Some(PathBuf::from(r"C:\Users\tester\AppData\Roaming").join(ID))
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_without_appdata_is_none() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    unset("APPDATA");
    assert_eq!(app_data_dir(), None);
}

#[test]
fn ends_with_identifier_when_present() {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = app_data_dir() {
        assert_eq!(p.file_name().unwrap(), ID);
    }
}
