use std::collections::HashSet;

use screen_translate_lib::manager::catalog::{by_file, entries, parse, recommend, required, Entry};
use screen_translate_lib::manager::hardware::{budget, Budget, Fit, Gpu, Hardware, Vendor, OVERHEAD};

const GB: u64 = 1_000_000_000;
const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn bundled_catalog_is_valid() {
    let all = entries();
    assert!(!all.is_empty());
    let ids: HashSet<_> = all.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids.len(), all.len(), "ids unique");
    for e in all {
        assert!(!e.id.is_empty() && !e.name.is_empty() && !e.prompt_template.is_empty(), "{}", e.id);
        assert!(e.file.ends_with(".gguf"), "{}", e.file);
        let parts: Vec<_> = e.repo.split('/').collect();
        assert!(parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && !p.contains(char::is_whitespace)), "{}", e.repo);
        assert!(e.size > 0, "{}", e.id);
        if let Some(s) = &e.sha256 {
            assert!(s.len() == 64 && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)), "{}: {s}", e.id);
        }
    }
}

#[test]
fn parse_rejects_bad_json() {
    assert!(parse("not json").is_err());
    assert!(parse("{}").is_err());
    assert_eq!(parse("[]").unwrap().len(), 0);
}

#[test]
fn parse_roundtrips_an_entry() {
    let j = r#"[{"id":"a","name":"A","description":"d","repo":"o/r","file":"a.gguf","size":5,"sha256":null,"min_vram":0,"tags":["x"],"prompt_template":"p"}]"#;
    let v = parse(j).unwrap();
    assert_eq!(v[0].id, "a");
    assert_eq!(v[0].sha256, None);
    assert_eq!(v[0].tags, vec!["x"]);
    assert_eq!(v[0].note, None);
}

#[test]
fn parse_reads_note() {
    let j = r#"[{"id":"a","name":"A","description":"d","repo":"o/r","file":"a.gguf","size":5,"sha256":null,"min_vram":0,"tags":[],"prompt_template":"p","note":"hello"}]"#;
    assert_eq!(parse(j).unwrap()[0].note.as_deref(), Some("hello"));
}

fn e(id: &str, size: u64, min_vram: u64) -> Entry {
    Entry {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        repo: "o/r".into(),
        file: format!("{id}.gguf"),
        size,
        sha256: None,
        min_vram,
        tags: vec![],
        prompt_template: "p".into(),
        note: None,
    }
}

fn sample() -> Vec<Entry> {
    // Catalog order is the quality rank: s9 is best, s20 is last.
    vec![e("s9", 9 * GB, 0), e("s5", 5 * GB, 0), e("s4", 4 * GB, 0), e("s2", 2 * GB, 0), e("s20", 20 * GB, 0)]
}

fn gpu(vendor: Vendor, vram: u64) -> Gpu {
    Gpu { name: "g".into(), vendor, vram, driver: None, max_cuda: None }
}

fn ids(r: &[screen_translate_lib::manager::catalog::Recommendation]) -> Vec<&str> {
    r.iter().map(|x| x.entry.id.as_str()).collect()
}

#[test]
fn required_is_max_of_size_plus_overhead_and_min_vram() {
    assert_eq!(required(&e("a", 1000, 0)), 1000 + OVERHEAD);
    assert_eq!(required(&e("a", 1000, 9_000_000_000)), 9_000_000_000);
    assert_eq!(required(&e("a", 1000, 1)), 1000 + OVERHEAD);
}

#[test]
fn recommend_8gb_discrete_16gb_ram() {
    let b = budget(&Hardware { gpu: Some(gpu(Vendor::Nvidia, 8 * GIB)), ram: 16 * GIB, unified: false });
    // gpu 8 GiB = 8.59 GB, total 16 GiB = 17.18 GB; required = size + 1.5 GB
    let r = recommend(&sample(), &b, 10);
    assert_eq!(ids(&r), vec!["s5", "s4", "s2", "s9"]);
    assert_eq!(r[0].fit, Fit::Full);
    assert_eq!(r[3].fit, Fit::Partial);
    assert_eq!(r.iter().map(|x| x.fit).collect::<Vec<_>>(), vec![Fit::Full, Fit::Full, Fit::Full, Fit::Partial]);
}

#[test]
fn recommend_limit_applies_after_ordering() {
    let b = budget(&Hardware { gpu: Some(gpu(Vendor::Nvidia, 8 * GIB)), ram: 16 * GIB, unified: false });
    assert_eq!(ids(&recommend(&sample(), &b, 2)), vec!["s5", "s4"]);
    assert!(recommend(&sample(), &b, 0).is_empty());
}

#[test]
fn recommend_badges_full() {
    let b = budget(&Hardware { gpu: Some(gpu(Vendor::Nvidia, 8 * GIB)), ram: 16 * GIB, unified: false });
    let r = recommend(&sample(), &b, 3);
    assert_eq!(r[0].badges, vec!["Best quality", "Fits your GPU"]);
    assert_eq!(r[1].badges, vec!["Fits your GPU"]);
    assert_eq!(r[2].badges, vec!["Fastest", "Fits your GPU"]);
}

#[test]
fn recommend_single_result_has_no_fastest_badge() {
    let b = Budget { gpu: 8 * GB, total: 8 * GB };
    let r = recommend(&[e("only", 2 * GB, 0)], &b, 5);
    assert_eq!(r[0].badges, vec!["Best quality", "Fits your GPU"]);
}

#[test]
fn recommend_4gb_discrete() {
    let b = budget(&Hardware { gpu: Some(gpu(Vendor::Amd, 4 * GIB)), ram: 16 * GIB, unified: false });
    // gpu 4.29 GB -> s2 (3.5 GB) full; s4 (5.5) partial; s5 (6.5) partial; s9 (10.5) partial; s20 (21.5) too big (total 12.9)
    let r = recommend(&sample(), &b, 10);
    assert_eq!(ids(&r), vec!["s2", "s9", "s5", "s4"]);
    assert_eq!(r[1].badges, vec!["Best quality", "Partly on CPU, slower"]);
    assert_eq!(r[0].badges, vec!["Fastest", "Fits your GPU"]);
    assert_eq!(r[3].badges, vec!["Partly on CPU, slower"]);
}

#[test]
fn recommend_cpu_only_16gb() {
    let b = budget(&Hardware { gpu: None, ram: 16 * GIB, unified: false });
    // total 8 GiB = 8.59 GB; s5 (6.5) and below fit; s9 (10.5) does not
    let r = recommend(&sample(), &b, 10);
    // CPU only: smaller is faster, so the smallest comes first
    assert_eq!(ids(&r), vec!["s2", "s4", "s5"]);
    assert!(r.iter().all(|x| x.fit == Fit::Partial));
    assert_eq!(r[0].badges, vec!["Fastest", "Runs on CPU, slower"]);
    assert_eq!(r[1].badges, vec!["Runs on CPU, slower"]);
    assert_eq!(r[2].badges, vec!["Best quality", "Runs on CPU, slower"]);
}

#[test]
fn recommend_16gb_mac() {
    let b = budget(&Hardware { gpu: Some(gpu(Vendor::Apple, 16 * GIB)), ram: 16 * GIB, unified: true });
    // 65% of 17.18 GB = 11.17 GB: s9 (10.5) fits fully, s20 does not
    let r = recommend(&sample(), &b, 10);
    assert_eq!(ids(&r), vec!["s9", "s5", "s4", "s2"]);
    assert!(r.iter().all(|x| x.fit == Fit::Full));
    assert_eq!(r[0].badges, vec!["Best quality", "Fits your GPU"]);
}

#[test]
fn recommend_tiny_machine_is_empty() {
    let b = budget(&Hardware { gpu: None, ram: 2 * GIB, unified: false });
    assert!(recommend(&sample(), &b, 10).is_empty());
}

#[test]
fn recommend_min_vram_pushes_entry_out_of_full() {
    let b = Budget { gpu: 8 * GB, total: 12 * GB };
    let r = recommend(&[e("small", 2 * GB, 0), e("hungry", 2 * GB, 10 * GB)], &b, 5);
    assert_eq!(ids(&r), vec!["small", "hungry"]);
    assert_eq!((r[0].fit, r[1].fit), (Fit::Full, Fit::Partial));
}

#[test]
fn recommend_works_on_bundled_catalog() {
    let b = Budget { gpu: 1000 * GB, total: 1000 * GB };
    assert_eq!(recommend(entries(), &b, 100).len(), entries().len());
}

#[test]
fn recommend_best_quality_is_lowest_catalog_index_not_largest() {
    let b = Budget { gpu: 100 * GB, total: 100 * GB };
    let r = recommend(&[e("first-small", 2 * GB, 0), e("second-big", 9 * GB, 0), e("third", 5 * GB, 0)], &b, 10);
    assert_eq!(ids(&r), vec!["first-small", "second-big", "third"]);
    assert_eq!(r[0].badges, vec!["Best quality", "Fits your GPU"]);
    assert_eq!(r[1].badges, vec!["Fits your GPU"]);
    assert_eq!(r[2].badges, vec!["Fits your GPU"]);
}

#[test]
fn recommend_fastest_is_smallest_when_not_best() {
    let b = Budget { gpu: 100 * GB, total: 100 * GB };
    let r = recommend(&[e("a", 9 * GB, 0), e("b", 2 * GB, 0), e("c", 5 * GB, 0)], &b, 10);
    assert_eq!(r[0].badges, vec!["Best quality", "Fits your GPU"]);
    assert_eq!(r[1].badges, vec!["Fastest", "Fits your GPU"]);
    assert_eq!(r[2].badges, vec!["Fits your GPU"]);
}

#[test]
fn bundled_catalog_has_the_four_benchmark_models_in_order() {
    let all = entries();
    assert_eq!(all.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["gemma-4-12b", "hy-mt2-7b", "hy-mt2-1.8b", "qwen3-8b"]);
    // (id, name, repo, file, size, sha256, min_vram, prompt_template)
    let want = [
        ("gemma-4-12b", "Gemma 4 12B (QAT)", "google/gemma-4-12B-it-qat-q4_0-gguf", "gemma-4-12b-it-qat-q4_0.gguf", 6975879296u64, "93567e57a8fe10b23569b9d9ec38cd005deedf71e29477c421a4b83f418a538b", 9_500_000_000u64, "default"),
        ("hy-mt2-7b", "Hy-MT2 7B", "tencent/Hy-MT2-7B-GGUF", "Hy-MT2-7B-Q4_K_M.gguf", 4624648896, "9f96256500f3fc1ab4d64336b58f52a949a95ad7516b0c229476eef782f9f77b", 6_200_000_000, "hy-mt"),
        ("hy-mt2-1.8b", "Hy-MT2 1.8B", "tencent/Hy-MT2-1.8B-GGUF", "Hy-MT2-1.8B-Q4_K_M.gguf", 1133080448, "dc5f44fcf1fa496ee7ad725982c0c8c553a4de00259b53af84c4b89fb0c06699", 2_700_000_000, "hy-mt"),
        ("qwen3-8b", "Qwen3 8B", "Qwen/Qwen3-8B-GGUF", "Qwen3-8B-Q4_K_M.gguf", 5027783488, "d98cdcbd03e17ce47681435b5150e34c1417f50b5c0019dd560e4882c5745785", 6_600_000_000, "default"),
    ];
    for (e, (id, name, repo, file, size, sha, min_vram, tpl)) in all.iter().zip(want) {
        assert_eq!(e.id, id);
        assert_eq!(e.name, name, "{id}");
        assert_eq!(e.repo, repo, "{id}");
        assert_eq!(e.file, file, "{id}");
        assert_eq!(e.size, size, "{id}");
        assert_eq!(e.sha256.as_deref(), Some(sha), "{id}");
        assert_eq!(e.min_vram, min_vram, "{id}");
        assert_eq!(e.prompt_template, tpl, "{id}");
        assert!(!e.description.is_empty() && !e.tags.is_empty(), "{id}");
    }
}

#[test]
fn bundled_catalog_has_no_notes() {
    // Hy-MT2 GGUF repos are Apache-2.0, so no license caveat is needed.
    for e in entries() {
        assert_eq!(e.note, None, "{}", e.id);
    }
}

#[test]
fn bundled_catalog_has_no_translategemma_or_qwen25() {
    for e in entries() {
        let s = format!("{} {} {}", e.id, e.name, e.file).to_lowercase();
        assert!(!s.contains("translategemma") && !s.contains("qwen2.5"), "{s}");
    }
}

#[test]
fn note_is_serialized_to_the_ui() {
    let j = r#"[{"id":"a","name":"A","description":"d","repo":"o/r","file":"a.gguf","size":5,"sha256":null,"min_vram":0,"tags":[],"prompt_template":"p","note":"hello"}]"#;
    let v: Vec<Entry> = serde_json::from_str(j).unwrap();
    assert_eq!(serde_json::to_value(&v[0]).unwrap()["note"], "hello");
    assert!(serde_json::to_value(&entries()[0]).unwrap()["note"].is_null());
}

#[test]
fn by_file_hit_and_miss() {
    assert_eq!(by_file(entries(), "Hy-MT2-7B-Q4_K_M.gguf").unwrap().id, "hy-mt2-7b");
    assert_eq!(by_file(entries(), "Qwen3-8B-Q4_K_M.gguf").unwrap().id, "qwen3-8b");
    assert!(by_file(entries(), "hy-mt2-7b-q4_k_m.gguf").is_none(), "exact match only");
    assert!(by_file(entries(), "nope.gguf").is_none());
    assert!(by_file(&[], "Hy-MT2-7B-Q4_K_M.gguf").is_none());
}

fn real(h: Hardware) -> Vec<(String, Vec<String>)> {
    recommend(entries(), &budget(&h), 3).into_iter().map(|r| (r.entry.id, r.badges)).collect()
}

fn pair(id: &str, badges: &[&str]) -> (String, Vec<String>) {
    (id.to_string(), badges.iter().map(|s| s.to_string()).collect())
}

#[test]
fn real_catalog_windows_nvidia_8gb() {
    let got = real(Hardware { gpu: Some(gpu(Vendor::Nvidia, 8 * GIB)), ram: 16 * GIB, unified: false });
    assert_eq!(
        got,
        vec![
            pair("hy-mt2-7b", &["Best quality", "Fits your GPU"]),
            pair("hy-mt2-1.8b", &["Fastest", "Fits your GPU"]),
            pair("qwen3-8b", &["Fits your GPU"]),
        ]
    );
}

#[test]
fn real_catalog_mac_unified_16gb() {
    let got = real(Hardware { gpu: Some(gpu(Vendor::Apple, 16 * GIB)), ram: 16 * GIB, unified: true });
    assert_eq!(
        got,
        vec![
            pair("gemma-4-12b", &["Best quality", "Fits your GPU"]),
            pair("hy-mt2-7b", &["Fits your GPU"]),
            pair("hy-mt2-1.8b", &["Fastest", "Fits your GPU"]),
        ]
    );
}

#[test]
fn real_catalog_cpu_only_16gb() {
    let got = real(Hardware { gpu: None, ram: 16 * GIB, unified: false });
    assert_eq!(
        got,
        vec![
            pair("hy-mt2-1.8b", &["Fastest", "Runs on CPU, slower"]),
            pair("hy-mt2-7b", &["Best quality", "Runs on CPU, slower"]),
            pair("qwen3-8b", &["Runs on CPU, slower"]),
        ]
    );
}
