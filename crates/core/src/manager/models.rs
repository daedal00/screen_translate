//! GGUF models: Hugging Face search and resolution, download, listing and deletion.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::download::{self, Progress};

pub const HF_BASE: &str = "https://huggingface.co";
pub const OFFLINE: &str = "Cannot reach Hugging Face. Check your internet connection.";

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
    let ok = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    };
    let (repo, quant) = input.split_once(':').ok_or_else(bad)?;
    let (owner, name) = repo.split_once('/').ok_or_else(bad)?;
    if ![owner, name, quant].into_iter().all(ok)
        || [owner, name].contains(&".")
        || [owner, name].contains(&"..")
    {
        return Err(bad());
    }
    Ok(Source::Hf {
        repo: repo.to_string(),
        quant: quant.to_string(),
    })
}

/// A bare `.gguf` file name that is safe to create on macOS and Windows and cannot escape the models dir.
pub(crate) fn valid_name(name: &str) -> Result<(), String> {
    let stem = name.get(..name.len().saturating_sub(5)).unwrap_or_default();
    let device = stem
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&device.as_str())
        || ["COM", "LPT"].iter().any(|p| {
            device
                .strip_prefix(p)
                .is_some_and(|d| matches!(d.as_bytes(), [b'1'..=b'9']))
        });
    let bad = name.is_empty()
        || stem.is_empty()
        || name.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*', '%'])
        || name.chars().any(char::is_control)
        || name.contains("..")
        || !name.to_ascii_lowercase().ends_with(".gguf")
        || stem.ends_with([' ', '.'])
        || reserved;
    if bad {
        Err(format!("Invalid model file name: {name:?}"))
    } else {
        Ok(())
    }
}

/// The validated last path segment of a URL.
fn url_file_name(url: &reqwest::Url) -> Result<String, String> {
    let name = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or_default()
        .to_string();
    valid_name(&name)?;
    Ok(name)
}

/// `<stem>-00001-of-00002.gguf` -> (stem, 1, 2).
fn split_info(name: &str) -> Option<(&str, u32, u32)> {
    let base = name
        .get(..name.len().checked_sub(5)?)
        .filter(|_| name.to_ascii_lowercase().ends_with(".gguf"))?;
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

/// One element of the HF tree JSON.
#[derive(serde::Deserialize)]
struct TreeEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    size: Option<u64>,
    lfs: Option<Lfs>,
}

#[derive(serde::Deserialize)]
struct Lfs {
    oid: Option<String>,
    size: Option<u64>,
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
/// A `quant` that is exactly the (first part's) file name of one model, e.g. `X-Q4_K_M.gguf`, selects that model
/// whatever else matches: it is the key `quant_files` rows download by.
pub fn pick_files(
    tree_json: &str,
    hf_base: &str,
    repo: &str,
    quant: &str,
) -> Result<Vec<RemoteFile>, String> {
    struct Group {
        key: String,
        total: Option<u32>,
        token: bool,
        parts: Vec<(u32, RemoteFile)>,
    }
    let entries: Vec<TreeEntry> = serde_json::from_str(tree_json)
        .map_err(|e| format!("Unexpected Hugging Face response: {e}"))?;
    let quant = quant.to_lowercase();
    // For a split part-1 name, match on the shared stem so the other parts are collected too.
    let needle = split_info(&quant).map_or(quant.clone(), |(stem, _, _)| stem.to_string());
    let mut groups: Vec<Group> = Vec::new();
    for e in entries.into_iter().filter(|e| e.kind == "file") {
        let name = e.path.rsplit('/').next().unwrap_or_default().to_string();
        let lower = name.to_lowercase();
        if valid_name(&name).is_err() || lower.starts_with("mmproj") || !lower.contains(&needle) {
            continue;
        }
        let mut url = reqwest::Url::parse(hf_base)
            .map_err(|_| format!("Invalid Hugging Face URL: {hf_base}"))?;
        url.path_segments_mut()
            .map_err(|_| format!("Invalid Hugging Face URL: {hf_base}"))?
            .pop_if_empty()
            .extend(
                repo.split('/')
                    .chain(["resolve", "main"])
                    .chain(e.path.split('/')),
            );
        let dir = &e.path[..e.path.len() - name.len()];
        let split = split_info(&name);
        let stem = split
            .map(|s| s.0)
            .unwrap_or(&name[..name.len() - 5])
            .to_lowercase();
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
    let describe = |g: &Group| {
        g.parts
            .iter()
            .map(|(_, f)| f.name.as_str())
            .min()
            .unwrap_or_default()
            .to_string()
    };
    let exact = groups.iter().position(|g| {
        g.parts
            .iter()
            .min_by_key(|p| p.0)
            .is_some_and(|(_, f)| f.name.to_lowercase() == quant)
    });
    let chosen = match groups.len() {
        0 => return Err(format!("No .gguf file matching {quant:?} in {repo}")),
        _ if exact.is_some() => groups.remove(exact.unwrap_or_default()),
        1 => groups.remove(0),
        _ => {
            if groups.iter().filter(|g| g.token).count() == 1 {
                groups.remove(groups.iter().position(|g| g.token).unwrap_or_default())
            } else {
                let names: Vec<_> = groups.iter().map(describe).collect();
                return Err(format!(
                    "{quant:?} matches several files, be more specific: {}",
                    names.join(", ")
                ));
            }
        }
    };
    let mut parts = chosen.parts;
    if let Some(total) = chosen.total
        && !complete(parts.iter().map(|(i, _)| *i).collect(), total)
    {
        return Err(format!(
            "Incomplete split model: found {} of {total} parts",
            parts.len()
        ));
    }
    parts.sort_by_key(|(i, _)| *i);
    Ok(parts.into_iter().map(|(_, f)| f).collect())
}

/// `{hf_base}/api/models?search=<query>&filter=gguf&sort=downloads&direction=-1&limit=20`, the query percent-encoded.
pub fn search_url(hf_base: &str, query: &str) -> String {
    let mut q = String::new();
    for b in query.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            q.push(b as char);
        } else {
            q.push_str(&format!("%{b:02X}"));
        }
    }
    format!("{hf_base}/api/models?search={q}&filter=gguf&sort=downloads&direction=-1&limit=20")
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HfRepo {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
}

/// Parses the HF model search JSON (an array of `{id | modelId, downloads?, likes?}`), keeping the API order.
pub fn parse_search(json: &str) -> Result<Vec<HfRepo>, String> {
    #[derive(serde::Deserialize)]
    struct Raw {
        id: Option<String>,
        #[serde(rename = "modelId")]
        model_id: Option<String>,
        downloads: Option<u64>,
        likes: Option<u64>,
    }
    let raw: Vec<Raw> =
        serde_json::from_str(json).map_err(|e| format!("Unexpected Hugging Face response: {e}"))?;
    Ok(raw
        .into_iter()
        .filter_map(|r| {
            Some(HfRepo {
                id: r.id.or(r.model_id)?,
                downloads: r.downloads.unwrap_or(0),
                likes: r.likes.unwrap_or(0),
            })
        })
        .collect())
}

/// One downloadable quantization of a repo.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct QuantFile {
    /// Quant token as written in the file name (`Q4_K_M`, `IQ4_XS`, `f16`; `UD-Q4_K_XL` gives `Q4_K_XL`), "" if none.
    pub quant: String,
    /// File name, the first part for a split set. Unique per row and made of `[A-Za-z0-9._-]`, so
    /// `owner/repo:<name>` is the key that downloads exactly this row (see `pick_files`).
    pub name: String,
    /// Bytes, summed over all parts.
    pub size: u64,
    pub parts: u32,
}

/// The last `-`/`.` separated token of `stem` that looks like a quant (`Q4_K_M`, `IQ4_XS`, `Q4_0_4_4`, `F16`, `BF16`).
fn quant_token(stem: &str) -> &str {
    let is_quant = |t: &str| {
        let t = t.to_ascii_uppercase();
        matches!(t.as_str(), "F16" | "BF16" | "F32" | "MXFP4")
            || t.strip_prefix("IQ")
                .or_else(|| t.strip_prefix('Q'))
                .is_some_and(|r| {
                    r.starts_with(|c: char| c.is_ascii_digit())
                        && r.split('_')
                            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_alphanumeric()))
                })
    };
    stem.rsplit(['-', '.'])
        .find(|t| is_quant(t))
        .unwrap_or_default()
}

/// One row per model in an HF tree JSON (the input of `pick_files`), smallest first: `.gguf` files except
/// `mmproj*`, a complete split set collapsed into one row (incomplete sets are dropped).
pub fn quant_files(tree_json: &str) -> Result<Vec<QuantFile>, String> {
    let entries: Vec<TreeEntry> = serde_json::from_str(tree_json)
        .map_err(|e| format!("Unexpected Hugging Face response: {e}"))?;
    // (set key, total, parts as (index, name, bytes))
    type Set = (String, u32, Vec<(u32, String, u64)>);
    let mut sets: Vec<Set> = Vec::new();
    for e in entries.into_iter().filter(|e| e.kind == "file") {
        let name = e.path.rsplit('/').next().unwrap_or_default().to_string();
        let key_safe = name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
        if valid_name(&name).is_err() || !key_safe || name.to_lowercase().starts_with("mmproj") {
            continue;
        }
        let size = e.lfs.and_then(|l| l.size).or(e.size).unwrap_or(0);
        let (key, idx, total) = match split_info(&name) {
            Some((stem, idx, total)) => (
                format!("{}{stem}", &e.path[..e.path.len() - name.len()]),
                idx,
                total,
            ),
            None => (e.path.clone(), 1, 1),
        };
        match sets.iter_mut().find(|s| s.0 == key && s.1 == total) {
            Some(s) => s.2.push((idx, name, size)),
            None => sets.push((key, total, vec![(idx, name, size)])),
        }
    }
    let mut rows: Vec<QuantFile> = sets
        .into_iter()
        .filter(|(_, total, parts)| complete(parts.iter().map(|p| p.0).collect(), *total))
        .map(|(_, total, mut parts)| {
            parts.sort_by_key(|p| p.0);
            let name = parts[0].1.clone();
            let stem = split_info(&name).map_or(&name[..name.len() - 5], |s| s.0);
            QuantFile {
                quant: quant_token(stem).to_string(),
                size: parts.iter().map(|p| p.2).sum(),
                name,
                parts: total,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.size.cmp(&b.size).then_with(|| a.name.cmp(&b.name)));
    Ok(rows)
}

/// Hf: fetches the tree from `hf_base` and calls `pick_files`. Url: one file, size and sha256 None.
pub async fn resolve(
    client: &reqwest::Client,
    hf_base: &str,
    source: &Source,
) -> Result<Vec<RemoteFile>, String> {
    match source {
        Source::Url(url) => {
            let parsed = reqwest::Url::parse(url).map_err(|_| format!("Invalid URL: {url}"))?;
            let name = url_file_name(&parsed)?;
            Ok(vec![RemoteFile {
                name,
                url: url.clone(),
                size: None,
                sha256: None,
            }])
        }
        Source::Hf { repo, quant } => pick_files(
            &fetch_tree(client, hf_base, repo).await?,
            hf_base,
            repo,
            quant,
        ),
    }
}

/// The tree JSON of `repo` (`{hf_base}/api/models/{repo}/tree/main?recursive=true`); Err when it is paginated.
pub async fn fetch_tree(
    client: &reqwest::Client,
    hf_base: &str,
    repo: &str,
) -> Result<String, String> {
    let resp = client
        .get(format!(
            "{hf_base}/api/models/{repo}/tree/main?recursive=true"
        ))
        .send()
        .await
        .map_err(|_| OFFLINE.to_string())?;
    if !resp.status().is_success() {
        return Err(format!(
            "Hugging Face returned HTTP {} for {repo}",
            resp.status().as_u16()
        ));
    }
    if resp
        .headers()
        .get_all("link")
        .iter()
        .any(|v| v.to_str().is_ok_and(|v| v.contains("rel=\"next\"")))
    {
        return Err(format!("{repo} has too many files to list"));
    }
    resp.text()
        .await
        .map_err(|e| format!("Cannot read Hugging Face response: {e}"))
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
    let Ok(read) = std::fs::read_dir(models_dir) else {
        return Vec::new();
    };
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
            Some((stem, idx, total)) => splits
                .entry((stem.to_string(), total))
                .or_default()
                .push((idx, name, meta.len())),
            None => out.push(InstalledModel {
                file: name,
                size: meta.len(),
            }),
        }
    }
    for ((_, total), mut parts) in splits {
        if complete(parts.iter().map(|p| p.0).collect(), total) {
            parts.sort_by_key(|p| p.0);
            out.push(InstalledModel {
                file: parts[0].1.clone(),
                size: parts.iter().map(|p| p.2).sum(),
            });
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
        for entry in std::fs::read_dir(models_dir)
            .map_err(|e| format!("Cannot read models folder: {e}"))?
            .flatten()
        {
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
        std::fs::remove_file(models_dir.join(&name))
            .map_err(|e| format!("Cannot delete {name}: {e}"))?;
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
    std::fs::create_dir_all(models_dir)
        .map_err(|e| format!("Cannot create {}: {e}", models_dir.display()))?;
    let mut done: Vec<&RemoteFile> = Vec::new();
    for f in files.iter().rev() {
        let dest = models_dir.join(&f.name);
        if let Err(e) =
            download::download(client, &f.url, &dest, f.sha256.as_deref(), cancel, |p| {
                on_progress(&f.name, p)
            })
            .await
        {
            for d in done {
                let _ = std::fs::remove_file(models_dir.join(&d.name));
            }
            return Err(e);
        }
        done.push(f);
    }
    Ok(first.name.clone())
}
