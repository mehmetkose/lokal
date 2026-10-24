//! Every filesystem location lokal uses (SPEC §5.1). No other module may
//! build paths itself.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const LABEL: &str = "dev.lokal.daemon";

#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/Library/Application Support/lokal`, or `$LOKAL_HOME`.
    pub base: PathBuf,
    pub state: PathBuf,
    pub state_tmp: PathBuf,
    pub state_lock: PathBuf,
    pub ca_dir: PathBuf,
    pub ca_cert: PathBuf,
    pub ca_key: PathBuf,
    pub ca_bundle: PathBuf,
    /// Replaced CAs (certificate only), kept until they're out of the keychain.
    pub ca_retired_dir: PathBuf,
    /// User-written copy of the OS resolver config that setup installs.
    pub resolver_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub daemon_bin: PathBuf,
    pub socket: PathBuf,
    pub daemon_lock: PathBuf,
    pub install_record: PathBuf,
    pub log_dir: PathBuf,
    pub daemon_log: PathBuf,
    pub launchd_out: PathBuf,
    pub launchd_err: PathBuf,
    /// The LaunchAgent plist. `None` under `LOKAL_HOME`: a sandbox never
    /// manages the real background service.
    pub plist: Option<PathBuf>,
    /// True when `LOKAL_HOME` (or `--base-dir`) overrides the locations.
    pub sandbox: bool,
}

impl Paths {
    /// The real locations, unless `LOKAL_HOME` is set.
    pub fn resolve() -> Result<Self> {
        if let Some(home) = std::env::var_os("LOKAL_HOME").filter(|v| !v.is_empty()) {
            return Ok(Self::sandbox(PathBuf::from(home)));
        }
        let dirs = directories::BaseDirs::new().context("cannot determine the home directory")?;
        let home = dirs.home_dir();
        // PORTING(linux): data_dir() is ~/.local/share there; logs would go to
        // ~/.local/state/lokal and the unit to ~/.config/systemd/user.
        // PORTING(windows): data_dir() is %APPDATA%; logs under %LOCALAPPDATA%.
        let base = dirs.data_dir().join("lokal");
        let log_dir = home.join("Library/Logs/lokal");
        let plist = home
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"));
        Ok(Self::build(base, log_dir, Some(plist), false))
    }

    /// Sandbox layout: everything under `dir`, logs in `dir/logs`.
    pub fn sandbox(dir: PathBuf) -> Self {
        let logs = dir.join("logs");
        Self::build(dir, logs, None, true)
    }

    fn build(base: PathBuf, log_dir: PathBuf, plist: Option<PathBuf>, sandbox: bool) -> Self {
        let ca_dir = base.join("ca");
        let bin_dir = base.join("bin");
        Self {
            state: base.join("state.toml"),
            state_tmp: base.join("state.toml.tmp"),
            state_lock: base.join("state.toml.lock"),
            ca_cert: ca_dir.join("root.pem"),
            ca_key: ca_dir.join("root-key.pem"),
            ca_bundle: ca_dir.join("bundle.pem"),
            ca_retired_dir: ca_dir.join("retired"),
            resolver_dir: base.join("resolver"),
            ca_dir,
            daemon_bin: bin_dir.join("lokal"),
            bin_dir,
            socket: base.join("lokal.sock"),
            daemon_lock: base.join("daemon.lock"),
            install_record: base.join("install.json"),
            daemon_log: log_dir.join("daemon.log"),
            launchd_out: log_dir.join("launchd.out.log"),
            launchd_err: log_dir.join("launchd.err.log"),
            log_dir,
            plist,
            base,
            sandbox,
        }
    }

    /// Rotated daemon log: `daemon.log.<n>` (n = 1 is the newest).
    pub fn rotated_log(&self, n: u32) -> PathBuf {
        self.log_dir.join(format!("daemon.log.{n}"))
    }

    /// Staged resolver config for `tld`.
    pub fn resolver_staged(&self, tld: &str) -> PathBuf {
        self.resolver_dir.join(tld)
    }

    /// Creates the base directory (0700) if needed.
    pub fn ensure_base(&self) -> Result<()> {
        crate::util::create_private_dir(&self.base)
    }

    /// Path for a temporary file next to `path` (same filesystem, so a
    /// rename is atomic).
    pub fn tmp_sibling(path: &Path) -> PathBuf {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".tmp.{}", std::process::id()));
        path.with_file_name(name)
    }
}
