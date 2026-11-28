use image::{Rgba, RgbaImage};
use screen_translate_core::capture::PhysicalRect;
use screen_translate_core::ocr::{TextLine, join_lines, language_available, recognize};

fn line(text: &str) -> TextLine {
    TextLine {
        text: text.to_string(),
        bbox: PhysicalRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
    }
}

#[test]
fn join_lines_trims_each_line() {
    assert_eq!(join_lines(&[line("  a "), line("\tb\n")]), "a\nb");
}

#[test]
fn join_lines_drops_empty_and_whitespace_only() {
    assert_eq!(
        join_lines(&[line("a"), line(""), line("   "), line("b")]),
        "a\nb"
    );
}

#[test]
fn join_lines_keeps_order() {
    assert_eq!(join_lines(&[line("3"), line("1"), line("2")]), "3\n1\n2");
}

#[test]
fn join_lines_empty_input() {
    assert_eq!(join_lines(&[]), "");
}

#[test]
fn join_lines_keeps_inner_spaces() {
    assert_eq!(join_lines(&[line(" hello  world ")]), "hello  world");
}

// ---- Platform OCR tests (macOS: Apple Vision, Windows: Windows.Media.Ocr) ----
//
// Contract: each fixture test skips (passes) when `language_available(tag)` is false, because
// Windows.Media.Ocr returns garbage (not an Err) when the OCR language pack is missing. On macOS
// Vision ships all languages, so `language_available` must be true there (see test below).
//
// Expected strings must match tests/fixtures/make_fixtures.swift.

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod platform {
    use super::*;

    fn strip_ws(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    fn skip_if_unavailable(tag: &str) -> bool {
        if language_available(tag) {
            return false;
        }
        eprintln!("skipping: OCR language {tag} not installed");
        true
    }

    fn check(name: &str, tag: &str, expected: &[&str]) {
        if skip_if_unavailable(tag) {
            return;
        }
        let path = format!("{}/tests/fixtures/{name}.png", env!("CARGO_MANIFEST_DIR"));
        let img = image::open(&path)
            .unwrap_or_else(|e| panic!("open {path}: {e}"))
            .to_rgba8();
        let lines = recognize(&img).unwrap_or_else(|e| panic!("recognize({name}) failed: {e}"));
        let joined = strip_ws(&join_lines(&lines));
        for exp in expected {
            assert!(
                joined.contains(&strip_ws(exp)),
                "{name}: expected {exp:?} in {joined:?}"
            );
        }
        let (w, h) = img.dimensions();
        assert!(!lines.is_empty(), "{name}: no lines");
        for l in &lines {
            let b = l.bbox;
            assert!(b.width > 0 && b.height > 0, "{name}: empty bbox {b:?}");
            assert!(
                b.x + b.width <= w && b.y + b.height <= h,
                "{name}: bbox {b:?} outside {w}x{h}"
            );
        }
    }

    #[test]
    fn english() {
        check("en", "en", &["Hello world", "Good morning"]);
    }

    #[test]
    fn german() {
        check("de", "de", &["Schöne Grüße", "Straße nach Köln"]);
    }

    #[test]
    fn japanese() {
        check("ja", "ja", &["こんにちは世界", "今日は天気です"]);
    }

    #[test]
    fn chinese() {
        check("zh", "zh-Hans", &["你好世界", "今天天气很好"]);
    }

    #[test]
    fn korean() {
        check("ko", "ko", &["안녕하세요", "오늘 날씨가 좋아요"]);
    }

    #[test]
    fn lines_are_in_reading_order() {
        if skip_if_unavailable("en") {
            return;
        }
        let path = format!("{}/tests/fixtures/en.png", env!("CARGO_MANIFEST_DIR"));
        let img = image::open(path).unwrap().to_rgba8();
        let lines = recognize(&img).unwrap();
        assert!(
            lines.windows(2).all(|p| p[0].bbox.y <= p[1].bbox.y),
            "{lines:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_has_all_languages() {
        for tag in ["en", "de", "ja", "zh-Hans", "ko"] {
            assert!(
                language_available(tag),
                "{tag} should be available on macOS"
            );
        }
    }

    #[test]
    fn blank_image_yields_no_lines() {
        if skip_if_unavailable("en") {
            return;
        }
        let img = RgbaImage::from_pixel(200, 100, Rgba([255, 255, 255, 255]));
        assert_eq!(recognize(&img), Ok(vec![]));
    }

    #[test]
    fn large_image_is_recognized() {
        if skip_if_unavailable("en") {
            return;
        }
        let path = format!("{}/tests/fixtures/en.png", env!("CARGO_MANIFEST_DIR"));
        let fixture = image::open(&path)
            .unwrap_or_else(|e| panic!("open {path}: {e}"))
            .to_rgba8();
        let (w, h) = (3000, 400);
        let mut img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
        image::imageops::overlay(&mut img, &fixture, 2600, 200);
        let lines = recognize(&img).unwrap_or_else(|e| panic!("recognize failed: {e}"));
        let joined = strip_ws(&join_lines(&lines));
        assert!(
            joined.contains("Helloworld"),
            "missing Hello world in {joined:?}"
        );
        assert!(
            joined.contains("Goodmorning"),
            "missing Good morning in {joined:?}"
        );
        for l in &lines {
            let b = l.bbox;
            assert!(
                b.x + b.width <= w && b.y + b.height <= h,
                "bbox {b:?} outside {w}x{h}"
            );
        }
        let hello = lines
            .iter()
            .find(|l| strip_ws(&l.text).contains("Helloworld"))
            .expect("no Hello world line");
        assert!(
            hello.bbox.x >= 2500,
            "Hello world bbox not in original coords: {:?}",
            hello.bbox
        );
    }
}
