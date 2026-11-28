use std::path::Path;
use std::sync::atomic::AtomicBool;

use screen_translate_core::manager::download::CANCELLED;
use screen_translate_core::manager::models::{
    InstalledModel, RemoteFile, Source, delete_model, download_files, list_installed, models_dir,
    parse_source, pick_files, resolve,
};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const BASE: &str = "https://huggingface.co";
const REPO: &str = "bartowski/Qwen2.5-3B-Instruct-GGUF";

#[test]
fn parse_source_hf() {
    assert_eq!(
        parse_source("bartowski/Qwen2.5-3B-Instruct-GGUF:Q4_K_M").unwrap(),
        Source::Hf {
            repo: REPO.into(),
            quant: "Q4_K_M".into()
        }
    );
    assert_eq!(
        parse_source("  a/b:Q8_0 \n").unwrap(),
        Source::Hf {
            repo: "a/b".into(),
            quant: "Q8_0".into()
        }
    );
}

#[test]
fn parse_source_url() {
    let u = "https://huggingface.co/a/b/resolve/main/x.GGUF";
    assert_eq!(parse_source(u).unwrap(), Source::Url(u.into()));
    assert_eq!(
        parse_source(&format!("  {u}  ")).unwrap(),
        Source::Url(u.into())
    );
}

#[test]
fn parse_source_rejects_bad_input() {
    for s in [
        "http://huggingface.co/a/b/resolve/main/x.gguf",
        "https://huggingface.co/a/b/resolve/main/x.bin",
        "repo:Q4_K_M",
        "a/b",
        "a/b:",
        "../b:Q4",
        "a/..:Q4",
        "a/b/c:Q4",
        "a b/c:Q4",
        "",
        "   ",
    ] {
        assert!(parse_source(s).is_err(), "should reject {s:?}");
    }
}

fn tree() -> String {
    let f = |p: &str, size: u64, oid: &str| json!({ "type": "file", "oid": "x", "size": size - 1, "path": p, "lfs": { "oid": oid, "size": size, "pointerSize": 135 } });
    json!([
        f("Qwen2.5-3B-Instruct-Q4_K_M.gguf", 1929903264, &"9c".repeat(32)),
        f("Qwen2.5-3B-Instruct-Q4_K_S.gguf", 1800000000, &"aa".repeat(32)),
        f("Qwen2.5-3B-Instruct-Q8_0.gguf", 3300000000, &"bb".repeat(32)),
        f("mmproj-Qwen2.5-3B-Instruct-Q4_K_M.gguf", 5000, &"cc".repeat(32)),
        { "type": "file", "path": "README.md", "size": 100 },
        { "type": "directory", "path": "sub" }
    ])
    .to_string()
}

#[test]
fn pick_single_match() {
    let got = pick_files(&tree(), BASE, REPO, "Q4_K_M").unwrap();
    assert_eq!(
        got,
        vec![RemoteFile {
            name: "Qwen2.5-3B-Instruct-Q4_K_M.gguf".into(),
            url: format!("{BASE}/{REPO}/resolve/main/Qwen2.5-3B-Instruct-Q4_K_M.gguf"),
            size: Some(1929903264),
            sha256: Some("9c".repeat(32)),
        }]
    );
}

#[test]
fn pick_is_case_insensitive_and_skips_mmproj() {
    let got = pick_files(&tree(), BASE, REPO, "q4_k_m").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].name, "Qwen2.5-3B-Instruct-Q4_K_M.gguf");
}

#[test]
fn pick_split_model_orders_parts() {
    let j = json!([
        { "type": "file", "path": "sub/qwen-q4_k_m-00002-of-00002.gguf", "size": 20, "lfs": { "oid": "BB", "size": 22 } },
        { "type": "file", "path": "sub/qwen-q4_k_m-00001-of-00002.gguf", "size": 10, "lfs": { "oid": "AA", "size": 11 } },
        { "type": "file", "path": "qwen-q8_0.gguf", "size": 99 }
    ])
    .to_string();
    let got = pick_files(&j, BASE, "o/r", "Q4_K_M").unwrap();
    let names: Vec<_> = got.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "qwen-q4_k_m-00001-of-00002.gguf",
            "qwen-q4_k_m-00002-of-00002.gguf"
        ]
    );
    assert_eq!(
        got[0].url,
        format!("{BASE}/o/r/resolve/main/sub/qwen-q4_k_m-00001-of-00002.gguf")
    );
    assert_eq!(
        got[1].url,
        format!("{BASE}/o/r/resolve/main/sub/qwen-q4_k_m-00002-of-00002.gguf")
    );
    assert_eq!(got[0].size, Some(11));
}

#[test]
fn pick_ambiguous_lists_candidates() {
    let err = pick_files(&tree(), BASE, REPO, "Q4_K").unwrap_err();
    assert!(err.contains("Qwen2.5-3B-Instruct-Q4_K_M.gguf"), "{err}");
    assert!(err.contains("Qwen2.5-3B-Instruct-Q4_K_S.gguf"), "{err}");
}

#[test]
fn pick_no_match_and_bad_json_err() {
    assert!(pick_files(&tree(), BASE, REPO, "IQ2_XS").is_err());
    assert!(pick_files("not json", BASE, REPO, "Q4_K_M").is_err());
}

#[tokio::test]
async fn resolve_hf_uses_recursive_tree() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/models/{REPO}/tree/main")))
        .and(query_param("recursive", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tree()))
        .mount(&server)
        .await;
    let src = Source::Hf {
        repo: REPO.into(),
        quant: "Q4_K_M".into(),
    };
    let got = resolve(&reqwest::Client::new(), &server.uri(), &src)
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0].url,
        format!(
            "{}/{REPO}/resolve/main/Qwen2.5-3B-Instruct-Q4_K_M.gguf",
            server.uri()
        )
    );
}

#[tokio::test]
async fn resolve_url_source() {
    let u = "https://example.com/some/dir/Model-Q4.gguf";
    let got = resolve(
        &reqwest::Client::new(),
        "http://unused.invalid",
        &Source::Url(u.into()),
    )
    .await
    .unwrap();
    assert_eq!(
        got,
        vec![RemoteFile {
            name: "Model-Q4.gguf".into(),
            url: u.into(),
            size: None,
            sha256: None
        }]
    );
}

#[tokio::test]
async fn resolve_404_errs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let src = Source::Hf {
        repo: "a/b".into(),
        quant: "Q4".into(),
    };
    assert!(
        resolve(&reqwest::Client::new(), &server.uri(), &src)
            .await
            .is_err()
    );
}

#[test]
fn models_dir_is_data_models() {
    assert_eq!(
        models_dir(Path::new("/data")),
        Path::new("/data").join("models")
    );
}

fn write(dir: &Path, name: &str, n: usize) {
    std::fs::write(dir.join(name), vec![0u8; n]).unwrap();
}

#[test]
fn list_missing_dir_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(list_installed(&dir.path().join("nope")).is_empty());
}

#[test]
fn list_filters_and_sorts() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "b.gguf", 5);
    write(dir.path(), "a.gguf", 3);
    write(dir.path(), "c.gguf.part", 9);
    write(dir.path(), "notes.txt", 9);
    std::fs::create_dir(dir.path().join("dir.gguf")).unwrap();
    assert_eq!(
        list_installed(dir.path()),
        vec![
            InstalledModel {
                file: "a.gguf".into(),
                size: 3
            },
            InstalledModel {
                file: "b.gguf".into(),
                size: 5
            },
        ]
    );
}

#[test]
fn list_folds_split_parts() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "m-q4-00001-of-00002.gguf", 10);
    write(dir.path(), "m-q4-00002-of-00002.gguf", 7);
    assert_eq!(
        list_installed(dir.path()),
        vec![InstalledModel {
            file: "m-q4-00001-of-00002.gguf".into(),
            size: 17
        }]
    );
}

#[test]
fn delete_single_file_only() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.gguf", 1);
    write(dir.path(), "b.gguf", 1);
    delete_model(dir.path(), "a.gguf").unwrap();
    assert!(!dir.path().join("a.gguf").exists());
    assert!(dir.path().join("b.gguf").exists());
}

#[test]
fn delete_split_removes_all_parts() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "m-00001-of-00002.gguf", 1);
    write(dir.path(), "m-00002-of-00002.gguf", 1);
    write(dir.path(), "other.gguf", 1);
    delete_model(dir.path(), "m-00001-of-00002.gguf").unwrap();
    assert!(!dir.path().join("m-00001-of-00002.gguf").exists());
    assert!(!dir.path().join("m-00002-of-00002.gguf").exists());
    assert!(dir.path().join("other.gguf").exists());
}

#[test]
fn delete_rejects_bad_names_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("x.gguf");
    write(dir.path(), "x.gguf", 1);
    let inner = dir.path().join("models");
    std::fs::create_dir(&inner).unwrap();
    for bad in ["../x.gguf", "a/b.gguf", "a\\b.gguf", "notes.txt"] {
        assert!(delete_model(&inner, bad).is_err(), "should reject {bad:?}");
    }
    assert!(outside.exists());
    assert!(delete_model(&inner, "missing.gguf").is_err());
}

async fn serve(server: &MockServer, p: &str, body: &[u8]) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
        .mount(server)
        .await;
}

fn rf(server: &MockServer, name: &str, sha: Option<&str>) -> RemoteFile {
    RemoteFile {
        name: name.into(),
        url: format!("{}/{name}", server.uri()),
        size: Some(3),
        sha256: sha.map(String::from),
    }
}

fn dir_entries(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|r| {
            r.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn download_two_parts_ok() {
    let server = MockServer::start().await;
    serve(&server, "/m-00001-of-00002.gguf", b"abc").await;
    serve(&server, "/m-00002-of-00002.gguf", b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    let md = dir.path().join("models");
    let files = [
        rf(&server, "m-00001-of-00002.gguf", Some(ABC_SHA)),
        rf(&server, "m-00002-of-00002.gguf", None),
    ];
    let mut names: Vec<String> = Vec::new();

    let first = download_files(
        &reqwest::Client::new(),
        &md,
        &files,
        &AtomicBool::new(false),
        |n, _| {
            if names.last().map(String::as_str) != Some(n) {
                names.push(n.to_string());
            }
        },
    )
    .await
    .unwrap();

    assert_eq!(first, "m-00001-of-00002.gguf");
    assert_eq!(
        std::fs::read(md.join("m-00001-of-00002.gguf")).unwrap(),
        b"abc"
    );
    assert_eq!(
        std::fs::read(md.join("m-00002-of-00002.gguf")).unwrap(),
        b"abc"
    );
    // parts download in reverse so part 1 (the file the model is identified by) lands last
    assert_eq!(
        names,
        vec!["m-00002-of-00002.gguf", "m-00001-of-00002.gguf"]
    );
}

#[tokio::test]
async fn download_sha_mismatch_on_part2_removes_part1() {
    let server = MockServer::start().await;
    serve(&server, "/m-00001-of-00002.gguf", b"abc").await;
    serve(&server, "/m-00002-of-00002.gguf", b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    let md = dir.path().join("models");
    let wrong = "0".repeat(64);
    let files = [
        rf(&server, "m-00001-of-00002.gguf", Some(ABC_SHA)),
        rf(&server, "m-00002-of-00002.gguf", Some(&wrong)),
    ];

    let err = download_files(
        &reqwest::Client::new(),
        &md,
        &files,
        &AtomicBool::new(false),
        |_, _| {},
    )
    .await
    .unwrap_err();

    assert!(err.contains("checksum"), "{err}");
    assert!(dir_entries(&md).is_empty(), "left: {:?}", dir_entries(&md));
}

#[tokio::test]
async fn download_existing_first_file_errs_untouched() {
    let server = MockServer::start().await;
    serve(&server, "/a.gguf", b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.gguf"), "old").unwrap();

    let err = download_files(
        &reqwest::Client::new(),
        dir.path(),
        &[rf(&server, "a.gguf", None)],
        &AtomicBool::new(false),
        |_, _| {},
    )
    .await
    .unwrap_err();

    assert!(err.contains("already installed"), "{err}");
    assert_eq!(std::fs::read(dir.path().join("a.gguf")).unwrap(), b"old");
}

#[tokio::test]
async fn download_cancelled_leaves_nothing() {
    let server = MockServer::start().await;
    serve(&server, "/a.gguf", b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    let md = dir.path().join("models");

    let err = download_files(
        &reqwest::Client::new(),
        &md,
        &[rf(&server, "a.gguf", None)],
        &AtomicBool::new(true),
        |_, _| {},
    )
    .await
    .unwrap_err();

    assert_eq!(err, CANCELLED);
    assert!(dir_entries(&md).is_empty(), "left: {:?}", dir_entries(&md));
}

// ---- review fixes ---------------------------------------------------------------------------

fn lfs_file(p: &str) -> serde_json::Value {
    json!({ "type": "file", "path": p, "size": 10, "lfs": { "oid": "ab".repeat(32), "size": 10 } })
}

fn tree_of(paths: &[&str]) -> String {
    serde_json::Value::Array(paths.iter().map(|p| lfs_file(p)).collect()).to_string()
}

const BARTOWSKI: [&str; 6] = [
    "Qwen2.5-3B-Instruct-Q4_0.gguf",
    "Qwen2.5-3B-Instruct-Q4_0_4_4.gguf",
    "Qwen2.5-3B-Instruct-Q4_0_4_8.gguf",
    "Qwen2.5-3B-Instruct-Q4_0_8_8.gguf",
    "Qwen2.5-3B-Instruct-Q4_K_M.gguf",
    "Qwen2.5-3B-Instruct-Q4_K_S.gguf",
];

#[test]
fn pick_quant_prefers_whole_token_match() {
    let got = pick_files(&tree_of(&BARTOWSKI), BASE, REPO, "Q4_0").unwrap();
    let names: Vec<_> = got.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["Qwen2.5-3B-Instruct-Q4_0.gguf"]);
}

#[test]
fn pick_quant_token_also_matches_dot_separator_and_split_infix() {
    let dot = tree_of(&["m.Q4_0.gguf", "m.Q4_0_4_4.gguf"]);
    assert_eq!(
        pick_files(&dot, BASE, "o/r", "q4_0").unwrap()[0].name,
        "m.Q4_0.gguf"
    );

    let split = tree_of(&[
        "m-Q4_0-00001-of-00002.gguf",
        "m-Q4_0-00002-of-00002.gguf",
        "m-Q4_0_4_4.gguf",
    ]);
    let got = pick_files(&split, BASE, "o/r", "Q4_0").unwrap();
    let names: Vec<_> = got.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["m-Q4_0-00001-of-00002.gguf", "m-Q4_0-00002-of-00002.gguf"]
    );
}

#[test]
fn pick_partial_quant_stays_ambiguous() {
    let err = pick_files(&tree_of(&BARTOWSKI), BASE, REPO, "Q4_K").unwrap_err();
    assert!(err.contains("Q4_K_M") && err.contains("Q4_K_S"), "{err}");
}

#[test]
fn pick_refuses_windows_device_name() {
    let err = pick_files(&tree_of(&["aux.Q4_K_M.gguf"]), BASE, "o/r", "Q4_K_M");
    assert!(err.is_err(), "{err:?}");
}

#[test]
fn pick_percent_encodes_url_segments() {
    let got = pick_files(
        &tree_of(&["dir with space/x#1-Q4_K_M.gguf"]),
        BASE,
        "o/r",
        "Q4_K_M",
    )
    .unwrap();
    assert_eq!(got[0].name, "x#1-Q4_K_M.gguf");
    assert!(
        got[0]
            .url
            .ends_with("/resolve/main/dir%20with%20space/x%231-Q4_K_M.gguf"),
        "{}",
        got[0].url
    );
}

#[test]
fn pick_split_requires_exact_index_range() {
    let zero = tree_of(&["m-q4-00000-of-00002.gguf", "m-q4-00002-of-00002.gguf"]);
    assert!(pick_files(&zero, BASE, "o/r", "q4").is_err());
    let gap = tree_of(&["m-q4-00001-of-00002.gguf", "m-q4-00003-of-00002.gguf"]);
    assert!(pick_files(&gap, BASE, "o/r", "q4").is_err());
}

#[test]
fn parse_source_rejects_windows_device_url() {
    assert!(parse_source("https://huggingface.co/a/b/resolve/main/NUL.gguf").is_err());
}

#[tokio::test]
async fn resolve_errs_on_paginated_tree() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "Link",
                    "<https://huggingface.co/api/models/a/b/tree/main?cursor=x>; rel=\"next\"",
                )
                .set_body_string(tree()),
        )
        .mount(&server)
        .await;
    let src = Source::Hf {
        repo: REPO.into(),
        quant: "Q4_K_M".into(),
    };
    let err = resolve(&reqwest::Client::new(), &server.uri(), &src)
        .await
        .unwrap_err()
        .to_lowercase();
    assert!(
        err.contains("too large") || err.contains("too many files"),
        "{err}"
    );
}

#[test]
fn list_ignores_incomplete_split_groups() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lone1-00001-of-00002.gguf", 1);
    write(dir.path(), "lone2-00002-of-00002.gguf", 1);
    write(dir.path(), "gap-00001-of-00003.gguf", 1);
    write(dir.path(), "gap-00003-of-00003.gguf", 1);
    write(dir.path(), "ok-00001-of-00002.gguf", 4);
    write(dir.path(), "ok-00002-of-00002.gguf", 6);
    write(dir.path(), "single.gguf", 2);
    assert_eq!(
        list_installed(dir.path()),
        vec![
            InstalledModel {
                file: "ok-00001-of-00002.gguf".into(),
                size: 10
            },
            InstalledModel {
                file: "single.gguf".into(),
                size: 2
            },
        ]
    );
}

#[test]
fn delete_rejects_unsafe_names_even_when_the_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    for bad in [
        "NUL.gguf",
        "con.q4.gguf",
        "COM1.gguf",
        "lpt9.gguf",
        "a?b.gguf",
        "a%20b.gguf",
        "a<b.gguf",
        "a>b.gguf",
        "a\"b.gguf",
        "a|b.gguf",
        "a*b.gguf",
        "a\u{1}b.gguf",
        "a .gguf",
        "a..gguf",
    ] {
        // Windows refuses to create most of these names; the rejection must hold either way.
        let created = std::fs::write(dir.path().join(bad), [0u8]).is_ok();
        assert!(
            delete_model(dir.path(), bad).is_err(),
            "should reject {bad:?}"
        );
        if created {
            assert!(dir.path().join(bad).exists(), "{bad:?} must not be deleted");
        }
    }
}

fn requested(reqs: &[wiremock::Request]) -> Vec<String> {
    reqs.iter().map(|r| r.url.path().to_string()).collect()
}

#[tokio::test]
async fn download_fetches_parts_in_reverse_order() {
    let server = MockServer::start().await;
    serve(&server, "/m-00001-of-00002.gguf", b"abc").await;
    serve(&server, "/m-00002-of-00002.gguf", b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    let files = [
        rf(&server, "m-00001-of-00002.gguf", None),
        rf(&server, "m-00002-of-00002.gguf", None),
    ];

    let first = download_files(
        &reqwest::Client::new(),
        dir.path(),
        &files,
        &AtomicBool::new(false),
        |_, _| {},
    )
    .await
    .unwrap();

    assert_eq!(first, "m-00001-of-00002.gguf");
    assert_eq!(
        requested(&server.received_requests().await.unwrap()),
        vec!["/m-00002-of-00002.gguf", "/m-00001-of-00002.gguf"]
    );
}

#[tokio::test]
async fn download_part2_failure_leaves_nothing_and_never_fetches_part1() {
    let server = MockServer::start().await;
    serve(&server, "/m-00001-of-00002.gguf", b"abc").await;
    serve(&server, "/m-00002-of-00002.gguf", b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    let wrong = "0".repeat(64);
    let files = [
        rf(&server, "m-00001-of-00002.gguf", None),
        rf(&server, "m-00002-of-00002.gguf", Some(&wrong)),
    ];

    let err = download_files(
        &reqwest::Client::new(),
        dir.path(),
        &files,
        &AtomicBool::new(false),
        |_, _| {},
    )
    .await
    .unwrap_err();

    assert!(err.contains("checksum"), "{err}");
    assert!(
        dir_entries(dir.path()).is_empty(),
        "left: {:?}",
        dir_entries(dir.path())
    );
    assert_eq!(
        requested(&server.received_requests().await.unwrap()),
        vec!["/m-00002-of-00002.gguf"]
    );
}

#[tokio::test]
async fn download_part1_failure_removes_part2_and_part1_never_appears() {
    let server = MockServer::start().await;
    serve(&server, "/m-00001-of-00002.gguf", b"abc").await;
    serve(&server, "/m-00002-of-00002.gguf", b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    let wrong = "0".repeat(64);
    let files = [
        rf(&server, "m-00001-of-00002.gguf", Some(&wrong)),
        rf(&server, "m-00002-of-00002.gguf", None),
    ];

    let err = download_files(
        &reqwest::Client::new(),
        dir.path(),
        &files,
        &AtomicBool::new(false),
        |_, _| {},
    )
    .await
    .unwrap_err();

    assert!(err.contains("checksum"), "{err}");
    assert!(
        dir_entries(dir.path()).is_empty(),
        "left: {:?}",
        dir_entries(dir.path())
    );
}

#[tokio::test]
async fn download_redownloads_over_stale_incomplete_split() {
    for stale in ["m-00001-of-00002.gguf", "m-00002-of-00002.gguf"] {
        let server = MockServer::start().await;
        serve(&server, "/m-00001-of-00002.gguf", b"abc").await;
        serve(&server, "/m-00002-of-00002.gguf", b"abc").await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(stale), "old").unwrap();
        let files = [
            rf(&server, "m-00001-of-00002.gguf", None),
            rf(&server, "m-00002-of-00002.gguf", None),
        ];

        let first = download_files(
            &reqwest::Client::new(),
            dir.path(),
            &files,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .await
        .unwrap_or_else(|e| panic!("stale {stale}: {e}"));

        assert_eq!(first, "m-00001-of-00002.gguf");
        assert_eq!(
            std::fs::read(dir.path().join("m-00001-of-00002.gguf")).unwrap(),
            b"abc"
        );
        assert_eq!(
            std::fs::read(dir.path().join("m-00002-of-00002.gguf")).unwrap(),
            b"abc"
        );
    }
}

#[tokio::test]
async fn download_complete_split_is_already_installed() {
    let server = MockServer::start().await;
    serve(&server, "/m-00001-of-00002.gguf", b"abc").await;
    serve(&server, "/m-00002-of-00002.gguf", b"abc").await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m-00001-of-00002.gguf"), "old1").unwrap();
    std::fs::write(dir.path().join("m-00002-of-00002.gguf"), "old2").unwrap();
    let files = [
        rf(&server, "m-00001-of-00002.gguf", None),
        rf(&server, "m-00002-of-00002.gguf", None),
    ];

    let err = download_files(
        &reqwest::Client::new(),
        dir.path(),
        &files,
        &AtomicBool::new(false),
        |_, _| {},
    )
    .await
    .unwrap_err();

    assert!(err.contains("already installed"), "{err}");
    assert_eq!(
        std::fs::read(dir.path().join("m-00001-of-00002.gguf")).unwrap(),
        b"old1"
    );
    assert_eq!(
        std::fs::read(dir.path().join("m-00002-of-00002.gguf")).unwrap(),
        b"old2"
    );
}

#[test]
fn delete_rejects_empty_stem_even_when_the_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".gguf", 1);
    assert!(delete_model(dir.path(), ".gguf").is_err());
    assert!(dir.path().join(".gguf").exists());
}
