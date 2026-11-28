//! M4 updater network layer. Pinned API (module `screen_translate_core::update`):
//!   #[derive(Debug, Clone)] pub struct Update { pub version: String, pub notes: Option<String>,
//!                                               pub url: String, pub signature: String }
//!   pub async fn check(client: &reqwest::Client, endpoint: &str, current: &str, keys: &[String])
//!       -> Result<Option<Update>, String>
//!     200 + newer version + platform match => Ok(Some); same/older version, 204, or no platform key => Ok(None);
//!     non-2xx (404/500), bad JSON, bad semver => Err. `Update.version` has no leading 'v'.
//!   pub async fn download(client: &reqwest::Client, update: &Update, pubkey_b64: &str,
//!                         on_progress: impl FnMut(u64, Option<u64>)) -> Result<Vec<u8>, String>
//!     GETs update.url, reports (downloaded_so_far, content_length) as bytes arrive (final call == body len),
//!     then `verify`s against `pubkey_b64` + update.version. Non-2xx, bad signature, tampered body or signed
//!     version mismatch => Err.

use screen_translate_core::update::*;
use std::path::PathBuf;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn read(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/update")
            .join(name),
    )
    .unwrap()
}
fn text(name: &str) -> String {
    String::from_utf8(read(name)).unwrap().trim().to_string()
}
fn keys() -> Vec<String> {
    platform_keys("darwin", "aarch64", "app").to_vec()
}
fn manifest(version: &str, platform: &str, url: &str, sig: &str) -> String {
    format!(
        r#"{{"version":"{version}","notes":"n","platforms":{{"{platform}":{{"url":"{url}","signature":"{sig}"}}}}}}"#
    )
}
async fn serve_manifest(s: &MockServer, body: String) {
    Mock::given(method("GET"))
        .and(path("/latest.json"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
        .mount(s)
        .await;
}
async fn serve_payload(s: &MockServer, body: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path("/payload.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(s)
        .await;
}
fn endpoint(s: &MockServer) -> String {
    format!("{}/latest.json", s.uri())
}
/// An Update for payload.bin (signed as 9.9.9) served by `s`.
async fn update_for(s: &MockServer, version: &str) -> Update {
    serve_manifest(
        s,
        manifest(
            version,
            "darwin-aarch64-app",
            &format!("{}/payload.bin", s.uri()),
            &text("payload.bin.sig"),
        ),
    )
    .await;
    check(&reqwest::Client::new(), &endpoint(s), "0.3.1", &keys())
        .await
        .unwrap()
        .expect("update")
}

#[tokio::test]
async fn same_version_is_none() {
    let s = MockServer::start().await;
    serve_manifest(
        &s,
        manifest("0.3.1", "darwin-aarch64-app", "http://x/p", "sig"),
    )
    .await;
    assert!(
        check(&reqwest::Client::new(), &endpoint(&s), "0.3.1", &keys())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn older_version_is_none() {
    let s = MockServer::start().await;
    serve_manifest(
        &s,
        manifest("0.3.0", "darwin-aarch64-app", "http://x/p", "sig"),
    )
    .await;
    assert!(
        check(&reqwest::Client::new(), &endpoint(&s), "0.3.1", &keys())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn newer_version_found_with_platform_key_fallback() {
    let s = MockServer::start().await;
    // Only the plain `{os}-{arch}` key exists; the `-app` key must fall back to it. Leading 'v' is stripped.
    serve_manifest(
        &s,
        manifest("v0.4.0", "darwin-aarch64", "http://x/app.tar.gz", "SIG"),
    )
    .await;
    let u = check(&reqwest::Client::new(), &endpoint(&s), "0.3.1", &keys())
        .await
        .unwrap()
        .expect("update");
    assert_eq!(
        (u.version.as_str(), u.url.as_str(), u.signature.as_str()),
        ("0.4.0", "http://x/app.tar.gz", "SIG")
    );
    assert_eq!(u.notes.as_deref(), Some("n"));
}

#[tokio::test]
async fn missing_platform_is_none() {
    let s = MockServer::start().await;
    serve_manifest(&s, manifest("0.4.0", "windows-x86_64", "http://x/p", "sig")).await;
    assert!(
        check(&reqwest::Client::new(), &endpoint(&s), "0.3.1", &keys())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn no_content_is_none() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/latest.json"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&s)
        .await;
    assert!(
        check(&reqwest::Client::new(), &endpoint(&s), "0.3.1", &keys())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn http_errors_and_bad_json_are_errors() {
    let s = MockServer::start().await; // nothing mounted => 404
    assert!(
        check(&reqwest::Client::new(), &endpoint(&s), "0.3.1", &keys())
            .await
            .is_err(),
        "404"
    );
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&s)
        .await;
    assert!(
        check(&reqwest::Client::new(), &endpoint(&s), "0.3.1", &keys())
            .await
            .is_err(),
        "500"
    );
    let s = MockServer::start().await;
    serve_manifest(&s, "not json".into()).await;
    assert!(
        check(&reqwest::Client::new(), &endpoint(&s), "0.3.1", &keys())
            .await
            .is_err(),
        "bad json"
    );
    let s = MockServer::start().await;
    serve_manifest(
        &s,
        manifest("not-semver", "darwin-aarch64", "http://x/p", "sig"),
    )
    .await;
    assert!(
        check(&reqwest::Client::new(), &endpoint(&s), "0.3.1", &keys())
            .await
            .is_err(),
        "bad semver"
    );
}

#[tokio::test]
async fn download_verifies_good_payload_and_reports_progress() {
    let s = MockServer::start().await;
    let u = update_for(&s, "9.9.9").await;
    let payload = read("payload.bin");
    serve_payload(&s, payload.clone()).await;
    let mut calls: Vec<(u64, Option<u64>)> = vec![];
    let got = download(
        &reqwest::Client::new(),
        &u,
        &text("throwaway.pub"),
        |d, t| calls.push((d, t)),
    )
    .await
    .unwrap();
    assert_eq!(got, payload);
    let (last, total) = *calls.last().expect("progress called");
    assert_eq!(last, payload.len() as u64, "final bytes");
    assert_eq!(
        total,
        Some(payload.len() as u64),
        "total from content-length"
    );
    assert!(calls.windows(2).all(|w| w[0].0 <= w[1].0), "monotonic");
}

#[tokio::test]
async fn download_rejects_tampered_payload() {
    let s = MockServer::start().await;
    let u = update_for(&s, "9.9.9").await;
    let mut bad = read("payload.bin");
    bad[0] ^= 1;
    serve_payload(&s, bad).await;
    assert!(
        download(
            &reqwest::Client::new(),
            &u,
            &text("throwaway.pub"),
            |_, _| {}
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn download_rejects_wrong_key() {
    let s = MockServer::start().await;
    let u = update_for(&s, "9.9.9").await;
    serve_payload(&s, read("payload.bin")).await;
    assert!(
        download(&reqwest::Client::new(), &u, &text("other.pub"), |_, _| {})
            .await
            .is_err()
    );
}

#[tokio::test]
async fn download_rejects_signed_version_mismatch() {
    // Manifest claims 9.9.10 but the artifact signature names 9.9.9.
    let s = MockServer::start().await;
    let u = update_for(&s, "9.9.10").await;
    serve_payload(&s, read("payload.bin")).await;
    assert!(
        download(
            &reqwest::Client::new(),
            &u,
            &text("throwaway.pub"),
            |_, _| {}
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn download_http_error_is_error() {
    let s = MockServer::start().await;
    let u = update_for(&s, "9.9.9").await; // payload.bin not mounted => 404
    assert!(
        download(
            &reqwest::Client::new(),
            &u,
            &text("throwaway.pub"),
            |_, _| {}
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn check_and_download_follow_redirects() {
    let s = MockServer::start().await;
    let payload = read("payload.bin");
    Mock::given(method("GET"))
        .and(path("/latest.json"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/real.json"))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/real.json"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            manifest(
                "9.9.9",
                "darwin-aarch64-app",
                &format!("{}/payload.bin", s.uri()),
                &text("payload.bin.sig"),
            ),
            "application/json",
        ))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/payload.bin"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/blob"))
        .mount(&s)
        .await;
    serve_blob(&s, payload.clone()).await;
    let client = reqwest::Client::new();
    let u = check(&client, &endpoint(&s), "0.3.1", &keys())
        .await
        .unwrap()
        .expect("update via redirect");
    let got = download(&client, &u, &text("throwaway.pub"), |_, _| {})
        .await
        .unwrap();
    assert_eq!(got, payload);
}

async fn serve_blob(s: &MockServer, body: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path("/blob"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(s)
        .await;
}
