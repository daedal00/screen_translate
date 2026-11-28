use std::sync::{Mutex, MutexGuard};

use crate::mode::Mode;
use crate::paths::app_data_dir;
use crate::store::Store;
use crate::translate::local::DEFAULT_BASE;

const SERVICE: &str = "com.screentranslate.app";
const USER: &str = "deepl";
const FILE: &str = "settings.json";
const TARGET_KEY: &str = "target_lang";
const PROVIDER_KEY: &str = "provider";
const URL_KEY: &str = "local_url";
const MODEL_KEY: &str = "local_model";
const MODE_KEY: &str = "mode";
const MANAGED_MODEL_KEY: &str = "managed_model";
const MANAGED_AUTOSTART_KEY: &str = "managed_autostart";
const LEGACY_MODEL_KEY: &str = "ollama_model";
pub const PROVIDERS: &[&str] = &["deepl", "local"];
pub const DEFAULT_PROVIDER: &str = "deepl";
pub const DEFAULT_TARGET: &str = "EN-US";
/// Must match the options in `settings.html`.
pub const TARGETS: &[&str] = &[
    "EN-US", "EN-GB", "DE", "FR", "ES", "IT", "PT-BR", "JA", "KO", "ZH-HANS",
];

/// Maps a stored or submitted provider name to a supported one; the legacy "ollama" becomes "local".
pub fn normalize_provider(p: &str) -> Option<&'static str> {
    match p {
        "deepl" => Some("deepl"),
        "local" | "ollama" => Some("local"),
        _ => None,
    }
}

pub fn is_valid_url(u: &str) -> bool {
    reqwest::Url::parse(u).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty())
    })
}

pub fn is_local_host(u: &str) -> bool {
    reqwest::Url::parse(u)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| matches!(h, "127.0.0.1" | "localhost" | "[::1]" | "::1"))
        })
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
    let normalized = provider
        .and_then(normalize_provider)
        .unwrap_or(DEFAULT_PROVIDER);
    let model = local_model.map(str::trim).filter(|m| !m.is_empty());
    if provider == Some("ollama") && local_url.is_none() {
        let model = model
            .or(legacy_ollama_model.map(str::trim).filter(|m| !m.is_empty()))
            .unwrap_or(LEGACY_OLLAMA_MODEL);
        return (normalized.into(), LEGACY_OLLAMA_URL.into(), model.into());
    }
    let url = local_url
        .map(str::trim)
        .filter(|u| is_valid_url(u))
        .unwrap_or(DEFAULT_BASE);
    (
        normalized.into(),
        url.into(),
        model.unwrap_or_default().into(),
    )
}

pub fn is_valid_target(code: &str) -> bool {
    TARGETS.contains(&code)
}

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, USER).map_err(|e| e.to_string())
}

/// The stored DeepL key. `Ok(None)` = no entry; `Err` = the keychain could not be read (retry later).
pub fn api_key() -> Result<Option<String>, String> {
    match entry()?.get_password() {
        Ok(k) => Ok(Some(k).filter(|k| !k.is_empty())),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn set_api_key(key: &str) -> Result<(), String> {
    entry()?.set_password(key).map_err(|e| e.to_string())
}

pub fn clear_api_key() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// What the Settings UI shows; `local_*` and `provider` are already migrated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub provider: String,
    pub local_url: String,
    pub local_model: String,
}

/// The app's `settings.json`, shared behind one lock so there is a single writer.
pub struct SettingsStore(Mutex<Store>);

impl SettingsStore {
    /// `<app data dir>/settings.json`, read as written by earlier versions.
    pub fn open() -> Result<Self, String> {
        let dir = app_data_dir().ok_or("cannot find the app data directory")?;
        Ok(Self(Mutex::new(Store::open(dir.join(FILE)))))
    }

    pub fn open_at(path: impl AsRef<std::path::Path>) -> Self {
        Self(Mutex::new(Store::open(path)))
    }

    fn lock(&self) -> MutexGuard<'_, Store> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn stored(&self, key: &str) -> Option<String> {
        self.lock().get(key)?.as_str().map(str::to_owned)
    }

    pub fn target_lang(&self) -> String {
        self.stored(TARGET_KEY)
            .filter(|t| is_valid_target(t))
            .unwrap_or_else(|| DEFAULT_TARGET.into())
    }

    pub fn resolved(&self) -> Resolved {
        let (provider, local_url, local_model) = migrate(
            self.stored(PROVIDER_KEY).as_deref(),
            self.stored(URL_KEY).as_deref(),
            self.stored(MODEL_KEY).as_deref(),
            self.stored(LEGACY_MODEL_KEY).as_deref(),
        );
        Resolved {
            provider,
            local_url,
            local_model,
        }
    }

    pub fn mode(&self) -> Mode {
        Mode::from_stored(self.stored(MODE_KEY).as_deref())
    }

    pub fn provider(&self) -> String {
        self.resolved().provider
    }

    pub fn local_url(&self) -> String {
        self.resolved().local_url
    }

    /// Empty means "use whatever model the server has loaded".
    pub fn local_model(&self) -> String {
        self.resolved().local_model
    }

    pub fn managed_model(&self) -> Option<String> {
        self.stored(MANAGED_MODEL_KEY).filter(|m| !m.is_empty())
    }

    pub fn managed_autostart(&self) -> bool {
        self.lock()
            .get(MANAGED_AUTOSTART_KEY)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    pub fn set_managed_model(&self, file: Option<&str>) -> Result<(), String> {
        let mut store = self.lock();
        match file {
            Some(f) => store.set(MANAGED_MODEL_KEY, f.into()),
            None => store.delete(MANAGED_MODEL_KEY),
        }
        store.save().map_err(|e| e.to_string())
    }

    pub fn set_managed_autostart(&self, enabled: bool) -> Result<(), String> {
        let mut store = self.lock();
        store.set(MANAGED_AUTOSTART_KEY, enabled.into());
        store.save().map_err(|e| e.to_string())
    }

    /// Points the Local provider at the managed server (any loaded model); leaves `provider` alone.
    pub fn use_managed_server(&self) -> Result<(), String> {
        let mut store = self.lock();
        store.set(URL_KEY, crate::manager::server::base_url().into());
        store.set(MODEL_KEY, "".into());
        store.save().map_err(|e| e.to_string())
    }

    /// The checks `save` makes before touching the keyring or the file, so a caller can run them first.
    pub fn validate(
        target_lang: &str,
        provider: &str,
        local_url: &str,
        mode: &str,
    ) -> Result<(), String> {
        Mode::parse(mode).ok_or_else(|| format!("unsupported mode {mode:?}"))?;
        let provider = normalize_provider(provider)
            .ok_or_else(|| format!("unsupported provider {provider:?}"))?;
        if provider == "local" && !is_valid_url(local_url.trim()) {
            return Err("server URL must start with http:// or https:// and include a host".into());
        }
        if !is_valid_target(target_lang) {
            return Err(format!("unsupported target language {target_lang:?}"));
        }
        Ok(())
    }

    /// Validates and stores the Settings form. A blank `key` leaves the saved DeepL key alone.
    pub fn save(
        &self,
        key: Option<String>,
        target_lang: String,
        provider: String,
        local_url: String,
        local_model: String,
        mode: String,
    ) -> Result<(), String> {
        Self::validate(&target_lang, &provider, &local_url, &mode)?;
        let mode = Mode::parse(&mode).expect("validated");
        let provider = normalize_provider(&provider).expect("validated");
        let local_url = local_url.trim();
        if let Some(key) = key.map(|k| k.trim().to_owned()).filter(|k| !k.is_empty()) {
            set_api_key(&key)?;
        }
        let mut store = self.lock();
        store.set(TARGET_KEY, target_lang.into());
        store.set(PROVIDER_KEY, provider.into());
        store.set(MODE_KEY, mode.as_str().into());
        if provider == "local" {
            store.set(URL_KEY, local_url.into());
            store.set(MODEL_KEY, local_model.trim().into());
        }
        store.save().map_err(|e| e.to_string())
    }
}
