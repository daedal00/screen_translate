use std::path::Path;
use std::time::{Duration, Instant};

use screen_translate_core::manager::server::Status;
use screen_translate_core::manager::{
    MANAGED_LOADING, MANAGED_STOPPED, PROGRESS_INTERVAL, managed_not_ready, model_path, should_emit,
};

fn later(t: Instant, d: Duration) -> Instant {
    t.checked_add(d).unwrap()
}

#[test]
fn should_emit_first_is_true() {
    assert!(should_emit(None, Instant::now(), false));
}

#[test]
fn should_emit_finished_is_true_right_after_emit() {
    let t = Instant::now();
    assert!(should_emit(Some(t), t, true));
}

#[test]
fn should_emit_within_interval_is_false() {
    let t = Instant::now();
    assert!(!should_emit(Some(t), t, false));
    assert!(!should_emit(
        Some(t),
        later(t, PROGRESS_INTERVAL / 2),
        false
    ));
}

#[test]
fn should_emit_at_and_after_interval_is_true() {
    let t = Instant::now();
    assert!(should_emit(Some(t), later(t, PROGRESS_INTERVAL), false));
    assert!(should_emit(Some(t), later(t, PROGRESS_INTERVAL * 5), false));
}

#[test]
fn model_path_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.gguf"), "x").unwrap();
    assert_eq!(
        model_path(dir.path(), "m.gguf").unwrap(),
        dir.path().join("m.gguf")
    );
}

#[test]
fn model_path_missing_is_err() {
    let dir = tempfile::tempdir().unwrap();
    assert!(model_path(dir.path(), "m.gguf").is_err());
}

#[test]
fn model_path_rejects_bad_names() {
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("models");
    std::fs::create_dir(&inner).unwrap();
    std::fs::write(dir.path().join("x.gguf"), "x").unwrap();
    std::fs::write(inner.join("x.bin"), "x").unwrap();
    std::fs::create_dir(inner.join("a")).unwrap();
    std::fs::write(inner.join("a").join("b.gguf"), "x").unwrap();
    for bad in ["../x.gguf", "a/b.gguf", "a\\b.gguf", "x.bin", ""] {
        assert!(model_path(&inner, bad).is_err(), "should reject {bad:?}");
    }
}

#[test]
fn model_path_rejects_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("d.gguf")).unwrap();
    assert!(model_path(dir.path(), "d.gguf").is_err());
}

fn statuses() -> [Status; 4] {
    [
        Status::Stopped,
        Status::Loading { model: "m".into() },
        Status::Ready {
            model: "m".into(),
            offload: None,
        },
        Status::Error {
            message: "e".into(),
            log: vec![],
        },
    ]
}

#[test]
fn managed_url_variants_map_status_to_message() {
    for url in [
        "http://127.0.0.1:18080",
        "http://127.0.0.1:18080/",
        "http://127.0.0.1:18080/v1",
        "http://127.0.0.1:18080/v1/",
    ] {
        assert_eq!(
            managed_not_ready(
                url,
                &Status::Ready {
                    model: "m".into(),
                    offload: None
                }
            ),
            None,
            "{url}"
        );
        assert_eq!(
            managed_not_ready(url, &Status::Loading { model: "m".into() }),
            Some(MANAGED_LOADING),
            "{url}"
        );
        assert_eq!(
            managed_not_ready(url, &Status::Stopped),
            Some(MANAGED_STOPPED),
            "{url}"
        );
        assert_eq!(
            managed_not_ready(
                url,
                &Status::Error {
                    message: "e".into(),
                    log: vec![]
                }
            ),
            Some(MANAGED_STOPPED),
            "{url}"
        );
    }
}

#[test]
fn other_urls_are_never_managed() {
    // `localhost` is a different string from 127.0.0.1: exact match is the spec.
    for url in [
        "http://127.0.0.1:8080",
        "http://127.0.0.1:11434",
        "http://localhost:18080",
        "",
        "http://127.0.0.1:18081",
    ] {
        for s in statuses() {
            assert_eq!(managed_not_ready(url, &s), None, "{url} {s:?}");
        }
    }
}

#[test]
fn model_path_is_not_checked_against_other_dirs() {
    // Sanity: the same name in another dir does not satisfy the lookup.
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    std::fs::write(a.path().join("m.gguf"), "x").unwrap();
    assert!(model_path(Path::new(b.path()), "m.gguf").is_err());
}

#[test]
fn is_managed_url_ignores_trailing_slash_and_v1() {
    use screen_translate_core::manager::is_managed_url;
    for url in [
        "http://127.0.0.1:18080",
        "http://127.0.0.1:18080/",
        "http://127.0.0.1:18080/v1",
        "http://127.0.0.1:18080/v1/",
        " http://127.0.0.1:18080 ",
    ] {
        assert!(is_managed_url(url), "{url}");
    }
    for url in [
        "http://127.0.0.1:8080",
        "http://localhost:18080",
        "http://127.0.0.1:18080/api",
        "",
    ] {
        assert!(!is_managed_url(url), "{url}");
    }
}
