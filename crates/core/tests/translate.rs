use screen_translate_core::translate::deepl::DeepL;
use screen_translate_core::translate::{Translation, TranslationCache};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok_body(text: &str, src: &str) -> Value {
    json!({"translations":[{"detected_source_language":src,"text":text}]})
}

fn tr(s: &str) -> Translation {
    Translation {
        text: s.into(),
        detected_source: None,
    }
}

async fn status_error(code: u16) -> String {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/translate"))
        .respond_with(ResponseTemplate::new(code))
        .mount(&server)
        .await;
    let d = DeepL::with_base_url("k".into(), server.uri());
    d.translate("hello", "EN-US").await.unwrap_err()
}

// ---- base URL ----

#[test]
fn new_free_key_uses_free_base() {
    assert_eq!(
        DeepL::new("abc:fx".into()).base_url(),
        "https://api-free.deepl.com"
    );
}

#[test]
fn new_pro_key_uses_pro_base() {
    assert_eq!(DeepL::new("abc".into()).base_url(), "https://api.deepl.com");
}

#[test]
fn with_base_url_overrides() {
    assert_eq!(
        DeepL::with_base_url("abc:fx".into(), "http://x".into()).base_url(),
        "http://x"
    );
}

// ---- request / response ----

#[tokio::test]
async fn sends_expected_request_and_parses_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/translate"))
        .and(header("authorization", "DeepL-Auth-Key secret:fx"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("Hello", "JA")))
        .expect(1)
        .mount(&server)
        .await;

    let d = DeepL::with_base_url("secret:fx".into(), server.uri());
    let t = d.translate("こんにちは", "EN-US").await.unwrap();
    assert_eq!(
        t,
        Translation {
            text: "Hello".into(),
            detected_source: Some("JA".into())
        }
    );

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["text"], json!(["こんにちは"]));
    assert_eq!(body["target_lang"], json!("EN-US"));
    assert!(body.get("source_lang").is_none(), "must auto-detect source");
}

#[tokio::test]
async fn missing_detected_source_is_none() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/translate"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"translations":[{"text":"Hi"}]})),
        )
        .mount(&server)
        .await;
    let d = DeepL::with_base_url("k".into(), server.uri());
    let t = d.translate("やあ", "EN-US").await.unwrap();
    assert_eq!(
        t,
        Translation {
            text: "Hi".into(),
            detected_source: None
        }
    );
}

// ---- errors ----

#[tokio::test]
async fn err_403_mentions_api_key() {
    assert!(status_error(403).await.contains("API key"));
}

#[tokio::test]
async fn err_456_mentions_quota() {
    assert!(status_error(456).await.contains("quota"));
}

#[tokio::test]
async fn err_429_mentions_rate() {
    assert!(status_error(429).await.contains("rate"));
}

#[tokio::test]
async fn err_500_mentions_status_code() {
    assert!(status_error(500).await.contains("500"));
}

// ---- empty input ----

#[tokio::test]
async fn empty_and_whitespace_make_no_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("x", "JA")))
        .expect(0)
        .mount(&server)
        .await;
    let d = DeepL::with_base_url("k".into(), server.uri());
    for input in ["", "   ", "\n\t "] {
        let t = d.translate(input, "EN-US").await.unwrap();
        assert_eq!(t.text, "");
    }
    // MockServer verifies expect(0) on drop.
}

// ---- cache ----

#[test]
fn cache_hit_and_miss() {
    let mut c = TranslationCache::new(2);
    assert_eq!(c.get("EN-US", "a"), None);
    c.put("EN-US", "a", tr("A"));
    assert_eq!(c.get("EN-US", "a"), Some(tr("A")));
    assert_eq!(c.get("EN-US", "b"), None);
    assert_eq!(c.len(), 1);
}

#[test]
fn cache_key_includes_target() {
    let mut c = TranslationCache::new(4);
    c.put("EN-US", "a", tr("en"));
    c.put("DE", "a", tr("de"));
    assert_eq!(c.get("EN-US", "a"), Some(tr("en")));
    assert_eq!(c.get("DE", "a"), Some(tr("de")));
    assert_eq!(c.get("FR", "a"), None);
    assert_eq!(c.len(), 2);
}

#[test]
fn cache_evicts_least_recently_used() {
    let mut c = TranslationCache::new(2);
    c.put("T", "a", tr("A"));
    c.put("T", "b", tr("B"));
    c.put("T", "c", tr("C"));
    assert_eq!(c.len(), 2);
    assert_eq!(c.get("T", "a"), None);
    assert_eq!(c.get("T", "b"), Some(tr("B")));
    assert_eq!(c.get("T", "c"), Some(tr("C")));
}

#[test]
fn cache_get_refreshes_recency() {
    let mut c = TranslationCache::new(2);
    c.put("T", "a", tr("A"));
    c.put("T", "b", tr("B"));
    assert!(c.get("T", "a").is_some()); // a is now most recent
    c.put("T", "c", tr("C")); // evicts b
    assert_eq!(c.get("T", "b"), None);
    assert_eq!(c.get("T", "a"), Some(tr("A")));
    assert_eq!(c.get("T", "c"), Some(tr("C")));
}

#[test]
fn cache_put_existing_key_updates_without_growing() {
    let mut c = TranslationCache::new(2);
    c.put("T", "a", tr("A"));
    c.put("T", "a", tr("A2"));
    assert_eq!(c.len(), 1);
    assert_eq!(c.get("T", "a"), Some(tr("A2")));
}

#[test]
fn cache_capacity_respected() {
    let mut c = TranslationCache::new(3);
    for i in 0..10 {
        c.put("T", &i.to_string(), tr("x"));
        assert!(c.len() <= 3);
    }
    assert_eq!(c.len(), 3);
}

// ---- robustness (M4 review) ----

use std::time::{Duration, Instant};

const SECRET: &str = "SECRET123:fx";

async fn error_for(template: ResponseTemplate, client: Option<reqwest::Client>) -> String {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/translate"))
        .respond_with(template)
        .mount(&server)
        .await;
    let client = client.unwrap_or_default();
    let d = DeepL::with_client(SECRET.into(), server.uri(), client);
    d.translate("hello", "EN-US").await.unwrap_err()
}

fn short_timeout_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap()
}

fn slow() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(ok_body("Hi", "JA"))
        .set_delay(Duration::from_secs(3))
}

#[tokio::test]
async fn timeout_maps_to_timed_out_quickly() {
    let start = Instant::now();
    let e = error_for(slow(), Some(short_timeout_client())).await;
    assert!(e.contains("timed out"), "got: {e}");
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn err_400_with_message() {
    let e = error_for(
        ResponseTemplate::new(400)
            .set_body_json(json!({"message":"Value for 'target_lang' not supported."})),
        None,
    )
    .await;
    assert!(e.contains("400"), "got: {e}");
    assert!(
        e.contains("Value for 'target_lang' not supported."),
        "got: {e}"
    );
}

#[tokio::test]
async fn err_400_without_message() {
    let e = error_for(ResponseTemplate::new(400).set_body_string("bad"), None).await;
    assert!(e.contains("400"), "got: {e}");
}

#[tokio::test]
async fn empty_translations_array_errors() {
    let e = error_for(
        ResponseTemplate::new(200).set_body_json(json!({"translations":[]})),
        None,
    )
    .await;
    assert!(e.contains("no translation"), "got: {e}");
}

#[tokio::test]
async fn malformed_json_errors_without_panic() {
    let e = error_for(
        ResponseTemplate::new(200).set_body_string("{not json"),
        None,
    )
    .await;
    assert!(!e.is_empty());
}

#[tokio::test]
async fn errors_never_contain_api_key() {
    let cases = [
        error_for(ResponseTemplate::new(403), None).await,
        error_for(ResponseTemplate::new(456), None).await,
        error_for(ResponseTemplate::new(429), None).await,
        error_for(ResponseTemplate::new(500), None).await,
        error_for(
            ResponseTemplate::new(400).set_body_json(json!({"message":"bad"})),
            None,
        )
        .await,
        error_for(
            ResponseTemplate::new(200).set_body_string("{not json"),
            None,
        )
        .await,
        error_for(slow(), Some(short_timeout_client())).await,
    ];
    for e in cases {
        assert!(!e.contains("SECRET123"), "key leaked: {e}");
    }
}

#[test]
fn http_client_builds() {
    let _ = screen_translate_core::translate::deepl::http_client();
}
