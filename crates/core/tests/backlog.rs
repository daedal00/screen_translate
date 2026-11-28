//! Milestone 6 backlog (core part): items 2, 4, 5, 6, 7, 8. See .rex-team/tester.report.md for the pinned APIs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use image::RgbaImage;
use screen_translate_core::capture::{LogicalRect, PhysicalRect};
use screen_translate_core::live::{self, OverlayLine};
use screen_translate_core::manager::ManagerState;
use screen_translate_core::ocr::TextLine;
use screen_translate_core::pipeline::{Live, NO_KEY, Translator, run_live};
use screen_translate_core::settings::SettingsStore;
use screen_translate_core::translate::deepl::DeepL;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn tl(x: u32, y: u32, w: u32, h: u32) -> TextLine {
    TextLine {
        text: "x".into(),
        bbox: PhysicalRect {
            x,
            y,
            width: w,
            height: h,
        },
    }
}

fn sorted(mut blocks: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
    blocks.sort();
    blocks
}

// ---- item 4: group_blocks ----

#[test]
fn group_two_line_subtitle_is_one_block() {
    let lines = [tl(100, 500, 300, 30), tl(120, 535, 260, 30)];
    assert_eq!(live::group_blocks(&lines), vec![vec![0, 1]]);
}

#[test]
fn group_three_line_paragraph_is_one_block_in_reading_order() {
    // Given out of order: indices must come back top to bottom.
    let lines = [
        tl(50, 138, 400, 20),
        tl(50, 100, 400, 20),
        tl(50, 119, 380, 20),
    ];
    assert_eq!(live::group_blocks(&lines), vec![vec![1, 2, 0]]);
}

#[test]
fn group_two_columns_are_two_blocks() {
    let lines = [
        tl(50, 100, 200, 20),
        tl(50, 122, 200, 20),
        tl(500, 100, 200, 20),
        tl(500, 122, 200, 20),
    ];
    assert_eq!(
        sorted(live::group_blocks(&lines)),
        vec![vec![0, 1], vec![2, 3]]
    );
}

#[test]
fn group_far_heading_is_separate() {
    let lines = [
        tl(50, 20, 300, 40),
        tl(50, 200, 400, 20),
        tl(50, 222, 400, 20),
    ];
    assert_eq!(
        sorted(live::group_blocks(&lines)),
        vec![vec![0], vec![1, 2]]
    );
}

#[test]
fn group_blocks_are_in_reading_order_and_cover_every_line_once() {
    let lines = [
        tl(50, 300, 200, 20),
        tl(50, 10, 200, 20),
        tl(50, 150, 200, 20),
    ];
    let blocks = live::group_blocks(&lines);
    assert_eq!(blocks, vec![vec![1], vec![2], vec![0]]);
}

#[test]
fn group_empty_is_empty() {
    assert!(live::group_blocks(&[]).is_empty());
}

// ---- item 4: split_translation ----

#[test]
fn split_preserves_words_in_order_and_line_count() {
    let text = "The quick brown fox jumps over the lazy dog";
    let out = live::split_translation(text, &[20, 20, 20]);
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|l| !l.trim().is_empty()), "{out:?}");
    let rejoined: Vec<&str> = out.iter().flat_map(|l| l.split_whitespace()).collect();
    let expected: Vec<&str> = text.split_whitespace().collect();
    assert_eq!(rejoined, expected);
}

#[test]
fn split_is_roughly_proportional() {
    let text = "aaaa bbbb cccc dddd eeee ffff gggg hhhh";
    let out = live::split_translation(text, &[10, 30]);
    let (a, b) = (out[0].chars().count() as f64, out[1].chars().count() as f64);
    assert!(b > a * 2.0 && b < a * 4.5, "lens {a} vs {b}: {out:?}");
}

#[test]
fn split_breaks_at_whitespace_when_possible() {
    let out = live::split_translation("alpha beta gamma delta", &[10, 12]);
    for l in &out {
        for w in l.split_whitespace() {
            assert!(
                ["alpha", "beta", "gamma", "delta"].contains(&w),
                "word was cut: {out:?}"
            );
        }
    }
}

#[test]
fn split_cjk_breaks_anywhere_and_keeps_every_char() {
    let text = "これは日本語の長い文章です";
    let out = live::split_translation(text, &[10, 10]);
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|l| !l.is_empty()));
    assert_eq!(out.concat(), text);
}

#[test]
fn split_single_line_gets_everything() {
    assert_eq!(
        live::split_translation("hello world", &[5]),
        vec!["hello world"]
    );
}

#[test]
fn split_empty_input_gives_empty_lines() {
    let out = live::split_translation("", &[5, 5]);
    assert_eq!(out, vec![String::new(), String::new()]);
}

// ---- finding 4: full-width punctuation and kinsoku ----

#[test]
fn split_never_starts_a_line_with_closing_punctuation() {
    let cases: [(&str, &[usize]); 4] = [
        // Ideal cut lands exactly before the full stop.
        ("日本語のテキストです。次の行", &[9, 5]),
        ("これは、テストです。「引用」です。", &[4, 4, 4]),
        ("彼は言った。「行こう！」と。", &[6, 7]),
        ("（注）これは？そうです。", &[3, 3, 3]),
    ];
    for (text, lens) in cases {
        let out = live::split_translation(text, lens);
        assert_eq!(out.len(), lens.len());
        assert_eq!(out.concat(), text, "chars lost");
        for l in &out {
            assert!(
                !l.starts_with(['、', '。', '」', '』', ')', '）', '！', '？']),
                "line starts with closing punctuation: {out:?}"
            );
        }
    }
}

#[test]
fn fit_font_counts_ideographic_punctuation_as_one_em() {
    // U+3000-303F: 10 glyphs, 1 em each, 100 px wide => 10 px.
    let px = live::fit_font_px("、。、。、。、。、。", 100.0, 40.0, MIN);
    assert!((px - 10.0).abs() < 1e-6, "{px}");
}

#[test]
fn fit_font_counts_fullwidth_forms_as_one_em() {
    // U+FF00-FFEF: full-width Latin and punctuation, 10 glyphs => 10 px.
    let px = live::fit_font_px("ＡＢＣ１２３！？（）", 100.0, 40.0, MIN);
    assert!((px - 10.0).abs() < 1e-6, "{px}");
}

// ---- item 7: fit_font_px / layout ----

const MIN: f64 = 6.0;

#[test]
fn fit_font_short_text_in_wide_box_uses_height_size() {
    // Height-based size is today's (h * 0.75).clamp(10, 48).
    assert_eq!(live::fit_font_px("Hi", 500.0, 20.0, MIN), 15.0);
    assert_eq!(live::fit_font_px("Hi", 500.0, 100.0, MIN), 48.0);
    assert_eq!(live::fit_font_px("Hi", 500.0, 8.0, MIN), 10.0);
}

#[test]
fn fit_font_long_latin_text_shrinks_by_average_advance() {
    // 20 Latin chars at 0.55 em: 110 / (20 * 0.55) = 10 px.
    let px = live::fit_font_px(&"a".repeat(20), 110.0, 40.0, MIN);
    assert!((px - 10.0).abs() < 1e-6, "{px}");
}

#[test]
fn fit_font_cjk_uses_one_em_per_glyph() {
    let px = live::fit_font_px("日本語日本語日本語日", 100.0, 40.0, MIN);
    assert!((px - 10.0).abs() < 1e-6, "{px}");
}

#[test]
fn fit_font_never_below_min() {
    assert_eq!(live::fit_font_px(&"a".repeat(500), 50.0, 40.0, MIN), MIN);
}

#[test]
fn fit_font_never_above_height_size() {
    for text in ["", "a", "hello", "日本"] {
        for (w, h) in [(10.0, 20.0), (1000.0, 20.0), (1000.0, 200.0), (300.0, 4.0)] {
            let height_size = (h * 0.75_f64).clamp(10.0, 48.0);
            let px = live::fit_font_px(text, w, h, MIN);
            assert!(
                px <= height_size + 1e-9,
                "{text:?} {w}x{h}: {px} > {height_size}"
            );
            assert!(px >= MIN);
        }
    }
}

#[test]
fn layout_font_shrinks_for_long_text_in_narrow_box() {
    let r = PhysicalRect {
        x: 0,
        y: 0,
        width: 100,
        height: 20,
    };
    let out = live::layout(&[(r, "a".repeat(40))], 1.0);
    // Height size would be 15; width fit is 100 / (40 * 0.55) = 4.5, floored at the layout minimum 8.
    assert_eq!(out[0].font_px, 8.0);
    let short = live::layout(&[(r, "Hi".into())], 1.0);
    assert_eq!(short[0].font_px, 15.0);
}

// ---- item 8: backoff ----

#[test]
fn backoff_doubles_and_caps() {
    let base = Duration::from_millis(100);
    let max = Duration::from_millis(1000);
    assert_eq!(live::backoff(1, base, max), Duration::from_millis(100));
    assert_eq!(live::backoff(2, base, max), Duration::from_millis(200));
    assert_eq!(live::backoff(3, base, max), Duration::from_millis(400));
    assert_eq!(live::backoff(4, base, max), Duration::from_millis(800));
    assert_eq!(live::backoff(5, base, max), max);
    assert_eq!(live::backoff(60, base, max), max);
    assert_eq!(live::backoff(u32::MAX, base, max), max);
}

#[test]
fn backoff_zero_failures_is_no_pause() {
    assert_eq!(
        live::backoff(0, Duration::from_secs(2), Duration::from_secs(60)),
        Duration::ZERO
    );
}

// ---- harness for Translator / run_live ----

const TICK: Duration = Duration::from_millis(1);
const RECT: LogicalRect = LogicalRect {
    x: 0.0,
    y: 0.0,
    width: 64.0,
    height: 64.0,
};

fn frame() -> RgbaImage {
    RgbaImage::from_pixel(64, 64, image::Rgba([120, 120, 120, 255]))
}

fn deepl_settings() -> (Arc<SettingsStore>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let settings = Arc::new(SettingsStore::open_at(dir.path().join("settings.json")));
    settings
        .save(
            None,
            "DE".into(),
            "deepl".into(),
            String::new(),
            String::new(),
            "live".into(),
        )
        .unwrap();
    (settings, dir)
}

/// Echoes every `text` entry as "T:<text>", preserving order and count.
struct Echo;
impl Respond for Echo {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let items: Vec<Value> = body["text"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| json!({"detected_source_language": "EN", "text": format!("T:{}", t.as_str().unwrap())}))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "translations": items }))
    }
}

async fn deepl_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/translate"))
        .respond_with(Echo)
        .mount(&server)
        .await;
    server
}

fn counting_translator(
    server_uri: &str,
    key: Option<&'static str>,
) -> (Translator, Arc<AtomicUsize>, tempfile::TempDir) {
    let (settings, dir) = deepl_settings();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let t = Translator::with_key_loader(settings, Arc::new(ManagerState::default()), move || {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(key.map(String::from))
    })
    .with_deepl_base(server_uri.to_string());
    (t, calls, dir)
}

// ---- item 2: key cached in memory ----

// Pinned loader signature: `Fn() -> Result<Option<String>, String> + Send + Sync + 'static`.
// Ok(None) = no entry (cached); Err = transient failure (NOT cached, retried next translation).

#[tokio::test]
async fn deepl_loader_error_is_not_cached_and_is_retried() {
    let server = deepl_server().await;
    let (settings, _dir) = deepl_settings();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let t =
        Translator::with_key_loader(settings, Arc::new(ManagerState::default()), move || match c
            .fetch_add(1, Ordering::SeqCst)
        {
            0 | 1 => Err("keychain locked".to_string()),
            _ => Ok(Some("k:fx".to_string())),
        })
        .with_deepl_base(server.uri());
    assert!(t.translate_text("a").await.is_err());
    assert!(t.translate_text("b").await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 2, "error was cached");
    // The third call finally succeeds, and the key is cached from then on.
    let (tr, _) = t.translate_text("c").await.unwrap();
    assert_eq!(tr.text, "T:c");
    t.translate_text("d").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn deepl_loader_error_in_translate_many_fails_all_and_is_retried() {
    let server = deepl_server().await;
    let (settings, _dir) = deepl_settings();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let t = Translator::with_key_loader(settings, Arc::new(ManagerState::default()), move || {
        if c.fetch_add(1, Ordering::SeqCst) == 0 {
            Err("denied".to_string())
        } else {
            Ok(Some("k:fx".to_string()))
        }
    })
    .with_deepl_base(server.uri());
    let texts = vec!["a".to_string(), "b".to_string()];
    let first = t.translate_many(&texts).await;
    assert!(first.iter().all(|r| r.is_err()), "{first:?}");
    let second = t.translate_many(&texts).await;
    assert!(second.iter().all(|r| r.is_ok()), "{second:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn deepl_key_loaded_once_across_many_translations() {
    let server = deepl_server().await;
    let (t, calls, _d) = counting_translator(&server.uri(), Some("k:fx"));
    for i in 0..5 {
        let (tr, _) = t.translate_text(&format!("text {i}")).await.unwrap();
        assert_eq!(tr.text, format!("T:text {i}"));
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "keychain read per translation"
    );
}

#[tokio::test]
async fn deepl_no_key_is_cached_too() {
    let server = deepl_server().await;
    let (t, calls, _d) = counting_translator(&server.uri(), None);
    for _ in 0..3 {
        assert_eq!(t.translate_text("hi").await.unwrap_err(), NO_KEY);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn set_api_key_none_returns_no_key_without_calling_loader_again() {
    let server = deepl_server().await;
    let (t, calls, _d) = counting_translator(&server.uri(), Some("k:fx"));
    t.translate_text("one").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    t.set_api_key(None);
    assert_eq!(t.translate_text("two").await.unwrap_err(), NO_KEY);
    assert_eq!(t.translate_text("three").await.unwrap_err(), NO_KEY);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "loader called again after clear"
    );
}

#[tokio::test]
async fn set_api_key_some_replaces_without_calling_loader() {
    let server = deepl_server().await;
    let (t, calls, _d) = counting_translator(&server.uri(), None);
    t.set_api_key(Some("new:fx".into()));
    let (tr, _) = t.translate_text("hi").await.unwrap();
    assert_eq!(tr.text, "T:hi");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(
        reqs[0]
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        "DeepL-Auth-Key new:fx"
    );
}

// ---- item 5: batch DeepL ----

#[tokio::test]
async fn translate_batch_is_one_request_in_order() {
    let server = deepl_server().await;
    let d = DeepL::with_base_url("k".into(), server.uri());
    let texts: Vec<String> = (0..5).map(|i| format!("text {i}")).collect();
    let out = d.translate_batch(&texts, "DE").await.unwrap();
    assert_eq!(out.len(), 5);
    for (i, t) in out.iter().enumerate() {
        assert_eq!(t.text, format!("T:text {i}"));
        assert_eq!(t.detected_source.as_deref(), Some("EN"));
    }
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1, "exactly one POST");
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["text"].as_array().unwrap().len(), 5);
    assert_eq!(body["target_lang"], "DE");
}

#[tokio::test]
async fn translate_batch_error_status_is_err() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/translate"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let d = DeepL::with_base_url("k".into(), server.uri());
    let err = d
        .translate_batch(&["a".into(), "b".into()], "DE")
        .await
        .unwrap_err();
    assert!(err.contains("429"), "{err}");
}

#[tokio::test]
async fn translate_batch_count_mismatch_is_err() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/translate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"translations":[{"detected_source_language":"EN","text":"only one"}]}),
        ))
        .mount(&server)
        .await;
    let d = DeepL::with_base_url("k".into(), server.uri());
    assert!(
        d.translate_batch(&["a".into(), "b".into()], "DE")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn translate_batch_empty_makes_no_request() {
    let server = deepl_server().await;
    let d = DeepL::with_base_url("k".into(), server.uri());
    assert!(d.translate_batch(&[], "DE").await.unwrap().is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
}

// ---- finding 1: DeepL accepts at most 50 texts per request ----

async fn many_with_n(
    n: usize,
) -> (
    Vec<Result<(screen_translate_core::translate::Translation, bool), String>>,
    Vec<usize>,
) {
    let server = deepl_server().await;
    let (t, _c, _d) = counting_translator(&server.uri(), Some("k:fx"));
    let texts: Vec<String> = (0..n).map(|i| format!("block {i}")).collect();
    let out = t.translate_many(&texts).await;
    for (i, r) in out.iter().enumerate() {
        let (tr, hit) = r.as_ref().unwrap_or_else(|e| panic!("{i}: {e}"));
        assert_eq!(tr.text, format!("T:block {i}"), "order/completeness at {i}");
        assert!(!hit);
    }
    assert_eq!(out.len(), n);
    let sizes = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| {
            let b: Value = serde_json::from_slice(&r.body).unwrap();
            b["text"].as_array().unwrap().len()
        })
        .collect();
    (out, sizes)
}

#[tokio::test]
async fn translate_many_50_texts_is_one_request() {
    let (_, sizes) = many_with_n(50).await;
    assert_eq!(sizes, vec![50]);
}

#[tokio::test]
async fn translate_many_51_texts_is_two_requests_complete_and_ordered() {
    let (_, sizes) = many_with_n(51).await;
    assert_eq!(sizes.len(), 2, "request sizes {sizes:?}");
    assert!(sizes.iter().all(|&n| n <= 50), "{sizes:?}");
    assert_eq!(sizes.iter().sum::<usize>(), 51);
}

#[tokio::test]
async fn translate_many_dedups_before_chunking() {
    // 60 entries but only 50 distinct: still one request.
    let server = deepl_server().await;
    let (t, _c, _d) = counting_translator(&server.uri(), Some("k:fx"));
    let texts: Vec<String> = (0..60).map(|i| format!("block {}", i % 50)).collect();
    let out = t.translate_many(&texts).await;
    assert!(out.iter().all(|r| r.is_ok()));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

// ---- items 4+5: run_live with deepl does one request per fire ----

fn three_lines_two_blocks(_: &RgbaImage) -> Result<Vec<TextLine>, String> {
    let line = |text: &str, y| TextLine {
        text: text.into(),
        bbox: PhysicalRect {
            x: 4,
            y,
            width: 50,
            height: 10,
        },
    };
    // First two lines touch (one block); the third is far below.
    Ok(vec![
        line("Hello", 2),
        line("world", 13),
        line("Goodbye", 50),
    ])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_live_deepl_makes_one_request_per_fire_for_all_blocks() {
    let server = deepl_server().await;
    let (t, _calls, _d) = counting_translator(&server.uri(), Some("k:fx"));
    let t = Arc::new(t);
    let live = Arc::new(Live::default());
    let session = live.start();
    let updates: Arc<Mutex<Vec<Vec<OverlayLine>>>> = Default::default();
    let u2 = updates.clone();
    let updated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let up2 = updated.clone();
    let l2 = live.clone();
    let rt = tokio::runtime::Handle::current();
    let h = std::thread::spawn(move || {
        run_live(
            &t,
            &live,
            session,
            RECT,
            TICK,
            Duration::from_millis(1),
            &rt,
            || Ok(Some(frame())),
            three_lines_two_blocks,
            move |lines, _| {
                u2.lock().unwrap().push(lines);
                // Never stop from inside on_update: it runs under the Live lock.
                up2.store(true, std::sync::atomic::Ordering::SeqCst);
            },
        );
    });
    tokio::task::spawn_blocking(move || {
        let t0 = Instant::now();
        while !h.is_finished() {
            assert!(t0.elapsed() < Duration::from_secs(10), "run_live hung");
            if updated.load(std::sync::atomic::Ordering::SeqCst) {
                l2.stop_with(|| {});
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })
    .await
    .unwrap();

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1, "one DeepL request for the whole fire");
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    // One entry per block; a block's lines are joined with a single space.
    assert_eq!(body["text"], json!(["Hello world", "Goodbye"]));

    let ups = updates.lock().unwrap();
    let lines = &ups[0];
    assert_eq!(
        lines.len(),
        3,
        "still one overlay line per OCR line: {lines:?}"
    );
    assert!(lines.iter().all(|l| !l.text.trim().is_empty()));
    let joined: Vec<String> = lines.iter().map(|l| l.text.clone()).collect();
    assert_eq!(joined[0..2].join(" "), "T:Hello world");
    assert_eq!(joined[2], "T:Goodbye");
}

// ---- item 8: Live::error and backoff in run_live ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_error_is_kept_for_late_overlays_and_cleared_on_success() {
    let live = Live::default();
    assert_eq!(live.error(), None);
    let session = live.start();
    assert!(live.show_with(session, vec![], |_| {}));
    assert_eq!(live.error(), None);

    // A failing translation: run_live records the error where `error()` can read it.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/translate"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let (t, _c, _d) = counting_translator(&server.uri(), Some("bad:fx"));
    let live = Arc::new(Live::default());
    let session = live.start();
    let seen: Arc<Mutex<Option<Option<String>>>> = Default::default();
    let (l2, s2, t) = (live.clone(), seen.clone(), Arc::new(t));
    let rt = tokio::runtime::Handle::current();
    let live2 = live.clone();
    let h = std::thread::spawn(move || {
        run_live(
            &t,
            &live2,
            session,
            RECT,
            TICK,
            Duration::from_millis(1),
            &rt,
            || Ok(Some(frame())),
            three_lines_two_blocks,
            move |_, e| {
                if e.is_some() {
                    *s2.lock().unwrap() = Some(e);
                }
            },
        );
    });
    let t0 = Instant::now();
    while l2.error().is_none() {
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "error never recorded"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(l2.error().unwrap().contains("403"));
    l2.stop();
    assert_eq!(l2.error(), None, "stop clears the error");
    tokio::task::spawn_blocking(move || h.join().unwrap())
        .await
        .unwrap();
    assert!(seen.lock().unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_live_backs_off_exponentially_on_repeated_failures() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/translate"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let (t, _c, _d) = counting_translator(&server.uri(), Some("k:fx"));
    let t = Arc::new(t);
    let live = Arc::new(Live::default());
    let session = live.start();
    let rt = tokio::runtime::Handle::current();
    let l2 = live.clone();
    let h = std::thread::spawn(move || {
        run_live(
            &t,
            &live,
            session,
            RECT,
            TICK,
            // Base pause 40 ms: fixed backoff would fire ~every 45 ms; doubling fires at ~0,40,120,280,600.
            Duration::from_millis(40),
            &rt,
            || Ok(Some(frame())),
            three_lines_two_blocks,
            |_, _| {},
        );
    });
    tokio::time::sleep(Duration::from_millis(700)).await;
    l2.stop();
    tokio::task::spawn_blocking(move || h.join().unwrap())
        .await
        .unwrap();
    let fires = server.received_requests().await.unwrap().len();
    assert!(
        (3..=6).contains(&fires),
        "expected about 5 attempts in 700 ms with doubling backoff, got {fires}"
    );
}

// ---- item 6: macOS OCR languages ----

#[cfg(target_os = "macos")]
#[test]
fn macos_ocr_languages_are_supported_and_cover_new_scripts() {
    use screen_translate_core::ocr::macos::{languages, supported_languages};
    let supported = supported_languages();
    assert!(!supported.is_empty(), "Vision reported no languages");
    let ours = languages();
    for l in ours {
        assert!(
            supported.iter().any(|s| s == l),
            "{l} not supported by Vision: {supported:?}"
        );
    }
    let primary = |s: &str| s.split(['-', '_']).next().unwrap().to_ascii_lowercase();
    for want in ["ru", "uk", "th", "vi", "ar"] {
        if supported.iter().any(|s| primary(s) == want) {
            assert!(
                ours.iter().any(|l| primary(l) == want),
                "{want} is supported by Vision but not configured: {ours:?}"
            );
        }
    }
}
