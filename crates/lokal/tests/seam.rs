//! The platform seam (SPEC §10.1): no `target_os` cfg and no macOS-only
//! command outside `src/platform/`, and every platform trait method carries
//! a PORTING note.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn no_target_os_outside_platform() {
    let src = src();
    let mut files = vec![];
    rust_files(&src, &mut files);
    assert!(files.len() > 20);
    let forbidden = [
        "target_os",
        "/usr/bin/security",
        "launchctl",
        "dns_sd",
        "DNSService",
        "launch_activate_socket",
        "/usr/bin/open",
        "Library/Keychains",
    ];
    let mut bad = vec![];
    for f in files {
        if f.starts_with(src.join("platform")) {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for (i, line) in text.lines().enumerate() {
            for word in forbidden {
                if line.contains(word) {
                    bad.push(format!(
                        "{}:{}: {word}",
                        f.strip_prefix(&src).unwrap().display(),
                        i + 1
                    ));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "OS-specific code outside src/platform/:\n{}",
        bad.join("\n")
    );
}

#[test]
fn every_platform_trait_method_has_a_porting_note() {
    let text = std::fs::read_to_string(src().join("platform/mod.rs")).unwrap();
    let mut in_trait = false;
    let mut porting_since_last = false;
    let mut missing = vec![];
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("pub trait ") {
            in_trait = true;
            porting_since_last = false;
            continue;
        }
        if in_trait && l == "}" {
            in_trait = false;
            continue;
        }
        if !in_trait {
            continue;
        }
        if l.contains("PORTING") {
            porting_since_last = true;
        }
        if l.starts_with("fn ") {
            // backend_name/states/display-only helpers are OS-neutral.
            let neutral = [
                "fn backend_name",
                "fn states",
                "fn render_definition",
                "fn is_installed",
                "fn is_loaded",
                "fn user_extras_status",
                "fn remove_user_extras",
                "fn is_root",
                "fn user_and_host",
                "fn os_version",
                "fn clear",
            ];
            if !porting_since_last && !neutral.iter().any(|n| l.starts_with(n)) {
                missing.push(l.to_string());
            }
            porting_since_last = false;
        }
    }
    assert!(
        missing.is_empty(),
        "platform trait methods without a PORTING note:\n{}",
        missing.join("\n")
    );
}
