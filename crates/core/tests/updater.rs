//! M4 updater core logic. Pinned API (module `screen_translate_core::update`):
//!   pub const PUBKEY: &str; pub const ENDPOINT: &str;
//!   pub struct Manifest { pub version: String, pub notes: Option<String>, pub pub_date: Option<String>,
//!                         pub platforms: HashMap<String, Platform> }
//!   pub struct Platform { pub url: String, pub signature: String }
//!   pub fn parse_manifest(&[u8]) -> Result<Manifest, E: Debug>
//!   pub fn platform_keys(os, arch, installer: &str) -> [String; 2]
//!   pub fn select<'a>(&'a Manifest, &[String]) -> Option<&'a Platform>
//!   pub fn newer(current: &str, remote: &str) -> Result<bool, E>
//!   pub fn verify(bytes: &[u8], sig_b64: &str, pubkey_b64: &str, version: &str) -> Result<(), E>
//!   pub fn installer_args() -> impl PartialEq<[&str; 3]>   (== ["/P", "/UPDATE", "/R"])
//!   macOS: pub fn extract_bundle(tar_gz: &[u8], staging: &Path) -> Result<(), E>
//!          pub fn swap_dirs(a: &Path, b: &Path) -> Result<(), E>          (renamex_np RENAME_SWAP)
//!          pub fn replace_bundle(staged: &Path, target: &Path) -> Result<(), E>  (rename + restore fallback)

use base64::{Engine, engine::general_purpose::STANDARD};
use screen_translate_core::update::*;
use std::fs;
use std::path::PathBuf;

fn fx(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/update")
        .join(name)
}
fn read(name: &str) -> Vec<u8> {
    fs::read(fx(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}
fn text(name: &str) -> String {
    String::from_utf8(read(name)).unwrap().trim().to_string()
}

// 1. parse_manifest
#[test]
fn parses_real_0_3_1_manifest() {
    let m = parse_manifest(&read("latest-0.3.1.json")).unwrap();
    assert_eq!(m.version, "0.3.1");
    assert_eq!(m.platforms.len(), 7);
    assert!(
        m.platforms["darwin-aarch64-app"]
            .url
            .starts_with("https://")
    );
    assert!(!m.platforms["windows-x86_64"].signature.is_empty());
}

#[test]
fn manifest_strips_leading_v() {
    let m = parse_manifest(br#"{"version":"v0.4.0","platforms":{}}"#).unwrap();
    assert_eq!(m.version, "0.4.0");
}

#[test]
fn manifest_rejects_bad_input() {
    assert!(
        parse_manifest(br#"{"platforms":{}}"#).is_err(),
        "missing version"
    );
    assert!(
        parse_manifest(br#"{"version":"not-semver","platforms":{}}"#).is_err(),
        "bad semver"
    );
    assert!(
        parse_manifest(br#"{"version":"0.4.0","platforms":{"darwin-aarch64":{"signature":"x"}}}"#)
            .is_err(),
        "missing url"
    );
    assert!(parse_manifest(b"not json").is_err());
}

// 2. platform_keys / select
#[test]
fn platform_keys_order() {
    assert_eq!(
        platform_keys("darwin", "aarch64", "app"),
        ["darwin-aarch64-app", "darwin-aarch64"]
    );
    assert_eq!(
        platform_keys("windows", "x86_64", "nsis"),
        ["windows-x86_64-nsis", "windows-x86_64"]
    );
}

#[test]
fn select_prefers_installer_key_then_plain_then_none() {
    let m = parse_manifest(
        br#"{"version":"1.0.0","platforms":{
            "darwin-aarch64":{"url":"https://x/plain","signature":"s"},
            "darwin-aarch64-app":{"url":"https://x/app","signature":"s"},
            "windows-x86_64":{"url":"https://x/win","signature":"s"}}}"#,
    )
    .unwrap();
    let k = platform_keys("darwin", "aarch64", "app");
    assert_eq!(select(&m, &k).unwrap().url, "https://x/app");
    let k = platform_keys("windows", "x86_64", "nsis");
    assert_eq!(select(&m, &k).unwrap().url, "https://x/win");
    let k = platform_keys("linux", "x86_64", "deb");
    assert!(select(&m, &k).is_none());
}

// 3. newer
#[test]
fn newer_is_strictly_greater_semver() {
    assert!(newer("0.3.1", "0.4.0").unwrap());
    assert!(!newer("0.4.0", "0.4.0").unwrap());
    assert!(!newer("0.4.0", "0.3.1").unwrap());
    assert!(newer("0.4.0-beta.1", "0.4.0").unwrap());
    assert!(!newer("0.4.0", "0.4.0-beta.1").unwrap());
    assert!(newer("0.3.1", "garbage").is_err());
}

// 4. verify
#[test]
fn verify_accepts_good_signature() {
    let (data, sig, key) = (
        read("payload.bin"),
        text("payload.bin.sig"),
        text("throwaway.pub"),
    );
    verify(&data, &sig, &key, "9.9.9").unwrap();
    verify(&data, &sig, &key, "v9.9.9").unwrap();
}

#[test]
fn verify_rejects_tampering_and_mismatches() {
    let (data, sig, key) = (
        read("payload.bin"),
        text("payload.bin.sig"),
        text("throwaway.pub"),
    );
    let mut flipped = data.clone();
    flipped[0] ^= 1;
    assert!(
        verify(&flipped, &sig, &key, "9.9.9").is_err(),
        "flipped byte"
    );
    assert!(
        verify(&data, &sig, &text("other.pub"), "9.9.9").is_err(),
        "other key"
    );
    assert!(
        verify(&data, &sig, &key, "9.9.8").is_err(),
        "signed version mismatch"
    );
    let nov = read("payload-noversion.bin");
    assert!(
        verify(&nov, &text("payload-noversion.bin.sig"), &key, "9.9.9").is_err(),
        "version required"
    );
}

#[test]
fn verify_garbage_does_not_panic() {
    let (data, sig, key) = (
        read("payload.bin"),
        text("payload.bin.sig"),
        text("throwaway.pub"),
    );
    assert!(verify(&data, "!!!not base64!!!", &key, "9.9.9").is_err());
    assert!(verify(&data, &sig, "!!!not base64!!!", "9.9.9").is_err());
    assert!(verify(&data, &STANDARD.encode("garbage"), &key, "9.9.9").is_err());
    assert!(verify(&data, &sig, &STANDARD.encode("garbage"), "9.9.9").is_err());
    assert!(verify(&data, "", "", "").is_err());
}

// 5. production key guard
fn key_id_hex(b64_line: &str) -> String {
    let raw = STANDARD.decode(b64_line).unwrap();
    raw[2..10]
        .iter()
        .rev()
        .map(|b| format!("{b:02X}"))
        .collect()
}

#[test]
fn production_pubkey_matches_v0_3_1_signing_key() {
    let pub_text = String::from_utf8(STANDARD.decode(PUBKEY).unwrap()).unwrap();
    let mut lines = pub_text.lines();
    assert_eq!(
        lines.next().unwrap(),
        "untrusted comment: minisign public key: A4532F120298EE3E"
    );
    assert_eq!(key_id_hex(lines.next().unwrap()), "A4532F120298EE3E");

    let sig_text = String::from_utf8(
        STANDARD
            .decode(text("Screen.Translate_universal.app.tar.gz.sig-0.3.1"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        key_id_hex(sig_text.lines().nth(1).unwrap()),
        "A4532F120298EE3E"
    );
    assert!(ENDPOINT.ends_with("/releases/latest/download/latest.json"));
}

// 8. Windows installer args (pure, runs everywhere)
#[test]
fn installer_args_match_tauri_passive_update_restart() {
    assert_eq!(installer_args(), ["/P", "/UPDATE", "/R"]);
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn header(name: &[u8], size: usize, kind: u8) -> tar::Header {
        let mut h = tar::Header::new_gnu();
        h.as_mut_bytes()[..100].fill(0);
        h.as_mut_bytes()[..name.len()].copy_from_slice(name); // bypasses Builder's `..` rejection
        h.set_size(size as u64);
        h.set_mode(0o755);
        h.set_entry_type(tar::EntryType::new(kind));
        h.set_cksum();
        h
    }

    fn tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, body) in entries {
            b.append(&header(name.as_bytes(), body.len(), b'0'), *body)
                .unwrap();
        }
        let raw = b.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &raw).unwrap();
        gz.finish().unwrap()
    }

    /// Entries are (path, link target). `Some` makes a symlink, `None` a regular file.
    fn tar_gz_links(entries: &[(&str, Option<&str>)]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, link) in entries {
            let mut h = header(name.as_bytes(), 0, if link.is_some() { b'2' } else { b'0' });
            if let Some(t) = link {
                h.set_link_name(t).unwrap();
                h.set_cksum();
            }
            b.append(&h, std::io::empty()).unwrap();
        }
        let raw = b.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &raw).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn extract_rejects_symlink_chain_escape() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("stage");
        fs::create_dir(&staging).unwrap();
        let gz = tar_gz_links(&[
            ("App.app/Contents/Info.plist", None),
            ("App.app/d", Some(".")),
            ("App.app/d/e", Some("..")),
            ("App.app/e/pwned", None),
        ]);
        let res = extract_bundle(&gz, &staging);
        let leaked = root.path().join("pwned").exists();
        let _ = fs::remove_file(root.path().join("pwned")); // keep the red run from littering
        assert!(!leaked, "pwned was written outside staging");
        assert!(res.is_err(), "symlink-chain archive was accepted");
    }

    #[test]
    fn extract_rejects_dir_entry_over_escaping_symlink_without_chmod() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
        let staging = outside.join("stage");
        fs::create_dir(&staging).unwrap();
        // `sub/d -> ..` is inside, so `sub/x -> d/..` stays inside lexically but resolves to `outside`
        // (staging's parent); the dir entry `sub/x/` must not chmod it.
        let mut b = tar::Builder::new(Vec::new());
        for (name, link, kind, mode) in [
            ("App.app/Contents/Info.plist", None, b'0', 0o644),
            ("App.app/sub/d", Some(".."), b'2', 0o755),
            ("App.app/sub/x", Some("d/.."), b'2', 0o755),
            ("App.app/sub/x/", None, b'5', 0o700),
        ] {
            let mut h = header(name.as_bytes(), 0, kind);
            h.set_mode(mode);
            if let Some(t) = link {
                h.set_link_name(t).unwrap();
            }
            h.set_cksum();
            b.append(&h, std::io::empty()).unwrap();
        }
        let raw = b.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &raw).unwrap();
        let gz = gz.finish().unwrap();
        let res = extract_bundle(&gz, &staging);
        let mode = fs::metadata(&outside).unwrap().permissions().mode() & 0o777;
        // The pinned tar-rs happens to fail with AlreadyExists; the updater must reject it on purpose.
        assert!(
            res.as_ref().is_err_and(|e| e.contains("symlink")),
            "not rejected as a symlink: {res:?}"
        );
        assert_eq!(mode, 0o755, "a dir outside staging was chmod-ed");
    }

    #[test]
    fn extract_rejects_symlink_target_resolving_outside_via_earlier_symlink() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("stage");
        fs::create_dir(&staging).unwrap();
        // Lexically `e -> sub/d/..` stays inside, but `d -> ..` makes it resolve to staging's parent.
        let gz = tar_gz_links(&[
            ("App.app/Contents/Info.plist", None),
            ("App.app/sub/f", None),
            ("App.app/sub/d", Some("..")),
            ("App.app/e", Some("sub/d/..")),
        ]);
        assert!(extract_bundle(&gz, &staging).is_err());
    }

    #[test]
    fn extract_rejects_dangling_symlink() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("stage");
        fs::create_dir(&staging).unwrap();
        let gz = tar_gz_links(&[
            ("App.app/Contents/Info.plist", None),
            ("App.app/Contents/dangling", Some("nowhere")),
        ]);
        assert!(extract_bundle(&gz, &staging).is_err());
    }

    #[test]
    fn extract_accepts_inside_leaf_symlink() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("stage");
        fs::create_dir(&staging).unwrap();
        let gz = tar_gz_links(&[
            ("App.app/Contents/Info.plist", None),
            ("App.app/Contents/MacOS/x", None),
            ("App.app/Contents/link", Some("MacOS/x")),
        ]);
        extract_bundle(&gz, &staging).unwrap();
        let link = staging.join("Contents/link");
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(&link).unwrap(),
            std::path::Path::new("MacOS/x")
        );
    }

    // 6. extract
    #[test]
    fn extract_strips_first_component_and_sets_0755() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("stage");
        fs::create_dir(&staging).unwrap();
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700)).unwrap();
        let gz = tar_gz(&[
            ("Screen Translate.app/Contents/Info.plist", b"<plist/>"),
            ("Screen Translate.app/Contents/MacOS/x", b"bin"),
        ]);
        extract_bundle(&gz, &staging).unwrap();
        assert_eq!(
            fs::read(staging.join("Contents/Info.plist")).unwrap(),
            b"<plist/>"
        );
        assert_eq!(fs::read(staging.join("Contents/MacOS/x")).unwrap(), b"bin");
        assert_eq!(
            fs::metadata(&staging).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn extract_rejects_traversal_and_absolute_paths() {
        for evil in ["App.app/../../evil", "/tmp/st-evil-abs"] {
            let root = tempfile::tempdir().unwrap();
            let staging = root.path().join("stage");
            fs::create_dir(&staging).unwrap();
            let gz = tar_gz(&[("App.app/Contents/Info.plist", b"<plist/>"), (evil, b"pwn")]);
            assert!(extract_bundle(&gz, &staging).is_err(), "{evil}");
            assert!(!root.path().join("evil").exists(), "{evil}");
            assert!(
                !root.path().parent().unwrap().join("evil").exists(),
                "{evil}"
            );
            assert!(!std::path::Path::new("/tmp/st-evil-abs").exists(), "{evil}");
        }
    }

    #[test]
    fn extract_rejects_dot_prefixed_layout_without_info_plist() {
        let root = tempfile::tempdir().unwrap();
        let staging = root.path().join("stage");
        fs::create_dir(&staging).unwrap();
        // `tar -C dir .` layout: first component `.` is dropped, leaving Screen Translate.app/... nested.
        let gz = tar_gz(&[("./Screen Translate.app/Contents/Info.plist", b"<plist/>")]);
        assert!(extract_bundle(&gz, &staging).is_err());
    }

    // 7. swap / replace
    fn bundle(dir: &std::path::Path, marker: &str) {
        fs::create_dir_all(dir.join("Contents")).unwrap();
        fs::write(dir.join("Contents/marker"), marker).unwrap();
    }

    #[test]
    fn swap_dirs_exchanges_contents() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        bundle(&a, "old");
        bundle(&b, "new");
        swap_dirs(&a, &b).unwrap();
        assert_eq!(
            fs::read_to_string(a.join("Contents/marker")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(b.join("Contents/marker")).unwrap(),
            "old"
        );
    }

    #[test]
    fn replace_bundle_puts_staged_at_target() {
        let t = tempfile::tempdir().unwrap();
        let (staged, target) = (
            t.path().join("staged"),
            t.path().join("Screen Translate.app"),
        );
        bundle(&staged, "new");
        bundle(&target, "old");
        replace_bundle(&staged, &target).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("Contents/marker")).unwrap(),
            "new"
        );
    }
}
