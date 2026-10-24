//! Version and build identity, embedded by `build.rs`.

use std::sync::OnceLock;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_HASH: &str = env!("LOKAL_GIT_HASH");
/// `YYYYMMDDTHHMMSSZ`
pub const BUILD_TIME: &str = env!("LOKAL_BUILD_TIME");

/// `0.1.0+abc1234.20261010T120000Z`: unique per build. The daemon reports it
/// in `ping`/`status`; the CLI compares it with its own.
pub fn build_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| format!("{VERSION}+{GIT_HASH}.{BUILD_TIME}"))
}

/// Build time as RFC 3339 (`2026-10-10T12:00:00Z`).
pub fn build_date() -> String {
    let t = BUILD_TIME;
    if t.len() != 16 {
        return t.to_string();
    }
    format!(
        "{}-{}-{}T{}:{}:{}Z",
        &t[0..4],
        &t[4..6],
        &t[6..8],
        &t[9..11],
        &t[11..13],
        &t[13..15]
    )
}

/// What `--version` prints after `lokal `: `0.1.0 (abc1234 2026-10-10T12:00:00Z)`.
pub fn long_version() -> &'static str {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| format!("{VERSION} ({GIT_HASH} {})", build_date()))
}

/// Short form for messages: `0.1.0 (abc1234)`.
pub fn short_version() -> String {
    format!("{VERSION} ({GIT_HASH})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_id_round_trips_from_version_string() {
        // The live test derives the build id from `lokal --version`.
        let v = long_version();
        let inner = v.split_once('(').unwrap().1.trim_end_matches(')');
        let (hash, date) = inner.split_once(' ').unwrap();
        let stamp: String = date.chars().filter(|c| *c != '-' && *c != ':').collect();
        assert_eq!(format!("{VERSION}+{hash}.{stamp}"), build_id());
    }
}
