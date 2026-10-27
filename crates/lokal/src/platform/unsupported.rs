//! Every OS other than macOS and Linux: compiles, and every operation fails
//! with "lokal currently supports macOS only". The CLI's state commands and
//! the daemon in sandbox mode (`--https-port`, `--name-backend none`) still
//! work. Linux uses [`Unsupported`] for the parts it hasn't ported yet.
//
// PORTING: replace this with src/platform/<os>/ (see docs/PORTING.md).

use std::path::PathBuf;

use anyhow::{Result, bail};

use super::*;

// Linux has its own (`platform/linux/`), which uses `Unsupported` for parts.
#[cfg(not(target_os = "linux"))]
pub fn platform() -> Platform {
    Platform {
        trust: Box::new(Unsupported),
        resolver: Box::new(Unsupported),
        service: Box::new(Unsupported),
        desktop: Box::new(Unsupported),
        system: Box::new(Unsupported),
        default_name_backend: "none",
    }
}

pub fn name_publisher(_backend: &str) -> Result<Box<dyn NamePublisher>> {
    bail!(UNSUPPORTED)
}

pub(super) struct Unsupported;

impl TrustStore for Unsupported {
    fn plan_trust(&self, _ca: &CaCert) -> Vec<PrivilegedCommand> {
        vec![]
    }
    fn plan_untrust(&self, _ca: &CaCert) -> Vec<PrivilegedCommand> {
        vec![]
    }
    fn is_trusted(&self, _ca: &CaCert) -> Result<TrustStatus> {
        bail!(UNSUPPORTED)
    }
    fn trust_too_broad(&self, _ca: &CaCert) -> Result<bool> {
        bail!(UNSUPPORTED)
    }
    fn is_present(&self, _ca: &CaCert) -> Result<bool> {
        bail!(UNSUPPORTED)
    }
    fn lokal_cas_pem(&self) -> Result<String> {
        bail!(UNSUPPORTED)
    }
    fn apply_user_extras(&self, _ca: &CaCert) -> Result<Vec<String>> {
        bail!(UNSUPPORTED)
    }
    fn remove_user_extras(&self) -> Result<Vec<String>> {
        bail!(UNSUPPORTED)
    }
    fn user_extras_status(&self) -> Option<(bool, String)> {
        None
    }
    fn system_roots_pem(&self) -> Result<String> {
        bail!(UNSUPPORTED)
    }
}

impl ServiceManager for Unsupported {
    fn install(&self, _ctx: &InstallCtx) -> Result<InstallOutcome> {
        bail!(UNSUPPORTED)
    }
    fn uninstall(&self, _ctx: &InstallCtx) -> Result<()> {
        bail!(UNSUPPORTED)
    }
    fn start(&self, _ctx: &InstallCtx) -> Result<()> {
        bail!(UNSUPPORTED)
    }
    fn stop(&self) -> Result<()> {
        bail!(UNSUPPORTED)
    }
    fn restart(&self) -> Result<()> {
        bail!(UNSUPPORTED)
    }
    fn is_installed(&self, _ctx: &InstallCtx) -> bool {
        false
    }
    fn is_loaded(&self) -> bool {
        false
    }
    fn render_definition(&self, _ctx: &InstallCtx) -> String {
        String::new()
    }
    fn inherited_listeners(&self) -> Result<Option<Listeners>> {
        Ok(None)
    }
}

impl SystemResolver for Unsupported {
    fn config(&self, _port: u16) -> String {
        String::new()
    }
    fn location(&self, tld: &str) -> String {
        format!("(.{tld} resolver: unsupported)")
    }
    fn plan_install(&self, _tld: &str, _staged: &std::path::Path) -> Vec<PrivilegedCommand> {
        vec![]
    }
    fn plan_uninstall(&self, _tld: &str) -> Vec<PrivilegedCommand> {
        vec![]
    }
    fn status(&self, _tld: &str, _port: u16) -> ResolverStatus {
        ResolverStatus::Wrong(UNSUPPORTED.into())
    }
}

impl Desktop for Unsupported {
    fn open_url(&self, _url: &str) -> Result<()> {
        bail!(UNSUPPORTED)
    }
}

impl System for Unsupported {
    fn is_root(&self) -> bool {
        false
    }
    fn user_and_host(&self) -> (String, String) {
        (
            std::env::var("USER").unwrap_or_else(|_| "user".into()),
            "localhost".into(),
        )
    }
    fn run_privileged(&self, _cmd: &PrivilegedCommand, _prompt: &str) -> Result<()> {
        bail!(UNSUPPORTED)
    }
    fn port_owners(&self, _port: u16) -> Vec<PortOwner> {
        vec![]
    }
    fn tcp_listen_ports(&self, _pid: u32) -> Vec<u16> {
        vec![]
    }
    fn hosts_file(&self) -> Option<PathBuf> {
        None
    }
    fn os_version(&self) -> String {
        std::env::consts::OS.to_string()
    }
}
