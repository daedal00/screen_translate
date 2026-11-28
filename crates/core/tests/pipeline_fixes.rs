//! M2a review-fix regressions (see .rex-team/m2a-review-report.md).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use image::RgbaImage;
use screen_translate_core::capture::{LogicalRect, PhysicalRect};
use screen_translate_core::live::OverlayLine;
use screen_translate_core::manager::ManagerState;
use screen_translate_core::ocr::TextLine;
use screen_translate_core::pipeline::{Latest, Live, RegionResult, Translator, run_live};
use screen_translate_core::settings::SettingsStore;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PATH: &str = "/v1/chat/completions";
const TICK: Duration = Duration::from_millis(1);
const BACKOFF: Duration = Duration::from_millis(1);
const RECT: LogicalRect = LogicalRect {
    x: 0.0,
    y: 0.0,
    width: 64.0,
    height: 64.0,
};

fn result(tag: &str) -> RegionResult {
    RegionResult {
        original: tag.into(),
        translation: Some(format!("t-{tag}")),
        detected_source: None,
        error: None,
        needs_settings: false,
    }
}

fn line(text: &str) -> OverlayLine {
    OverlayLine {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
        text: text.into(),
        font_px: 10.0,
    }
}

fn frame() -> RgbaImage {
    RgbaImage::from_pixel(64, 64, image::Rgba([120, 120, 120, 255]))
}

fn fixed_ocr(_: &RgbaImage) -> Result<Vec<TextLine>, String> {
    Ok(vec![TextLine {
        text: "Hello".into(),
        bbox: PhysicalRect {
            x: 4,
            y: 4,
            width: 40,
            height: 12,
        },
    }])
}

fn ok_body(reply: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": reply }, "finish_reason": "stop" }]
    }))
}

async fn local_server(reply: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PATH))
        .respond_with(ok_body(reply))
        .mount(&server)
        .await;
    server
}

fn translator(url: &str) -> (Arc<Translator>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let settings = Arc::new(SettingsStore::open_at(dir.path().join("settings.json")));
    settings
        .save(
            None,
            "DE".into(),
            "local".into(),
            url.into(),
            "m1".into(),
            "popup".into(),
        )
        .unwrap();
    (
        Arc::new(Translator::new(settings, Arc::new(ManagerState::default()))),
        dir,
    )
}

type Update = (Vec<OverlayLine>, Option<String>);

/// Plain thread with no ambient runtime (as production); `rt` is the app runtime handle.
fn spawn_live(
    translator: Arc<Translator>,
    live: Arc<Live>,
    session: u64,
    rt: tokio::runtime::Handle,
    grab: impl FnMut() -> Result<Option<RgbaImage>, String> + Send + 'static,
    on_update: impl FnMut(Vec<OverlayLine>, Option<String>) + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        run_live(
            &translator,
            &live,
            session,
            RECT,
            TICK,
            BACKOFF,
            &rt,
            grab,
            fixed_ocr,
            on_update,
        );
    })
}

fn join_within(h: std::thread::JoinHandle<()>, secs: u64) {
    let t = Instant::now();
    while !h.is_finished() {
        assert!(
            t.elapsed() < Duration::from_secs(secs),
            "run_live did not return within {secs}s"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    h.join().unwrap();
}

// ---- Finding 1: popup translation after live went idle must not hang ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn popup_translation_works_while_live_session_idles() {
    let server = local_server("Hallo").await;
    let (t, _d) = translator(&server.uri());
    let live = Arc::new(Live::default());
    let session = live.start();
    let updates: Arc<Mutex<Vec<Update>>> = Default::default();
    let u2 = updates.clone();
    let h = spawn_live(
        t.clone(),
        live.clone(),
        session,
        tokio::runtime::Handle::current(),
        || Ok(Some(frame())),
        move |l, e| u2.lock().unwrap().push((l, e)),
    );
    // Wait for the one fire to finish; the session then idles on a static frame.
    let t0 = Instant::now();
    while updates.lock().unwrap().is_empty() {
        assert!(t0.elapsed() < Duration::from_secs(5), "live never fired");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    // New text so the cache misses and a pooled connection from the live fire may be reused.
    let r = tokio::time::timeout(
        Duration::from_secs(2),
        t.translate_text("A different sentence"),
    )
    .await;
    live.stop();
    let r =
        r.expect("popup translation hung after live went idle (connection pinned to live runtime)");
    assert_eq!(r.unwrap().0.text, "Hallo");
    tokio::task::spawn_blocking(move || join_within(h, 5))
        .await
        .unwrap();
}

// ---- Finding 2: emit under the lock ----

#[test]
fn latest_finish_with_emits_once_for_current_and_stores() {
    let latest = Latest::default();
    let g = latest.begin();
    let mut calls = Vec::new();
    assert!(latest.finish_with(g, result("a"), |r| calls.push(r.clone())));
    assert_eq!(calls, vec![result("a")]);
    assert_eq!(latest.get(), Some(result("a")));
}

#[test]
fn latest_finish_with_skips_emit_for_stale_generation() {
    let latest = Latest::default();
    let old = latest.begin();
    latest.begin();
    let mut called = false;
    assert!(!latest.finish_with(old, result("old"), |_| called = true));
    assert!(!called);
    assert_eq!(latest.get(), None);
    let g = latest.begin();
    latest.invalidate();
    assert!(!latest.finish_with(g, result("late"), |_| called = true));
    assert!(!called);
}

#[test]
fn latest_invalidate_blocks_until_emit_returns() {
    let latest = Arc::new(Latest::default());
    let g = latest.begin();
    let emit_done = Arc::new(AtomicBool::new(false));
    let (started_tx, started_rx) = mpsc::channel();
    let (l2, d2) = (latest.clone(), emit_done.clone());
    let emitter = std::thread::spawn(move || {
        l2.finish_with(g, result("a"), |_| {
            started_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            d2.store(true, Ordering::SeqCst);
        })
    });
    started_rx.recv().unwrap();
    latest.invalidate();
    assert!(
        emit_done.load(Ordering::SeqCst),
        "invalidate returned while emit was still running"
    );
    assert!(emitter.join().unwrap());
    assert_eq!(latest.get(), None);
}

#[test]
fn live_show_with_runs_f_for_current_session_only() {
    let live = Live::default();
    let s1 = live.start();
    let mut seen = Vec::new();
    assert!(live.show_with(s1, vec![line("a")], |l| seen.extend_from_slice(l)));
    assert_eq!(seen, vec![line("a")]);
    assert_eq!(live.lines(), vec![line("a")]);
    let s2 = live.start();
    let mut called = false;
    assert!(!live.show_with(s1, vec![line("old")], |_| called = true));
    assert!(!called);
    live.stop();
    assert!(!live.show_with(s2, vec![line("late")], |_| called = true));
    assert!(!called);
}

#[test]
fn live_stop_blocks_until_show_with_f_returns() {
    let live = Arc::new(Live::default());
    let s = live.start();
    let done = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let (l2, d2) = (live.clone(), done.clone());
    let shower = std::thread::spawn(move || {
        l2.show_with(s, vec![line("a")], |_| {
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            d2.store(true, Ordering::SeqCst);
        })
    });
    rx.recv().unwrap();
    assert!(live.stop());
    assert!(
        done.load(Ordering::SeqCst),
        "stop returned while show_with's f was still running"
    );
    assert!(shower.join().unwrap());
}

#[test]
fn live_stop_with_runs_f_only_when_running() {
    let live = Live::default();
    let mut calls = 0;
    assert!(!live.stop_with(|| calls += 1), "not running");
    assert_eq!(calls, 0);
    let s = live.start();
    assert!(live.stop_with(|| calls += 1));
    assert_eq!(calls, 1);
    assert!(!live.running());
    assert!(!live.is_current(s));
    assert!(!live.stop_with(|| calls += 1), "second stop");
    assert_eq!(calls, 1);
}

#[test]
fn live_stop_with_blocks_concurrent_show_until_f_returns() {
    let live = Arc::new(Live::default());
    let s = live.start();
    let f_done = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let (l2, d2) = (live.clone(), f_done.clone());
    let stopper = std::thread::spawn(move || {
        l2.stop_with(|| {
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            d2.store(true, Ordering::SeqCst);
        })
    });
    rx.recv().unwrap();
    assert!(!live.show(s, vec![line("x")]));
    assert!(
        f_done.load(Ordering::SeqCst),
        "show got the lock while stop_with's f was running"
    );
    assert!(stopper.join().unwrap());
}

/// on_update must never run once stop() has returned. Updates flow every tick (a distinct grab error each time).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_live_on_update_never_runs_after_stop_returned() {
    let server = local_server("Hallo").await;
    let (t, _d) = translator(&server.uri());
    for round in 0..15 {
        let live = Arc::new(Live::default());
        let session = live.start();
        let stopped = Arc::new(AtomicBool::new(false));
        let violations = Arc::new(AtomicUsize::new(0));
        let updates = Arc::new(AtomicUsize::new(0));
        let n = Arc::new(AtomicUsize::new(0));
        let grab = move || Err(format!("err {}", n.fetch_add(1, Ordering::SeqCst)));
        let (s2, v2, u2) = (stopped.clone(), violations.clone(), updates.clone());
        let on_update = move |_: Vec<OverlayLine>, _: Option<String>| {
            u2.fetch_add(1, Ordering::SeqCst);
            if s2.load(Ordering::SeqCst) {
                v2.fetch_add(1, Ordering::SeqCst);
            }
            // Widen the window in which stop() can return mid-callback.
            std::thread::sleep(Duration::from_millis(2));
            if s2.load(Ordering::SeqCst) {
                v2.fetch_add(1, Ordering::SeqCst);
            }
        };
        let h = spawn_live(
            t.clone(),
            live.clone(),
            session,
            tokio::runtime::Handle::current(),
            grab,
            on_update,
        );
        let t0 = Instant::now();
        while updates.load(Ordering::SeqCst) < 3 {
            assert!(t0.elapsed() < Duration::from_secs(5), "no updates flowing");
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(round % 3));
        assert!(live.stop());
        stopped.store(true, Ordering::SeqCst);
        tokio::task::spawn_blocking(move || join_within(h, 5))
            .await
            .unwrap();
        assert_eq!(
            violations.load(Ordering::SeqCst),
            0,
            "on_update ran after stop() returned (round {round})"
        );
    }
}

// ---- Finding 3: translate error path, and stop() from on_update ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_live_translate_error_reported_once_then_recovers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PATH))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(PATH))
        .respond_with(ok_body("Hallo"))
        .mount(&server)
        .await;
    let (t, _d) = translator(&server.uri());
    let live = Arc::new(Live::default());
    let session = live.start();
    let updates: Arc<Mutex<Vec<Update>>> = Default::default();
    let u2 = updates.clone();
    let (l2, frames) = (live.clone(), Arc::new(AtomicUsize::new(0)));
    let grab = move || {
        if frames.fetch_add(1, Ordering::SeqCst) >= 120 {
            l2.stop();
            return Ok(None);
        }
        Ok(Some(frame()))
    };
    let h = spawn_live(
        t,
        live,
        session,
        tokio::runtime::Handle::current(),
        grab,
        move |l, e| u2.lock().unwrap().push((l, e)),
    );
    tokio::task::spawn_blocking(move || join_within(h, 10))
        .await
        .unwrap();

    let ups = updates.lock().unwrap();
    let errs: Vec<_> = ups
        .iter()
        .enumerate()
        .filter(|(_, (_, e))| e.is_some())
        .collect();
    assert_eq!(errs.len(), 1, "exactly one error update: {ups:?}");
    assert!(errs[0].1.0.is_empty(), "the error update carries no lines");
    let ok = ups
        .iter()
        .enumerate()
        .find(|(_, (l, e))| e.is_none() && l.first().is_some_and(|l| l.text == "Hallo"));
    let (ok_idx, _) = ok.unwrap_or_else(|| panic!("no later successful update: {ups:?}"));
    assert!(ok_idx > errs[0].0, "success comes after the error: {ups:?}");
}
