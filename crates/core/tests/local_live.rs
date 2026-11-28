//! Needs a llama-server on 127.0.0.1:8080. Run: cargo test --test local_live -- --ignored --nocapture
use std::time::Instant;

use screen_translate_core::ocr::{join_lines, recognize};
use screen_translate_core::translate::local::{DEFAULT_BASE, Local, http_client};

fn local() -> Local {
    Local::new(DEFAULT_BASE.into(), String::new(), http_client())
}

#[tokio::test]
#[ignore]
async fn translates_japanese_literal() {
    let t = Instant::now();
    let out = local().translate("こんにちは世界", "EN-US").await.unwrap();
    println!("{:?} in {:?}", out.text, t.elapsed());
    assert!(out.text.to_lowercase().contains("hello"), "{}", out.text);
}

#[tokio::test]
#[ignore]
async fn translates_ocr_fixture() {
    let path = format!("{}/tests/fixtures/ja.png", env!("CARGO_MANIFEST_DIR"));
    let img = image::open(path).unwrap().to_rgba8();
    let text = join_lines(&recognize(&img).unwrap());
    let t = Instant::now();
    let out = local().translate(&text, "EN-US").await.unwrap();
    println!("OCR {text:?} -> {:?} in {:?}", out.text, t.elapsed());
    assert!(!out.text.is_empty());
}
