//! llama.cpp runtime: picking the prebuilt release asset, unpacking it, and finding llama-server.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::download::{self, Progress};

pub const RELEASES_API: &str = "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=10";

/// GPU backend for the Windows build. Ignored on macOS (Metal is built in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Cpu,
    Vulkan,
    Cuda,
}

/// A CUDA toolkit version, as in the `cuda-12.4` release asset names and the driver's CUDA level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct CudaVer {
    pub major: u32,
    pub minor: u32,
}

impl std::fmt::Display for CudaVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// Lowercase hex, from the GitHub `digest` field ("sha256:<hex>").
    pub sha256: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub assets: Vec<Asset>,
}

/// Parses the GitHub "list releases" JSON and returns every release whose tag is `b<digits>`, newest first
/// (llama.cpp marks every build as a prerelease, so /releases/latest is not usable). Err when there is none.
pub fn parse_releases(json: &str) -> Result<Vec<Release>, String> {
    #[derive(serde::Deserialize)]
    struct RawAsset {
        name: String,
        browser_download_url: String,
        size: u64,
        digest: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct RawRelease {
        tag_name: String,
        assets: Vec<RawAsset>,
    }
    let raw: Vec<RawRelease> = serde_json::from_str(json).map_err(|e| format!("Unexpected releases response: {e}"))?;
    let is_build = |t: &str| t.strip_prefix('b').is_some_and(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()));
    let releases: Vec<Release> = raw
        .into_iter()
        .filter(|r| is_build(&r.tag_name))
        .map(|r| Release {
            tag: r.tag_name,
            assets: r
                .assets
                .into_iter()
                .map(|a| Asset {
                    name: a.name,
                    url: a.browser_download_url,
                    sha256: a.digest.and_then(|d| d.strip_prefix("sha256:").map(str::to_lowercase)),
                    size: a.size,
                })
                .collect(),
        })
        .collect();
    if releases.is_empty() {
        return Err("No llama.cpp build release found".into());
    }
    Ok(releases)
}

/// CUDA versions `v` for which the release has both `llama-<tag>-bin-win-cuda-<v>-x64.zip` and
/// `cudart-llama-bin-win-cuda-<v>-x64.zip`, newest first.
pub fn cuda_variants(release: &Release) -> Vec<CudaVer> {
    let prefix = format!("llama-{}-bin-win-cuda-", release.tag);
    let has = |n: &str| release.assets.iter().any(|a| a.name == n);
    let mut versions: Vec<CudaVer> = release
        .assets
        .iter()
        .filter_map(|a| a.name.strip_prefix(&prefix)?.strip_suffix("-x64.zip"))
        .filter_map(|v| {
            let (major, minor) = v.split_once('.')?;
            Some(CudaVer { major: major.parse().ok()?, minor: minor.parse().ok()? })
        })
        .filter(|v| has(&format!("cudart-llama-bin-win-cuda-{v}-x64.zip")))
        .collect();
    versions.sort_by(|a, b| b.cmp(a));
    versions.dedup();
    versions
}

/// Assets to download for `os`/`arch` (values of `std::env::consts::{OS, ARCH}`):
/// - macos aarch64: `llama-<tag>-bin-macos-arm64.tar.gz`; macos x86_64: `llama-<tag>-bin-macos-x64.tar.gz`
/// - windows x86_64: cpu `llama-<tag>-bin-win-cpu-x64.zip`, vulkan `llama-<tag>-bin-win-vulkan-x64.zip`,
///   cuda `llama-<tag>-bin-win-cuda-<v>-x64.zip` followed by `cudart-llama-bin-win-cuda-<v>-x64.zip`, for the
///   newest `cuda_variants` version `<= max_cuda` (any when `max_cuda` is None)
///
/// Anything else, or a missing asset, is an Err naming the platform or asset.
pub fn select_assets(release: &Release, os: &str, arch: &str, backend: Backend, max_cuda: Option<CudaVer>) -> Result<Vec<Asset>, String> {
    let tag = &release.tag;
    let names: Vec<String> = match (os, arch) {
        ("macos", "aarch64") => vec![format!("llama-{tag}-bin-macos-arm64.tar.gz")],
        ("macos", "x86_64") => vec![format!("llama-{tag}-bin-macos-x64.tar.gz")],
        ("windows", "x86_64") => match backend {
            Backend::Cpu => vec![format!("llama-{tag}-bin-win-cpu-x64.zip")],
            Backend::Vulkan => vec![format!("llama-{tag}-bin-win-vulkan-x64.zip")],
            Backend::Cuda => {
                let v = cuda_variants(release)
                    .into_iter()
                    .find(|v| max_cuda.is_none_or(|m| *v <= m))
                    .ok_or_else(|| format!("Release {tag} has no CUDA build for this driver"))?;
                vec![format!("llama-{tag}-bin-win-cuda-{v}-x64.zip"), format!("cudart-llama-bin-win-cuda-{v}-x64.zip")]
            }
        },
        _ => return Err(format!("Unsupported platform: {os} {arch}")),
    };
    names
        .iter()
        .map(|n| {
            release
                .assets
                .iter()
                .find(|a| &a.name == n)
                .cloned()
                .ok_or_else(|| format!("Release {tag} has no asset {n}"))
        })
        .collect()
}

/// Assets from the first release that has every asset for this platform, each with a sha256 and an https URL
/// (a fresh release can lack assets for a few minutes while it uploads). Otherwise the newest release's error.
pub fn pick_release(releases: &[Release], os: &str, arch: &str, backend: Backend, max_cuda: Option<CudaVer>) -> Result<Vec<Asset>, String> {
    let usable = |r: &Release| -> Result<Vec<Asset>, String> {
        let assets = select_assets(r, os, arch, backend, max_cuda)?;
        for a in &assets {
            download::require_https(&a.url)?;
            if a.sha256.is_none() {
                return Err(format!("No checksum published for {}", a.name));
            }
        }
        Ok(assets)
    };
    let mut first_err = None;
    for r in releases {
        match usable(r) {
            Ok(assets) => return Ok(assets),
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    Err(first_err.unwrap_or_else(|| "No llama.cpp build release found".into()))
}

/// "llama-server.exe" on windows, else "llama-server".
pub fn server_exe_name(os: &str) -> &'static str {
    if os == "windows" { "llama-server.exe" } else { "llama-server" }
}

/// `<data_dir>/llama`: archives are downloaded into `<runtime_dir>/downloads`, unpacked into `<runtime_dir>/bin`.
pub fn runtime_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("llama")
}

/// First existing llama-server: searched under `<runtime_dir>/bin` (up to 4 levels deep), then each
/// absolute entry of `path_var` (a PATH-style list; empty and relative entries are ignored), then on macos
/// `/opt/homebrew/bin` and `/usr/local/bin` (GUI apps get a minimal PATH). Only regular files named
/// `server_exe_name(os)` match. Inside `bin` symlinks are never followed; PATH and homebrew entries may be
/// symlinks to a regular file (Homebrew links into its Cellar).
pub fn find_server(runtime_dir: &Path, path_var: Option<&OsStr>, os: &str) -> Option<PathBuf> {
    let name = server_exe_name(os);
    if let Some(found) = search(&runtime_dir.join("bin"), name, 4) {
        return Some(found);
    }
    let mut dirs: Vec<PathBuf> = path_var.map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    dirs.retain(|d| d.is_absolute());
    if os == "macos" {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

fn search(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
    let mut subdirs = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_file() && entry.file_name() == name {
            return Some(entry.path());
        }
        if kind.is_dir() {
            subdirs.push(entry.path());
        }
    }
    if depth == 0 {
        return None;
    }
    subdirs.into_iter().find_map(|d| search(&d, name, depth - 1))
}

/// Unpacks a .zip or .tar.gz into `dest` (created if missing) with the OS tar
/// (`/usr/bin/tar` on macos, `%SystemRoot%\System32\tar.exe` on windows; both are bsdtar,
/// which rejects absolute and `..` entries). Errors include tar's stderr.
pub fn unpack(archive: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("Cannot create {}: {e}", dest.display()))?;
    let tar = if cfg!(windows) {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("tar.exe")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/usr/bin/tar")
    } else {
        PathBuf::from("tar")
    };
    let mut cmd = std::process::Command::new(tar);
    cmd.arg("-xf").arg(archive).arg("-C").arg(dest);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let out = cmd.output().map_err(|e| format!("Cannot run tar: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("Unpack failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// GETs `api_url` (with a User-Agent, as GitHub requires) and parses it with `parse_releases`.
pub async fn fetch_release(client: &reqwest::Client, api_url: &str) -> Result<Vec<Release>, String> {
    let resp = client
        .get(api_url)
        .header("User-Agent", "screen-translate")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Cannot reach GitHub: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub returned HTTP {}", resp.status().as_u16()));
    }
    let body = resp.text().await.map_err(|e| format!("Cannot read GitHub response: {e}"))?;
    parse_releases(&body)
}

/// Full install: fetch the releases from `RELEASES_API`, `pick_release` for this platform, then `install_assets`.
/// The caller must stop llama-server first (a running exe blocks replacing `bin` on Windows) and must not run
/// two installs at once (they share `downloads/` and `bin.new`).
pub async fn install(
    client: &reqwest::Client,
    data_dir: &Path,
    backend: Backend,
    max_cuda: Option<CudaVer>,
    cancel: &AtomicBool,
    on_progress: impl FnMut(&str, Progress),
) -> Result<PathBuf, String> {
    let releases = fetch_release(client, RELEASES_API).await?;
    let assets = pick_release(&releases, std::env::consts::OS, std::env::consts::ARCH, backend, max_cuda)?;
    install_assets(client, &runtime_dir(data_dir), &assets, cancel, on_progress).await
}

/// Removes the scratch dirs on every exit, including the future being dropped mid-install.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        for d in ["downloads", "bin.new"] {
            let _ = std::fs::remove_dir_all(self.0.join(d));
        }
        // bin.old is the only copy of a working install when bin is missing: keep it then.
        if self.0.join("bin").exists() {
            let _ = std::fs::remove_dir_all(self.0.join("bin.old"));
        }
    }
}

/// Downloads+verifies each asset (sha256 required up front, before any request; callers vet https via `pick_release`) into
/// `<runtime_dir>/downloads`, unpacks all into a fresh `<runtime_dir>/bin.new`, checks it contains
/// llama-server (a single top-level folder in the archives becomes `bin` itself), then swaps it in for `<runtime_dir>/bin` (old one kept as `bin.old` until the swap
/// succeeds) and returns the llama-server path. Any failure leaves an existing `bin` untouched.
/// `downloads`, `bin.new` and `bin.old` are gone afterwards. `on_progress` gets the asset name.
pub async fn install_assets(
    client: &reqwest::Client,
    runtime_dir: &Path,
    assets: &[Asset],
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(&str, Progress),
) -> Result<PathBuf, String> {
    if let Some(a) = assets.iter().find(|a| a.sha256.is_none()) {
        return Err(format!("No checksum published for {}", a.name));
    }
    let _scratch = Scratch(runtime_dir.to_path_buf());
    let (downloads, new_bin, bin, old_bin) =
        (runtime_dir.join("downloads"), runtime_dir.join("bin.new"), runtime_dir.join("bin"), runtime_dir.join("bin.old"));
    if !bin.exists() && old_bin.exists() {
        // A crash between the two swap renames left only bin.old: bring it back first.
        std::fs::rename(&old_bin, &bin).map_err(|e| format!("Cannot restore the previous llama.cpp runtime: {e}"))?;
    }
    for d in [&downloads, &new_bin] {
        let _ = std::fs::remove_dir_all(d);
    }
    if bin.exists() {
        let _ = std::fs::remove_dir_all(&old_bin);
    }
    for a in assets {
        let archive = downloads.join(&a.name);
        download::download(client, &a.url, &archive, a.sha256.as_deref(), cancel, |p| on_progress(&a.name, p)).await?;
        let (from, to) = (archive.clone(), new_bin.clone());
        // tar is blocking and can take seconds; keep it off the async worker. A dropped install may leave
        // a partial bin.new behind (tar keeps running); the next install cleans it.
        tauri::async_runtime::spawn_blocking(move || unpack(&from, &to)).await.map_err(|e| e.to_string())??;
    }
    // GitHub's tarballs wrap everything in one top-level folder; use it as the root.
    let root = match std::fs::read_dir(&new_bin).map_err(|e| format!("Cannot read {}: {e}", new_bin.display()))?.flatten().collect::<Vec<_>>().as_slice() {
        [only] if only.file_type().is_ok_and(|t| t.is_dir()) => only.path(),
        _ => new_bin.clone(),
    };
    let found = search(&root, server_exe_name(std::env::consts::OS), 4)
        .ok_or("llama-server not found in the downloaded runtime")?;
    let had_old = bin.exists();
    if had_old {
        std::fs::rename(&bin, &old_bin).map_err(|e| format!("Cannot replace the old runtime: {e}"))?;
    }
    if let Err(e) = std::fs::rename(&root, &bin) {
        if had_old {
            let _ = std::fs::rename(&old_bin, &bin);
        }
        return Err(format!("Cannot install runtime: {e}"));
    }
    Ok(bin.join(found.strip_prefix(&root).map_err(|e| e.to_string())?))
}
