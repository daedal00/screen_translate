use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// A flat JSON object on disk (the format tauri-plugin-store wrote). A missing or corrupt file opens empty.
pub struct Store {
    path: PathBuf,
    map: Map<String, Value>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let map = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default();
        Self { path, map }
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.map.get(key).cloned()
    }

    pub fn set(&mut self, key: impl Into<String>, value: Value) {
        self.map.insert(key.into(), value);
    }

    pub fn delete(&mut self, key: &str) {
        self.map.remove(key);
    }

    /// Writes a temp file next to the target, then renames it over the target.
    pub fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let json = serde_json::to_string_pretty(&self.map).map_err(io::Error::other)?;
        let written = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, &self.path));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written
    }
}
