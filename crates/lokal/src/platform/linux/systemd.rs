//! The background service on Linux: systemd user units, the equivalent of
//! macOS's LaunchAgent (SPEC §8.1, VERIFY V12). Three socket units hold the
//! ports (HTTPS and HTTP on 127.0.0.1 and ::1, and DNS on UDP 127.0.0.1) and
//! hand them to the service, so connections wait in the backlog while the
//! daemon restarts. Each kind has its own unit because `FileDescriptorName=`
//! names every fd of a unit, and the daemon finds its sockets by name: their
//! order across units changes from one start to the next. Everything runs in
//! the user's manager (`systemctl --user`), with no root, except that a user
//! unit can't bind below `net.ipv4.ip_unprivileged_port_start` (1024 by
//! default): `port_config` lowers it once, as root.

use std::net::{TcpListener, UdpSocket};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::paths::LABEL;
use crate::platform::sd_listen;
use crate::platform::{InstallCtx, InstallOutcome, Listeners, PrivilegedCommand, ServiceManager};
use crate::process;
use crate::util;

/// Where systemctl may be, in order: an absolute path, so `$PATH` can't swap it.
const SYSTEMCTL: [&str; 2] = ["/usr/bin/systemctl", "/bin/systemctl"];

/// The kernel setting that keeps a user's unit off ports 443 and 80.
const PORT_START: &str = "net.ipv4.ip_unprivileged_port_start";
const PORT_START_PROC: &str = "/proc/sys/net/ipv4/ip_unprivileged_port_start";
/// The kernel's default for it.
const PORT_START_DEFAULT: u16 = 1024;
/// The file `plan_install` puts in place, read at boot by systemd-sysctl.
const SYSCTL_FILE: &str = "/etc/sysctl.d/50-lokal.conf";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Https,
    Http,
    /// UDP, for the built-in `.lokal` DNS server.
    Dns,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Https, Kind::Http, Kind::Dns];

    /// Its `FileDescriptorName=`, and the end of its socket unit's name.
    fn name(self) -> &'static str {
        match self {
            Kind::Https => "https",
            Kind::Http => "http",
            Kind::Dns => "dns",
        }
    }
}

pub struct Systemd {
    /// Names the units: `<label>.service` and `<label>-https.socket`, ….
    label: String,
    https: u16,
    http: u16,
    dns: u16,
}

impl Default for Systemd {
    fn default() -> Self {
        Self {
            label: LABEL.into(),
            https: 443,
            http: 80,
            dns: crate::dns::DEFAULT_PORT,
        }
    }
}

/// `systemctl --user <args>`.
fn systemctl(args: &[&str]) -> Result<process::CmdOutput> {
    let Some(bin) = SYSTEMCTL.into_iter().find(|p| Path::new(p).is_file()) else {
        bail!("systemctl not found (lokal's background service needs systemd)");
    };
    let mut all = vec!["--user"];
    all.extend_from_slice(args);
    process::run(bin, &all)
}

fn systemctl_ok(args: &[&str]) -> Result<()> {
    let out = systemctl(args)?;
    if !out.success() {
        bail!(
            "`systemctl --user {}` failed ({}): {}{}",
            args.join(" "),
            out.status,
            out.stdout.trim(),
            out.stderr.trim()
        );
    }
    Ok(())
}

/// A value in a unit file: `%` is a specifier there, so it's doubled.
fn unit_value(s: &str) -> String {
    s.replace('%', "%%")
}

/// One word of `ExecStart=`: specifiers (`%`) and variables (`$`) doubled,
/// and quoted when it has spaces, quotes or backslashes.
fn exec_word(s: &str) -> String {
    let s = unit_value(s).replace('$', "$$");
    if s.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '\\') {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s
    }
}

/// The lowest port a user's process may bind, or None if it can't be read.
fn port_start() -> Option<u16> {
    std::fs::read_to_string(PORT_START_PROC)
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The first of `paths` that exists, else the first one.
fn program(paths: &[&str]) -> String {
    paths
        .iter()
        .find(|p| Path::new(p).is_file())
        .unwrap_or(&paths[0])
        .to_string()
}

impl Systemd {
    fn service(&self) -> String {
        format!("{}.service", self.label)
    }

    fn socket(&self, kind: Kind) -> String {
        format!("{}-{}.socket", self.label, kind.name())
    }

    /// The units to start or stop: the sockets, then the service.
    fn units(&self) -> Vec<String> {
        let mut u: Vec<String> = Kind::ALL.iter().map(|k| self.socket(*k)).collect();
        u.push(self.service());
        u
    }

    fn render_socket(&self, kind: Kind) -> String {
        let listen = match kind {
            Kind::Https | Kind::Http => {
                let port = if kind == Kind::Https {
                    self.https
                } else {
                    self.http
                };
                format!("ListenStream=127.0.0.1:{port}\nListenStream=[::1]:{port}\n")
            }
            Kind::Dns => format!("ListenDatagram=127.0.0.1:{}\n", self.dns),
        };
        format!(
            "[Unit]\nDescription=lokal's {name} sockets\n\n[Socket]\n{listen}FileDescriptorName={name}\nService={service}\n\n[Install]\nWantedBy=sockets.target\n",
            name = kind.name(),
            service = self.service(),
        )
    }

    /// Every unit file with what `install` writes to it: the service at
    /// `ctx.definition`, the sockets next to it.
    fn files(&self, ctx: &InstallCtx) -> Vec<(PathBuf, String)> {
        let dir = ctx.definition.parent().unwrap_or(Path::new("."));
        let mut files = vec![(ctx.definition.clone(), self.render_definition(ctx))];
        for kind in Kind::ALL {
            files.push((dir.join(self.socket(kind)), self.render_socket(kind)));
        }
        files
    }

    fn is_active(&self, unit: &str) -> bool {
        systemctl(&["is-active", "--quiet", unit]).is_ok_and(|o| o.success())
    }

    /// The port the kernel must let users bind from, if it doesn't yet:
    /// `current` is `ip_unprivileged_port_start`, None if unknown.
    fn port_start_needed(&self, current: Option<u16>) -> Option<u16> {
        let lowest = self.https.min(self.http);
        (!current.is_some_and(|c| c <= lowest)).then_some(lowest)
    }

    fn all_units(&self, verb: &str) -> Result<()> {
        let units = self.units();
        let mut args = vec![verb];
        args.extend(units.iter().map(String::as_str));
        systemctl_ok(&args)
    }
}

impl ServiceManager for Systemd {
    fn name(&self) -> &'static str {
        "systemd"
    }

    fn render_definition(&self, ctx: &InstallCtx) -> String {
        let sockets: Vec<String> = Kind::ALL.iter().map(|k| self.socket(*k)).collect();
        let sockets = sockets.join(" ");
        format!(
            "[Unit]\nDescription=lokal, local HTTPS names for dev servers\nRequires={sockets}\nAfter={sockets}\n\n\
             [Service]\nExecStart={bin} daemon run\nSockets={sockets}\n\
             # names are published only while the daemon runs, so it must always run\nRestart=always\n\
             # respawn a crashed daemon after 1s; its sockets queue connections meanwhile (VERIFY V12)\nRestartSec=1\n\
             Environment=RUST_LOG=info\nStandardOutput=append:{out}\nStandardError=append:{err}\n\n\
             [Install]\nWantedBy=default.target\n",
            bin = exec_word(&ctx.binary.display().to_string()),
            out = unit_value(&ctx.stdout_log.display().to_string()),
            err = unit_value(&ctx.stderr_log.display().to_string()),
        )
    }

    fn definition_current(&self, ctx: &InstallCtx) -> bool {
        self.files(ctx)
            .iter()
            .all(|(path, want)| std::fs::read_to_string(path).ok().as_deref() == Some(want))
    }

    fn install(&self, ctx: &InstallCtx) -> Result<InstallOutcome> {
        util::create_private_dir(&ctx.log_dir)?;
        let mut changed = false;
        for (path, want) in self.files(ctx) {
            if std::fs::read_to_string(&path).ok().as_deref() != Some(want.as_str()) {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                util::write_atomic(&path, want.as_bytes(), 0o644)?;
                changed = true;
            }
        }
        if changed {
            systemctl_ok(&["daemon-reload"])?;
        }
        if !self.is_loaded() {
            self.all_units("enable")?;
            self.all_units("start")?;
            Ok(InstallOutcome::Started)
        } else if changed {
            // A socket that changed must close its old ports, and it can't
            // while the service runs on them.
            self.all_units("enable")?;
            systemctl_ok(&["stop", &self.service()])?;
            self.all_units("restart")?;
            Ok(InstallOutcome::Reloaded)
        } else if ctx.binary_changed {
            self.restart()?;
            Ok(InstallOutcome::Restarted)
        } else {
            Ok(InstallOutcome::Unchanged)
        }
    }

    fn uninstall(&self, ctx: &InstallCtx) -> Result<()> {
        // Either may fail for units that are already gone; what counts is
        // that the sockets are closed after.
        let _ = self.all_units("disable");
        let _ = self.all_units("stop");
        if self.is_loaded() {
            bail!("lokal's sockets are still active after `systemctl --user stop`");
        }
        for (path, _) in self.files(ctx) {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        systemctl_ok(&["daemon-reload"])?;
        // A unit that failed stays listed until reset; it's gone now.
        let _ = self.all_units("reset-failed");
        Ok(())
    }

    fn start(&self, _ctx: &InstallCtx) -> Result<()> {
        self.all_units("start")
    }

    fn stop(&self) -> Result<()> {
        // One transaction: systemd stops the service before its sockets.
        self.all_units("stop")
    }

    fn restart(&self) -> Result<()> {
        systemctl_ok(&["restart", &self.service()])
    }

    fn is_installed(&self, ctx: &InstallCtx) -> bool {
        self.files(ctx).iter().all(|(path, _)| path.is_file())
    }

    fn is_loaded(&self) -> bool {
        self.is_active(&self.socket(Kind::Https))
    }

    fn inherited_listeners(&self) -> Result<Option<Listeners>> {
        let env = |k: &str| std::env::var(k).ok();
        let fds = sd_listen::passed_fds(
            std::process::id(),
            env("LISTEN_PID").as_deref(),
            env("LISTEN_FDS").as_deref(),
            env("LISTEN_FDNAMES").as_deref(),
        );
        let fds = match fds {
            Ok(Some(fds)) => fds,
            Ok(None) => return Ok(None),
            Err(e) => bail!("systemd passed sockets lokal can't read: {e}"),
        };
        // LISTEN_* stay set: a child's pid isn't LISTEN_PID, so it ignores
        // them, and unsetting them needs a process with no other threads.
        let mut l = Listeners::default();
        for (fd, name) in fds {
            // SAFETY: fcntl on an fd systemd passed to this process.
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
            match Kind::ALL.into_iter().find(|k| k.name() == name) {
                // SAFETY: systemd hands this process ownership of a
                // listening socket (stream) or a bound one (datagram).
                Some(Kind::Https) => l.https.push(unsafe { TcpListener::from_raw_fd(fd) }),
                Some(Kind::Http) => l.http.push(unsafe { TcpListener::from_raw_fd(fd) }),
                Some(Kind::Dns) => l.dns.push(unsafe { UdpSocket::from_raw_fd(fd) }),
                None => {
                    tracing::warn!("systemd passed a socket named {name:?}; closing it");
                    // SAFETY: the fd is this process's, and nothing else uses it.
                    unsafe { libc::close(fd) };
                }
            }
        }
        Ok(Some(l))
    }

    fn port_config(&self) -> Option<String> {
        let lowest = self.port_start_needed(port_start())?;
        Some(format!(
            "# lokal: lets a user's service bind ports {lowest} and up, for lokal's\n\
             # HTTPS and HTTP sockets on 127.0.0.1 and ::1. Removed by 'lokal uninstall'.\n\
             {PORT_START} = {lowest}\n"
        ))
    }

    fn plan_install(&self, staged: &Path) -> Vec<PrivilegedCommand> {
        let Some(lowest) = self.port_start_needed(port_start()) else {
            return vec![];
        };
        vec![
            PrivilegedCommand {
                program: program(&["/usr/bin/install", "/bin/install"]),
                args: vec![
                    "-m".into(),
                    "0644".into(),
                    staged.display().to_string(),
                    SYSCTL_FILE.into(),
                ],
            },
            PrivilegedCommand {
                program: program(&["/usr/sbin/sysctl", "/sbin/sysctl"]),
                args: vec!["-w".into(), format!("{PORT_START}={lowest}")],
            },
        ]
    }

    fn plan_uninstall(&self) -> Vec<PrivilegedCommand> {
        if !Path::new(SYSCTL_FILE).is_file() {
            return vec![];
        }
        vec![
            PrivilegedCommand {
                program: program(&["/usr/bin/rm", "/bin/rm"]),
                args: vec!["-f".into(), SYSCTL_FILE.into()],
            },
            PrivilegedCommand {
                program: program(&["/usr/sbin/sysctl", "/sbin/sysctl"]),
                args: vec!["-w".into(), format!("{PORT_START}={PORT_START_DEFAULT}")],
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &Path) -> InstallCtx {
        InstallCtx {
            label: LABEL.into(),
            definition: dir.join("systemd/user/dev.lokal.daemon.service"),
            binary: PathBuf::from("/home/me/.local/share/lokal/bin/lokal"),
            log_dir: dir.join("logs"),
            stdout_log: dir.join("logs/launchd.out.log"),
            stderr_log: dir.join("logs/launchd.err.log"),
            binary_changed: false,
        }
    }

    #[test]
    fn renders_one_socket_unit_per_kind() {
        let s = Systemd::default();
        assert_eq!(
            s.render_socket(Kind::Https),
            "[Unit]\nDescription=lokal's https sockets\n\n[Socket]\nListenStream=127.0.0.1:443\nListenStream=[::1]:443\nFileDescriptorName=https\nService=dev.lokal.daemon.service\n\n[Install]\nWantedBy=sockets.target\n"
        );
        assert!(s.render_socket(Kind::Http).contains(
            "ListenStream=127.0.0.1:80\nListenStream=[::1]:80\nFileDescriptorName=http\n"
        ));
        assert!(s.render_socket(Kind::Dns).contains(&format!(
            "ListenDatagram=127.0.0.1:{}\nFileDescriptorName=dns\n",
            crate::dns::DEFAULT_PORT
        )));
    }

    #[test]
    fn renders_the_service() {
        let dir = Path::new("/home/me/.config");
        let unit = Systemd::default().render_definition(&ctx(dir));
        let sockets = "dev.lokal.daemon-https.socket dev.lokal.daemon-http.socket dev.lokal.daemon-dns.socket";
        assert!(unit.contains(&format!("Requires={sockets}\nAfter={sockets}\n")));
        assert!(unit.contains(&format!("Sockets={sockets}\n")));
        assert!(unit.contains("ExecStart=/home/me/.local/share/lokal/bin/lokal daemon run\n"));
        assert!(unit.contains("Restart=always\n"));
        assert!(unit.contains("RestartSec=1\n"));
        assert!(unit.contains("StandardOutput=append:/home/me/.config/logs/launchd.out.log\n"));
        assert!(unit.ends_with("[Install]\nWantedBy=default.target\n"));
    }

    #[test]
    fn the_unit_files_sit_together() {
        let dir = Path::new("/x");
        let files: Vec<PathBuf> = Systemd::default()
            .files(&ctx(dir))
            .into_iter()
            .map(|f| f.0)
            .collect();
        let unit_dir = Path::new("/x/systemd/user");
        assert_eq!(
            files,
            [
                unit_dir.join("dev.lokal.daemon.service"),
                unit_dir.join("dev.lokal.daemon-https.socket"),
                unit_dir.join("dev.lokal.daemon-http.socket"),
                unit_dir.join("dev.lokal.daemon-dns.socket"),
            ]
        );
    }

    #[test]
    fn escapes_unit_values() {
        assert_eq!(exec_word("/usr/bin/lokal"), "/usr/bin/lokal");
        assert_eq!(exec_word("/home/a b/lokal"), "\"/home/a b/lokal\"");
        assert_eq!(exec_word("/home/100%/$x"), "/home/100%%/$$x");
        assert_eq!(exec_word("/q\"\\"), "\"/q\\\"\\\\\"");
        assert_eq!(unit_value("/logs/50%.log"), "/logs/50%%.log");
    }

    #[test]
    fn lowers_the_port_limit_only_when_its_too_high() {
        let s = Systemd::default();
        assert_eq!(s.port_start_needed(Some(1024)), Some(80));
        assert_eq!(s.port_start_needed(Some(81)), Some(80));
        assert_eq!(s.port_start_needed(Some(80)), None);
        assert_eq!(s.port_start_needed(Some(0)), None);
        // Unknown: plan it; sysctl says what's wrong if it can't.
        assert_eq!(s.port_start_needed(None), Some(80));
        let high = Systemd {
            https: 18443,
            http: 18080,
            ..Systemd::default()
        };
        assert_eq!(high.port_start_needed(Some(1024)), None);
    }

    #[test]
    fn plans_the_sysctl_file_then_the_live_value() {
        let s = Systemd::default();
        let plan = s.plan_install(Path::new("/s/50-lokal.conf"));
        // On a machine that already allows port 80 there is nothing to do.
        if port_start().is_some_and(|p| p <= 80) {
            assert!(plan.is_empty());
            return;
        }
        assert_eq!(plan.len(), 2);
        assert!(plan[0].program.ends_with("/install"));
        assert_eq!(
            plan[0].args,
            ["-m", "0644", "/s/50-lokal.conf", SYSCTL_FILE]
        );
        assert!(plan[1].program.ends_with("/sysctl"));
        assert_eq!(
            plan[1].args,
            ["-w", "net.ipv4.ip_unprivileged_port_start=80"]
        );
        assert_eq!(
            s.port_config().unwrap().lines().last(),
            Some("net.ipv4.ip_unprivileged_port_start = 80")
        );
    }

    /// Removes the e2e units even when the test fails.
    struct Installed(Systemd, InstallCtx);

    impl Drop for Installed {
        fn drop(&mut self) {
            let _ = self.0.uninstall(&self.1);
        }
    }

    /// The real thing, end to end: units in the user's own
    /// `~/.config/systemd/user` under a test label, run by the user's
    /// manager, starting the `lokal` built next to this test (`cargo build`
    /// first) on a sandbox (`LOKAL_HOME`). High free ports by default; set
    /// `LOKAL_E2E_PORTS=443,80,53535` to try lokal's own, which needs
    /// `ip_unprivileged_port_start` at 80 or less.
    ///
    /// cargo build -p lokal && cargo test -p lokal --lib systemd_e2e -- --ignored
    #[test]
    #[ignore = "drives the user's systemd manager"]
    fn systemd_e2e() {
        use std::io::{Read, Write};
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;

        use crate::setup::wait_for_ping;

        if !systemctl(&["is-system-running"])
            .is_ok_and(|o| o.success() || o.stdout.trim() == "degraded")
        {
            eprintln!("skipped: no systemd user manager");
            return;
        }
        let exe = std::env::current_exe().unwrap();
        let lokal = exe.parent().unwrap().parent().unwrap().join("lokal");
        assert!(lokal.is_file(), "build lokal first: {}", lokal.display());
        let free = || {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let (https, http, dns) = match std::env::var("LOKAL_E2E_PORTS") {
            Ok(v) => {
                let p: Vec<u16> = v.split(',').map(|x| x.trim().parse().unwrap()).collect();
                (p[0], p[1], p[2])
            }
            Err(_) => (free(), free(), free()),
        };
        let home = tempfile::Builder::new().prefix("lk").tempdir().unwrap();
        let st = std::process::Command::new(&lokal)
            .env("LOKAL_HOME", home.path())
            .args(["internal", "create-ca"])
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "{}",
            String::from_utf8_lossy(&st.stderr)
        );
        // The service runs `<binary> daemon run`; this runs lokal on the
        // sandbox instead.
        let wrapper = home.path().join("lokal-e2e");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexec env LOKAL_HOME='{h}' '{l}' \"$@\" --base-dir '{h}' --name-backend none\n",
                h = home.path().display(),
                l = lokal.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()).join(".config"));
        let label = "dev.lokal.e2e";
        let paths = crate::paths::Paths::sandbox(home.path().to_path_buf());
        let ctx = InstallCtx {
            label: label.into(),
            definition: config.join(format!("systemd/user/{label}.service")),
            binary: wrapper,
            log_dir: paths.log_dir.clone(),
            stdout_log: paths.launchd_out.clone(),
            stderr_log: paths.launchd_err.clone(),
            binary_changed: false,
        };
        let make = || Systemd {
            label: label.into(),
            https,
            http,
            dns,
        };
        let s = make();
        let installed = Installed(make(), ctx.clone());
        let ping = |not_pid: Option<u32>| {
            wait_for_ping(&paths, None, not_pid, Duration::from_secs(10)).unwrap_or_else(|e| {
                let err = std::fs::read_to_string(&ctx.stderr_log).unwrap_or_default();
                panic!("no ping: {e:#}\n{err}")
            })
        };

        assert_eq!(s.install(&ctx).unwrap(), InstallOutcome::Started);
        assert!(s.is_installed(&ctx) && s.is_loaded() && s.definition_current(&ctx));
        let first = ping(None);
        // The daemon got its listeners from systemd, by name.
        let daemon_log =
            std::fs::read_to_string(paths.log_dir.join("daemon.log")).unwrap_or_default();
        let ready = daemon_log
            .lines()
            .rev()
            .find(|l| l.contains("ready in"))
            .unwrap_or_default()
            .to_string();
        assert!(ready.contains("[systemd, "), "{ready}");
        for port in [https, http] {
            assert!(
                ready.contains(&format!("127.0.0.1:{port}"))
                    && ready.contains(&format!("[::1]:{port}")),
                "{ready}"
            );
        }
        assert!(ready.contains(&format!("127.0.0.1:{dns}/udp")), "{ready}");
        // HTTP answers.
        let mut c = std::net::TcpStream::connect(("127.0.0.1", http)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"GET / HTTP/1.1\r\nHost: x.lokal\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut answer = String::new();
        let _ = c.read_to_string(&mut answer);
        assert!(answer.starts_with("HTTP/1.1 "), "{answer:?}");

        // Unchanged, then a restart: a new daemon on the same sockets.
        assert_eq!(s.install(&ctx).unwrap(), InstallOutcome::Unchanged);
        s.restart().unwrap();
        let second = ping(Some(first.pid));
        // A crash: systemd starts it again after RestartSec.
        // SAFETY: kill has no memory preconditions.
        unsafe { libc::kill(second.pid as i32, libc::SIGKILL) };
        let third = ping(Some(second.pid));
        // binary_changed restarts it.
        let changed = InstallCtx {
            binary_changed: true,
            ..ctx.clone()
        };
        assert_eq!(s.install(&changed).unwrap(), InstallOutcome::Restarted);
        let fourth = ping(Some(third.pid));
        // Stop, then start.
        s.stop().unwrap();
        assert!(!s.is_loaded());
        assert!(crate::ipc::ping(&paths, Duration::from_millis(300)).is_err());
        s.start(&ctx).unwrap();
        ping(Some(fourth.pid));

        drop(installed);
        assert!(!s.is_loaded());
        assert!(!s.is_installed(&ctx));
        assert!(!ctx.definition.exists());
    }
}
