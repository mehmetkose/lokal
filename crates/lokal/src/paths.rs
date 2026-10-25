//! Every filesystem location lokal uses (SPEC §5.1). No other module may
//! build paths itself.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const LABEL: &str = "dev.lokal.daemon";

/// A repo's file for `lokal up` and `lokal down` (SPEC §4.11).
pub const PROJECT_FILE: &str = "lokal.toml";

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

    /// The `lokal.toml` for `lokal up` (SPEC §4.11): `start`, then each
    /// parent, and the first regular file of that name wins (a symlink to
    /// one counts). It lives in the user's repo, not in lokal's folders,
    /// but its path is still built here, like every other path.
    pub fn find_project_file(start: &Path) -> Option<PathBuf> {
        start
            .ancestors()
            .map(|dir| dir.join(PROJECT_FILE))
            .find(|p| p.is_file())
    }

    /// The directory a `lokal.toml` service runs in: `cwd` from the file,
    /// relative to the file's directory `dir` (an absolute `cwd` stays as
    /// it is). `.` components are dropped, so `cwd = "."` is `dir` itself.
    pub fn project_cwd(dir: &Path, cwd: &str) -> PathBuf {
        dir.join(cwd)
            .components()
            .filter(|c| !matches!(c, std::path::Component::CurDir))
            .collect()
    }

    /// Path for a temporary file next to `path` (same filesystem, so a
    /// rename is atomic).
    pub fn tmp_sibling(path: &Path) -> PathBuf {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".tmp.{}", std::process::id()));
        path.with_file_name(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_file_is_found_here_or_in_a_parent() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let deep = repo.join("services/api/src");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(Paths::find_project_file(&deep), None);
        std::fs::write(repo.join(PROJECT_FILE), "").unwrap();
        assert_eq!(
            Paths::find_project_file(&deep),
            Some(repo.join(PROJECT_FILE))
        );
        assert_eq!(
            Paths::find_project_file(&repo),
            Some(repo.join(PROJECT_FILE))
        );
        // A nested file is its own project: the nearest one wins.
        let api = repo.join("services/api");
        std::fs::write(api.join(PROJECT_FILE), "").unwrap();
        assert_eq!(
            Paths::find_project_file(&deep),
            Some(api.join(PROJECT_FILE))
        );
        // Only a regular file counts: a directory of that name is skipped.
        std::fs::create_dir(deep.join(PROJECT_FILE)).unwrap();
        assert_eq!(
            Paths::find_project_file(&deep),
            Some(api.join(PROJECT_FILE))
        );
    }

    #[test]
    fn project_cwd_is_relative_to_the_file() {
        let dir = Path::new("/r/shop");
        assert_eq!(Paths::project_cwd(dir, "."), PathBuf::from("/r/shop"));
        assert_eq!(
            Paths::project_cwd(dir, "./services/api"),
            PathBuf::from("/r/shop/services/api")
        );
        assert_eq!(Paths::project_cwd(dir, "/abs"), PathBuf::from("/abs"));
        assert_eq!(Paths::project_cwd(dir, ""), PathBuf::from("/r/shop"));
    }
}
