use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use screen_translate_core::manager::download::{
    CANCELLED, Options, Progress, download_with, plan_chunks,
};
use sha2::{Digest, Sha256};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const LEN: usize = 10_007;

fn body() -> Vec<u8> {
    (0..LEN).map(|i| (i * 31 % 251) as u8).collect()
}

fn sha_hex(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

fn part(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    s.into()
}

fn opts() -> Options {
    Options {
        connections: 4,
        chunk_size: 1000,
        retries: 3,
        min_parallel_size: 1,
    }
}

#[derive(Clone, Copy)]
enum Fail {
    Status500,
    Truncated,
}

#[derive(Clone, Debug, PartialEq)]
struct Hit {
    path: String,
    range: Option<(u64, u64)>,
}

/// Range-aware responder. A request with `Range: bytes=a-b` gets 206 + Content-Range (when `ranges` is on).
struct RangeBody {
    body: Vec<u8>,
    ranges: bool,
    hits: Arc<Mutex<Vec<Hit>>>,
    /// range start -> (remaining failures, kind)
    failures: Mutex<HashMap<u64, (u32, Fail)>>,
    /// delay by range start; applied to ranges longer than one byte (never the 0-0 probe)
    delay: Box<dyn Fn(u64) -> Duration + Send + Sync>,
}

impl RangeBody {
    fn new(body: Vec<u8>) -> (Self, Arc<Mutex<Vec<Hit>>>) {
        let hits = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                body,
                ranges: true,
                hits: hits.clone(),
                failures: Mutex::new(HashMap::new()),
                delay: Box::new(|_| Duration::ZERO),
            },
            hits,
        )
    }
    fn fail(self, start: u64, times: u32, kind: Fail) -> Self {
        self.failures.lock().unwrap().insert(start, (times, kind));
        self
    }
    fn delay(mut self, f: impl Fn(u64) -> Duration + Send + Sync + 'static) -> Self {
        self.delay = Box::new(f);
        self
    }
    fn no_ranges(mut self) -> Self {
        self.ranges = false;
        self
    }
}

fn parse_range(req: &Request) -> Option<(u64, u64)> {
    let v = req.headers.get("range")?.to_str().ok()?;
    let (a, b) = v.strip_prefix("bytes=")?.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

impl Respond for RangeBody {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let range = parse_range(req);
        self.hits.lock().unwrap().push(Hit {
            path: req.url.path().to_string(),
            range,
        });
        let total = self.body.len() as u64;
        let Some((a, b)) = range.filter(|_| self.ranges) else {
            return ResponseTemplate::new(200).set_body_bytes(self.body.clone());
        };
        let b = b.min(total - 1);
        let mut t = ResponseTemplate::new(206)
            .insert_header("Content-Range", format!("bytes {a}-{b}/{total}"))
            .insert_header("Accept-Ranges", "bytes");
        let mut slice = self.body[a as usize..=b as usize].to_vec();
        if b > a {
            let d = (self.delay)(a);
            if !d.is_zero() {
                t = t.set_delay(d);
            }
            let mut f = self.failures.lock().unwrap();
            if let Some((left, kind)) = f.get_mut(&a)
                && *left > 0
            {
                *left -= 1;
                match kind {
                    Fail::Status500 => return ResponseTemplate::new(500),
                    Fail::Truncated => slice.truncate(slice.len() / 2),
                }
            }
        }
        t.set_body_bytes(slice)
    }
}

async fn serve(r: RangeBody) -> MockServer {
    serve_with(r).await
}

async fn serve_with(r: impl Respond + 'static) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/f"))
        .respond_with(r)
        .mount(&server)
        .await;
    server
}

fn data_ranged(hits: &Arc<Mutex<Vec<Hit>>>) -> Vec<(u64, u64)> {
    hits.lock()
        .unwrap()
        .iter()
        .filter_map(|h| h.range)
        .filter(|(a, b)| b > a)
        .collect()
}

async fn run(
    url: &str,
    dest: &Path,
    sha: Option<&str>,
    cancel: &AtomicBool,
    o: &Options,
) -> (Result<(), String>, Vec<Progress>) {
    let mut seen = Vec::new();
    let r = download_with(
        &reqwest::Client::new(),
        url,
        dest,
        sha,
        cancel,
        |p| seen.push(p),
        o,
    )
    .await;
    (r, seen)
}

fn assert_clean_failure(dest: &Path) {
    assert!(!dest.exists(), "dest must not exist");
    assert!(!part(dest).exists(), ".part must be removed");
}

// 1. plan_chunks

#[test]
fn plan_chunks_exact_multiple() {
    assert_eq!(
        plan_chunks(3000, 1000),
        vec![(0, 999), (1000, 1999), (2000, 2999)]
    );
}

#[test]
fn plan_chunks_remainder_is_last_chunk() {
    assert_eq!(
        plan_chunks(2500, 1000),
        vec![(0, 999), (1000, 1999), (2000, 2499)]
    );
}

#[test]
fn plan_chunks_total_smaller_than_chunk() {
    assert_eq!(plan_chunks(10, 1000), vec![(0, 9)]);
}

#[test]
fn plan_chunks_zero_is_empty() {
    assert!(plan_chunks(0, 1000).is_empty());
}

#[test]
fn plan_chunks_one_byte() {
    assert_eq!(plan_chunks(1, 1000), vec![(0, 0)]);
}

#[test]
fn plan_chunks_contiguous_and_exact_cover() {
    for (total, size) in [
        (10_007u64, 1000u64),
        (1, 1),
        (999, 1000),
        (1000, 1000),
        (1001, 1000),
        (65_537, 4096),
    ] {
        let c = plan_chunks(total, size);
        assert_eq!(c.first().unwrap().0, 0);
        assert_eq!(c.last().unwrap().1, total - 1);
        for w in c.windows(2) {
            assert_eq!(
                w[0].1 + 1,
                w[1].0,
                "gap or overlap for total={total} size={size}"
            );
        }
        for (i, (a, b)) in c.iter().enumerate() {
            assert!(b >= a);
            if i + 1 < c.len() {
                assert_eq!(b - a + 1, size);
            } else {
                assert!(b - a < size);
            }
        }
    }
}

// 2. parallel success

#[tokio::test]
async fn parallel_success_is_byte_exact_with_progress() {
    let b = body();
    let (r, hits) = RangeBody::new(b.clone());
    let server = serve(r).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let upper = sha_hex(&b).to_uppercase();

    let (res, seen) = run(
        &format!("{}/f", server.uri()),
        &dest,
        Some(&upper),
        &AtomicBool::new(false),
        &opts(),
    )
    .await;

    res.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b);
    assert!(!part(&dest).exists());
    assert!(
        data_ranged(&hits).len() > 1,
        "expected several ranged chunk requests, got {:?}",
        hits.lock().unwrap()
    );
    let last = seen.last().expect("progress callbacks");
    assert_eq!(last.downloaded, b.len() as u64);
    assert!(seen.iter().all(|p| p.total == Some(b.len() as u64)));
    assert!(
        seen.windows(2).all(|w| w[0].downloaded <= w[1].downloaded),
        "progress must be monotonic"
    );
}

// 3. out-of-order assembly

#[tokio::test]
async fn out_of_order_chunks_assemble_correctly() {
    let b = body();
    let (r, _hits) = RangeBody::new(b.clone());
    // early ranges are slowest, so later chunks land first
    let r = r.delay(|start| Duration::from_millis(400u64.saturating_sub(start / 25)));
    let server = serve(r).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let (res, _) = run(
        &format!("{}/f", server.uri()),
        &dest,
        Some(&sha_hex(&b)),
        &AtomicBool::new(false),
        &opts(),
    )
    .await;

    res.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b);
}

// 4. fallbacks

#[tokio::test]
async fn fallback_when_server_ignores_range() {
    let b = body();
    let (r, hits) = RangeBody::new(b.clone());
    let server = serve(r.no_ranges()).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let (res, seen) = run(
        &format!("{}/f", server.uri()),
        &dest,
        Some(&sha_hex(&b)),
        &AtomicBool::new(false),
        &opts(),
    )
    .await;

    res.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b);
    assert!(!part(&dest).exists());
    assert_eq!(seen.last().unwrap().downloaded, b.len() as u64);
    assert!(data_ranged(&hits).is_empty());
    assert!(
        hits.lock().unwrap().len() <= 2,
        "probe + one stream at most"
    );
}

/// Raw server: every request gets a 200 chunked response with no Content-Length and no range support.
async fn serve_chunked(body: Vec<u8>) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let h = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
                    .await;
                for c in body.chunks(1500) {
                    let _ = sock
                        .write_all(format!("{:x}\r\n", c.len()).as_bytes())
                        .await;
                    let _ = sock.write_all(c).await;
                    let _ = sock.write_all(b"\r\n").await;
                }
                let _ = sock.write_all(b"0\r\n\r\n").await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (addr, h)
}

#[tokio::test]
async fn fallback_when_no_content_length() {
    let b = body();
    let (addr, srv) = serve_chunked(b.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let (res, seen) = run(
        &format!("http://{addr}/f"),
        &dest,
        Some(&sha_hex(&b)),
        &AtomicBool::new(false),
        &opts(),
    )
    .await;
    srv.abort();

    res.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b);
    assert!(!part(&dest).exists());
    assert_eq!(seen.last().unwrap().downloaded, b.len() as u64);
}

#[tokio::test]
async fn small_file_below_min_parallel_size_streams_once() {
    let b = body();
    let (r, hits) = RangeBody::new(b.clone());
    let server = serve(r).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let o = Options {
        min_parallel_size: 1_000_000,
        ..opts()
    };

    let (res, seen) = run(
        &format!("{}/f", server.uri()),
        &dest,
        Some(&sha_hex(&b)),
        &AtomicBool::new(false),
        &o,
    )
    .await;

    res.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b);
    assert_eq!(seen.last().unwrap().downloaded, b.len() as u64);
    assert!(
        data_ranged(&hits).is_empty(),
        "no ranged chunk requests below the threshold"
    );
    assert!(
        hits.lock().unwrap().len() <= 2,
        "{:?}",
        hits.lock().unwrap()
    );
}

// 5. redirect

#[tokio::test]
async fn chunks_hit_the_final_url_after_redirect() {
    let b = body();
    let (r, hits) = RangeBody::new(b.clone());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/real", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/real"))
        .respond_with(r)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let (res, _) = run(
        &format!("{}/start", server.uri()),
        &dest,
        Some(&sha_hex(&b)),
        &AtomicBool::new(false),
        &opts(),
    )
    .await;

    res.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b);
    let all = server.received_requests().await.unwrap();
    let starts = all.iter().filter(|r| r.url.path() == "/start").count();
    assert!(
        starts <= 1,
        "/start hit {starts} times; chunks must target the final URL"
    );
    assert!(data_ranged(&hits).len() > 1);
    assert!(hits.lock().unwrap().iter().all(|h| h.path == "/real"));
}

// 6. checksum mismatch

#[tokio::test]
async fn parallel_checksum_mismatch_errs_and_cleans_up() {
    let b = body();
    let (r, hits) = RangeBody::new(b);
    let server = serve(r).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let (res, _) = run(
        &format!("{}/f", server.uri()),
        &dest,
        Some(&"0".repeat(64)),
        &AtomicBool::new(false),
        &opts(),
    )
    .await;

    let err = res.unwrap_err();
    assert!(err.contains("checksum"), "{err}");
    assert!(
        data_ranged(&hits).len() > 1,
        "must have gone through the parallel path"
    );
    assert_clean_failure(&dest);
}

// 7. chunk retry

#[tokio::test]
async fn chunk_failing_twice_is_retried_and_succeeds() {
    let b = body();
    let (r, hits) = RangeBody::new(b.clone());
    let server = serve(r.fail(3000, 2, Fail::Status500)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let (res, _) = run(
        &format!("{}/f", server.uri()),
        &dest,
        Some(&sha_hex(&b)),
        &AtomicBool::new(false),
        &opts(),
    )
    .await;

    res.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b);
    let tries = data_ranged(&hits)
        .iter()
        .filter(|(a, _)| *a == 3000)
        .count();
    assert_eq!(tries, 3, "two failures then one success");
}

#[tokio::test]
async fn chunk_failing_beyond_retries_fails_and_cleans_up() {
    let b = body();
    let (r, _hits) = RangeBody::new(b.clone());
    let server = serve(r.fail(3000, 100, Fail::Status500)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let (res, _) = run(
        &format!("{}/f", server.uri()),
        &dest,
        Some(&sha_hex(&b)),
        &AtomicBool::new(false),
        &opts(),
    )
    .await;

    assert!(res.is_err());
    assert_clean_failure(&dest);
}

#[tokio::test]
async fn truncated_chunk_body_counts_as_failure_then_retries() {
    let b = body();
    let (r, hits) = RangeBody::new(b.clone());
    let server = serve(r.fail(5000, 1, Fail::Truncated)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let (res, _) = run(
        &format!("{}/f", server.uri()),
        &dest,
        Some(&sha_hex(&b)),
        &AtomicBool::new(false),
        &opts(),
    )
    .await;

    res.unwrap();
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        b,
        "a short chunk must not leave a hole"
    );
    let tries = data_ranged(&hits)
        .iter()
        .filter(|(a, _)| *a == 5000)
        .count();
    assert_eq!(tries, 2, "truncated once, then retried");
}

#[tokio::test]
async fn persistently_truncated_chunk_fails() {
    let b = body();
    let (r, _hits) = RangeBody::new(b.clone());
    let server = serve(r.fail(5000, 100, Fail::Truncated)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    // no checksum given: the length check alone must catch the short body
    let (res, _) = run(
        &format!("{}/f", server.uri()),
        &dest,
        None,
        &AtomicBool::new(false),
        &opts(),
    )
    .await;

    assert!(res.is_err());
    assert_clean_failure(&dest);
}

// 8. cancel

#[tokio::test]
async fn cancel_stops_all_connections_promptly() {
    let b = body();
    let (r, _hits) = RangeBody::new(b);
    let server = serve(r.delay(|_| Duration::from_secs(10))).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    std::fs::write(&dest, "old").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let url = format!("{}/f", server.uri());

    let started = Instant::now();
    let flag = cancel.clone();
    let canceller = async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        flag.store(true, Ordering::SeqCst);
    };
    let o = opts();
    let (res, _) = tokio::join!(run(&url, &dest, None, &cancel, &o), canceller);

    assert_eq!(res.0.unwrap_err(), CANCELLED);
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "took {:?}",
        started.elapsed()
    );
    assert!(!part(&dest).exists());
    assert_eq!(std::fs::read(&dest).unwrap(), b"old");
}

// 9. existing dest untouched on parallel failure

#[tokio::test]
async fn parallel_failure_leaves_existing_dest_untouched() {
    let b = body();
    let (r, hits) = RangeBody::new(b.clone());
    let server = serve(r.fail(2000, 100, Fail::Status500)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    std::fs::write(&dest, "old").unwrap();

    let (res, _) = run(
        &format!("{}/f", server.uri()),
        &dest,
        Some(&sha_hex(&b)),
        &AtomicBool::new(false),
        &opts(),
    )
    .await;

    assert!(res.is_err());
    assert!(!data_ranged(&hits).is_empty());
    assert_eq!(std::fs::read(&dest).unwrap(), b"old");
    assert!(!part(&dest).exists());
}

// 10. expiring signed URLs

/// 403s every request once `left` requests have been served; otherwise delegates.
struct Expiring {
    inner: RangeBody,
    left: std::sync::atomic::AtomicI64,
}

impl Respond for Expiring {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        if self.left.fetch_sub(1, Ordering::SeqCst) <= 0 {
            return ResponseTemplate::new(403);
        }
        self.inner.respond(req)
    }
}

/// First hit redirects to `/real1`, every later hit to `/real2`.
struct Rotating {
    hits: std::sync::atomic::AtomicUsize,
    base: String,
}

impl Respond for Rotating {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let n = if self.hits.fetch_add(1, Ordering::SeqCst) == 0 {
            1
        } else {
            2
        };
        ResponseTemplate::new(302).insert_header("Location", format!("{}/real{n}", self.base))
    }
}

#[tokio::test]
async fn expired_final_url_is_re_resolved_from_the_original_url() {
    let b = body();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(Rotating {
            hits: Default::default(),
            base: server.uri(),
        })
        .mount(&server)
        .await;
    let (r1, _) = RangeBody::new(b.clone());
    let (r2, hits2) = RangeBody::new(b.clone());
    // probe + two chunks succeed, then the signed URL "expires"
    Mock::given(method("GET"))
        .and(path("/real1"))
        .respond_with(Expiring {
            inner: r1,
            left: 3.into(),
        })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/real2"))
        .respond_with(r2)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let (res, _) = run(
        &format!("{}/start", server.uri()),
        &dest,
        Some(&sha_hex(&b)),
        &AtomicBool::new(false),
        &opts(),
    )
    .await;

    res.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b);
    assert!(
        !data_ranged(&hits2).is_empty(),
        "later chunks must use the fresh URL"
    );
}

// 11. chunk response validation

/// Serves a shifted slice under a Content-Range for the shifted range (right length, wrong bytes) `times` times for chunk `at`.
struct WrongRange {
    inner: RangeBody,
    at: u64,
    times: std::sync::atomic::AtomicU32,
}

impl Respond for WrongRange {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        if let Some((a, b)) = parse_range(req)
            && a == self.at
            && b > a
            && self.times.load(Ordering::SeqCst) > 0
        {
            self.times.fetch_sub(1, Ordering::SeqCst);
            self.inner.hits.lock().unwrap().push(Hit {
                path: req.url.path().to_string(),
                range: Some((a, b)),
            });
            let total = self.inner.body.len() as u64;
            let (a, b) = (a + 1, (b + 1).min(total - 1));
            return ResponseTemplate::new(206)
                .insert_header("Content-Range", format!("bytes {a}-{b}/{total}"))
                .set_body_bytes(self.inner.body[a as usize..=b as usize].to_vec());
        }
        self.inner.respond(req)
    }
}

#[tokio::test]
async fn chunk_with_mismatched_content_range_is_rejected_and_retried() {
    let b = body();
    let (r, hits) = RangeBody::new(b.clone());
    let server = serve_with(WrongRange {
        inner: r,
        at: 4000,
        times: 1.into(),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let o = Options {
        retries: 1,
        ..opts()
    };

    // no checksum: only the Content-Range check can catch the shifted bytes
    let (res, _) = run(
        &format!("{}/f", server.uri()),
        &dest,
        None,
        &AtomicBool::new(false),
        &o,
    )
    .await;

    res.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b);
    assert_eq!(
        data_ranged(&hits)
            .iter()
            .filter(|(a, _)| *a == 4000)
            .count(),
        2
    );
}

#[tokio::test]
async fn persistently_wrong_content_range_fails() {
    let b = body();
    let (r, _) = RangeBody::new(b);
    let server = serve_with(WrongRange {
        inner: r,
        at: 4000,
        times: 100.into(),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let o = Options {
        retries: 1,
        ..opts()
    };

    let (res, _) = run(
        &format!("{}/f", server.uri()),
        &dest,
        None,
        &AtomicBool::new(false),
        &o,
    )
    .await;

    assert!(res.is_err());
    assert_clean_failure(&dest);
}

/// 206 for the 0-0 probe, then 200 with the whole body for every longer range.
struct IgnoresChunkRanges(RangeBody);

impl Respond for IgnoresChunkRanges {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        match parse_range(req) {
            Some((a, b)) if b > a => ResponseTemplate::new(200).set_body_bytes(self.0.body.clone()),
            _ => self.0.respond(req),
        }
    }
}

#[tokio::test]
async fn ranged_chunk_answered_with_200_says_range_was_ignored() {
    let (r, _) = RangeBody::new(body());
    let server = serve_with(IgnoresChunkRanges(r)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let o = Options {
        retries: 1,
        ..opts()
    };

    let (res, _) = run(
        &format!("{}/f", server.uri()),
        &dest,
        None,
        &AtomicBool::new(false),
        &o,
    )
    .await;

    let err = res.unwrap_err();
    assert!(err.contains("ignored the range"), "{err}");
    assert_clean_failure(&dest);
}

// 12. dropping the future

#[tokio::test]
async fn dropping_the_future_mid_transfer_removes_part() {
    let (r, hits) = RangeBody::new(body());
    let server = serve(r.delay(|_| Duration::from_secs(10))).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let cancel = AtomicBool::new(false);
    let o = opts();
    let url = format!("{}/f", server.uri());

    let timed_out = tokio::time::timeout(
        Duration::from_millis(300),
        run(&url, &dest, None, &cancel, &o),
    )
    .await;

    assert!(timed_out.is_err(), "transfer should still be running");
    assert!(
        !data_ranged(&hits).is_empty(),
        "must have been mid parallel transfer"
    );
    assert_clean_failure(&dest);
}
