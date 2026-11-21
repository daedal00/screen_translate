use std::time::Duration;

use screen_translate_lib::settings::TARGETS;
use screen_translate_lib::translate::ollama::{clean, language_name, Ollama, DEFAULT_BASE, DEFAULT_MODEL};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "qwen2.5:7b";

fn ok(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "response": body, "done": true }))
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

fn ollama(base: String) -> Ollama {
    Ollama::new(base, MODEL.to_string(), client())
}

#[test]
fn defaults() {
    assert_eq!(DEFAULT_BASE, "http://127.0.0.1:11434");
    assert_eq!(DEFAULT_MODEL, "qwen2.5:7b");
}

#[test]
fn language_names() {
    assert_eq!(language_name("EN-US"), "English");
    assert_eq!(language_name("EN-GB"), "English");
    assert_eq!(language_name("DE"), "German");
    assert_eq!(language_name("FR"), "French");
    assert_eq!(language_name("ES"), "Spanish");
    assert_eq!(language_name("JA"), "Japanese");
    assert_eq!(language_name("ZH-HANS"), "Simplified Chinese");
    assert_eq!(language_name("KO"), "Korean");
    assert_eq!(language_name("XX"), "English");
}

#[tokio::test]
async fn request_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ok("こんにちは"))
        .expect(1)
        .mount(&server)
        .await;

    let src = "Hello, world! \"quoted\"\nline two";
    ollama(server.uri()).translate(src, "JA").await.unwrap();

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["model"], MODEL);
    assert_eq!(body["stream"], json!(false));
    assert_eq!(body["options"]["temperature"], json!(0));
    let system = body["system"].as_str().unwrap();
    assert!(system.contains("Japanese"), "system missing language: {system}");
    assert!(system.contains("only"), "system missing 'only': {system}");
    assert_eq!(body["prompt"], format!("<text>\n{src}\n</text>"));
}

#[tokio::test]
async fn prompt_names_simplified_chinese() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ok("你好"))
        .mount(&server)
        .await;

    ollama(server.uri()).translate("hi", "ZH-HANS").await.unwrap();

    let reqs = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert!(body["system"].as_str().unwrap().contains("Simplified Chinese"));
}

#[tokio::test]
async fn response_is_trimmed_and_source_unknown() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ok("  Hello\n"))
        .mount(&server)
        .await;

    let t = ollama(server.uri()).translate("Bonjour", "EN-US").await.unwrap();
    assert_eq!(t.text, "Hello");
    assert_eq!(t.detected_source, None);
}

#[tokio::test]
async fn not_found_names_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "model not found"})))
        .mount(&server)
        .await;

    let err = ollama(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("model"), "{err}");
    assert!(err.contains(MODEL), "{err}");
}

#[tokio::test]
async fn server_error_includes_status() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let err = ollama(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("500"), "{err}");
}

#[tokio::test]
async fn malformed_json_is_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    assert!(ollama(server.uri()).translate("hi", "DE").await.is_err());
}

#[tokio::test]
async fn timeout_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ok("late").set_delay(Duration::from_millis(500)))
        .mount(&server)
        .await;

    let c = reqwest::Client::builder()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    let err = Ollama::new(server.uri(), MODEL.to_string(), c)
        .translate("hi", "DE")
        .await
        .unwrap_err();
    assert!(err.contains("timed out"), "{err}");
}

#[tokio::test]
async fn connection_refused_says_not_running() {
    let err = ollama("http://127.0.0.1:1".to_string())
        .translate("hi", "DE")
        .await
        .unwrap_err();
    assert!(err.contains("Ollama is not running"), "{err}");
}

#[tokio::test]
async fn empty_input_makes_no_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ok("x"))
        .expect(0)
        .mount(&server)
        .await;

    let o = ollama(server.uri());
    for input in ["", "   \n\t"] {
        let t = o.translate(input, "DE").await.unwrap();
        assert_eq!(t.text, "");
        assert_eq!(t.detected_source, None);
    }
}

#[test]
fn language_names_cover_targets() {
    for t in TARGETS.iter().filter(|t| !t.starts_with("EN-")) {
        assert_ne!(language_name(t), "English", "{t}");
    }
}

#[test]
fn clean_strips_think_block() {
    assert_eq!(clean("hi", "<think>reasoning</think>\n\n Hallo"), "Hallo");
}

#[test]
fn clean_unterminated_think_is_empty() {
    assert_eq!(clean("hi", "<think>unterminated"), "");
}

#[test]
fn clean_strips_label() {
    assert_eq!(clean("hi", "Translation: Hallo"), "Hallo");
    assert_eq!(clean("hi", "translation: Hallo"), "Hallo");
    assert_eq!(clean("hi", "Here is the translation:\nHallo"), "Hallo");
}

#[test]
fn clean_keeps_label_when_source_has_it() {
    assert_eq!(clean("Translation: hello", "Translation: Hallo"), "Translation: Hallo");
}

#[test]
fn clean_strips_wrapping_quotes() {
    assert_eq!(clean("hi", "\"Hallo\""), "Hallo");
    assert_eq!(clean("hi", "“Hallo”"), "Hallo");
    assert_eq!(clean("hi", "「こんにちは」"), "こんにちは");
}

#[test]
fn clean_keeps_quotes_when_source_wrapped() {
    assert_eq!(clean("\"hi\"", "\"Hallo\""), "\"Hallo\"");
    assert_eq!(clean("「hi」", "「こんにちは」"), "「こんにちは」");
}

#[test]
fn clean_keeps_quotes_when_source_wrapped_by_any_pair() {
    assert_eq!(
        clean("「こんにちは」と言った。「さようなら」", "\"Hello,\" he said. \"Goodbye.\""),
        "\"Hello,\" he said. \"Goodbye.\""
    );
}

#[test]
fn clean_keeps_quotes_when_source_wrapped_by_other_pair() {
    assert_eq!(clean("「こんにちは」", "\"Hello\""), "\"Hello\"");
}

#[test]
fn clean_keeps_quotes_with_inner_quotes() {
    assert_eq!(clean("こんにちは", "\"A\" and \"B\""), "\"A\" and \"B\"");
}

#[test]
fn clean_strips_simple_quotes_plain_source() {
    assert_eq!(clean("こんにちは", "\"Hello\""), "Hello");
}

#[test]
fn clean_passthrough_and_trim() {
    assert_eq!(clean("hi", "Hallo"), "Hallo");
    assert_eq!(clean("hi", "  Hallo \n"), "Hallo");
}

#[tokio::test]
async fn think_block_is_stripped_from_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ok("<think>x</think>\n  Hallo "))
        .mount(&server)
        .await;

    let t = ollama(server.uri()).translate("hi", "DE").await.unwrap();
    assert_eq!(t.text, "Hallo");
}

#[tokio::test]
async fn unterminated_think_is_no_translation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ok("<think>unterminated"))
        .mount(&server)
        .await;

    let err = ollama(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("no translation"), "{err}");
}

#[tokio::test]
async fn error_body_text_is_surfaced() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "invalid model name"})))
        .mount(&server)
        .await;

    let err = ollama(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("invalid model name"), "{err}");
}

#[tokio::test]
async fn long_error_body_is_truncated() {
    let server = MockServer::start().await;
    let long = "e".repeat(1000);
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "error": long })))
        .mount(&server)
        .await;

    let err = ollama(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains(&"e".repeat(200)), "{err}");
    assert!(!err.contains(&"e".repeat(201)), "{err}");
}
