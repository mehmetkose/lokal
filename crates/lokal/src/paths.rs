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
    /// A person's approvals of `lokal.toml` files (SPEC §16.11), one
    /// record per file: see [`Paths::trust_record`].
    pub trusted_dir: PathBuf,
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
            trusted_dir: base.join("trusted"),
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

    /// The approval record of the `lokal.toml` at `canonical` (SPEC §16.11):
    /// `trusted/<SHA-256 of the canonical path>`. Keyed by the canonical
    /// path, so a symlink or `..` leads to the real file's record, never to
    /// another file's.
    pub fn trust_record(&self, canonical: &Path) -> PathBuf {
        // PORTING(windows): the encoded bytes are WTF-8 there (stable across
        // runs), and paths are case-insensitive, so the key depends on the
        // casing canonicalize returns.
        let name = crate::util::sha256_hex(canonical.as_os_str().as_encoded_bytes());
        self.trusted_dir.join(name)
    }

    /// `lokal up`'s folder for `project` (SPEC §5.1, mode 0700):
    /// `<log dir>/up/<project>/`, which holds [`Paths::up_lock`],
    /// [`Paths::up_socket`], the services' logs ([`Paths::up_log`]) and
    /// [`Paths::up_detached`]. `project` must be a valid project name (a
    /// label).
    pub fn up_dir(&self, project: &str) -> PathBuf {
        self.log_dir.join("up").join(project)
    }

    /// The lock `lokal up` holds while it runs `project`'s services (SPEC
    /// §4.11, "The lock"): `<up dir>/up.lock` (0600), holding its pid.
    pub fn up_lock(&self, project: &str) -> PathBuf {
        self.up_dir(project).join("up.lock")
    }

    /// The socket a running `lokal up` takes `lokal restart <SUB>`'s
    /// requests on (SPEC §4.11, "Restarting a service"): `<up dir>/up.sock`
    /// (0600).
    pub fn up_socket(&self, project: &str) -> PathBuf {
        self.up_dir(project).join("up.sock")
    }

    /// The servers that `lokal up`'s commands left running outside it, on
    /// their ports (SPEC §4.11, "A server that detaches"):
    /// `<up dir>/detached.json` (0600). Only for the next `up`'s busy-port
    /// error: lokal never signals a pid in it.
    pub fn up_detached(&self, project: &str) -> PathBuf {
        self.up_dir(project).join("detached.json")
    }

    /// The log of a service that `lokal up` started (SPEC §4.11, "Logs"):
    /// `<up dir>/<sub>.log`, so `@.log` for the apex and `v1.api.log` for
    /// `v1.api`. `sub` must be a validated subdomain (`@` or labels), which
    /// can't leave the folder.
    pub fn up_log(&self, project: &str, sub: &str) -> PathBuf {
        self.up_dir(project).join(format!("{sub}.log"))
    }

    /// Where [`Paths::up_log`] goes when it would grow past its cap:
    /// `<up dir>/<sub>.log.1`.
    pub fn up_log_rotated(&self, project: &str, sub: &str) -> PathBuf {
        self.up_dir(project).join(format!("{sub}.log.1"))
    }

    /// Creates the base directory (0700) if needed.
    pub fn ensure_base(&self) -> Result<()> {
        crate::util::create_private_dir(&self.base)
    }

    /// The `lokal.toml` for `lokal up` (SPEC §4.11): `start`, then each
    /// parent, and the first regular file of that name wins (a symlink to
    /// one counts; `project_file`'s loaders resolve it to the real file,
    /// whose directory then counts). It lives in the user's repo, not in
    /// lokal's folders, but its path is still built here, like every other
    /// path.
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
    fn a_trust_record_is_named_by_the_sha256_of_the_canonical_path() {
        let p = Paths::sandbox(PathBuf::from("/sb"));
        assert_eq!(p.trusted_dir, PathBuf::from("/sb/trusted"));
        // echo -n /Users/me/shop/lokal.toml | shasum -a 256
        assert_eq!(
            p.trust_record(Path::new("/Users/me/shop/lokal.toml")),
            PathBuf::from(
                "/sb/trusted/290ce42f1ac7768c54b08b954490c02032977dfa73d851dabd5935f82943fdea"
            )
        );
        assert_ne!(
            p.trust_record(Path::new("/Users/me/shop/lokal.toml")),
            p.trust_record(Path::new("/Users/me/shop2/lokal.toml"))
        );
    }

    #[test]
    fn up_files_are_under_the_log_dir() {
        let p = Paths::sandbox(PathBuf::from("/sb"));
        assert_eq!(p.up_dir("shop"), PathBuf::from("/sb/logs/up/shop"));
        assert_eq!(p.up_lock("shop"), PathBuf::from("/sb/logs/up/shop/up.lock"));
        assert_eq!(
            p.up_socket("shop"),
            PathBuf::from("/sb/logs/up/shop/up.sock")
        );
        assert_eq!(
            p.up_detached("shop"),
            PathBuf::from("/sb/logs/up/shop/detached.json")
        );
        assert_eq!(
            p.up_log("shop", "api"),
            PathBuf::from("/sb/logs/up/shop/api.log")
        );
        // The apex keeps its name, and a dotted subdomain stays one file.
        assert_eq!(
            p.up_log("shop", "@"),
            PathBuf::from("/sb/logs/up/shop/@.log")
        );
        assert_eq!(
            p.up_log("shop", "v1.api"),
            PathBuf::from("/sb/logs/up/shop/v1.api.log")
        );
        assert_eq!(
            p.up_log_rotated("shop", "v1.api"),
            PathBuf::from("/sb/logs/up/shop/v1.api.log.1")
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
