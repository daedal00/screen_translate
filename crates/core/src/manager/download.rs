use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;

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
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Tuning for `download_with`.
#[derive(Debug, Clone)]
pub struct Options {
    /// Concurrent ranged requests.
    pub connections: usize,
    /// Bytes per ranged request.
    pub chunk_size: u64,
    /// Extra attempts per chunk after the first failure.
    pub retries: u32,
    /// Files smaller than this are streamed over one connection.
    pub min_parallel_size: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            connections: 8,
            chunk_size: 16 << 20,
            retries: 3,
            min_parallel_size: 32 << 20,
        }
    }
}

/// Inclusive `(start, end)` byte ranges of `chunk_size` covering `total` bytes; the last may be shorter.
pub fn plan_chunks(total: u64, chunk_size: u64) -> Vec<(u64, u64)> {
    let size = chunk_size.max(1);
    (0..total.div_ceil(size))
        .map(|i| (i * size, ((i + 1) * size).min(total) - 1))
        .collect()
}

/// `download_with` using `Options::default()`.
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    cancel: &AtomicBool,
    on_progress: impl FnMut(Progress),
) -> Result<(), String> {
    download_with(
        client,
        url,
        dest,
        expected_sha256,
        cancel,
        on_progress,
        &Options::default(),
    )
    .await
}

/// Downloads `url` into `dest`.part (creating parent dirs), then renames it to `dest`.
/// When the server supports HTTP Range and the file is at least `opts.min_parallel_size`, it is fetched in
/// `opts.chunk_size` pieces over `opts.connections` connections (each retried `opts.retries` times);
/// otherwise it is streamed over one connection.
/// `expected_sha256` (hex, any case) is checked before the rename. `cancel` is polled while transferring.
/// On any failure (non-2xx, IO, checksum mismatch, cancel) the .part file is removed and `dest` is untouched.
/// Errors: cancel returns exactly `CANCELLED`; non-2xx mentions the status code; mismatch mentions "checksum".
/// Callers must check `require_https` first; this function does not, so tests can use a local http server.
pub async fn download_with(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    cancel: &AtomicBool,
    on_progress: impl FnMut(Progress),
    opts: &Options,
) -> Result<(), String> {
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    // Removes the .part on every exit, including the future being dropped mid-transfer.
    let mut guard = PartGuard(Some(part.clone()));
    transfer(
        client,
        url,
        &part,
        expected_sha256,
        cancel,
        on_progress,
        opts,
    )
    .await?;
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

fn http_err(status: reqwest::StatusCode) -> String {
    format!("Download failed: HTTP {}", status.as_u16())
}

fn io_err(part: &Path, e: std::io::Error) -> String {
    format!("Cannot write {}: {e}", part.display())
}

/// Total size from a `Content-Range: bytes a-b/N` header.
fn content_range_total(resp: &reqwest::Response) -> Option<u64> {
    let v = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)?
        .to_str()
        .ok()?;
    v.rsplit_once('/')?.1.parse().ok()
}

async fn transfer(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    expected_sha256: Option<&str>,
    cancel: &AtomicBool,
    on_progress: impl FnMut(Progress),
    opts: &Options,
) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(CANCELLED.into());
    }
    if let Some(dir) = part.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    }
    // A 1-byte ranged probe: 206 tells us the size and that ranges work; 200 is the full body, streamed as is.
    let mut resp = client
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(http_err(resp.status()));
    }
    if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        match content_range_total(&resp) {
            Some(total) if total >= opts.min_parallel_size.max(1) => {
                let target = resp.url().clone();
                drop(resp);
                let fetch = Fetch {
                    client,
                    original: url,
                    current: Mutex::new(target),
                    part,
                    total,
                    done: AtomicU64::new(0),
                };
                fetch_parallel(&fetch, cancel, on_progress, opts).await?;
                return verify(part, expected_sha256).await;
            }
            _ => {
                resp = client
                    .get(url)
                    .send()
                    .await
                    .map_err(|e| format!("Download failed: {e}"))?;
                if !resp.status().is_success() {
                    return Err(http_err(resp.status()));
                }
            }
        }
    }
    stream_single(resp, part, expected_sha256, cancel, on_progress).await
}

async fn verify(part: &Path, expected_sha256: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected_sha256 else {
        return Ok(());
    };
    let path = part.to_owned();
    let actual = tokio::task::spawn_blocking(move || sha256_file(&path))
        .await
        .map_err(|e| format!("Download failed: {e}"))??;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err("Download failed: checksum mismatch".into())
    }
}

async fn stream_single(
    mut resp: reqwest::Response,
    part: &Path,
    expected_sha256: Option<&str>,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
) -> Result<(), String> {
    let total = resp.content_length();
    let mut file = std::fs::File::create(part).map_err(|e| io_err(part, e))?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("Download failed: {e}"))?
    {
        if cancel.load(Ordering::SeqCst) {
            return Err(CANCELLED.into());
        }
        file.write_all(&chunk).map_err(|e| io_err(part, e))?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        on_progress(Progress { downloaded, total });
    }
    file.flush().map_err(|e| io_err(part, e))?;
    if let Some(expected) = expected_sha256
        && !format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(expected)
    {
        return Err("Download failed: checksum mismatch".into());
    }
    Ok(())
}

/// State shared by the ranged requests of one parallel download.
struct Fetch<'a> {
    client: &'a reqwest::Client,
    /// The URL the caller gave; re-probed when the final URL expires.
    original: &'a str,
    /// Final URL after redirects, replaced when it expires.
    current: Mutex<reqwest::Url>,
    part: &'a Path,
    total: u64,
    done: AtomicU64,
}

/// Fills `part` (preallocated to `total`) with ranged GETs. Chunk futures live inside this future, so
/// cancelling or dropping it aborts every in-flight request.
async fn fetch_parallel(
    f: &Fetch<'_>,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
    opts: &Options,
) -> Result<(), String> {
    let (part, total) = (f.part, f.total);
    let file = std::fs::File::create(part).map_err(|e| io_err(part, e))?;
    file.set_len(total).map_err(|e| io_err(part, e))?;
    drop(file);

    let mut chunks = plan_chunks(total, opts.chunk_size).into_iter();
    let mut running = FuturesUnordered::new();
    let mut shown = 0u64;
    let mut emit = |done: &AtomicU64| {
        // Retried chunks give bytes back; never report progress going backwards.
        shown = shown.max(done.load(Ordering::SeqCst));
        on_progress(Progress {
            downloaded: shown,
            total: Some(total),
        });
    };
    loop {
        while running.len() < opts.connections.max(1)
            && let Some(range) = chunks.next()
        {
            running.push(fetch_chunk(f, range, opts.retries));
        }
        if running.is_empty() {
            break;
        }
        // A timeout just means no chunk finished within the tick; fall through to cancel and progress.
        if let Ok(Some(Err(e))) =
            tokio::time::timeout(Duration::from_millis(50), running.next()).await
        {
            return Err(e);
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(CANCELLED.into());
        }
        emit(&f.done);
    }
    emit(&f.done);
    Ok(())
}

/// One ranged GET written at its offset; retried up to `retries` times on any failure, after a
/// `500ms * attempt` pause. A 401/403/410 means the signed URL expired: the original URL is probed again
/// and the shared `current` URL replaced, then the chunk is retried without a pause.
async fn fetch_chunk(f: &Fetch<'_>, range: (u64, u64), retries: u32) -> Result<(), String> {
    let mut attempt = 0;
    loop {
        let mut got = 0u64;
        let url = f.current.lock().unwrap().clone();
        let (err, expired) = match try_chunk(f, &url, range, &mut got).await {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        f.done.fetch_sub(got, Ordering::SeqCst);
        if attempt >= retries {
            return Err(err);
        }
        attempt += 1;
        if expired {
            // Another chunk may have refreshed it already.
            if *f.current.lock().unwrap() == url {
                let fresh = probe_url(f.client, f.original).await?;
                *f.current.lock().unwrap() = fresh;
            }
        } else {
            tokio::time::sleep(Duration::from_millis(500) * attempt).await;
        }
    }
}

/// Final URL (after redirects) of `url`, via a 1-byte ranged GET.
async fn probe_url(client: &reqwest::Client, url: &str) -> Result<reqwest::Url, String> {
    let resp = client
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;
    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(http_err(resp.status()));
    }
    Ok(resp.url().clone())
}

/// Error text plus whether the status says the (signed) URL has expired.
type ChunkError = (String, bool);

async fn try_chunk(
    f: &Fetch<'_>,
    url: &reqwest::Url,
    (start, end): (u64, u64),
    got: &mut u64,
) -> Result<(), ChunkError> {
    let (part, total, done) = (f.part, f.total, &f.done);
    let fail = |e: String| (e, false);
    let mut resp = f
        .client
        .get(url.clone())
        .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
        .send()
        .await
        .map_err(|e| fail(format!("Download failed: {e}")))?;
    match resp.status() {
        reqwest::StatusCode::PARTIAL_CONTENT => {}
        reqwest::StatusCode::OK => {
            return Err(fail(
                "Download failed: server ignored the range request".into(),
            ));
        }
        s => return Err((http_err(s), matches!(s.as_u16(), 401 | 403 | 410))),
    }
    let expected = format!("bytes {start}-{end}/{total}");
    let ok = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        == Some(expected.as_str());
    if !ok {
        return Err(fail(
            "Download failed: server answered with a different byte range".into(),
        ));
    }
    let want = end - start + 1;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(part)
        .map_err(|e| fail(io_err(part, e)))?;
    file.seek(SeekFrom::Start(start))
        .map_err(|e| fail(io_err(part, e)))?;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| fail(format!("Download failed: {e}")))?
    {
        if *got + chunk.len() as u64 > want {
            return Err(fail(
                "Download failed: server sent more than the requested range".into(),
            ));
        }
        file.write_all(&chunk).map_err(|e| fail(io_err(part, e)))?;
        *got += chunk.len() as u64;
        done.fetch_add(chunk.len() as u64, Ordering::SeqCst);
    }
    if *got != want {
        return Err(fail("Download failed: incomplete chunk".into()));
    }
    Ok(())
}
