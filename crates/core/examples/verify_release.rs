//! Release tooling, used by the release workflow and for local checks.
//!   verify_release verify <file> <sig-file> <pubkey-file | production> <version>
//! `production` checks against the key compiled into the app.
//!   verify_release manifest <latest.json> <version>
use screen_translate_core::update::{PUBKEY, parse_manifest, select, verify};
use std::fs;

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let read = |p: &String| fs::read(p).map_err(|e| format!("{p}: {e}"));
    let text = |p: &String| read(p).map(|b| String::from_utf8_lossy(&b).trim().to_string());
    match a.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["verify", _, _, _, version] => {
            let key = if a[3] == "production" {
                PUBKEY.to_string()
            } else {
                text(&a[3])?
            };
            verify(&read(&a[1])?, &text(&a[2])?, &key, version)?;
            println!("ok: {} is signed for {version}", a[1]);
        }
        ["manifest", _, version] => {
            let m = parse_manifest(&read(&a[1])?)?;
            if m.version != version {
                return Err(format!("manifest version {} != {version}", m.version));
            }
            for (os, arch, installer) in [
                ("darwin", "aarch64", "app"),
                ("darwin", "x86_64", "app"),
                ("windows", "x86_64", "nsis"),
                ("windows", "x86_64", "msi"),
            ] {
                let keys = screen_translate_core::update::platform_keys(os, arch, installer);
                let p = select(&m, &keys).ok_or(format!("no platform for {keys:?}"))?;
                if p.url.is_empty() || p.signature.is_empty() {
                    return Err(format!("empty url or signature for {keys:?}"));
                }
            }
            println!("ok: manifest {version}, {} platforms", m.platforms.len());
        }
        _ => return Err("usage: verify_release verify <file> <sig> <pubkey> <version> | manifest <latest.json> <version>".into()),
    }
    Ok(())
}
