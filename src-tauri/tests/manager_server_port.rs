//! Own binary: binds the managed port (18080), which must not interfere with other tests.

#![cfg(unix)]

use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use screen_translate_lib::manager::server::{Server, Status, HOST, PORT};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

static LOCK: Mutex<()> = Mutex::new(());

fn script(dir: &Path, body: &str) -> std::path::PathBuf {
    let p = dir.join("srv");
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

async fn health() -> MockServer {
    let m = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&m).await;
    m
}

fn start(server: &Server, exe: &Path, health: String) -> Result<(), String> {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    server.start(exe, vec![], "m.gguf".into(), health, client, Duration::from_millis(50), Duration::from_secs(30), |_| {})
}

#[tokio::test(flavor = "multi_thread")]
async fn start_errs_without_spawning_when_port_is_taken() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let blocker = TcpListener::bind((HOST, PORT)).expect("port 18080 must be free to run this test");
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let exe = script(dir.path(), &format!("touch {}\nexec sleep 30", marker.display()));
    let h = health().await;
    let server = Server::new();

    let result = start(&server, &exe, format!("{}/health", h.uri()));
    server.stop(); // do not leak a child if start wrongly succeeded
    let err = result.expect_err("start must fail while the port is taken");

    assert!(err.contains("18080"), "{err}");
    assert!(err.contains("already in use"), "{err}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!marker.exists(), "the executable must not be spawned");
    assert!(!matches!(server.status(), Status::Loading { .. } | Status::Ready { .. }), "{:?}", server.status());
    drop(blocker);
}

#[tokio::test(flavor = "multi_thread")]
async fn start_works_once_the_port_is_free() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let exe = script(dir.path(), "exec sleep 30");
    let h = health().await;
    let server = Server::new();

    start(&server, &exe, format!("{}/health", h.uri())).unwrap();

    assert_eq!(server.status(), Status::Loading { model: "m.gguf".into() });
    server.stop();
}
