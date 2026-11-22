use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Progress {
    pub downloaded: u64,
    /// From Content-Length, when the server sends it.
    pub total: Option<u64>,
}

pub const CANCELLED: &str = "Download cancelled";

/// Ok when `url` parses, uses https, and has a non-empty host.
pub fn require_https(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| format!("Invalid URL: {url}"))?;
    if parsed.scheme() != "https" || parsed.host_str().is_none_or(str::is_empty) {
        return Err(format!("Only https URLs are allowed: {url}"));
    }
    Ok(())
}

/// Lowercase hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Streams `url` into `dest`.part (creating parent dirs), then renames it to `dest`.
/// `expected_sha256` (hex, any case) is checked before the rename. `cancel` is checked between chunks.
/// On any failure (non-2xx, IO, checksum mismatch, cancel) the .part file is removed and `dest` is untouched.
/// Errors: cancel returns exactly `CANCELLED`; non-2xx mentions the status code; mismatch mentions "checksum".
/// Callers must check `require_https` first; this function does not, so tests can use a local http server.
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    cancel: &AtomicBool,
    on_progress: impl FnMut(Progress),
) -> Result<(), String> {
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    // Removes the .part on every exit, including the future being dropped mid-transfer.
    let mut guard = PartGuard(Some(part.clone()));
    stream_to(client, url, &part, expected_sha256, cancel, on_progress).await?;
    std::fs::rename(&part, dest).map_err(|e| format!("Cannot save {}: {e}", dest.display()))?;
    guard.0 = None;
    Ok(())
}

struct PartGuard(Option<PathBuf>);

impl Drop for PartGuard {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// HTTP client for downloads: redirects only to https (max 10), 15s connect timeout, 30s read timeout
/// so a stalled connection fails instead of hanging.
pub fn client() -> reqwest::Client {
    let policy = reqwest::redirect::Policy::custom(|a| {
        if a.previous().len() >= 10 {
            a.error("too many redirects")
        } else if a.url().scheme() != "https" {
            a.error("redirect to a non-https URL refused")
        } else {
            a.follow()
        }
    });
    reqwest::Client::builder()
        .redirect(policy)
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("download client")
}

async fn stream_to(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    expected_sha256: Option<&str>,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
) -> Result<(), String> {
    let cancelled = || cancel.load(Ordering::SeqCst);
    if cancelled() {
        return Err(CANCELLED.into());
    }
    let mut resp = client.get(url).send().await.map_err(|e| format!("Download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status().as_u16()));
    }
    let total = resp.content_length();
    if let Some(dir) = part.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    }
    let mut file = std::fs::File::create(part).map_err(|e| format!("Cannot write {}: {e}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("Download failed: {e}"))? {
        if cancelled() {
            return Err(CANCELLED.into());
        }
        file.write_all(&chunk).map_err(|e| format!("Cannot write {}: {e}", part.display()))?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        on_progress(Progress { downloaded, total });
    }
    file.flush().map_err(|e| format!("Cannot write {}: {e}", part.display()))?;
    if let Some(expected) = expected_sha256
        && !format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(expected)
    {
        return Err("Download failed: checksum mismatch".into());
    }
    Ok(())
}
