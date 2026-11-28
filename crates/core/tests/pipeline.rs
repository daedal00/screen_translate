#![allow(clippy::await_holding_lock)]

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
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

// ---- RegionResult ----

#[test]
fn failed_result_carries_error_only() {
    let r = RegionResult::failed("orig".into(), "boom");
    assert_eq!(r.original, "orig");
    assert_eq!(r.error.as_deref(), Some("boom"));
    assert_eq!(r.translation, None);
    assert_eq!(r.detected_source, None);
    assert!(!r.needs_settings);
}

#[test]
fn no_key_text_unchanged() {
    assert_eq!(
        screen_translate_core::pipeline::NO_KEY,
        "Add your DeepL API key in Settings"
    );
}

// ---- Latest ----

#[test]
fn latest_current_generation_is_stored() {
    let latest = Latest::default();
    assert_eq!(latest.get(), None);
    let g = latest.begin();
    assert!(latest.is_current(g));
    assert!(latest.finish(g, result("a")));
    assert_eq!(latest.get(), Some(result("a")));
}

#[test]
fn latest_stale_generation_is_dropped() {
    let latest = Latest::default();
    let old = latest.begin();
    let new = latest.begin();
    assert!(!latest.is_current(old));
    assert!(!latest.finish(old, result("old")));
    assert_eq!(latest.get(), None);
    assert!(latest.finish(new, result("new")));
    assert_eq!(latest.get(), Some(result("new")));
}

#[test]
fn latest_begin_clears_previous_result() {
    let latest = Latest::default();
    let g = latest.begin();
    latest.finish(g, result("a"));
    latest.begin();
    assert_eq!(latest.get(), None);
}

#[test]
fn latest_invalidate_drops_late_finish_and_clears() {
    let latest = Latest::default();
    let g = latest.begin();
    assert!(latest.finish(g, result("a")));
    latest.invalidate();
    assert_eq!(latest.get(), None);
    let g2 = latest.begin();
    latest.invalidate();
    assert!(!latest.is_current(g2));
    assert!(!latest.finish(g2, result("late")));
    assert_eq!(latest.get(), None);
}

// ---- Live ----

#[test]
fn live_start_stop_running() {
    let live = Live::default();
    assert!(!live.running());
    assert!(!live.stop(), "stop when not running reports false");
    let s = live.start();
    assert!(live.running());
    assert!(live.is_current(s));
    assert!(live.stop());
    assert!(!live.running());
    assert!(!live.is_current(s));
    assert!(!live.stop(), "second stop reports false");
}

#[test]
fn live_show_only_for_current_session() {
    let live = Live::default();
    let s1 = live.start();
    assert!(live.show(s1, vec![line("a")]));
    assert_eq!(live.lines(), vec![line("a")]);
    let s2 = live.start();
    assert_ne!(s1, s2);
    assert!(!live.show(s1, vec![line("old")]), "old session rejected");
    assert!(live.show(s2, vec![line("b")]));
    assert_eq!(live.lines(), vec![line("b")]);
}

#[test]
fn live_show_rejected_after_stop() {
    let live = Live::default();
    let s = live.start();
    live.stop();
    assert!(!live.show(s, vec![line("late")]));
    assert!(live.lines().is_empty());
}

#[test]
fn live_lines_cleared_on_start_and_stop() {
    let live = Live::default();
    let s = live.start();
    live.show(s, vec![line("a")]);
    assert!(live.stop());
    assert!(live.lines().is_empty());
    let s = live.start();
    live.show(s, vec![line("b")]);
    live.start();
    assert!(
        live.lines().is_empty(),
        "start clears the previous session's lines"
    );
}

// ---- Translator (local provider against wiremock) ----

fn translator(url: &str, model: &str) -> (Translator, Arc<SettingsStore>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let settings = Arc::new(SettingsStore::open_at(dir.path().join("settings.json")));
    settings
        .save(
            None,
            "DE".into(),
            "local".into(),
            url.into(),
            model.into(),
            "popup".into(),
        )
        .unwrap();
    let t = Translator::new(settings.clone(), Arc::new(ManagerState::default()));
    (t, settings, dir)
}

async fn local_server(reply: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": reply }, "finish_reason": "stop" }]
        })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn translate_text_caches_second_call() {
    let server = local_server("Hallo").await;
    let (t, _s, _d) = translator(&server.uri(), "m1");

    let (tr, hit) = t.translate_text("Hello").await.unwrap();
    assert_eq!(tr.text, "Hallo");
    assert!(!hit);
    let (tr, hit) = t.translate_text("Hello").await.unwrap();
    assert_eq!(tr.text, "Hallo");
    assert!(hit, "second call is served from the cache");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn translate_text_model_change_misses_cache() {
    let server = local_server("Hallo").await;
    let (t, settings, _d) = translator(&server.uri(), "m1");

    t.translate_text("Hello").await.unwrap();
    settings
        .save(
            None,
            "DE".into(),
            "local".into(),
            server.uri(),
            "m2".into(),
            "popup".into(),
        )
        .unwrap();
    let (_, hit) = t.translate_text("Hello").await.unwrap();
    assert!(
        !hit,
        "a different local model must not reuse the cached result"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn translate_text_target_change_misses_cache() {
    let server = local_server("Bonjour").await;
    let (t, settings, _d) = translator(&server.uri(), "m1");

    t.translate_text("Hello").await.unwrap();
    settings
        .save(
            None,
            "FR".into(),
            "local".into(),
            server.uri(),
            "m1".into(),
            "popup".into(),
        )
        .unwrap();
    let (_, hit) = t.translate_text("Hello").await.unwrap();
    assert!(!hit);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn translate_text_server_error_is_err_and_not_cached() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PATH))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let (t, _s, _d) = translator(&server.uri(), "m1");
    assert!(t.translate_text("Hello").await.is_err());
    assert!(t.translate_text("Hello").await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

/// OCR runs for real (Vision), so this is macOS only.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn translate_crop_blank_image_reports_no_text() {
    let server = local_server("unused").await;
    let (t, _s, _d) = translator(&server.uri(), "m1");
    let img = RgbaImage::from_pixel(200, 80, image::Rgba([255, 255, 255, 255]));
    let r = t.translate_crop(img).await;
    assert_eq!(r.error.as_deref(), Some("No text found"));
    assert_eq!(r.translation, None);
    assert!(!r.needs_settings);
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "no text means no translation request"
    );
}

// ---- run_live ----

const TICK: Duration = Duration::from_millis(1);
const BACKOFF: Duration = Duration::from_millis(1);
const RECT: LogicalRect = LogicalRect {
    x: 0.0,
    y: 0.0,
    width: 64.0,
    height: 64.0,
};

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

type Updates = Arc<Mutex<Vec<(Vec<OverlayLine>, Option<String>)>>>;

/// Runs `run_live` on a plain thread with no ambient tokio runtime (as production does), so it must own its async bridge.
fn spawn_live(
    translator: Arc<Translator>,
    live: Arc<Live>,
    session: u64,
    grab: impl FnMut() -> Result<Option<RgbaImage>, String> + Send + 'static,
    updates: Updates,
) -> std::thread::JoinHandle<()> {
    let rt = tokio::runtime::Handle::current();
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
            |lines, err| {
                updates.lock().unwrap().push((lines, err));
            },
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_live_static_frame_translates_exactly_once() {
    let server = local_server("Hallo").await;
    let (t, _s, _d) = translator(&server.uri(), "m1");
    let live = Arc::new(Live::default());
    let session = live.start();
    let updates: Updates = Default::default();
    let frames = Arc::new(AtomicUsize::new(0));
    let (l2, f2) = (live.clone(), frames.clone());
    let grab = move || {
        // Plenty of identical frames past the settle window, then end the session.
        if f2.fetch_add(1, Ordering::SeqCst) >= 40 {
            l2.stop();
            return Ok(None);
        }
        Ok(Some(frame()))
    };
    let h = spawn_live(Arc::new(t), live.clone(), session, grab, updates.clone());
    tokio::task::spawn_blocking(move || join_within(h, 10))
        .await
        .unwrap();

    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "one line, one request, fired once"
    );
    let ups = updates.lock().unwrap();
    let with_lines: Vec<_> = ups
        .iter()
        .filter(|(l, e)| !l.is_empty() && e.is_none())
        .collect();
    assert_eq!(with_lines.len(), 1, "one successful update: {ups:?}");
    assert_eq!(with_lines[0].0[0].text, "Hallo");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_live_grab_error_reported_once() {
    let server = local_server("Hallo").await;
    let (t, _s, _d) = translator(&server.uri(), "m1");
    let live = Arc::new(Live::default());
    let session = live.start();
    let updates: Updates = Default::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let (l2, c2) = (live.clone(), calls.clone());
    let grab = move || {
        if c2.fetch_add(1, Ordering::SeqCst) >= 8 {
            l2.stop();
            return Ok(None);
        }
        Err("capture denied".to_string())
    };
    let h = spawn_live(Arc::new(t), live.clone(), session, grab, updates.clone());
    tokio::task::spawn_blocking(move || join_within(h, 10))
        .await
        .unwrap();

    let ups = updates.lock().unwrap();
    let errs: Vec<_> = ups
        .iter()
        .filter(|(_, e)| e.as_deref() == Some("capture denied"))
        .collect();
    assert_eq!(
        errs.len(),
        1,
        "repeated identical grab errors are reported once: {ups:?}"
    );
    assert!(errs[0].0.is_empty(), "an error update carries no lines");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_live_stops_promptly_when_stopped_from_another_thread() {
    let server = local_server("Hallo").await;
    let (t, _s, _d) = translator(&server.uri(), "m1");
    let live = Arc::new(Live::default());
    let session = live.start();
    let updates: Updates = Default::default();
    // Endless identical frames: only stop() can end the loop.
    let h = spawn_live(
        Arc::new(t),
        live.clone(),
        session,
        || Ok(Some(frame())),
        updates,
    );
    std::thread::sleep(Duration::from_millis(100));
    let stopped = Instant::now();
    assert!(live.stop());
    tokio::task::spawn_blocking(move || join_within(h, 5))
        .await
        .unwrap();
    assert!(
        stopped.elapsed() < Duration::from_secs(2),
        "returned {:?} after stop",
        stopped.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_live_returns_immediately_for_stale_session() {
    let server = local_server("Hallo").await;
    let (t, _s, _d) = translator(&server.uri(), "m1");
    let live = Arc::new(Live::default());
    let old = live.start();
    live.start();
    let updates: Updates = Default::default();
    let h = spawn_live(
        Arc::new(t),
        live.clone(),
        old,
        || Ok(Some(frame())),
        updates.clone(),
    );
    tokio::task::spawn_blocking(move || join_within(h, 5))
        .await
        .unwrap();
    assert!(updates.lock().unwrap().is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
}
