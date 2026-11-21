use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

use crate::translate::ollama::DEFAULT_MODEL;

const SERVICE: &str = "com.screentranslate.app";
const USER: &str = "deepl";
const STORE: &str = "settings.json";
const TARGET_KEY: &str = "target_lang";
const PROVIDER_KEY: &str = "provider";
const MODEL_KEY: &str = "ollama_model";
pub const PROVIDERS: &[&str] = &["deepl", "ollama"];
pub const DEFAULT_PROVIDER: &str = "deepl";
pub const DEFAULT_TARGET: &str = "EN-US";
/// Must match the options in `settings.html`.
pub const TARGETS: &[&str] = &["EN-US", "EN-GB", "DE", "FR", "ES", "IT", "PT-BR", "JA", "KO", "ZH-HANS"];

pub fn is_valid_target(code: &str) -> bool {
    TARGETS.contains(&code)
}

#[derive(Serialize)]
pub struct Settings {
    has_key: bool,
    target_lang: String,
    provider: String,
    ollama_model: String,
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

pub fn provider(app: &AppHandle) -> String {
    app.store(STORE)
        .ok()
        .and_then(|s| s.get(PROVIDER_KEY))
        .and_then(|v| v.as_str().map(str::to_owned))
        .filter(|p| PROVIDERS.contains(&p.as_str()))
        .unwrap_or_else(|| DEFAULT_PROVIDER.into())
}

pub fn ollama_model(app: &AppHandle) -> String {
    app.store(STORE)
        .ok()
        .and_then(|s| s.get(MODEL_KEY))
        .and_then(|v| v.as_str().map(|m| m.trim().to_owned()))
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.into())
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    Settings {
        has_key: api_key().is_some(),
        target_lang: target_lang(&app),
        provider: provider(&app),
        ollama_model: ollama_model(&app),
    }
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    key: Option<String>,
    target_lang: String,
    provider: String,
    ollama_model: String,
) -> Result<(), String> {
    if !PROVIDERS.contains(&provider.as_str()) {
        return Err(format!("unsupported provider {provider:?}"));
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
    store.set(MODEL_KEY, ollama_model.trim());
    store.save().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_api_key() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
