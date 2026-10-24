//! Firefox (SPEC §4.6, VERIFY V5): Firefox keeps its own trust store. With
//! `security.enterprise_roots.enabled` it also trusts roots from the macOS
//! keychain. lokal appends that pref to `user.js` in each profile, tagged
//! so uninstall can remove exactly its own line.

use std::path::{Path, PathBuf};

use anyhow::Result;

const APP: &str = "/Applications/Firefox.app";
const PREF: &str = r#"user_pref("security.enterprise_roots.enabled", true);"#;
const TAG: &str = "// added by lokal";

fn installed() -> bool {
    Path::new(APP).exists()
}

fn profiles() -> Vec<PathBuf> {
    let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) else {
        return vec![];
    };
    let dir = home.join("Library/Application Support/Firefox/Profiles");
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

fn has_pref(user_js: &str) -> bool {
    user_js
        .lines()
        .any(|l| l.contains("security.enterprise_roots.enabled") && l.contains("true"))
}

pub fn enable_enterprise_roots() -> Result<Vec<String>> {
    if !installed() {
        return Ok(vec![]);
    }
    let mut done = vec![];
    for p in profiles() {
        let file = p.join("user.js");
        let cur = std::fs::read_to_string(&file).unwrap_or_default();
        if has_pref(&cur) {
            continue;
        }
        let mut next = cur.clone();
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(&format!("{PREF} {TAG}\n"));
        std::fs::write(&file, next)?;
        done.push(format!(
            "Firefox profile {}: enabled security.enterprise_roots.enabled (user.js)",
            p.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    Ok(done)
}

pub fn remove_pref() -> Result<Vec<String>> {
    let mut done = vec![];
    for p in profiles() {
        let file = p.join("user.js");
        let Ok(cur) = std::fs::read_to_string(&file) else {
            continue;
        };
        if !cur.lines().any(|l| l.trim_end().ends_with(TAG)) {
            continue;
        }
        let kept: Vec<&str> = cur
            .lines()
            .filter(|l| !l.trim_end().ends_with(TAG))
            .collect();
        let mut text = kept.join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        std::fs::write(&file, text)?;
        done.push(format!(
            "Firefox profile {}: removed lokal's user.js line",
            p.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    Ok(done)
}

pub fn status() -> Option<(bool, String)> {
    if !installed() {
        return None;
    }
    let ps = profiles();
    if ps.is_empty() {
        return Some((
            false,
            "Firefox has no profile yet; start it once, then run 'lokal setup' again".into(),
        ));
    }
    let missing: Vec<String> = ps
        .iter()
        .filter(|p| !has_pref(&std::fs::read_to_string(p.join("user.js")).unwrap_or_default()))
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    if missing.is_empty() {
        Some((
            true,
            "keychain roots enabled in every Firefox profile".into(),
        ))
    } else {
        Some((
            false,
            format!(
                "Firefox profile(s) {} don't trust the keychain; run 'lokal setup'",
                missing.join(", ")
            ),
        ))
    }
}
