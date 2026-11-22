//! GGUF models: curated catalog, Hugging Face resolution, download, listing and deletion.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::download::{self, Progress};

pub const HF_BASE: &str = "https://huggingface.co";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct CatalogModel {
    pub id: &'static str,
    pub label: &'static str,
    /// Same syntax as the custom field: `owner/repo:quant`.
    pub source: &'static str,
    /// Download size in bytes.
    pub size: u64,
}

/// Smallest first. Single-file Q4_K_M quants (Qwen's own 7B repo splits the file in two).
pub const CATALOG: &[CatalogModel] = &[
    CatalogModel {
        id: "qwen2.5-3b",
        label: "Qwen2.5 3B Instruct Q4_K_M (fast, rougher)",
        source: "bartowski/Qwen2.5-3B-Instruct-GGUF:Q4_K_M",
        size: 1_929_903_264,
    },
    CatalogModel {
        id: "qwen2.5-7b",
        label: "Qwen2.5 7B Instruct Q4_K_M (recommended)",
        source: "bartowski/Qwen2.5-7B-Instruct-GGUF:Q4_K_M",
        size: 4_683_074_240,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Hf { repo: String, quant: String },
    Url(String),
}

/// `owner/repo:quant` (owner, repo and quant are `[A-Za-z0-9._-]+`, owner/repo not `.` or `..`),
/// or an https URL whose path ends in `.gguf` (any case). Input is trimmed. Anything else is an Err.
pub fn parse_source(input: &str) -> Result<Source, String> {
    let input = input.trim();
    if input.contains("://") {
        let url = reqwest::Url::parse(input).map_err(|_| format!("Invalid URL: {input}"))?;
        download::require_https(input)?;
        if !url.path().to_ascii_lowercase().ends_with(".gguf") {
            return Err("URL must point to a .gguf file".into());
        }
        url_file_name(&url)?;
        return Ok(Source::Url(input.to_string()));
    }
    let bad = || format!("Expected owner/repo:quant or an https .gguf URL, got {input:?}");
    let ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    let (repo, quant) = input.split_once(':').ok_or_else(bad)?;
    let (owner, name) = repo.split_once('/').ok_or_else(bad)?;
    if ![owner, name, quant].into_iter().all(ok) || [owner, name].contains(&".") || [owner, name].contains(&"..") {
        return Err(bad());
    }
    Ok(Source::Hf { repo: repo.to_string(), quant: quant.to_string() })
}

/// A bare `.gguf` file name that is safe to create on macOS and Windows and cannot escape the models dir.
pub(crate) fn valid_name(name: &str) -> Result<(), String> {
    let stem = name.get(..name.len().saturating_sub(5)).unwrap_or_default();
    let device = stem.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&device.as_str())
        || ["COM", "LPT"].iter().any(|p| device.strip_prefix(p).is_some_and(|d| matches!(d.as_bytes(), [b'1'..=b'9'])));
    let bad = name.is_empty()
        || stem.is_empty()
        || name.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*', '%'])
        || name.chars().any(char::is_control)
        || name.contains("..")
        || !name.to_ascii_lowercase().ends_with(".gguf")
        || stem.ends_with([' ', '.'])
        || reserved;
    if bad { Err(format!("Invalid model file name: {name:?}")) } else { Ok(()) }
}

/// The validated last path segment of a URL.
fn url_file_name(url: &reqwest::Url) -> Result<String, String> {
    let name = url.path_segments().and_then(|mut s| s.next_back()).unwrap_or_default().to_string();
    valid_name(&name)?;
    Ok(name)
}

/// `<stem>-00001-of-00002.gguf` -> (stem, 1, 2).
fn split_info(name: &str) -> Option<(&str, u32, u32)> {
    let base = name.get(..name.len().checked_sub(5)?).filter(|_| name.to_ascii_lowercase().ends_with(".gguf"))?;
    let (rest, total) = base.rsplit_once("-of-")?;
    let (stem, idx) = rest.rsplit_once('-')?;
    let digits = |d: &str| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit());
    (digits(idx) && digits(total)).then_some(())?;
    Some((stem, idx.parse().ok()?, total.parse().ok()?))
}

/// True when the part indices are exactly 1..=total.
fn complete(mut indices: Vec<u32>, total: u32) -> bool {
    indices.sort_unstable();
    indices.into_iter().eq(1..=total)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    /// Local file name (last path component of the remote path or URL).
    pub name: String,
    pub url: String,
    pub size: Option<u64>,
    /// Lowercase hex sha256 (the LFS oid), when known.
    pub sha256: Option<String>,
}

/// From the HF tree JSON (`GET {hf_base}/api/models/{repo}/tree/main?recursive=true`, an array of
/// `{type, path, size, lfs: {oid, size}?}`), the `.gguf` files whose file name contains `quant`
/// (case-insensitive), skipping names starting with `mmproj` and names unsafe to create locally. A split
/// model (`<stem>-00001-of-0000N.gguf`) yields all N parts in order (indices must be exactly 1..=N).
/// URL is `{hf_base}/{repo}/resolve/main/{path}` with each segment percent-encoded; size prefers `lfs.size`.
/// When several models match, the one whose name ends in the quant as a whole token (`-Q4_0.gguf`,
/// `.Q4_0.gguf`, `-Q4_0-00001-of-N.gguf`) wins (so "Q4_0" is not confused with "Q4_0_4_4"). Otherwise Err
/// listing the candidate file names (e.g. "Q4_K" matching Q4_K_M and Q4_K_S). Err when nothing matches.
pub fn pick_files(tree_json: &str, hf_base: &str, repo: &str, quant: &str) -> Result<Vec<RemoteFile>, String> {
    #[derive(serde::Deserialize)]
    struct Lfs {
        oid: Option<String>,
        size: Option<u64>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        #[serde(rename = "type")]
        kind: String,
        path: String,
        size: Option<u64>,
        lfs: Option<Lfs>,
    }
    struct Group {
        key: String,
        total: Option<u32>,
        token: bool,
        parts: Vec<(u32, RemoteFile)>,
    }
    let entries: Vec<Entry> = serde_json::from_str(tree_json).map_err(|e| format!("Unexpected Hugging Face response: {e}"))?;
    let quant = quant.to_lowercase();
    let mut groups: Vec<Group> = Vec::new();
    for e in entries.into_iter().filter(|e| e.kind == "file") {
        let name = e.path.rsplit('/').next().unwrap_or_default().to_string();
        let lower = name.to_lowercase();
        if valid_name(&name).is_err() || lower.starts_with("mmproj") || !lower.contains(&quant) {
            continue;
        }
        let mut url = reqwest::Url::parse(hf_base).map_err(|_| format!("Invalid Hugging Face URL: {hf_base}"))?;
        url.path_segments_mut()
            .map_err(|_| format!("Invalid Hugging Face URL: {hf_base}"))?
            .pop_if_empty()
            .extend(repo.split('/').chain(["resolve", "main"]).chain(e.path.split('/')));
        let dir = &e.path[..e.path.len() - name.len()];
        let split = split_info(&name);
        let stem = split.map(|s| s.0).unwrap_or(&name[..name.len() - 5]).to_lowercase();
        let (key, idx, total) = match split {
            Some((stem, idx, total)) => (format!("{dir}{stem}-of-{total}"), idx, Some(total)),
            None => (e.path.clone(), 0, None),
        };
        let file = RemoteFile {
            url: url.to_string(),
            size: e.lfs.as_ref().and_then(|l| l.size).or(e.size),
            sha256: e.lfs.and_then(|l| l.oid).map(|o| o.to_lowercase()),
            name,
        };
        match groups.iter_mut().find(|g| g.key == key) {
            Some(g) => g.parts.push((idx, file)),
            None => groups.push(Group {
                key,
                total,
                token: stem.ends_with(&format!("-{quant}")) || stem.ends_with(&format!(".{quant}")),
                parts: vec![(idx, file)],
            }),
        }
    }
    let describe = |g: &Group| g.parts.iter().map(|(_, f)| f.name.as_str()).min().unwrap_or_default().to_string();
    let chosen = match groups.len() {
        0 => return Err(format!("No .gguf file matching {quant:?} in {repo}")),
        1 => groups.remove(0),
        _ => {
            if groups.iter().filter(|g| g.token).count() == 1 {
                groups.remove(groups.iter().position(|g| g.token).unwrap_or_default())
            } else {
                let names: Vec<_> = groups.iter().map(describe).collect();
                return Err(format!("{quant:?} matches several files, be more specific: {}", names.join(", ")));
            }
        }
    };
    let mut parts = chosen.parts;
    if let Some(total) = chosen.total
        && !complete(parts.iter().map(|(i, _)| *i).collect(), total)
    {
        return Err(format!("Incomplete split model: found {} of {total} parts", parts.len()));
    }
    parts.sort_by_key(|(i, _)| *i);
    Ok(parts.into_iter().map(|(_, f)| f).collect())
}

/// Hf: fetches the tree from `hf_base` and calls `pick_files`. Url: one file, size and sha256 None.
pub async fn resolve(client: &reqwest::Client, hf_base: &str, source: &Source) -> Result<Vec<RemoteFile>, String> {
    match source {
        Source::Url(url) => {
            let parsed = reqwest::Url::parse(url).map_err(|_| format!("Invalid URL: {url}"))?;
            let name = url_file_name(&parsed)?;
            Ok(vec![RemoteFile { name, url: url.clone(), size: None, sha256: None }])
        }
        Source::Hf { repo, quant } => {
            let resp = client
                .get(format!("{hf_base}/api/models/{repo}/tree/main?recursive=true"))
                .send()
                .await
                .map_err(|e| format!("Cannot reach Hugging Face: {e}"))?;
            if !resp.status().is_success() {
                return Err(format!("Hugging Face returned HTTP {} for {repo}", resp.status().as_u16()));
            }
            if resp.headers().get_all("link").iter().any(|v| v.to_str().is_ok_and(|v| v.contains("rel=\"next\""))) {
                return Err(format!("{repo} has too many files to list"));
            }
            let body = resp.text().await.map_err(|e| format!("Cannot read Hugging Face response: {e}"))?;
            pick_files(&body, hf_base, repo, quant)
        }
    }
}

/// `<data_dir>/models`.
pub fn models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct InstalledModel {
    /// File name to pass to llama-server / delete; the first part for a split model.
    pub file: String,
    /// Bytes, summed over all parts.
    pub size: u64,
}

/// `.gguf` files in `models_dir` (missing dir -> empty), sorted by file name. Parts 2..N of a split
/// model are folded into part 1; a split with missing parts is not listed. `.part` files and
/// directories are ignored.
pub fn list_installed(models_dir: &Path) -> Vec<InstalledModel> {
    let Ok(read) = std::fs::read_dir(models_dir) else { return Vec::new() };
    let mut out = Vec::new();
    // (stem, total) -> (index, file name, bytes) of the parts found
    type Parts = Vec<(u32, String, u64)>;
    let mut splits: std::collections::HashMap<(String, u32), Parts> = Default::default();
    for entry in read.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !meta.is_file() || valid_name(&name).is_err() {
            continue;
        }
        match split_info(&name) {
            Some((stem, idx, total)) => splits.entry((stem.to_string(), total)).or_default().push((idx, name, meta.len())),
            None => out.push(InstalledModel { file: name, size: meta.len() }),
        }
    }
    for ((_, total), mut parts) in splits {
        if complete(parts.iter().map(|p| p.0).collect(), total) {
            parts.sort_by_key(|p| p.0);
            out.push(InstalledModel { file: parts[0].1.clone(), size: parts.iter().map(|p| p.2).sum() });
        }
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

/// Deletes `file` (a bare `.gguf` name as returned by `list_installed`; unsafe names are an Err)
/// and, for a split model, all its parts (part 1 last, so a partial failure still lists the model).
/// Err when it does not exist.
pub fn delete_model(models_dir: &Path, file: &str) -> Result<(), String> {
    valid_name(file)?;
    if !models_dir.join(file).is_file() {
        return Err(format!("{file} is not installed"));
    }
    let mut targets = vec![(split_info(file).map_or(0, |s| s.1), file.to_string())];
    if let Some((stem, _, total)) = split_info(file) {
        for entry in std::fs::read_dir(models_dir).map_err(|e| format!("Cannot read models folder: {e}"))?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some((s, idx, t)) = split_info(&name)
                && name != file
                && s == stem
                && t == total
            {
                targets.push((idx, name));
            }
        }
    }
    targets.sort_by_key(|t| std::cmp::Reverse(t.0));
    for (_, name) in targets {
        std::fs::remove_file(models_dir.join(&name)).map_err(|e| format!("Cannot delete {name}: {e}"))?;
    }
    Ok(())
}

/// Downloads `files` into `models_dir` with `download::download` (sha256 checked when known) and
/// returns the first file's name. Parts are fetched last to first, so part 1 appears only once every
/// other part is complete. Err ("already installed") if every file already exists; a partial leftover
/// set is downloaded again. If any file fails, files completed earlier in this call are removed.
/// Callers check https on every URL first.
pub async fn download_files(
    client: &reqwest::Client,
    models_dir: &Path,
    files: &[RemoteFile],
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(&str, Progress),
) -> Result<String, String> {
    let first = files.first().ok_or("Nothing to download")?;
    for f in files {
        valid_name(&f.name)?;
    }
    if files.iter().all(|f| models_dir.join(&f.name).exists()) {
        return Err(format!("{} is already installed", first.name));
    }
    std::fs::create_dir_all(models_dir).map_err(|e| format!("Cannot create {}: {e}", models_dir.display()))?;
    let mut done: Vec<&RemoteFile> = Vec::new();
    for f in files.iter().rev() {
        let dest = models_dir.join(&f.name);
        if let Err(e) = download::download(client, &f.url, &dest, f.sha256.as_deref(), cancel, |p| on_progress(&f.name, p)).await {
            for d in done {
                let _ = std::fs::remove_file(models_dir.join(&d.name));
            }
            return Err(e);
        }
        done.push(f);
    }
    Ok(first.name.clone())
}
