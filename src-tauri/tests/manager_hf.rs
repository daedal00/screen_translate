use screen_translate_lib::manager::models::{parse_search, pick_files, quant_files, search_url, HfRepo, QuantFile};
use serde_json::json;

const BASE: &str = "https://huggingface.co";
const REPO: &str = "bartowski/Qwen2.5-7B-Instruct-GGUF";

#[test]
fn search_url_encodes_query() {
    assert_eq!(
        search_url(BASE, "qwen 2.5/7b"),
        "https://huggingface.co/api/models?search=qwen%202.5%2F7b&filter=gguf&sort=downloads&direction=-1&limit=20"
    );
    assert_eq!(
        search_url("http://x", "a&b=c"),
        "http://x/api/models?search=a%26b%3Dc&filter=gguf&sort=downloads&direction=-1&limit=20"
    );
}

#[test]
fn parse_search_reads_ids_and_counts_in_order() {
    let j = json!([
        { "id": "a/one", "downloads": 100, "likes": 5, "tags": ["gguf"] },
        { "modelId": "b/two", "downloads": 7 },
        { "id": "c/three" }
    ])
    .to_string();
    assert_eq!(
        parse_search(&j).unwrap(),
        vec![
            HfRepo { id: "a/one".into(), downloads: 100, likes: 5 },
            HfRepo { id: "b/two".into(), downloads: 7, likes: 0 },
            HfRepo { id: "c/three".into(), downloads: 0, likes: 0 },
        ]
    );
}

#[test]
fn parse_search_empty_and_errors() {
    assert_eq!(parse_search("[]").unwrap(), vec![]);
    assert!(parse_search("{\"error\":\"x\"}").is_err());
    assert!(parse_search("not json").is_err());
    assert!(parse_search("\"str\"").is_err());
}

fn file(path: &str, size: u64) -> serde_json::Value {
    json!({ "type": "file", "path": path, "size": 1000, "lfs": { "oid": "ab".repeat(32), "size": size } })
}

fn tree() -> String {
    let mut v = vec![
        json!({ "type": "file", "path": "README.md", "size": 100 }),
        json!({ "type": "file", "path": ".gitattributes", "size": 10 }),
        json!({ "type": "directory", "path": "extra" }),
        file("Qwen2.5-7B-Instruct-IQ4_XS.gguf", 4_200_000_000),
        file("Qwen2.5-7B-Instruct-Q2_K.gguf", 3_000_000_000),
        file("Qwen2.5-7B-Instruct-Q3_K_M.gguf", 3_800_000_000),
        file("Qwen2.5-7B-Instruct-Q4_0.gguf", 4_400_000_000),
        file("Qwen2.5-7B-Instruct-Q4_0_4_4.gguf", 4_400_000_001),
        file("Qwen2.5-7B-Instruct-Q4_K_M.gguf", 4_680_000_000),
        file("Qwen2.5-7B-Instruct-Q6_K.gguf", 6_250_000_000),
        file("Qwen2.5-7B-Instruct-f16.gguf", 15_200_000_000),
        file("mmproj-Qwen2.5-7B-Instruct-f16.gguf", 600_000_000),
        file("Qwen2.5-7B-Instruct-Q8_0/Qwen2.5-7B-Instruct-Q8_0-00001-of-00002.gguf", 5_000_000_000),
        file("Qwen2.5-7B-Instruct-Q8_0/Qwen2.5-7B-Instruct-Q8_0-00002-of-00002.gguf", 3_000_000_000),
        // incomplete split set: part 2 of 3 is missing
        file("Qwen2.5-7B-Instruct-Q5_K_L-00001-of-00003.gguf", 2_000_000_000),
        file("Qwen2.5-7B-Instruct-Q5_K_L-00003-of-00003.gguf", 2_000_000_000),
    ];
    v.push(json!({ "type": "file", "path": "notes.txt", "size": 1 }));
    serde_json::Value::Array(v).to_string()
}

fn rows() -> Vec<QuantFile> {
    quant_files(&tree()).unwrap()
}

#[test]
fn quant_files_sorted_ascending_by_size() {
    let r = rows();
    assert!(r.windows(2).all(|w| w[0].size <= w[1].size), "{r:?}");
    assert_eq!(r.first().unwrap().quant.to_ascii_uppercase(), "Q2_K");
    assert_eq!(r.last().unwrap().quant.to_ascii_uppercase(), "F16");
}

#[test]
fn quant_files_skips_mmproj_non_gguf_and_incomplete_sets() {
    let r = rows();
    assert!(r.iter().all(|x| !x.name.contains("mmproj") && x.name.ends_with(".gguf")));
    assert!(r.iter().all(|x| !x.name.contains("Q5_K_L")));
    assert_eq!(r.len(), 9, "{r:?}");
}

#[test]
fn quant_files_collapses_split_set() {
    let r = rows();
    let q8: Vec<_> = r.iter().filter(|x| x.parts > 1).collect();
    assert_eq!(q8.len(), 1);
    assert_eq!(q8[0].name, "Qwen2.5-7B-Instruct-Q8_0-00001-of-00002.gguf");
    assert_eq!(q8[0].size, 8_000_000_000);
    assert_eq!(q8[0].parts, 2);
    assert_eq!(q8[0].quant, "Q8_0");
}

#[test]
fn quant_files_single_file_row() {
    let r = rows();
    let k = r.iter().find(|x| x.name == "Qwen2.5-7B-Instruct-Q4_K_M.gguf").unwrap();
    assert_eq!((k.quant.as_str(), k.size, k.parts), ("Q4_K_M", 4_680_000_000, 1));
}

#[test]
fn quant_tokens() {
    let r = rows();
    let q = |n: &str| r.iter().find(|x| x.name.contains(n)).unwrap().quant.clone();
    assert_eq!(q("IQ4_XS"), "IQ4_XS");
    assert_eq!(q("Q4_0_4_4"), "Q4_0_4_4");
    assert_eq!(q("Q3_K_M"), "Q3_K_M");
    assert_eq!(q("Q6_K"), "Q6_K");
    assert!(q("-f16").eq_ignore_ascii_case("F16"));
}

#[test]
fn quant_empty_when_no_token() {
    let t = serde_json::Value::Array(vec![file("model.gguf", 10)]).to_string();
    let r = quant_files(&t).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].quant, "");
}

#[test]
fn quant_files_bad_json_errs() {
    assert!(quant_files("not json").is_err());
}

#[test]
fn every_row_downloads_exactly_its_file_set() {
    let t = tree();
    for row in rows().into_iter().filter(|r| !r.quant.is_empty()) {
        let got = pick_files(&t, BASE, REPO, &row.quant).unwrap_or_else(|e| panic!("{}: {e}", row.quant));
        assert_eq!(got.len() as u32, row.parts, "{}", row.quant);
        assert_eq!(got[0].name, row.name, "{}", row.quant);
        assert_eq!(got.iter().filter_map(|f| f.size).sum::<u64>(), row.size, "{}", row.quant);
    }
}

/// The tricky repo: shared tokens, UD- prefix, no token, a split set whose part 1 name is the key.
fn tricky_tree() -> String {
    serde_json::Value::Array(vec![
        file("X-Q4_K_M.gguf", 4_000_000_000),
        file("X-abliterated-Q4_K_M.gguf", 4_100_000_000),
        file("X-Q4_K_XL.gguf", 5_000_000_000),
        file("X-UD-Q4_K_XL.gguf", 5_100_000_000),
        file("model.gguf", 1_000_000_000),
        file("X-Q8_0-00001-of-00002.gguf", 6_000_000_000),
        file("X-Q8_0-00002-of-00002.gguf", 2_000_000_000),
        file("weird name+1.gguf", 10),
    ])
    .to_string()
}

#[test]
fn quant_rows_are_all_downloadable_by_file_name() {
    let t = tricky_tree();
    let rows = quant_files(&t).unwrap();
    let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
    // the row whose name cannot be a download key is not offered
    assert!(!names.contains(&"weird name+1.gguf"), "{names:?}");
    assert_eq!(rows.len(), 6, "{names:?}");
    for row in rows {
        let got = pick_files(&t, BASE, REPO, &row.name).unwrap_or_else(|e| panic!("{}: {e}", row.name));
        assert_eq!(got.len() as u32, row.parts, "{}", row.name);
        assert_eq!(got[0].name, row.name, "{}", row.name);
        assert_eq!(got.iter().filter_map(|f| f.size).sum::<u64>(), row.size, "{}", row.name);
    }
}

#[test]
fn quant_tokens_for_ud_and_no_token() {
    let rows = quant_files(&tricky_tree()).unwrap();
    let q = |n: &str| rows.iter().find(|r| r.name == n).unwrap().quant.clone();
    assert_eq!(q("X-UD-Q4_K_XL.gguf"), "Q4_K_XL");
    assert_eq!(q("model.gguf"), "");
}

#[test]
fn exact_name_wins_over_token_match() {
    let t = tricky_tree();
    let got = pick_files(&t, BASE, REPO, "X-Q4_K_M.gguf").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].name, "X-Q4_K_M.gguf");
    // a bare token that matches several files still errors
    assert!(pick_files(&t, BASE, REPO, "Q4_K_M").is_err());
}
