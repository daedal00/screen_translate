use std::ffi::OsString;
use std::path::Path;

use screen_translate_lib::manager::runtime::{
    cuda_variants, fetch_release, find_server, parse_releases, pick_release, runtime_dir, select_assets, server_exe_name,
    Asset, Backend, CudaVer, Release,
};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const TAG: &str = "b12627";
const BASE: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b12627";

const NAMES: [&str; 9] = [
    "llama-b12627-bin-macos-arm64.tar.gz",
    "llama-b12627-bin-macos-x64.tar.gz",
    "llama-b12627-bin-win-cpu-x64.zip",
    "llama-b12627-bin-win-vulkan-x64.zip",
    "llama-b12627-bin-win-cuda-12.4-x64.zip",
    "llama-b12627-bin-win-cuda-13.5-x64.zip",
    "cudart-llama-bin-win-cuda-12.4-x64.zip",
    "cudart-llama-bin-win-cuda-13.5-x64.zip",
    "llama-b12627-bin-win-vulkan-arm64.zip",
];

fn releases_json() -> String {
    let assets: Vec<_> = NAMES
        .iter()
        .map(|n| json!({ "name": n, "browser_download_url": format!("{BASE}/{n}"), "size": 1234, "digest": format!("sha256:{}", "AB".repeat(32)) }))
        .collect();
    json!([
        { "tag_name": "nightly", "assets": [{ "name": "x.zip", "browser_download_url": "https://e/x.zip", "size": 1, "digest": null }] },
        { "tag_name": TAG, "assets": assets },
        { "tag_name": "b12000", "assets": [] }
    ])
    .to_string()
}

fn release() -> Release {
    parse_releases(&releases_json()).unwrap().remove(0)
}

#[test]
fn parse_releases_returns_all_build_tags_in_order_skipping_others() {
    let all = parse_releases(&releases_json()).unwrap();
    let tags: Vec<_> = all.iter().map(|r| r.tag.as_str()).collect();
    assert_eq!(tags, vec![TAG, "b12000"]);
    assert_eq!(all[0].assets.len(), NAMES.len());
}

#[test]
fn parse_releases_asset_fields() {
    let r = release();
    let a = &r.assets[0];
    assert_eq!(a.name, NAMES[0]);
    assert_eq!(a.url, format!("{BASE}/{}", NAMES[0]));
    assert_eq!(a.size, 1234);
    assert_eq!(a.sha256.as_deref(), Some("ab".repeat(32).as_str()));
}

#[test]
fn parse_releases_null_digest_is_none() {
    let j = json!([{ "tag_name": "b1", "assets": [{ "name": "a", "browser_download_url": "https://e/a", "size": 5, "digest": null }] }]).to_string();
    let r = parse_releases(&j).unwrap();
    assert_eq!(r[0].assets[0].sha256, None);
}

#[test]
fn parse_releases_invalid_json_errs() {
    assert!(parse_releases("not json").is_err());
}

#[test]
fn parse_releases_no_matching_release_errs() {
    let j = json!([{ "tag_name": "nightly", "assets": [] }, { "tag_name": "v1.0", "assets": [] }]).to_string();
    assert!(parse_releases(&j).is_err());
    assert!(parse_releases("[]").is_err());
}

fn cv(major: u32, minor: u32) -> CudaVer {
    CudaVer { major, minor }
}

fn names(v: &[Asset]) -> Vec<&str> {
    v.iter().map(|a| a.name.as_str()).collect()
}

#[test]
fn select_macos_arm64_ignores_backend() {
    for b in [Backend::Cpu, Backend::Vulkan, Backend::Cuda] {
        let got = select_assets(&release(), "macos", "aarch64", b, None).unwrap();
        assert_eq!(names(&got), vec!["llama-b12627-bin-macos-arm64.tar.gz"]);
    }
}

#[test]
fn select_macos_x64() {
    let got = select_assets(&release(), "macos", "x86_64", Backend::Cpu, None).unwrap();
    assert_eq!(names(&got), vec!["llama-b12627-bin-macos-x64.tar.gz"]);
}

#[test]
fn select_windows_cpu_and_vulkan() {
    let cpu = select_assets(&release(), "windows", "x86_64", Backend::Cpu, None).unwrap();
    assert_eq!(names(&cpu), vec!["llama-b12627-bin-win-cpu-x64.zip"]);
    let vk = select_assets(&release(), "windows", "x86_64", Backend::Vulkan, None).unwrap();
    assert_eq!(names(&vk), vec!["llama-b12627-bin-win-vulkan-x64.zip"]);
}

#[test]
fn select_windows_cuda_returns_llama_then_cudart() {
    let got = select_assets(&release(), "windows", "x86_64", Backend::Cuda, Some(cv(12, 4))).unwrap();
    assert_eq!(
        names(&got),
        vec!["llama-b12627-bin-win-cuda-12.4-x64.zip", "cudart-llama-bin-win-cuda-12.4-x64.zip"]
    );
}

#[test]
fn select_unsupported_platforms_err() {
    assert!(select_assets(&release(), "windows", "aarch64", Backend::Cpu, None).is_err());
    assert!(select_assets(&release(), "linux", "x86_64", Backend::Cpu, None).is_err());
}

#[test]
fn select_missing_asset_errs() {
    let mut r = release();
    r.assets.retain(|a| !a.name.contains("macos-arm64"));
    let err = select_assets(&r, "macos", "aarch64", Backend::Cpu, None).unwrap_err();
    assert!(err.contains("macos-arm64"), "{err}");
}

#[test]
fn exe_name_per_os() {
    assert_eq!(server_exe_name("windows"), "llama-server.exe");
    assert_eq!(server_exe_name("macos"), "llama-server");
    assert_eq!(server_exe_name("linux"), "llama-server");
}

#[test]
fn runtime_dir_is_data_llama() {
    assert_eq!(runtime_dir(Path::new("/data")), Path::new("/data").join("llama"));
}

fn touch(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"x").unwrap();
}

// os "linux" is used wherever nothing should be found, so the macOS homebrew
// fallbacks (/opt/homebrew/bin, /usr/local/bin) cannot hit a real install on a dev machine.

#[test]
fn find_server_in_nested_runtime_bin() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("bin/a/b/llama-server");
    touch(&exe);
    assert_eq!(find_server(dir.path(), None, "linux"), Some(exe));
}

#[test]
fn find_server_windows_exe_name() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("bin/llama-server.exe");
    touch(&exe);
    assert_eq!(find_server(dir.path(), None, "windows"), Some(exe));
    assert_eq!(find_server(dir.path(), None, "linux"), None);
}

#[test]
fn find_server_prefers_runtime_over_path() {
    let rt = tempfile::tempdir().unwrap();
    let pathdir = tempfile::tempdir().unwrap();
    let in_rt = rt.path().join("bin/llama-server");
    touch(&in_rt);
    touch(&pathdir.path().join("llama-server"));
    let pv = std::env::join_paths([pathdir.path()]).unwrap();
    assert_eq!(find_server(rt.path(), Some(&pv), "linux"), Some(in_rt));
}

#[test]
fn find_server_falls_back_to_path_entry() {
    let rt = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let pathdir = tempfile::tempdir().unwrap();
    let exe = pathdir.path().join("llama-server");
    touch(&exe);
    let pv: OsString = std::env::join_paths([empty.path(), pathdir.path()]).unwrap();
    assert_eq!(find_server(rt.path(), Some(&pv), "linux"), Some(exe));
}

#[test]
fn find_server_ignores_directory_named_like_server() {
    let rt = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(rt.path().join("bin/llama-server")).unwrap();
    assert_eq!(find_server(rt.path(), None, "linux"), None);
}

#[test]
fn find_server_none_when_nothing_exists() {
    let rt = tempfile::tempdir().unwrap();
    let pathdir = tempfile::tempdir().unwrap();
    let pv = std::env::join_paths([pathdir.path()]).unwrap();
    assert_eq!(find_server(rt.path(), Some(&pv), "linux"), None);
    assert_eq!(find_server(&rt.path().join("missing"), None, "linux"), None);
}

#[cfg(target_os = "macos")]
mod unpack_tests {
    use super::*;
    use screen_translate_lib::manager::runtime::unpack;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    fn make_src(root: &Path) -> std::path::PathBuf {
        let src = root.join("src");
        let exe = src.join("sub/llama-server");
        touch(&exe);
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        src
    }

    fn assert_unpacked(dest: &Path) {
        let exe = dest.join("sub/llama-server");
        assert!(exe.is_file(), "missing {exe:?}");
        assert_ne!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o111, 0, "not executable");
    }

    #[test]
    fn unpack_tar_gz() {
        let dir = tempfile::tempdir().unwrap();
        let src = make_src(dir.path());
        let archive = dir.path().join("a.tar.gz");
        let st = Command::new("/usr/bin/tar").arg("-czf").arg(&archive).arg("-C").arg(&src).arg(".").status().unwrap();
        assert!(st.success());
        let dest = dir.path().join("out/new");
        unpack(&archive, &dest).unwrap();
        assert_unpacked(&dest);
    }

    #[test]
    fn unpack_zip() {
        let dir = tempfile::tempdir().unwrap();
        let src = make_src(dir.path());
        let archive = dir.path().join("a.zip");
        let st = Command::new("/usr/bin/ditto").args(["-c", "-k"]).arg(&src).arg(&archive).status().unwrap();
        assert!(st.success());
        let dest = dir.path().join("out");
        unpack(&archive, &dest).unwrap();
        assert_unpacked(&dest);
    }

    #[test]
    fn unpack_non_archive_errs() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.tar.gz");
        std::fs::write(&bad, "definitely not an archive").unwrap();
        assert!(unpack(&bad, &dir.path().join("out")).is_err());
    }
}

#[tokio::test]
async fn fetch_release_parses_and_sends_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(releases_json()))
        .mount(&server)
        .await;

    let r = fetch_release(&reqwest::Client::new(), &format!("{}/releases", server.uri())).await.unwrap();
    assert_eq!(r[0].tag, TAG);

    let reqs = server.received_requests().await.unwrap();
    let ua = reqs[0].headers.get("user-agent").expect("User-Agent header").to_str().unwrap();
    assert!(!ua.is_empty());
}

#[tokio::test]
async fn fetch_release_500_errs() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    assert!(fetch_release(&reqwest::Client::new(), &server.uri()).await.is_err());
}

// ---- pick_release -------------------------------------------------------------------------

fn rel(tag: &str, names: &[&str], sha: Option<&str>, base: &str) -> Release {
    Release {
        tag: tag.into(),
        assets: names
            .iter()
            .map(|n| Asset { name: (*n).into(), url: format!("{base}/{n}"), sha256: sha.map(String::from), size: 1 })
            .collect(),
    }
}

fn names_for(tag: &str) -> Vec<String> {
    NAMES.iter().map(|n| n.replace("b12627", tag)).collect()
}

fn full(tag: &str) -> Release {
    let owned = names_for(tag);
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    rel(tag, &refs, Some(&"ab".repeat(32)), &format!("https://github.com/x/releases/download/{tag}"))
}

#[test]
fn pick_release_uses_newest_when_it_qualifies() {
    let got = pick_release(&[full("b2"), full("b1")], "macos", "aarch64", Backend::Cpu, None).unwrap();
    assert_eq!(names(&got), vec!["llama-b2-bin-macos-arm64.tar.gz"]);
}

#[test]
fn pick_release_falls_back_when_newest_misses_an_asset() {
    let mut newest = full("b2");
    newest.assets.retain(|a| !a.name.starts_with("cudart"));
    let got = pick_release(&[newest, full("b1")], "windows", "x86_64", Backend::Cuda, Some(cv(12, 4))).unwrap();
    assert_eq!(
        names(&got),
        vec!["llama-b1-bin-win-cuda-12.4-x64.zip", "cudart-llama-bin-win-cuda-12.4-x64.zip"]
    );
}

#[test]
fn pick_release_falls_back_when_newest_has_null_digest() {
    let mut newest = full("b2");
    for a in &mut newest.assets {
        a.sha256 = None;
    }
    let got = pick_release(&[newest, full("b1")], "macos", "x86_64", Backend::Cpu, None).unwrap();
    assert_eq!(names(&got), vec!["llama-b1-bin-macos-x64.tar.gz"]);
}

#[test]
fn pick_release_falls_back_when_newest_url_is_not_https() {
    let mut newest = full("b2");
    for a in &mut newest.assets {
        a.url = a.url.replace("https://", "http://");
    }
    let got = pick_release(&[newest, full("b1")], "macos", "aarch64", Backend::Cpu, None).unwrap();
    assert_eq!(names(&got), vec!["llama-b1-bin-macos-arm64.tar.gz"]);
}

#[test]
fn pick_release_errs_when_none_qualifies() {
    let mut a = full("b2");
    let mut b = full("b1");
    a.assets.clear();
    for x in &mut b.assets {
        x.sha256 = None;
    }
    assert!(pick_release(&[a, b], "macos", "aarch64", Backend::Cpu, None).is_err());
    assert!(pick_release(&[], "macos", "aarch64", Backend::Cpu, None).is_err());
}

#[test]
fn pick_release_unsupported_platform_errs() {
    assert!(pick_release(&[full("b2")], "linux", "x86_64", Backend::Cpu, None).is_err());
}

// ---- find_server hardening ----------------------------------------------------------------

#[cfg(unix)]
#[test]
fn find_server_ignores_symlink_in_runtime_bin() {
    use std::os::unix::fs::symlink;
    let rt = tempfile::tempdir().unwrap();
    let real_dir = tempfile::tempdir().unwrap();
    let real = real_dir.path().join("real-server");
    touch(&real);
    std::fs::create_dir_all(rt.path().join("bin")).unwrap();
    symlink(&real, rt.path().join("bin/llama-server")).unwrap();

    assert_eq!(find_server(rt.path(), None, "linux"), None);
}

#[cfg(unix)]
#[test]
fn find_server_accepts_symlink_to_file_on_path() {
    use std::os::unix::fs::symlink;
    let rt = tempfile::tempdir().unwrap();
    let real_dir = tempfile::tempdir().unwrap();
    let real = real_dir.path().join("real-server");
    touch(&real);
    let pathdir = tempfile::tempdir().unwrap();
    let link = pathdir.path().join("llama-server");
    symlink(&real, &link).unwrap();
    let pv = std::env::join_paths([pathdir.path()]).unwrap();

    // Homebrew installs llama-server as a symlink into the Cellar. The link path or its target are both fine.
    let got = find_server(rt.path(), Some(&pv), "linux").expect("symlink to a regular file on PATH");
    assert!(got == link || got == real, "{got:?}");
}

#[cfg(unix)]
#[test]
fn find_server_ignores_path_symlink_to_directory_or_dangling() {
    use std::os::unix::fs::symlink;
    let rt = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();

    let dir_target = elsewhere.path().join("some-dir");
    std::fs::create_dir(&dir_target).unwrap();
    let to_dir = tempfile::tempdir().unwrap();
    symlink(&dir_target, to_dir.path().join("llama-server")).unwrap();

    let dangling = tempfile::tempdir().unwrap();
    symlink(elsewhere.path().join("missing"), dangling.path().join("llama-server")).unwrap();

    let pv = std::env::join_paths([to_dir.path(), dangling.path()]).unwrap();
    assert_eq!(find_server(rt.path(), Some(&pv), "linux"), None);
}

// ---- install_assets -----------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod install {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::atomic::AtomicBool;

    use screen_translate_lib::manager::download::sha256_file;
    use screen_translate_lib::manager::runtime::install_assets;
    use wiremock::matchers::path as url_path;

    /// A tar.gz holding `pkg/<files>`; `with_server` adds an executable `pkg/bin/llama-server`.
    fn archive(dir: &Path, with_server: bool) -> std::path::PathBuf {
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("pkg/bin")).unwrap();
        std::fs::write(src.join("pkg/new-marker"), "n").unwrap();
        if with_server {
            let exe = src.join("pkg/bin/llama-server");
            std::fs::write(&exe, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let out = dir.join("a.tar.gz");
        let st = Command::new("/usr/bin/tar").arg("-czf").arg(&out).arg("-C").arg(&src).arg(".").status().unwrap();
        assert!(st.success());
        out
    }

    async fn serve(archive: &Path) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(url_path("/a.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(std::fs::read(archive).unwrap()))
            .mount(&server)
            .await;
        server
    }

    fn asset(server: &MockServer, sha: Option<String>) -> Asset {
        Asset { name: "a.tar.gz".into(), url: format!("{}/a.tar.gz", server.uri()), sha256: sha, size: 1 }
    }

    fn assert_no_leftovers(rt: &Path) {
        for d in ["downloads", "bin.new", "bin.old"] {
            assert!(!rt.join(d).exists(), "{d} should not exist");
        }
    }

    fn old_bin(rt: &Path) {
        std::fs::create_dir_all(rt.join("bin")).unwrap();
        std::fs::write(rt.join("bin/old-marker"), "o").unwrap();
    }

    fn assert_old_bin_intact(rt: &Path) {
        assert!(rt.join("bin/old-marker").is_file(), "old bin must be intact");
        assert!(!rt.join("bin/new-marker").exists() && !rt.join("bin/pkg").exists());
    }

    #[tokio::test]
    async fn fresh_install() {
        let dir = tempfile::tempdir().unwrap();
        let arc = archive(dir.path(), true);
        let server = serve(&arc).await;
        let rt = dir.path().join("llama");
        let a = asset(&server, Some(sha256_file(&arc).unwrap()));

        let exe = install_assets(&reqwest::Client::new(), &rt, &[a], &AtomicBool::new(false), |_, _| {}).await.unwrap();

        assert!(exe.starts_with(rt.join("bin")), "{exe:?}");
        assert!(exe.is_file());
        assert_eq!(exe.file_name().unwrap(), "llama-server");
        assert!(rt.join("bin/new-marker").is_file());
        assert_no_leftovers(&rt);
    }

    #[tokio::test]
    async fn reinstall_replaces_old_bin() {
        let dir = tempfile::tempdir().unwrap();
        let arc = archive(dir.path(), true);
        let server = serve(&arc).await;
        let rt = dir.path().join("llama");
        old_bin(&rt);
        let a = asset(&server, Some(sha256_file(&arc).unwrap()));

        let exe = install_assets(&reqwest::Client::new(), &rt, &[a], &AtomicBool::new(false), |_, _| {}).await.unwrap();

        assert!(exe.is_file());
        assert!(!rt.join("bin/old-marker").exists(), "old bin content must be gone");
        assert!(rt.join("bin/new-marker").is_file());
        assert_no_leftovers(&rt);
    }

    #[tokio::test]
    async fn sha_mismatch_keeps_old_bin() {
        let dir = tempfile::tempdir().unwrap();
        let arc = archive(dir.path(), true);
        let server = serve(&arc).await;
        let rt = dir.path().join("llama");
        old_bin(&rt);
        let a = asset(&server, Some("0".repeat(64)));

        let err = install_assets(&reqwest::Client::new(), &rt, &[a], &AtomicBool::new(false), |_, _| {}).await.unwrap_err();

        assert!(err.contains("checksum"), "{err}");
        assert_old_bin_intact(&rt);
        assert_no_leftovers(&rt);
    }

    #[tokio::test]
    async fn archive_without_server_keeps_old_bin() {
        let dir = tempfile::tempdir().unwrap();
        let arc = archive(dir.path(), false);
        let server = serve(&arc).await;
        let rt = dir.path().join("llama");
        old_bin(&rt);
        let a = asset(&server, Some(sha256_file(&arc).unwrap()));

        let err = install_assets(&reqwest::Client::new(), &rt, &[a], &AtomicBool::new(false), |_, _| {}).await;

        assert!(err.is_err());
        assert_old_bin_intact(&rt);
        assert_no_leftovers(&rt);
    }

    /// Pre-state of a crash between the two renames of an earlier install: only `bin.old` exists.
    fn crashed_pre_state(rt: &Path) {
        std::fs::create_dir_all(rt.join("bin.old")).unwrap();
        std::fs::write(rt.join("bin.old/llama-server"), "#!/bin/sh\n").unwrap();
        std::fs::write(rt.join("bin.old/old-marker"), "o").unwrap();
    }

    #[tokio::test]
    async fn failed_install_restores_bin_old_left_by_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let arc = archive(dir.path(), true);
        let server = serve(&arc).await;
        let rt = dir.path().join("llama");
        crashed_pre_state(&rt);
        let a = asset(&server, Some("0".repeat(64)));

        let err = install_assets(&reqwest::Client::new(), &rt, &[a], &AtomicBool::new(false), |_, _| {}).await;

        assert!(err.is_err());
        assert!(rt.join("bin/llama-server").is_file(), "bin.old must be restored to bin");
        assert!(rt.join("bin/old-marker").is_file());
        assert!(!rt.join("bin.old").exists());
        assert!(!rt.join("bin.new").exists() && !rt.join("downloads").exists());
    }

    #[tokio::test]
    async fn successful_install_after_crash_replaces_restored_bin() {
        let dir = tempfile::tempdir().unwrap();
        let arc = archive(dir.path(), true);
        let server = serve(&arc).await;
        let rt = dir.path().join("llama");
        crashed_pre_state(&rt);
        let a = asset(&server, Some(sha256_file(&arc).unwrap()));

        let exe = install_assets(&reqwest::Client::new(), &rt, &[a], &AtomicBool::new(false), |_, _| {}).await.unwrap();

        assert!(exe.starts_with(rt.join("bin")) && exe.is_file());
        assert!(rt.join("bin/new-marker").is_file());
        assert!(!rt.join("bin/old-marker").exists(), "bin must be the new tree");
        assert_no_leftovers(&rt);
    }

    #[tokio::test]
    async fn missing_sha_errs_before_any_request() {
        let dir = tempfile::tempdir().unwrap();
        let arc = archive(dir.path(), true);
        let server = serve(&arc).await;
        let rt = dir.path().join("llama");
        old_bin(&rt);
        let a = asset(&server, None);

        let err = install_assets(&reqwest::Client::new(), &rt, &[a], &AtomicBool::new(false), |_, _| {}).await;

        assert!(err.is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
        assert_old_bin_intact(&rt);
        assert_no_leftovers(&rt);
    }
}

// ---- CUDA variants ------------------------------------------------------------------------

fn cuda_release(extra: &[&str]) -> Release {
    let mut n = vec![
        "llama-b9-bin-win-cuda-12.4-x64.zip",
        "cudart-llama-bin-win-cuda-12.4-x64.zip",
        "llama-b9-bin-win-cuda-13.1-x64.zip",
        "cudart-llama-bin-win-cuda-13.1-x64.zip",
    ];
    n.extend_from_slice(extra);
    rel("b9", &n, Some(&"ab".repeat(32)), "https://e/b9")
}

#[test]
fn cuda_ver_display_order_and_serde() {
    assert_eq!(cv(12, 4).to_string(), "12.4");
    assert!(cv(12, 9) < cv(13, 0));
    assert!(cv(13, 1) > cv(13, 0));
    assert_eq!(serde_json::to_value(cv(12, 6)).unwrap(), json!({ "major": 12, "minor": 6 }));
}

#[test]
fn cuda_variants_newest_first() {
    assert_eq!(cuda_variants(&cuda_release(&[])), vec![cv(13, 1), cv(12, 4)]);
}

#[test]
fn cuda_variants_requires_both_zips_and_ignores_other_names() {
    let r = cuda_release(&[
        "llama-b9-bin-win-cuda-12.8-x64.zip",
        "cudart-llama-bin-win-cuda-11.8-x64.zip",
        "llama-b9-bin-win-cuda-12.6-arm64.zip",
        "cudart-llama-bin-win-cuda-12.6-arm64.zip",
        "llama-b9-bin-macos-cuda-12.2-x64.zip",
        "llama-b9-bin-win-cuda-abc-x64.zip",
        "cudart-llama-bin-win-cuda-abc-x64.zip",
    ]);
    assert_eq!(cuda_variants(&r), vec![cv(13, 1), cv(12, 4)]);
}

#[test]
fn cuda_variants_empty_without_any() {
    assert!(cuda_variants(&rel("b9", &["llama-b9-bin-win-cpu-x64.zip"], None, "https://e")).is_empty());
}

fn cuda_pick(r: &Release, max: Option<CudaVer>) -> Result<Vec<String>, String> {
    select_assets(r, "windows", "x86_64", Backend::Cuda, max).map(|v| v.into_iter().map(|a| a.name).collect())
}

#[test]
fn select_cuda_driver_12_6_gets_12_4_pair() {
    assert_eq!(
        cuda_pick(&cuda_release(&[]), Some(cv(12, 6))).unwrap(),
        vec!["llama-b9-bin-win-cuda-12.4-x64.zip", "cudart-llama-bin-win-cuda-12.4-x64.zip"]
    );
}

#[test]
fn select_cuda_driver_13_1_gets_13_1_pair() {
    assert_eq!(
        cuda_pick(&cuda_release(&[]), Some(cv(13, 1))).unwrap(),
        vec!["llama-b9-bin-win-cuda-13.1-x64.zip", "cudart-llama-bin-win-cuda-13.1-x64.zip"]
    );
}

#[test]
fn select_cuda_none_picks_newest() {
    assert_eq!(cuda_pick(&cuda_release(&[]), None).unwrap()[0], "llama-b9-bin-win-cuda-13.1-x64.zip");
}

#[test]
fn select_cuda_driver_too_old_errs_mentioning_cuda() {
    let err = cuda_pick(&cuda_release(&[]), Some(cv(11, 8))).unwrap_err();
    assert!(err.contains("CUDA"), "{err}");
}

#[test]
fn select_cuda_variant_without_cudart_is_not_offered() {
    let mut r = cuda_release(&[]);
    r.assets.retain(|a| a.name != "cudart-llama-bin-win-cuda-13.1-x64.zip");
    assert_eq!(cuda_pick(&r, None).unwrap()[0], "llama-b9-bin-win-cuda-12.4-x64.zip");
    assert_eq!(cuda_pick(&r, Some(cv(13, 1))).unwrap()[0], "llama-b9-bin-win-cuda-12.4-x64.zip");
}

#[test]
fn pick_release_cuda_respects_max_and_falls_back_to_older_release() {
    let mut newest = cuda_release(&[]);
    newest.tag = "b10".into();
    newest.assets.retain(|a| !a.name.contains("12.4"));
    for a in &mut newest.assets {
        a.name = a.name.replace("b9", "b10");
    }
    let older = cuda_release(&[]);
    let got = pick_release(&[newest, older], "windows", "x86_64", Backend::Cuda, Some(cv(12, 6))).unwrap();
    assert_eq!(names(&got), vec!["llama-b9-bin-win-cuda-12.4-x64.zip", "cudart-llama-bin-win-cuda-12.4-x64.zip"]);
}
