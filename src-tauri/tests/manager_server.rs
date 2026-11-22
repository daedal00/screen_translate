use std::path::Path;

use screen_translate_lib::manager::runtime::Backend;
use screen_translate_lib::manager::server::{
    base_url, gpu_args, next, parse_offload_line, server_args, should_fallback_to_vulkan, Event, Exit, LogTail, Offload, Status,
};
use serde_json::json;

#[test]
fn base_url_is_managed_port() {
    assert_eq!(base_url(), "http://127.0.0.1:18080");
}

#[test]
fn server_args_exact() {
    let got: Vec<String> = server_args(Path::new("/m/x.gguf"), &[]).iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(got, vec!["-m", "/m/x.gguf", "--host", "127.0.0.1", "--port", "18080", "-c", "4096", "-lv", "4"]);
}

#[test]
fn server_args_appends_gpu_args_last() {
    let gpu = vec!["-ngl".to_string(), "99".to_string()];
    let got: Vec<String> = server_args(Path::new("/m/x.gguf"), &gpu).iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(&got[got.len() - 4..], ["-lv", "4", "-ngl", "99"]);
}

fn loading() -> Status {
    Status::Loading { model: "m".into() }
}
fn ready() -> Status {
    Status::Ready { model: "m".into(), offload: None }
}
fn err() -> Status {
    Status::Error { message: "boom".into(), log: vec!["l".into()] }
}
fn log() -> Vec<String> {
    vec!["a".to_string(), "b".to_string()]
}

#[test]
fn next_started_goes_loading_from_any_state() {
    for from in [Status::Stopped, loading(), ready(), err()] {
        assert_eq!(next(&from, Event::Started { model: "new".into() }), Status::Loading { model: "new".into() });
    }
}

#[test]
fn next_health_200_in_loading_is_ready() {
    assert_eq!(next(&loading(), Event::Health(Some(200))), ready());
}

#[test]
fn next_other_health_results_leave_loading_unchanged() {
    for h in [Some(503), Some(404), Some(500), None] {
        assert_eq!(next(&loading(), Event::Health(h)), loading(), "{h:?}");
    }
}

#[test]
fn next_health_ignored_in_stopped_and_error() {
    for h in [Some(200), Some(503), None] {
        assert_eq!(next(&Status::Stopped, Event::Health(h)), Status::Stopped);
        assert_eq!(next(&err(), Event::Health(h)), err());
    }
}

#[test]
fn next_exited_with_code() {
    for from in [loading(), ready()] {
        assert_eq!(
            next(&from, Event::Exited { code: Some(3), log: log() }),
            Status::Error { message: "llama-server exited (code 3)".into(), log: log() }
        );
    }
}

#[test]
fn next_exited_without_code() {
    for from in [loading(), ready()] {
        assert_eq!(
            next(&from, Event::Exited { code: None, log: log() }),
            Status::Error { message: "llama-server exited".into(), log: log() }
        );
    }
}

#[test]
fn next_exited_ignored_in_stopped_and_error() {
    assert_eq!(next(&Status::Stopped, Event::Exited { code: Some(1), log: log() }), Status::Stopped);
    assert_eq!(next(&err(), Event::Exited { code: Some(1), log: log() }), err());
}

#[test]
fn next_timed_out_in_loading_is_error_with_log() {
    assert_eq!(
        next(&loading(), Event::TimedOut { log: log() }),
        Status::Error { message: "llama-server did not become ready in time".into(), log: log() }
    );
}

#[test]
fn next_timed_out_ignored_elsewhere() {
    for from in [ready(), Status::Stopped, err()] {
        assert_eq!(next(&from, Event::TimedOut { log: log() }), from);
    }
}

#[test]
fn next_stopped_from_any_state() {
    for from in [Status::Stopped, loading(), ready(), err()] {
        assert_eq!(next(&from, Event::Stopped), Status::Stopped);
    }
}

#[test]
fn status_serde_shapes() {
    assert_eq!(serde_json::to_value(Status::Stopped).unwrap(), json!({ "state": "stopped" }));
    assert_eq!(serde_json::to_value(Status::Ready { model: "m".into(), offload: None }).unwrap(), json!({ "state": "ready", "model": "m", "offload": null }));
    assert_eq!(serde_json::to_value(loading()).unwrap(), json!({ "state": "loading", "model": "m" }));
    assert_eq!(
        serde_json::to_value(err()).unwrap(),
        json!({ "state": "error", "message": "boom", "log": ["l"] })
    );
}

#[test]
fn log_tail_keeps_last_cap_lines_in_order() {
    let mut t = LogTail::new(3);
    for i in 1..=5 {
        t.push(&format!("line{i}"));
    }
    assert_eq!(t.lines(), vec!["line3", "line4", "line5"]);
}

#[test]
fn log_tail_trims_and_skips_empty() {
    let mut t = LogTail::new(10);
    t.push("hello \r");
    t.push("");
    t.push("   \r\n");
    t.push("  indented\t");
    assert_eq!(t.lines(), vec!["hello", "  indented"]);
}

#[test]
fn log_tail_truncates_long_lines_to_1000_chars() {
    let mut t = LogTail::new(10);
    t.push(&"a".repeat(2000));
    t.push(&"b".repeat(1000));
    t.push("short");
    let lines = t.lines();
    assert_eq!(lines[0].chars().count(), 1000);
    assert_eq!(lines[1], "b".repeat(1000), "a line of exactly 1000 chars is untouched");
    assert_eq!(lines[2], "short");
}

#[test]
fn log_tail_truncation_is_char_boundary_safe() {
    let mut t = LogTail::new(10);
    // 2-byte, 3-byte and 4-byte chars; a byte-offset cut at 1000 would land inside one of them.
    for ch in ['é', '世', '🦀'] {
        t.push(&ch.to_string().repeat(2000));
    }
    for line in t.lines() {
        let n = line.chars().count();
        assert!((1..=1000).contains(&n), "{n} chars");
        assert!(line.len() < 2000, "was not truncated");
    }
}

#[cfg(unix)]
mod process {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use screen_translate_lib::manager::server::{Exit, Server, Status};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// `start` probes the fixed managed port, so process tests must not overlap.
    static LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    const POLL: Duration = Duration::from_millis(50);
    const LONG: Duration = Duration::from_secs(30);

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    type Changes = Arc<Mutex<Vec<Status>>>;

    #[allow(clippy::too_many_arguments)]
    fn start(server: &Server, exe: &Path, health: String, load_timeout: Duration) -> (Result<(), String>, Changes) {
        let changes: Changes = Arc::default();
        let sink = changes.clone();
        let r = server.start(exe, vec![], "m.gguf".into(), health, client(), POLL, load_timeout, move |s| {
            sink.lock().unwrap().push(s)
        });
        (r, changes)
    }

    async fn health_mock(statuses_503: u64) -> MockServer {
        let m = MockServer::start().await;
        if statuses_503 > 0 {
            Mock::given(method("GET"))
                .and(path("/health"))
                .respond_with(ResponseTemplate::new(503))
                .up_to_n_times(statuses_503)
                .mount(&m)
                .await;
        }
        Mock::given(method("GET")).and(path("/health")).respond_with(ResponseTemplate::new(200)).mount(&m).await;
        m
    }

    async fn always_503() -> MockServer {
        let m = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&m).await;
        m
    }

    async fn wait_for(server: &Server, what: &str, pred: impl Fn(&Status) -> bool) -> Status {
        wait_within(server, what, Duration::from_secs(5), pred).await
    }

    async fn wait_within(server: &Server, what: &str, limit: Duration, pred: impl Fn(&Status) -> bool) -> Status {
        let deadline = Instant::now() + limit;
        loop {
            let s = server.status();
            if pred(&s) {
                return s;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; last status {s:?}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn pid_alive(pid: &str) -> bool {
        std::process::Command::new("/bin/sh")
            .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
            .status()
            .unwrap()
            .success()
    }

    async fn read_pid(file: &Path) -> String {
        for _ in 0..100 {
            if let Ok(s) = std::fs::read_to_string(file) {
                if !s.trim().is_empty() {
                    return s.trim().to_string();
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("pid file never written");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ready_carries_offload_parsed_from_child_output() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let exe = script(
            dir.path(),
            "srv",
            "echo '0.00.1 I llama_prepare_model_devices: using device CUDA0 (NVIDIA GeForce RTX 3070) (0000:01:00.0) - 7100 MiB free'\n\
             echo '0.01.0 I load_tensors: offloaded 20/33 layers to GPU' >&2\nexec sleep 30",
        );
        let health = health_mock(2).await;
        let server = Server::new();
        let (r, _) = start(&server, &exe, format!("{}/health", health.uri()), LONG);
        r.unwrap();
        let s = wait_for(&server, "ready", |s| matches!(s, Status::Ready { .. })).await;
        let Status::Ready { offload, .. } = s else { unreachable!() };
        assert_eq!(offload, super::off(Some("NVIDIA GeForce RTX 3070"), 20, 33));
        server.stop();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exit_while_loading_records_exit_evidence() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let exe = script(dir.path(), "srv", "echo '0.00.1 I llama_prepare_model_devices: using device CUDA0 (RTX) (x) - 1 MiB free'\nexit 7");
        let health = always_503().await;
        let server = Server::new();
        let (r, _) = start(&server, &exe, format!("{}/health", health.uri()), LONG);
        r.unwrap();
        wait_for(&server, "error", |s| matches!(s, Status::Error { .. })).await;
        assert_eq!(server.last_exit(), Some(Exit { code: Some(7), device_seen: true }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exit_with_code_becomes_error_with_log() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let exe = script(dir.path(), "srv", "echo first-line >&2\necho second-line >&2\nexit 3");
        let health = always_503().await;
        let server = Server::new();
        let (r, _) = start(&server, &exe, format!("{}/health", health.uri()), LONG);
        r.unwrap();

        let s = wait_for(&server, "error", |s| matches!(s, Status::Error { .. })).await;
        let Status::Error { message, log } = s else { unreachable!() };
        assert!(message.contains("code 3"), "{message}");
        assert!(log.iter().any(|l| l == "first-line"), "{log:?}");
        assert!(log.iter().any(|l| l == "second-line"), "{log:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn loading_then_ready_then_stop_kills_child() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let exe = script(dir.path(), "srv", &format!("echo $$ > {}\nexec sleep 30", pidfile.display()));
        let health = health_mock(2).await;
        let server = Server::new();
        let (r, changes) = start(&server, &exe, format!("{}/health", health.uri()), LONG);
        r.unwrap();
        assert_eq!(server.status(), Status::Loading { model: "m.gguf".into() });

        wait_for(&server, "ready", |s| matches!(s, Status::Ready { .. })).await;
        assert_eq!(server.status(), Status::Ready { model: "m.gguf".into(), offload: None });
        {
            let seen = changes.lock().unwrap();
            let li = seen.iter().position(|s| matches!(s, Status::Loading { .. })).expect("Loading notified");
            let ri = seen.iter().position(|s| matches!(s, Status::Ready { .. })).expect("Ready notified");
            assert!(li < ri, "{seen:?}");
        }

        let pid = read_pid(&pidfile).await;
        assert!(pid_alive(&pid));
        let t = Instant::now();
        server.stop();
        assert!(t.elapsed() < Duration::from_secs(3), "stop took {:?}", t.elapsed());
        assert_eq!(server.status(), Status::Stopped);
        assert!(!pid_alive(&pid), "child still alive after stop");
        assert_eq!(changes.lock().unwrap().last(), Some(&Status::Stopped));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn start_while_running_errs_and_restart_after_stop_works() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let exe = script(dir.path(), "srv", "exec sleep 30");
        let health = always_503().await;
        let url = format!("{}/health", health.uri());
        let server = Server::new();

        start(&server, &exe, url.clone(), LONG).0.unwrap();
        let (second, _) = start(&server, &exe, url.clone(), LONG);
        assert!(second.is_err());
        assert_eq!(server.status(), Status::Loading { model: "m.gguf".into() });

        server.stop();
        assert_eq!(server.status(), Status::Stopped);
        start(&server, &exe, url, LONG).0.unwrap();
        assert_eq!(server.status(), Status::Loading { model: "m.gguf".into() });
        server.stop();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn load_timeout_becomes_error() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let exe = script(dir.path(), "srv", "exec sleep 30");
        let health = always_503().await;
        let server = Server::new();
        start(&server, &exe, format!("{}/health", health.uri()), Duration::from_millis(300)).0.unwrap();

        let s = wait_for(&server, "timeout error", |s| matches!(s, Status::Error { .. })).await;
        let Status::Error { message, .. } = s else { unreachable!() };
        assert!(message.contains("did not become ready"), "{message}");
        server.stop();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nonexistent_exe_errs_and_sets_error_status() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let server = Server::new();
        let (r, _) = start(&server, &dir.path().join("missing"), "http://127.0.0.1:1/health".into(), LONG);
        assert!(r.is_err());
        assert!(matches!(server.status(), Status::Error { .. }), "{:?}", server.status());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stop_with_nothing_running_is_noop() {
        let _guard = lock();
        let server = Server::new();
        server.stop();
        assert_eq!(server.status(), Status::Stopped);
        server.stop();
        assert_eq!(server.status(), Status::Stopped);
    }

    fn kill_pid(pid: &str) {
        let _ = std::process::Command::new("/bin/sh").args(["-c", &format!("kill {pid} 2>/dev/null")]).status();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn last_notification_matches_status_after_stop_racing_ready() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let exe = script(dir.path(), "srv", "exec sleep 30");
        let health = health_mock(0).await;
        let url = format!("{}/health", health.uri());
        let server = Arc::new(Server::new());

        for i in 0..30u64 {
            let (r, changes) = start(&server, &exe, url.clone(), LONG);
            r.unwrap();
            let s2 = server.clone();
            let stopper = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(i % 9 * 6));
                s2.stop();
            });
            stopper.join().unwrap();
            tokio::time::sleep(POLL + Duration::from_millis(30)).await;

            let last = changes.lock().unwrap().last().cloned();
            assert_eq!(server.status(), Status::Stopped, "iteration {i}");
            assert_eq!(last, Some(Status::Stopped), "iteration {i}: last notification must equal status()");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_notification_after_stop_during_loading() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let exe = script(dir.path(), "srv", "exec sleep 30");
        let health = always_503().await;
        let server = Server::new();
        let (r, changes) = start(&server, &exe, format!("{}/health", health.uri()), LONG);
        r.unwrap();
        tokio::time::sleep(POLL * 2).await;

        server.stop();
        let count = changes.lock().unwrap().len();
        tokio::time::sleep(POLL * 6).await;

        let seen = changes.lock().unwrap();
        assert_eq!(seen.len(), count, "late notifications: {:?}", &seen[count..]);
        assert_eq!(seen.last(), Some(&Status::Stopped));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn load_timeout_kills_the_child() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let exe = script(dir.path(), "srv", &format!("echo $$ > {}\nexec sleep 30", pidfile.display()));
        let health = always_503().await;
        let server = Server::new();
        start(&server, &exe, format!("{}/health", health.uri()), Duration::from_millis(300)).0.unwrap();
        let pid = read_pid(&pidfile).await;

        wait_for(&server, "timeout error", |s| matches!(s, Status::Error { .. })).await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        let alive = pid_alive(&pid);
        if alive {
            kill_pid(&pid);
        }
        assert!(!alive, "child must be gone after the load timeout (no stop() called)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exit_is_reported_promptly_even_if_a_grandchild_holds_the_pipes() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("bg.pid");
        let exe = script(
            dir.path(),
            "srv",
            &format!("sleep 30 &\necho $! > {}\necho hi >&2\nexit 4", pidfile.display()),
        );
        let health = always_503().await;
        let server = Server::new();
        start(&server, &exe, format!("{}/health", health.uri()), LONG).0.unwrap();

        let res = tokio::time::timeout(Duration::from_secs(1), async {
            wait_within(&server, "error", Duration::from_secs(2), |s| matches!(s, Status::Error { .. })).await
        })
        .await;
        if let Ok(pid) = std::fs::read_to_string(&pidfile) {
            kill_pid(pid.trim());
        }
        let s = res.expect("Error not reported within ~2s while a grandchild holds stdout/stderr");
        let Status::Error { message, log } = s else { unreachable!() };
        assert!(message.contains("code 4"), "{message}");
        assert!(log.iter().any(|l| l == "hi"), "{log:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn child_dying_after_ready_becomes_error() {
        let _guard = lock();
        let dir = tempfile::tempdir().unwrap();
        let exe = script(dir.path(), "srv", "sleep 0.5\nexit 2");
        let health = health_mock(0).await;
        let server = Server::new();
        let (r, changes) = start(&server, &exe, format!("{}/health", health.uri()), LONG);
        r.unwrap();

        let s = wait_for(&server, "error", |s| matches!(s, Status::Error { .. })).await;
        let Status::Error { message, .. } = s else { unreachable!() };
        assert!(message.contains("code 2"), "{message}");
        let seen = changes.lock().unwrap();
        assert!(seen.iter().any(|s| matches!(s, Status::Ready { .. })), "should have been Ready first: {seen:?}");
    }
}

// ---- gpu_args -----------------------------------------------------------------------------

fn gpu(os: &str, b: Option<Backend>) -> Vec<String> {
    gpu_args(os, b)
}

#[test]
fn gpu_args_never_force_offload_layers() {
    // The bundled build's `-ngl auto --fit on` measures free device memory; only a recorded CPU install overrides it.
    for os in ["macos", "windows"] {
        for b in [None, Some(Backend::Vulkan)] {
            assert!(gpu(os, b).is_empty(), "{os} {b:?}");
        }
    }
    assert!(gpu("macos", Some(Backend::Cpu)).is_empty());
}

#[test]
fn gpu_args_windows_cpu_disables_offload() {
    assert_eq!(gpu("windows", Some(Backend::Cpu)), vec!["-ngl", "0"]);
}

#[test]
fn gpu_args_cuda_adds_flash_attention_only() {
    assert_eq!(gpu("windows", Some(Backend::Cuda)), vec!["-fa", "on"]);
}

// ---- offload log parsing ------------------------------------------------------------------

fn feed(lines: &[&str]) -> Option<Offload> {
    let mut acc = None;
    for l in lines {
        parse_offload_line(l, &mut acc);
    }
    acc
}

fn off(device: Option<&str>, layers: u32, total: u32) -> Option<Offload> {
    Some(Offload { device: device.map(String::from), layers, total })
}

#[test]
fn offload_device_metal() {
    let got = feed(&["0.00.198.046 I llama_prepare_model_devices: using device MTL0 (Apple M1 Pro) (unknown id) - 12123 MiB free"]);
    assert_eq!(got.and_then(|o| o.device), Some("Apple M1 Pro".to_string()));
}

#[test]
fn offload_device_cuda_and_vulkan_without_timestamp() {
    let c = feed(&["llama_prepare_model_devices: using device CUDA0 (NVIDIA GeForce RTX 3070) (0000:01:00.0) - 7100 MiB free"]);
    assert_eq!(c.and_then(|o| o.device), Some("NVIDIA GeForce RTX 3070".to_string()));
    let v = feed(&["0.00.1 I llama_prepare_model_devices: using device Vulkan0 (AMD Radeon RX 6800 XT) (0000:03:00.0) - 16000 MiB free"]);
    assert_eq!(v.and_then(|o| o.device), Some("AMD Radeon RX 6800 XT".to_string()));
}

#[test]
fn offload_layers_creates_acc() {
    assert_eq!(feed(&["0.01.020.998 I load_tensors: offloaded 33/33 layers to GPU"]), off(None, 33, 33));
    assert_eq!(feed(&["load_tensors: offloaded 20/33 layers to GPU"]), off(None, 20, 33));
}

#[test]
fn offload_device_then_layers_combine() {
    let got = feed(&[
        "0.00.1 I llama_prepare_model_devices: using device CUDA0 (NVIDIA GeForce RTX 3070) (0000:01:00.0) - 7100 MiB free",
        "0.01.0 I load_tensors: offloaded 20/33 layers to GPU",
    ]);
    assert_eq!(got, off(Some("NVIDIA GeForce RTX 3070"), 20, 33));
}

#[test]
fn offload_unrelated_lines_leave_acc_unchanged() {
    assert_eq!(feed(&["srv  llama_server: initializing ...", "", "using device", "offloaded x/y layers"]), None);
    let mut acc = off(Some("X"), 1, 2);
    parse_offload_line("0.0.1 I main: server is listening", &mut acc);
    assert_eq!(acc, off(Some("X"), 1, 2));
}

#[test]
fn offload_real_metal_log() {
    let log = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/llama-server-metal-lv4.log")).unwrap();
    let mut acc = None;
    for l in log.lines() {
        parse_offload_line(l, &mut acc);
    }
    assert_eq!(acc, off(Some("Apple M1 Pro"), 33, 33));
}

// ---- vulkan fallback ----------------------------------------------------------------------

fn exit(code: Option<i32>, device_seen: bool) -> Exit {
    Exit { code, device_seen }
}

#[test]
fn fallback_when_cuda_never_came_up() {
    assert!(should_fallback_to_vulkan("windows", Backend::Cuda, &loading(), Some(&exit(Some(1), false))));
    assert!(should_fallback_to_vulkan("windows", Backend::Cuda, &loading(), Some(&exit(None, false))));
}

#[test]
fn fallback_on_missing_dll_even_if_a_device_was_seen() {
    assert!(should_fallback_to_vulkan("windows", Backend::Cuda, &loading(), Some(&exit(Some(-1073741515), true))));
}

#[test]
fn no_fallback_when_cuda_worked_and_something_else_failed() {
    // A bad GGUF or out-of-memory after the CUDA device was found must not swap the backend.
    assert!(!should_fallback_to_vulkan("windows", Backend::Cuda, &loading(), Some(&exit(Some(1), true))));
}

#[test]
fn no_fallback_elsewhere() {
    let e = exit(Some(1), false);
    assert!(!should_fallback_to_vulkan("macos", Backend::Cuda, &loading(), Some(&e)));
    for b in [Backend::Cpu, Backend::Vulkan] {
        assert!(!should_fallback_to_vulkan("windows", b, &loading(), Some(&e)));
    }
    for st in [ready(), Status::Stopped, err()] {
        assert!(!should_fallback_to_vulkan("windows", Backend::Cuda, &st, Some(&e)));
    }
    // no exit (timeout, stop): nothing to fall back from
    assert!(!should_fallback_to_vulkan("windows", Backend::Cuda, &loading(), None));
}

// ---- log tail after Ready -----------------------------------------------------------------

#[test]
fn log_tail_after_ready_keeps_only_non_info_lines_but_still_parses_offload() {
    let mut t = LogTail::new(20);
    t.push("0.00.1 I load_tensors: offloaded 33/33 layers to GPU");
    t.mark_ready();
    t.push("0.05.1 I slot launch_slot_: id  0 | task 0 | processing task");
    t.push("0.05.2 I srv  log_server_r: done request: POST /v1/chat/completions 127.0.0.1 200");
    t.push("0.05.3 W srv  something odd");
    t.push("0.05.4 E ggml_cuda: out of memory");
    t.push("plain untagged line");
    assert_eq!(
        t.lines(),
        vec![
            "0.00.1 I load_tensors: offloaded 33/33 layers to GPU",
            "0.05.3 W srv  something odd",
            "0.05.4 E ggml_cuda: out of memory",
            "plain untagged line",
        ]
    );
    t.push("0.06.0 I load_tensors: offloaded 1/2 layers to GPU");
    assert_eq!(t.offload(), off(None, 1, 2));
}

