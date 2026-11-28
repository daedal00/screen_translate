use std::path::{Path, PathBuf};

use screen_translate_core::update::{
    UpdateState, bundle_from_exe, current_platform_keys, installer_key, platform_keys,
    should_background_check, windows_installer_path,
};

#[test]
fn bundle_from_exe_finds_the_app_bundle() {
    let exe = Path::new("/Applications/Screen Translate.app/Contents/MacOS/screen-translate");
    assert_eq!(
        bundle_from_exe(exe),
        Some(PathBuf::from("/Applications/Screen Translate.app"))
    );
}

#[test]
fn bundle_from_exe_rejects_dev_and_non_macos_paths() {
    assert_eq!(
        bundle_from_exe(Path::new("target/debug/screen-translate")),
        None
    );
    assert_eq!(
        bundle_from_exe(Path::new("/Applications/X.app/Contents/Resources/x")),
        None
    );
}

#[test]
fn windows_installer_path_nests_versioned_dir_under_temp() {
    let temp = Path::new("tmp-root");
    let p = windows_installer_path(temp, "0.3.1");
    assert!(p.starts_with(temp));
    let rel: Vec<_> = p
        .strip_prefix(temp)
        .unwrap()
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        rel,
        [
            "screen-translate-0.3.1",
            "Screen.Translate_0.3.1_x64-setup.exe"
        ]
    );
}

#[test]
fn installer_key_matches_platform() {
    #[cfg(target_os = "macos")]
    assert_eq!(installer_key(), "app");
    #[cfg(target_os = "windows")]
    assert_eq!(installer_key(), "nsis");
}

#[test]
fn current_platform_keys_use_os_arch_and_installer_key() {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "windows"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    assert_eq!(
        current_platform_keys(),
        platform_keys(os, arch, installer_key())
    );
}

#[test]
fn background_check_runs_only_when_not_busy_or_pending() {
    for s in [
        UpdateState::Idle,
        UpdateState::UpToDate,
        UpdateState::Failed,
    ] {
        assert!(should_background_check(&s), "{s:?}");
    }
    for s in [
        UpdateState::Checking,
        UpdateState::Available("0.3.1".into()),
        UpdateState::Installing,
    ] {
        assert!(!should_background_check(&s), "{s:?}");
    }
}
