use std::time::Duration;

use screen_translate_lib::settings::TARGETS;
use screen_translate_lib::translate::local::{clean, http_client, language_name, Local, Template, DEFAULT_BASE};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "qwen2.5:7b";
const PATH: &str = "/v1/chat/completions";

fn ok(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": body }, "finish_reason": "stop" }]
    }))
}

fn local(base: String) -> Local {
    Local::new(base, String::new(), reqwest::Client::new())
}

fn local_with_model(base: String) -> Local {
    Local::new(base, MODEL.to_string(), reqwest::Client::new())
}

async fn mount(server: &MockServer, resp: ResponseTemplate) {
    Mock::given(method("POST")).and(path(PATH)).respond_with(resp).mount(server).await;
}

async fn first_body(server: &MockServer) -> Value {
    let reqs = server.received_requests().await.unwrap();
    serde_json::from_slice(&reqs[0].body).unwrap()
}

#[test]
fn defaults() {
    assert_eq!(DEFAULT_BASE, "http://127.0.0.1:8080");
}

#[test]
fn http_client_builds() {
    let _ = http_client();
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
async fn request_shape_without_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PATH))
        .respond_with(ok("こんにちは"))
        .expect(1)
        .mount(&server)
        .await;

    let src = "Hello, world! \"quoted\"\nline two";
    local(server.uri()).translate(src, "JA").await.unwrap();

    let body = first_body(&server).await;
    assert!(body.get("model").is_none(), "model must be omitted when empty: {body}");
    assert_eq!(body["stream"], json!(false));
    assert_eq!(body["temperature"], json!(0));
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["role"], "system");
    let system = msgs[0]["content"].as_str().unwrap();
    assert!(system.contains("Japanese"), "system missing language: {system}");
    assert!(system.contains("only"), "system missing 'only': {system}");
    assert_eq!(msgs[1]["role"], "user");
    assert_eq!(msgs[1]["content"], format!("<text>\n{src}\n</text>"));
}

#[tokio::test]
async fn request_includes_model_when_set() {
    let server = MockServer::start().await;
    mount(&server, ok("Hallo")).await;

    local_with_model(server.uri()).translate("hi", "DE").await.unwrap();

    assert_eq!(first_body(&server).await["model"], MODEL);
}

#[tokio::test]
async fn trailing_slash_base_has_no_double_slash() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PATH))
        .respond_with(ok("Hallo"))
        .expect(1)
        .mount(&server)
        .await;

    let t = local(format!("{}/", server.uri())).translate("hi", "DE").await.unwrap();
    assert_eq!(t.text, "Hallo");
}

#[tokio::test]
async fn prompt_names_simplified_chinese() {
    let server = MockServer::start().await;
    mount(&server, ok("你好")).await;

    local(server.uri()).translate("hi", "ZH-HANS").await.unwrap();

    let body = first_body(&server).await;
    assert!(body["messages"][0]["content"].as_str().unwrap().contains("Simplified Chinese"));
}

#[tokio::test]
async fn response_is_trimmed_and_source_unknown() {
    let server = MockServer::start().await;
    mount(&server, ok("  Hello\n")).await;

    let t = local(server.uri()).translate("Bonjour", "EN-US").await.unwrap();
    assert_eq!(t.text, "Hello");
    assert_eq!(t.detected_source, None);
}

#[tokio::test]
async fn empty_choices_is_no_translation() {
    let server = MockServer::start().await;
    mount(&server, ResponseTemplate::new(200).set_body_json(json!({ "choices": [] }))).await;

    let err = local(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("no translation"), "{err}");
}

#[tokio::test]
async fn blank_content_is_no_translation() {
    let server = MockServer::start().await;
    mount(&server, ok("  \n")).await;

    let err = local(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("no translation"), "{err}");
}

#[tokio::test]
async fn not_found_hints_endpoint_path() {
    let server = MockServer::start().await;
    mount(&server, ResponseTemplate::new(404)).await;

    let err = local(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("/v1/chat/completions"), "{err}");
}

#[tokio::test]
async fn server_error_includes_status() {
    let server = MockServer::start().await;
    mount(&server, ResponseTemplate::new(500)).await;

    let err = local(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("500"), "{err}");
}

#[tokio::test]
async fn malformed_json_is_error() {
    let server = MockServer::start().await;
    mount(&server, ResponseTemplate::new(200).set_body_string("not json")).await;

    assert!(local(server.uri()).translate("hi", "DE").await.is_err());
}

#[tokio::test]
async fn timeout_is_reported() {
    let server = MockServer::start().await;
    mount(&server, ok("late").set_delay(Duration::from_millis(500))).await;

    let c = reqwest::Client::builder()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    let err = Local::new(server.uri(), String::new(), c)
        .translate("hi", "DE")
        .await
        .unwrap_err();
    assert!(err.contains("timed out"), "{err}");
}

#[tokio::test]
async fn connection_refused_says_not_running() {
    let err = local("http://127.0.0.1:1".to_string())
        .translate("hi", "DE")
        .await
        .unwrap_err();
    assert!(err.contains("not running"), "{err}");
    assert!(err.contains("http://127.0.0.1:1"), "{err}");
}

#[tokio::test]
async fn not_found_surfaces_openai_error_message() {
    let server = MockServer::start().await;
    let msg = "model \"qwen2.5:7b\" not found, try pulling it first";
    mount(&server, ResponseTemplate::new(404).set_body_json(json!({"error": {"message": msg}}))).await;

    let err = local_with_model(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains(msg), "{err}");
}

#[tokio::test]
async fn base_ending_in_v1_has_no_double_v1() {
    for suffix in ["/v1", "/v1/"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(PATH))
            .respond_with(ok("Hallo"))
            .expect(1)
            .mount(&server)
            .await;

        let t = local(format!("{}{suffix}", server.uri())).translate("hi", "DE").await.unwrap();
        assert_eq!(t.text, "Hallo", "{suffix}");
        let reqs = server.received_requests().await.unwrap();
        assert_eq!(reqs[0].url.path(), PATH, "{suffix}");
    }
}

#[tokio::test]
async fn null_content_is_no_translation() {
    let server = MockServer::start().await;
    mount(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "content": null }, "finish_reason": "stop" }]
        })),
    )
    .await;

    let err = local(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("no translation"), "{err}");
}

#[tokio::test]
async fn reasoning_content_is_ignored() {
    let server = MockServer::start().await;
    mount(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "reasoning_content": "thinking...", "content": "Hello" } }]
        })),
    )
    .await;

    let t = local(server.uri()).translate("Bonjour", "EN-US").await.unwrap();
    assert_eq!(t.text, "Hello");
}

#[tokio::test]
async fn array_content_parts_are_joined() {
    let server = MockServer::start().await;
    mount(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "content": [{"type": "text", "text": "Hello"}] } }]
        })),
    )
    .await;

    let t = local(server.uri()).translate("Bonjour", "EN-US").await.unwrap();
    assert_eq!(t.text, "Hello");
}

#[tokio::test]
async fn empty_input_makes_no_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(PATH))
        .respond_with(ok("x"))
        .expect(0)
        .mount(&server)
        .await;

    let l = local(server.uri());
    for input in ["", "   \n\t"] {
        let t = l.translate(input, "DE").await.unwrap();
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
    mount(&server, ok("<think>x</think>\n  Hallo ")).await;

    let t = local(server.uri()).translate("hi", "DE").await.unwrap();
    assert_eq!(t.text, "Hallo");
}

#[tokio::test]
async fn unterminated_think_is_no_translation() {
    let server = MockServer::start().await;
    mount(&server, ok("<think>unterminated")).await;

    let err = local(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("no translation"), "{err}");
}

#[tokio::test]
async fn openai_error_message_is_surfaced() {
    let server = MockServer::start().await;
    mount(
        &server,
        ResponseTemplate::new(400).set_body_json(json!({"error": {"message": "context too long", "type": "invalid_request_error"}})),
    )
    .await;

    let err = local(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("context too long"), "{err}");
}

#[tokio::test]
async fn string_error_body_is_surfaced() {
    let server = MockServer::start().await;
    mount(&server, ResponseTemplate::new(400).set_body_json(json!({"error": "invalid model name"}))).await;

    let err = local(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains("invalid model name"), "{err}");
}

#[tokio::test]
async fn long_error_message_is_truncated() {
    let server = MockServer::start().await;
    let long = "e".repeat(1000);
    mount(&server, ResponseTemplate::new(500).set_body_json(json!({ "error": { "message": long } }))).await;

    let err = local(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains(&"e".repeat(200)), "{err}");
    assert!(!err.contains(&"e".repeat(201)), "{err}");
}

#[tokio::test]
async fn long_string_error_is_truncated() {
    let server = MockServer::start().await;
    let long = "e".repeat(1000);
    mount(&server, ResponseTemplate::new(500).set_body_json(json!({ "error": long }))).await;

    let err = local(server.uri()).translate("hi", "DE").await.unwrap_err();
    assert!(err.contains(&"e".repeat(200)), "{err}");
    assert!(!err.contains(&"e".repeat(201)), "{err}");
}

#[test]
fn template_from_id() {
    assert_eq!(Template::from_id("hy-mt"), Template::HyMt);
    assert_eq!(Template::from_id("default"), Template::Default);
    assert_eq!(Template::from_id(""), Template::Default);
    assert_eq!(Template::from_id("unknown"), Template::Default);
    assert_eq!(Template::from_id("HY-MT"), Template::Default);
}

fn hy(base: String, model: &str) -> Local {
    Local::new(base, model.to_string(), reqwest::Client::new()).with_template(Template::HyMt)
}

#[tokio::test]
async fn hy_mt_request_is_one_user_message() {
    let server = MockServer::start().await;
    mount(&server, ok("Hello")).await;

    let t = hy(server.uri(), "").translate("你好\nworld", "DE").await.unwrap();
    assert_eq!(t.text, "Hello");

    let body = first_body(&server).await;
    assert!(body.get("model").is_none(), "{body}");
    assert_eq!(body["stream"], json!(false));
    assert_eq!(body["temperature"], json!(0));
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1, "{body}");
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(
        msgs[0]["content"],
        "Translate the following text into German. Note that you should only output the translated result without any additional explanation:\n\n你好\nworld"
    );
}

#[tokio::test]
async fn hy_mt_request_names_non_english_target() {
    let server = MockServer::start().await;
    mount(&server, ok("こんにちは")).await;

    hy(server.uri(), "").translate("Hello", "JA").await.unwrap();

    let body = first_body(&server).await;
    assert_eq!(
        body["messages"][0]["content"],
        "Translate the following text into Japanese. Note that you should only output the translated result without any additional explanation:\n\nHello"
    );
}

#[tokio::test]
async fn hy_mt_request_includes_model_when_set() {
    let server = MockServer::start().await;
    mount(&server, ok("Hallo")).await;

    hy(server.uri(), MODEL).translate("hi", "DE").await.unwrap();

    assert_eq!(first_body(&server).await["model"], MODEL);
}

#[tokio::test]
async fn hy_mt_response_is_cleaned() {
    let server = MockServer::start().await;
    mount(&server, ok("<think>x</think>\n \"Hallo\" ")).await;

    assert_eq!(hy(server.uri(), "").translate("hi", "DE").await.unwrap().text, "Hallo");
}

#[tokio::test]
async fn hy_mt_blank_input_makes_no_request() {
    let server = MockServer::start().await;
    mount(&server, ok("x")).await;

    assert_eq!(hy(server.uri(), "").translate("  ", "DE").await.unwrap().text, "");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn explicit_default_template_matches_new() {
    let server = MockServer::start().await;
    mount(&server, ok("Hallo")).await;

    Local::new(server.uri(), String::new(), reqwest::Client::new()).with_template(Template::Default).translate("hi", "DE").await.unwrap();

    let msgs = first_body(&server).await["messages"].clone();
    assert_eq!(msgs.as_array().unwrap().len(), 2);
    assert_eq!(msgs[1]["content"], "<text>\nhi\n</text>");
}

#[test]
fn clean_strips_balanced_text_wrapper() {
    assert_eq!(clean("hi", "<text>Hallo</text>"), "Hallo");
    assert_eq!(clean("hi", "  <text>\n Hallo \n</text>\n"), "Hallo");
    assert_eq!(clean("hi", "<text>\nLine one\nLine two\n</text>"), "Line one\nLine two");
}

#[test]
fn clean_strips_unbalanced_text_tags() {
    assert_eq!(clean("hi", "<text>Hallo"), "Hallo");
    assert_eq!(clean("hi", "Hallo</text>"), "Hallo");
    assert_eq!(clean("hi", "<text>\nHallo\n"), "Hallo");
}

#[test]
fn clean_keeps_text_wrapper_when_source_is_wrapped() {
    assert_eq!(clean("<text>hi</text>", "<text>Hallo</text>"), "<text>Hallo</text>");
    assert_eq!(clean("  <text>\nhi\n</text> ", "<text>\nHallo\n</text>"), "<text>\nHallo\n</text>");
}

#[test]
fn clean_keeps_unbalanced_tag_the_source_has() {
    assert_eq!(clean("<text>hi", "<text>Hallo"), "<text>Hallo");
    assert_eq!(clean("hi</text>", "Hallo</text>"), "Hallo</text>");
}

#[test]
fn clean_text_wrapper_after_think_and_label() {
    assert_eq!(clean("hi", "<think>r</think><text>Hallo</text>"), "Hallo");
    assert_eq!(clean("hi", "Translation: <text>Hallo</text>"), "Hallo");
}

#[test]
fn clean_keeps_inner_text_tags_in_the_middle() {
    assert_eq!(clean("hi", "Use <text> tags"), "Use <text> tags");
}

#[tokio::test]
async fn default_response_strips_text_wrapper() {
    let server = MockServer::start().await;
    mount(&server, ok("<text>Hallo</text>")).await;

    assert_eq!(local(server.uri()).translate("hi", "DE").await.unwrap().text, "Hallo");
}
