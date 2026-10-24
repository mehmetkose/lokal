//! `install.json`: what `setup` did, so `uninstall` can revert exactly that.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstallRecord {
    pub schema: u32,
    pub version: String,
    pub build_id: String,
    pub installed_at: String,
    pub label: String,
    pub plist: PathBuf,
    pub daemon_binary: PathBuf,
    pub ca_cert: PathBuf,
    pub ca_sha256: String,
    pub ca_sha1: String,
    /// The CA is trusted in the System keychain (by this setup or earlier).
    pub trusted: bool,
    /// `setup --skip-trust` (test-only) skipped the trust step.
    #[serde(default)]
    pub trust_skipped: bool,
    /// The OS resolver config for `.lokal` is installed.
    #[serde(default)]
    pub resolver_installed: bool,
    /// Per-user extras applied (Firefox profiles).
    #[serde(default)]
    pub user_extras: Vec<String>,
}

impl InstallRecord {
    pub fn load(paths: &Paths) -> Option<Self> {
        let text = std::fs::read_to_string(&paths.install_record).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        crate::util::write_atomic(&paths.install_record, text.as_bytes(), 0o644)
            .context("cannot write install.json")
    }
}
