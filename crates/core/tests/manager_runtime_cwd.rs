//! Own test binary: it changes the process working directory, so it must not share a process
//! with other tests.

use screen_translate_core::manager::runtime::find_server;

#[test]
fn find_server_ignores_empty_and_relative_path_entries() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("llama-server"), b"x").unwrap();
    std::fs::create_dir(cwd.path().join("sub")).unwrap();
    std::fs::write(cwd.path().join("sub").join("llama-server"), b"x").unwrap();
    std::env::set_current_dir(cwd.path()).unwrap();

    // "" and "." both resolve to the cwd, "sub" is cwd-relative: none may be searched.
    let pv = std::env::join_paths(["", ".", "sub"]).unwrap();
    let rt = tempfile::tempdir().unwrap();
    assert_eq!(find_server(rt.path(), Some(&pv), "linux"), None);
}
