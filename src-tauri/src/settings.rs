use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

use crate::mode::Mode;
use crate::translate::local::DEFAULT_BASE;

const SERVICE: &str = "com.screentranslate.app";
const USER: &str = "deepl";
const STORE: &str = "settings.json";
const TARGET_KEY: &str = "target_lang";
const PROVIDER_KEY: &str = "provider";
const URL_KEY: &str = "local_url";
const MODEL_KEY: &str = "local_model";
const MODE_KEY: &str = "mode";
const LEGACY_MODEL_KEY: &str = "ollama_model";
pub const PROVIDERS: &[&str] = &["deepl", "local"];
pub const DEFAULT_PROVIDER: &str = "deepl";
pub const DEFAULT_TARGET: &str = "EN-US";
/// Must match the options in `settings.html`.
pub const TARGETS: &[&str] = &["EN-US", "EN-GB", "DE", "FR", "ES", "IT", "PT-BR", "JA", "KO", "ZH-HANS"];

/// Maps a stored or submitted provider name to a supported one; the legacy "ollama" becomes "local".
pub fn normalize_provider(p: &str) -> Option<&'static str> {
    match p {
        "deepl" => Some("deepl"),
        "local" | "ollama" => Some("local"),
        _ => None,
    }
}

pub fn is_valid_url(u: &str) -> bool {
    reqwest::Url::parse(u).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty()))
}

pub fn is_local_host(u: &str) -> bool {
    reqwest::Url::parse(u)
        .ok()
        .and_then(|u| u.host_str().map(|h| matches!(h, "127.0.0.1" | "localhost" | "[::1]" | "::1")))
        .unwrap_or(false)
}

const LEGACY_OLLAMA_URL: &str = "http://127.0.0.1:11434";
const LEGACY_OLLAMA_MODEL: &str = "qwen2.5:7b";

/// Resolves stored values into (provider, url, model), upgrading the legacy "ollama" provider.
pub fn migrate(
    provider: Option<&str>,
    local_url: Option<&str>,
    local_model: Option<&str>,
    legacy_ollama_model: Option<&str>,
) -> (String, String, String) {
    let normalized = provider.and_then(normalize_provider).unwrap_or(DEFAULT_PROVIDER);
    let model = local_model.map(str::trim).filter(|m| !m.is_empty());
    if provider == Some("ollama") && local_url.is_none() {
        let model = model.or(legacy_ollama_model.map(str::trim).filter(|m| !m.is_empty())).unwrap_or(LEGACY_OLLAMA_MODEL);
        return (normalized.into(), LEGACY_OLLAMA_URL.into(), model.into());
    }
    let url = local_url.map(str::trim).filter(|u| is_valid_url(u)).unwrap_or(DEFAULT_BASE);
    (normalized.into(), url.into(), model.unwrap_or_default().into())
}

pub fn is_valid_target(code: &str) -> bool {
    TARGETS.contains(&code)
}

#[derive(Serialize)]
pub struct Settings {
    has_key: bool,
    target_lang: String,
    provider: String,
    local_url: String,
    local_model: String,
    mode: String,
}

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, USER).map_err(|e| e.to_string())
}

pub fn api_key() -> Option<String> {
    entry().ok()?.get_password().ok().filter(|k| !k.is_empty())
}

pub fn target_lang(app: &AppHandle) -> String {
    app.store(STORE)
        .ok()
        .and_then(|s| s.get(TARGET_KEY))
        .and_then(|v| v.as_str().map(str::to_owned))
        .filter(|t| is_valid_target(t))
        .unwrap_or_else(|| DEFAULT_TARGET.into())
}

fn stored(app: &AppHandle, key: &str) -> Option<String> {
    app.store(STORE).ok()?.get(key)?.as_str().map(str::to_owned)
}

fn resolved(app: &AppHandle) -> (String, String, String) {
    migrate(
        stored(app, PROVIDER_KEY).as_deref(),
        stored(app, URL_KEY).as_deref(),
        stored(app, MODEL_KEY).as_deref(),
        stored(app, LEGACY_MODEL_KEY).as_deref(),
    )
}

pub fn mode(app: &AppHandle) -> Mode {
    Mode::from_stored(stored(app, MODE_KEY).as_deref())
}

pub fn provider(app: &AppHandle) -> String {
    resolved(app).0
}

pub fn local_url(app: &AppHandle) -> String {
    resolved(app).1
}

/// Empty means "use whatever model the server has loaded".
pub fn local_model(app: &AppHandle) -> String {
    resolved(app).2
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    let (provider, local_url, local_model) = resolved(&app);
    Settings {
        has_key: api_key().is_some(),
        target_lang: target_lang(&app),
        provider,
        local_url,
        local_model,
        mode: mode(&app).as_str().into(),
    }
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    key: Option<String>,
    target_lang: String,
    provider: String,
    local_url: String,
    local_model: String,
    mode: String,
) -> Result<(), String> {
    let mode = Mode::parse(&mode).ok_or_else(|| format!("unsupported mode {mode:?}"))?;
    let provider = normalize_provider(&provider).ok_or_else(|| format!("unsupported provider {provider:?}"))?;
    let local_url = local_url.trim();
    if provider == "local" && !is_valid_url(local_url) {
        return Err("server URL must start with http:// or https:// and include a host".into());
    }
    if !is_valid_target(&target_lang) {
        return Err(format!("unsupported target language {target_lang:?}"));
    }
    if let Some(key) = key.map(|k| k.trim().to_owned()).filter(|k| !k.is_empty()) {
        entry()?.set_password(&key).map_err(|e| e.to_string())?;
    }
    let store = app.store(STORE).map_err(|e| e.to_string())?;
    store.set(TARGET_KEY, target_lang);
    store.set(PROVIDER_KEY, provider);
    store.set(MODE_KEY, mode.as_str());
    if provider == "local" {
        store.set(URL_KEY, local_url);
        store.set(MODEL_KEY, local_model.trim());
    }
    crate::refresh_tray(&app);
    store.save().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_api_key() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
