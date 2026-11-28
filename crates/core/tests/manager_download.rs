use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use screen_translate_core::manager::download::{
    CANCELLED, Progress, client, download, require_https, sha256_file,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn require_https_accepts_https() {
    assert!(require_https("https://huggingface.co/x").is_ok());
}

#[test]
fn require_https_rejects_other_schemes_and_garbage() {
    for url in [
        "http://huggingface.co/x",
        "file:///etc/passwd",
        "ftp://example.com/x",
        "not a url",
        "",
        "https://",
    ] {
        assert!(require_https(url).is_err(), "should reject {url:?}");
    }
}

#[test]
fn sha256_file_known_content() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f.bin");
    std::fs::write(&p, "abc").unwrap();
    assert_eq!(sha256_file(&p).unwrap(), ABC_SHA);
}

#[test]
fn sha256_file_missing_is_err() {
    let dir = tempfile::tempdir().unwrap();
    assert!(sha256_file(&dir.path().join("nope")).is_err());
}

fn part(dest: &Path) -> std::path::PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    s.into()
}

async fn serve(body: &[u8]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/f"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn download_success_writes_dest_and_reports_progress() {
    let body = vec![7u8; 200_000];
    let server = serve(&body).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let cancel = AtomicBool::new(false);
    let mut seen: Vec<Progress> = Vec::new();

    download(
        &reqwest::Client::new(),
        &format!("{}/f", server.uri()),
        &dest,
        None,
        &cancel,
        |p| seen.push(p),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert!(!part(&dest).exists());
    let last = seen.last().expect("progress callbacks");
    assert_eq!(last.downloaded, body.len() as u64);
    assert_eq!(last.total, Some(body.len() as u64));
    assert!(seen.windows(2).all(|w| w[0].downloaded <= w[1].downloaded));
}

#[tokio::test]
async fn download_sha_match_uppercase_ok() {
    let server = serve(b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let upper = ABC_SHA.to_uppercase();

    download(
        &reqwest::Client::new(),
        &format!("{}/f", server.uri()),
        &dest,
        Some(&upper),
        &AtomicBool::new(false),
        |_| {},
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(&dest).unwrap(), b"abc");
    assert!(!part(&dest).exists());
}

#[tokio::test]
async fn download_sha_mismatch_errs_and_cleans_up() {
    let server = serve(b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let wrong = "0".repeat(64);

    let err = download(
        &reqwest::Client::new(),
        &format!("{}/f", server.uri()),
        &dest,
        Some(&wrong),
        &AtomicBool::new(false),
        |_| {},
    )
    .await
    .unwrap_err();

    assert!(err.contains("checksum"), "{err}");
    assert!(!dest.exists());
    assert!(!part(&dest).exists());
}

#[tokio::test]
async fn download_404_errs_with_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let err = download(
        &reqwest::Client::new(),
        &format!("{}/f", server.uri()),
        &dest,
        None,
        &AtomicBool::new(false),
        |_| {},
    )
    .await
    .unwrap_err();

    assert!(err.contains("404"), "{err}");
    assert!(!dest.exists());
    assert!(!part(&dest).exists());
}

#[tokio::test]
async fn download_cancelled_before_start() {
    let server = serve(&vec![1u8; 100_000]).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let cancel = AtomicBool::new(true);

    let err = download(
        &reqwest::Client::new(),
        &format!("{}/f", server.uri()),
        &dest,
        None,
        &cancel,
        |_| {},
    )
    .await
    .unwrap_err();

    assert_eq!(err, CANCELLED);
    assert!(!dest.exists());
    assert!(!part(&dest).exists());
    assert!(cancel.load(Ordering::SeqCst));
}

#[tokio::test]
async fn download_creates_missing_parent_dirs() {
    let server = serve(b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a").join("b").join("out.bin");

    download(
        &reqwest::Client::new(),
        &format!("{}/f", server.uri()),
        &dest,
        None,
        &AtomicBool::new(false),
        |_| {},
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(&dest).unwrap(), b"abc");
}

#[tokio::test]
async fn download_failure_leaves_existing_dest_untouched() {
    let server = serve(b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    std::fs::write(&dest, "old").unwrap();
    let wrong = "f".repeat(64);

    let err = download(
        &reqwest::Client::new(),
        &format!("{}/f", server.uri()),
        &dest,
        Some(&wrong),
        &AtomicBool::new(false),
        |_| {},
    )
    .await;

    assert!(err.is_err());
    assert_eq!(std::fs::read(&dest).unwrap(), b"old");
    assert!(!part(&dest).exists());
}

#[tokio::test]
async fn client_refuses_redirect_to_http() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/target", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/target"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes("abc"))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let r = download(
        &client(),
        &format!("{}/start", server.uri()),
        &dest,
        None,
        &AtomicBool::new(false),
        |_| {},
    )
    .await;

    assert!(r.is_err(), "redirect to http must be refused");
    assert!(!dest.exists());
    assert!(!part(&dest).exists());
}

/// Drops the future mid-body (headers and 1000 bytes sent, then the server stalls) once `.part` exists,
/// so the test only passes if drop itself removes `.part`.
#[tokio::test]
async fn dropping_download_mid_transfer_removes_part() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
        sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\n")
            .await
            .unwrap();
        sock.write_all(&[1u8; 1000]).await.unwrap();
        sock.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    });
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let p = part(&dest);
    let cancel = AtomicBool::new(false);
    let url = format!("http://{addr}/f");
    let c = reqwest::Client::new();

    let dl = download(&c, &url, &dest, None, &cancel, |_| {});
    let wait_for_part = async {
        for _ in 0..200 {
            if p.exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!(".part never appeared");
    };
    tokio::select! {
        _ = dl => panic!("download should still be stalled"),
        _ = wait_for_part => {}
    }
    // the download future has been dropped here
    assert!(!dest.exists());
    assert!(
        !p.exists(),
        ".part must be removed when the future is dropped"
    );
    srv.abort();
}
