//! Updater core: tray update item state, latest.json parsing, minisign verification and the install steps
//! (macOS bundle swap, Windows installer arguments). Network and UI glue live in the app shell.

use std::collections::HashMap;

use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::Deserialize;

/// Base64 of the minisign public key of the v0.3.1 release (key id A4532F120298EE3E). Never change it:
/// installed apps only accept updates signed by this key.
pub const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEE0NTMyRjEyMDI5OEVFM0UKUldRKzdwZ0NFaTlUcEUxcGx0UC9sOGhLY05WU1BpYzVYdUJoREFNWXJMQ1BQMUwzUjFXUzZ2TXQK";
pub const ENDPOINT: &str =
    "https://github.com/daedal00/screen_translate/releases/latest/download/latest.json";

#[derive(Debug, Clone, Deserialize)]
pub struct Platform {
    pub url: String,
    pub signature: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    /// Semver without a leading `v`.
    #[serde(alias = "name")]
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub platforms: HashMap<String, Platform>,
}

/// Parses a static-format `latest.json`: strips a leading `v` and checks that the version is semver.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    let mut m: Manifest = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    m.version = m.version.trim_start_matches('v').to_string();
    Version::parse(&m.version).map_err(|e| format!("invalid version {:?}: {e}", m.version))?;
    Ok(m)
}

/// Manifest keys to try, most specific first: `{os}-{arch}-{installer}` then `{os}-{arch}`.
pub fn platform_keys(os: &str, arch: &str, installer: &str) -> [String; 2] {
    [format!("{os}-{arch}-{installer}"), format!("{os}-{arch}")]
}

pub fn select<'a>(m: &'a Manifest, keys: &[String]) -> Option<&'a Platform> {
    keys.iter().find_map(|k| m.platforms.get(k))
}

fn parse_version(v: &str) -> Result<Version, String> {
    Version::parse(v.trim_start_matches('v')).map_err(|e| format!("invalid version {v:?}: {e}"))
}

/// True when `remote` is strictly newer than `current`.
pub fn newer(current: &str, remote: &str) -> Result<bool, String> {
    Ok(parse_version(remote)? > parse_version(current)?)
}

fn decode_text(b64: &str, what: &str) -> Result<String, String> {
    let bytes = STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("{what} is not base64: {e}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{what} is not text"))
}

/// Checks `bytes` against a Tauri-style signature: `sig_b64` and `pubkey_b64` are the base64 of minisign files.
/// The signed trusted comment must carry `version:<version>` (a missing or different version fails, so an old
/// artifact cannot be paired with a newer manifest).
pub fn verify(bytes: &[u8], sig_b64: &str, pubkey_b64: &str, version: &str) -> Result<(), String> {
    let key = PublicKey::decode(&decode_text(pubkey_b64, "public key")?)
        .map_err(|e| format!("invalid public key: {e}"))?;
    let sig = Signature::decode(&decode_text(sig_b64, "signature")?)
        .map_err(|e| format!("invalid signature: {e}"))?;
    key.verify(bytes, &sig, true)
        .map_err(|e| format!("signature check failed: {e}"))?;
    let signed = sig
        .trusted_comment()
        .split('\t')
        .find_map(|f| f.strip_prefix("version:"))
        .ok_or("the signature does not name a version")?;
    if parse_version(signed)? != parse_version(version)? {
        return Err(format!("signed version {signed} does not match {version}"));
    }
    Ok(())
}

/// A newer release for this platform.
#[derive(Debug, Clone)]
pub struct Update {
    pub version: String,
    pub notes: Option<String>,
    pub url: String,
    pub signature: String,
}

/// Fetches the manifest at `endpoint`; `Some` when it has a strictly newer version with an entry for `keys`.
/// HTTP 204 and a missing platform entry mean no update.
pub async fn check(
    client: &reqwest::Client,
    endpoint: &str,
    current: &str,
    keys: &[String],
) -> Result<Option<Update>, String> {
    let resp = client
        .get(endpoint)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    let m = parse_manifest(&resp.bytes().await.map_err(|e| e.to_string())?)?;
    if !newer(current, &m.version)? {
        return Ok(None);
    }
    Ok(select(&m, keys).map(|p| Update {
        version: m.version.clone(),
        notes: m.notes.clone(),
        url: p.url.clone(),
        signature: p.signature.clone(),
    }))
}

/// Downloads the update, reporting `(downloaded, total)`, and verifies it against `pubkey_b64` before returning it.
pub async fn download(
    client: &reqwest::Client,
    update: &Update,
    pubkey_b64: &str,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, String> {
    let mut resp = client
        .get(&update.url)
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let total = resp.content_length();
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        bytes.extend_from_slice(&chunk);
        on_progress(bytes.len() as u64, total);
    }
    verify(&bytes, &update.signature, pubkey_b64, &update.version)?;
    Ok(bytes)
}

/// NSIS installer arguments of a passive in-place update that restarts the app, as Tauri's updater passes them.
pub fn installer_args() -> [&'static str; 3] {
    ["/P", "/UPDATE", "/R"]
}

#[cfg(target_os = "macos")]
pub use bundle::{extract_bundle, replace_bundle, swap_dirs};

#[cfg(target_os = "macos")]
mod bundle {
    use std::ffi::CString;
    use std::fs;
    use std::io::Cursor;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Component, Path, PathBuf};

    /// Unpacks a `.app.tar.gz` into the existing dir `staging`, dropping the first path component (the
    /// `.app` dir, whatever its name) so the bundle contents land directly in `staging`. Rejects `..`,
    /// absolute paths, hard links and symlinks that leave `staging`, and a result without `Contents/Info.plist`.
    pub fn extract_bundle(tar_gz: &[u8], staging: &Path) -> Result<(), String> {
        let err = |e: std::io::Error| e.to_string();
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(tar_gz)));
        for entry in archive.entries().map_err(err)? {
            let mut entry = entry.map_err(err)?;
            let path = entry.path().map_err(err)?.into_owned();
            if path.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            }) {
                return Err(format!("unsafe path in the update: {}", path.display()));
            }
            let rest: PathBuf = path.components().skip(1).collect();
            if rest.as_os_str().is_empty() {
                continue;
            }
            let dest = staging.join(&rest);
            let kind = entry.header().entry_type();
            if kind.is_hard_link() {
                return Err(format!("hard link in the update: {}", path.display()));
            }
            if kind.is_symlink() {
                let target = entry
                    .link_name()
                    .map_err(err)?
                    .ok_or("symlink without a target")?;
                if !stays_inside(staging, dest.parent().unwrap_or(staging), &target) {
                    return Err(format!("symlink leaves the bundle: {}", path.display()));
                }
            }
            if !(kind.is_file() || kind.is_dir() || kind.is_symlink()) {
                continue;
            }
            // No entry is written beneath a symlink: earlier entries could have planted one to redirect this write.
            let mut at = staging.to_path_buf();
            for c in rest.components().take(rest.components().count() - 1) {
                at.push(c);
                match fs::symlink_metadata(&at) {
                    Ok(m) if m.file_type().is_symlink() => {
                        return Err(format!(
                            "entry written beneath a symlink: {}",
                            path.display()
                        ));
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                    Err(e) => return Err(e.to_string()),
                }
            }
            // An entry never replaces a link planted earlier (a dir entry over one would chmod its target).
            if fs::symlink_metadata(&dest).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(format!(
                    "entry over a symlink in the update: {}",
                    path.display()
                ));
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(err)?;
            }
            entry.unpack(&dest).map_err(err)?;
        }
        // Resolution on disk follows earlier symlinks, so check every link's real target once all entries exist.
        let root = fs::canonicalize(staging).map_err(err)?;
        let mut dirs = vec![staging.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for e in fs::read_dir(&dir).map_err(err)? {
                let path = e.map_err(err)?.path();
                let ft = fs::symlink_metadata(&path).map_err(err)?.file_type();
                if ft.is_dir() {
                    dirs.push(path);
                } else if ft.is_symlink()
                    && !fs::canonicalize(&path).is_ok_and(|p| p.starts_with(&root))
                {
                    return Err(format!("symlink leaves the bundle: {}", path.display()));
                }
            }
        }
        fs::set_permissions(staging, fs::Permissions::from_mode(0o755)).map_err(err)?;
        if !staging.join("Contents/Info.plist").is_file() {
            return Err("the update is not an app bundle (no Contents/Info.plist)".into());
        }
        Ok(())
    }

    /// True when `target`, relative to `from`, stays inside `root`.
    fn stays_inside(root: &Path, from: &Path, target: &Path) -> bool {
        let mut at = from.to_path_buf();
        for c in target.components() {
            match c {
                Component::Normal(n) => at.push(n),
                Component::CurDir => {}
                Component::ParentDir => {
                    at.pop();
                }
                Component::RootDir | Component::Prefix(_) => return false,
            }
            if !at.starts_with(root) {
                return false;
            }
        }
        at.starts_with(root)
    }

    /// Atomically exchanges two paths (`renamex_np` with `RENAME_SWAP`).
    pub fn swap_dirs(a: &Path, b: &Path) -> Result<(), String> {
        let c = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(|e| e.to_string());
        let (a, b) = (c(a)?, c(b)?);
        // SAFETY: both are valid NUL-terminated paths that outlive the call.
        if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }

    /// Puts the bundle at `staged` in place of `target`: a swap, else two renames with a restore on failure.
    /// Afterwards `target` holds the new bundle and the old one is deleted.
    pub fn replace_bundle(staged: &Path, target: &Path) -> Result<(), String> {
        if swap_dirs(staged, target).is_ok() {
            let _ = fs::remove_dir_all(staged);
            return Ok(());
        }
        rename_replace(staged, target)
    }

    fn rename_replace(staged: &Path, target: &Path) -> Result<(), String> {
        let mut backup = target.as_os_str().to_owned();
        backup.push(".old");
        let backup = PathBuf::from(backup);
        let _ = fs::remove_dir_all(&backup);
        fs::rename(target, &backup).map_err(|e| e.to_string())?;
        if let Err(e) = fs::rename(staged, target) {
            return Err(match fs::rename(&backup, target) {
                Ok(()) => e.to_string(),
                Err(_) => format!("{e}; old app left at {}", backup.display()),
            });
        }
        let _ = fs::remove_dir_all(&backup);
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn rename_replace_swaps_and_removes_backup() {
            let d = tempfile::tempdir().unwrap();
            let (staged, target) = (d.path().join("new"), d.path().join("app"));
            fs::create_dir(&staged).unwrap();
            fs::create_dir(&target).unwrap();
            fs::write(staged.join("m"), "new").unwrap();
            fs::write(target.join("m"), "old").unwrap();
            rename_replace(&staged, &target).unwrap();
            assert_eq!(fs::read_to_string(target.join("m")).unwrap(), "new");
            assert!(!d.path().join("app.old").exists() && !staged.exists());
        }

        #[test]
        fn rename_replace_restores_old_on_failure() {
            let d = tempfile::tempdir().unwrap();
            let target = d.path().join("app");
            fs::create_dir(&target).unwrap();
            fs::write(target.join("m"), "old").unwrap();
            assert!(rename_replace(&d.path().join("missing"), &target).is_err());
            assert_eq!(fs::read_to_string(target.join("m")).unwrap(), "old");
        }
    }
}

/// Tray update item state.

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    Available(String),
    Installing,
    UpToDate,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Check,
    Install,
    Ignore,
}

impl UpdateState {
    pub fn label(&self) -> String {
        match self {
            UpdateState::Idle => "Check for Updates…".into(),
            UpdateState::Checking => "Checking for Updates…".into(),
            UpdateState::Available(v) => format!("Install Update v{v} and Restart"),
            UpdateState::Installing => "Installing Update…".into(),
            UpdateState::UpToDate => "Up to Date, Check Again".into(),
            UpdateState::Failed => "Update Failed, Try Again".into(),
        }
    }

    pub fn enabled(&self) -> bool {
        !matches!(self, UpdateState::Checking | UpdateState::Installing)
    }

    pub fn click(&self) -> Click {
        match self {
            UpdateState::Checking | UpdateState::Installing => Click::Ignore,
            UpdateState::Available(_) => Click::Install,
            _ => Click::Check,
        }
    }
}

/// The `.app` dir of a running bundle (`X.app/Contents/MacOS/exe`); `None` for dev runs and other layouts.
pub fn bundle_from_exe(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let named = |p: &std::path::Path, n: &str| p.file_name().is_some_and(|f| f == n);
    (named(macos, "MacOS")
        && named(contents, "Contents")
        && bundle.extension().is_some_and(|e| e == "app"))
    .then(|| bundle.to_path_buf())
}

/// Where the Windows installer is written: `<temp>/screen-translate-{V}/Screen.Translate_{V}_x64-setup.exe`.
pub fn windows_installer_path(temp: &std::path::Path, version: &str) -> std::path::PathBuf {
    temp.join(format!("screen-translate-{version}"))
        .join(format!("Screen.Translate_{version}_x64-setup.exe"))
}

pub fn installer_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "app"
    } else {
        "nsis"
    }
}

pub fn current_platform_keys() -> [String; 2] {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "windows"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    platform_keys(os, arch, installer_key())
}

/// Background checks never overwrite a found update or an install in progress.
pub fn should_background_check(state: &UpdateState) -> bool {
    matches!(
        state,
        UpdateState::Idle | UpdateState::UpToDate | UpdateState::Failed
    )
}
