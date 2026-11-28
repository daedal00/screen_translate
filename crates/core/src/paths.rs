use std::path::PathBuf;

const IDENTIFIER: &str = "com.screentranslate.app";

/// Per-user data directory: `~/Library/Application Support/<id>` on macOS, `%APPDATA%\<id>` on Windows.
/// None when the environment lacks `HOME` / `APPDATA`.
pub fn app_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Library").join("Application Support"));
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", windows)))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
        });
    base.map(|b| b.join(IDENTIFIER))
}
