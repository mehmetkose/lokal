// PORTING: everything OS-specific lives behind these traits. To add an OS,
// create src/platform/<os>/ implementing them and add a cfg arm in `current()`.
// See SPEC Appendix A (docs/PORTING.md) for what each trait needs on Linux
// and Windows.
//
// No `cfg(target_os)` and no OS API may appear outside this directory; a
// test (tests/seam.rs) enforces it.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::net::{TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::ca::CaCert;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A command that must run as root. Printed before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivilegedCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl PrivilegedCommand {
    /// `sudo security add-trusted-cert …`, for display.
    pub fn display(&self) -> String {
        format!(
            "sudo {}",
            crate::process::display_command(&self.program, &self.args)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustStatus {
    Trusted,
    NotTrusted(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameResult {
    pub fqdn: String,
    /// `Err(reason)` when the name could not be published.
    pub outcome: Result<(), String>,
}

/// Per-name state as shown by `status`: `registered`, `pending` or `failed: <reason>`.
pub type NameStates = Arc<Mutex<BTreeMap<String, String>>>;

/// Listeners handed over by the service manager.
#[derive(Debug, Default)]
pub struct Listeners {
    pub https: Vec<TcpListener>,
    pub http: Vec<TcpListener>,
    /// UDP sockets for the built-in `.lokal` DNS server.
    pub dns: Vec<UdpSocket>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolverStatus {
    /// The OS sends `.<tld>` lookups to lokal's DNS server.
    Installed,
    Missing,
    /// Present but different, or not active (detail).
    Wrong(String),
}

/// Everything the service manager needs to install the background service.
#[derive(Debug, Clone)]
pub struct InstallCtx {
    pub label: String,
    /// Service definition file (the LaunchAgent plist on macOS).
    pub definition: PathBuf,
    pub binary: PathBuf,
    pub log_dir: PathBuf,
    pub stdout_log: PathBuf,
    pub stderr_log: PathBuf,
    /// The daemon binary changed since the service last started.
    pub binary_changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Was not loaded; now started.
    Started,
    /// Definition changed; reloaded.
    Reloaded,
    /// Binary changed; restarted.
    Restarted,
    /// Loaded and unchanged.
    Unchanged,
}

/// Trusting the CA in the OS (and browser) trust stores.
pub trait TrustStore: Send + Sync {
    /// Commands that must run as root to trust the CA. Shown to the user before running.
    // PORTING(linux): copy to /usr/local/share/ca-certificates + update-ca-certificates
    // (Debian/Ubuntu) or `trust anchor --store` (Arch/Fedora); detect by command presence.
    // PORTING(windows): `certutil -addstore Root <ca.pem>` (elevated).
    fn plan_trust(&self, ca: &CaCert) -> Vec<PrivilegedCommand>;
    /// Commands that must run as root to remove the CA from the trust store.
    // PORTING(linux): remove the anchor file and rerun the update tool.
    // PORTING(windows): `certutil -delstore Root <serial>`.
    fn plan_untrust(&self, ca: &CaCert) -> Vec<PrivilegedCommand>;
    /// Does the OS trust a leaf issued by this CA? (no root)
    // PORTING(linux): verify a leaf against the system bundle (openssl/rustls with
    // /etc/ssl/certs). PORTING(windows): CertGetCertificateChain.
    fn is_trusted(&self, ca: &CaCert) -> Result<TrustStatus>;
    /// Is the CA trusted for more than TLS (S/MIME, code signing, …)? Only
    /// asked once `is_trusted` passes. Setup narrows such trust with
    /// `plan_trust`; doctor warns about it (SPEC §6.3).
    // PORTING(linux): anchors in the system bundle have no policy scope: false.
    // PORTING(windows): a Root-store certificate is trusted for every purpose
    // unless its enhanced key usage property (CERT_ENHKEY_USAGE_PROP_ID) limits it.
    fn trust_too_broad(&self, ca: &CaCert) -> Result<bool>;
    /// Is the CA certificate present in the system store at all (trusted or
    /// not)? Decides whether uninstall needs root.
    // PORTING(linux): does the anchor file exist. PORTING(windows): certutil -verifystore.
    fn is_present(&self, ca: &CaCert) -> Result<bool>;
    /// PEM of every lokal CA certificate in the system store (read-only), so
    /// setup and uninstall can still untrust the CA `install.json` records
    /// after its files are gone. Callers match on a SHA-1 they compute.
    // PORTING(linux): read the anchor files lokal installed in the ca-certificates dir.
    // PORTING(windows): `certutil -store Root`, filtered by subject, exported as PEM.
    fn lokal_cas_pem(&self) -> Result<String>;
    /// Per-user extras such as browser profiles. No root. Returns what was done.
    // PORTING(linux): NSS databases (~/.pki/nssdb, Firefox profiles incl. snap) via
    // `certutil` as the user, never as root. PORTING(windows): Firefox enterprise roots pref.
    fn apply_user_extras(&self, ca: &CaCert) -> Result<Vec<String>>;
    /// Undo `apply_user_extras`. Returns what was done.
    fn remove_user_extras(&self) -> Result<Vec<String>>;
    /// Doctor: `None` if not applicable (e.g. Firefox not installed),
    /// else `(ok, detail)`.
    fn user_extras_status(&self) -> Option<(bool, String)>;
    /// PEM of the OS root certificates, for `bundle.pem`.
    // PORTING(linux): /etc/ssl/certs/ca-certificates.crt or /etc/pki/tls/certs/ca-bundle.crt.
    // PORTING(windows): export the ROOT system store.
    fn system_roots_pem(&self) -> Result<String>;
}

/// Makes `.local` names resolve to loopback.
pub trait NamePublisher: Send {
    fn backend_name(&self) -> &'static str;
    /// Make exactly `fqdns` resolve to loopback. Returns per-name results
    /// once settled (for newly added names: once registered or failed).
    // PORTING(linux): Avahi publishes to the LAN; the reliable option is a root
    // oneshot unit (.path-triggered) syncing a managed /etc/hosts block.
    // PORTING(windows): hosts file via an elevated helper, or NRPT rules.
    fn apply<'a>(&'a mut self, fqdns: &'a BTreeSet<String>) -> BoxFuture<'a, Vec<NameResult>>;
    /// Remove every name.
    fn clear(&mut self) -> BoxFuture<'_, ()>;
    /// Live per-name state for `status`.
    fn states(&self) -> NameStates;
}

/// The background service (LaunchAgent on macOS).
pub trait ServiceManager: Send + Sync {
    /// Write the service definition and make sure it runs this binary. No root on macOS.
    // PORTING(linux): systemd user units with socket activation (lokal.socket,
    // ListenStream=127.0.0.1:443…) or a system unit with CAP_NET_BIND_SERVICE.
    // PORTING(windows): a per-user scheduled task or a Windows service.
    fn install(&self, ctx: &InstallCtx) -> Result<InstallOutcome>;
    /// Stop the service and remove its definition.
    // PORTING(linux): systemctl --user disable --now; remove units.
    fn uninstall(&self, ctx: &InstallCtx) -> Result<()>;
    /// Start it (load the definition if needed).
    // PORTING(linux): systemctl --user start lokal.socket lokal.service.
    fn start(&self, ctx: &InstallCtx) -> Result<()>;
    /// Stop it. It stays installed and comes back at next login (or `start`).
    // PORTING(linux): systemctl --user stop.
    fn stop(&self) -> Result<()>;
    /// Restart the daemon process (listeners stay with the service manager).
    // PORTING(linux): systemctl --user restart lokal.service (the socket unit keeps the ports).
    fn restart(&self) -> Result<()>;
    /// Is the service definition installed?
    fn is_installed(&self, ctx: &InstallCtx) -> bool;
    /// Is the service loaded (known to the service manager)?
    fn is_loaded(&self) -> bool;
    /// The definition `install` would write, to detect changes.
    fn render_definition(&self, ctx: &InstallCtx) -> String;
    /// Daemon side: listeners handed over by the service manager (launchd sockets),
    /// or None to make the daemon bind directly.
    // PORTING(linux): sd_listen_fds (LISTEN_FDS/LISTEN_PID env, fds from 3).
    // PORTING(windows): none; bind 127.0.0.1:443 directly (no privilege needed).
    fn inherited_listeners(&self) -> Result<Option<Listeners>>;
}

/// Routes lookups for a TLD (`.lokal`) to lokal's DNS server on loopback.
pub trait SystemResolver: Send + Sync {
    /// The resolver configuration lokal wants for `port` (staged by setup in
    /// the user's data dir, then installed by `plan_install`).
    // PORTING(linux): no file; systemd-resolved split DNS instead (a dummy link with
    // `resolvectl dns lokal0 127.0.0.1:<port>` + `resolvectl domain lokal0 '~lokal'`).
    // PORTING(windows): an NRPT rule (Add-DnsClientNrptRule -Namespace .lokal); NRPT can't
    // take a port, so the server would need 127.0.0.1:53.
    fn config(&self, port: u16) -> String;
    /// Where it lives, for messages (`/etc/resolver/lokal`).
    // PORTING(linux): "systemd-resolved (link lokal0)". PORTING(windows): "NRPT rule .lokal".
    fn location(&self, tld: &str) -> String;
    /// Commands that must run as root to install `staged` for `.<tld>`.
    // PORTING(linux): the resolvectl commands above (as root), plus a unit to persist them.
    fn plan_install(&self, tld: &str, staged: &Path) -> Vec<PrivilegedCommand>;
    /// Commands that must run as root to remove it.
    // PORTING(linux): resolvectl revert lokal0; remove the link/unit.
    fn plan_uninstall(&self, tld: &str) -> Vec<PrivilegedCommand>;
    /// Is it installed with the right server, and active? (no root)
    // PORTING(linux): `resolvectl status lokal0`. PORTING(windows): Get-DnsClientNrptRule.
    fn status(&self, tld: &str, port: u16) -> ResolverStatus;
}

pub trait Desktop: Send + Sync {
    // PORTING(linux): xdg-open. PORTING(windows): ShellExecuteW / `cmd /c start`.
    fn open_url(&self, url: &str) -> Result<()>;
}

/// OS facts and privileged execution.
pub trait System: Send + Sync {
    fn is_root(&self) -> bool;
    /// `(user, short hostname)` for the CA's common name.
    fn user_and_host(&self) -> (String, String);
    /// Runs one privileged command with the terminal attached
    /// (sudo password prompt). Only `setup` and `uninstall` call this.
    // PORTING(linux): sudo as here. PORTING(windows): an elevated child (UAC).
    fn run_privileged(&self, cmd: &PrivilegedCommand, prompt: &str) -> Result<()>;
    /// Which process listens on TCP `port` (for doctor), if any.
    // PORTING(linux): ss -ltnp. PORTING(windows): netstat -ano.
    fn port_owner(&self, port: u16) -> Option<String>;
    /// TCP ports that process `pid` and every process descended from it
    /// listen on, sorted (for `lokal run`'s wrong-port hint: the command and
    /// the processes it started). Empty if none, or unknown.
    // PORTING(linux): the ppid of every process from /proc/<pid>/stat, walked with
    // `process_tree`, then `ss -ltnp` filtered to those pids.
    // PORTING(windows): a job object holding the command's processes, or the parent
    // pids from a process snapshot (CreateToolhelp32Snapshot) walked with
    // `process_tree`, then `netstat -ano` filtered to those pids.
    fn tcp_listen_ports(&self, pid: u32) -> Vec<u16>;
    /// The static hosts file, for the conflict check.
    // PORTING(windows): C:\Windows\System32\drivers\etc\hosts.
    fn hosts_file(&self) -> Option<PathBuf>;
    /// Short OS description for reports (`macOS 27.0`).
    fn os_version(&self) -> String;
}

pub struct Platform {
    pub trust: Box<dyn TrustStore>,
    pub resolver: Box<dyn SystemResolver>,
    pub service: Box<dyn ServiceManager>,
    pub desktop: Box<dyn Desktop>,
    pub system: Box<dyn System>,
    /// Name of the default name backend (`mdns-local-only` on macOS).
    pub default_name_backend: &'static str,
}

/// The current OS's implementation.
pub fn current() -> Platform {
    #[cfg(target_os = "macos")]
    {
        macos::platform()
    }
    #[cfg(not(target_os = "macos"))]
    {
        unsupported::platform()
    }
}

/// Creates the name publisher `backend` (`none`, or a platform backend).
/// Must be called inside a tokio runtime.
pub fn name_publisher(backend: &str) -> Result<Box<dyn NamePublisher>> {
    if backend == "none" {
        return Ok(Box::new(NullPublisher::default()));
    }
    #[cfg(target_os = "macos")]
    {
        macos::name_publisher(backend)
    }
    #[cfg(not(target_os = "macos"))]
    {
        unsupported::name_publisher(backend)
    }
}

/// `--name-backend none`: names are not published (sandbox tests connect
/// by address). Every name reports `registered` immediately.
#[derive(Default)]
pub struct NullPublisher {
    states: NameStates,
}

impl NamePublisher for NullPublisher {
    fn backend_name(&self) -> &'static str {
        "none"
    }

    fn apply<'a>(&'a mut self, fqdns: &'a BTreeSet<String>) -> BoxFuture<'a, Vec<NameResult>> {
        Box::pin(async move {
            let mut st = self.states.lock().expect("lock");
            st.clear();
            fqdns
                .iter()
                .map(|f| {
                    st.insert(f.clone(), "registered".into());
                    NameResult {
                        fqdn: f.clone(),
                        outcome: Ok(()),
                    }
                })
                .collect()
        })
    }

    fn clear(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.states.lock().expect("lock").clear();
        })
    }

    fn states(&self) -> NameStates {
        self.states.clone()
    }
}

/// CLI only: die quietly on a closed stdout pipe (`lokal list | head -1`)
/// instead of panicking in `println!`, like other command-line tools. The
/// daemon must not call this: it relies on EPIPE errors from sockets.
pub fn restore_default_sigpipe() {
    // PORTING(windows): no SIGPIPE; nothing to do.
    #[cfg(unix)]
    // SAFETY: setting a signal disposition to SIG_DFL has no preconditions.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// A signal `lokal run` catches and may relay to its command (SPEC §4.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaySignal {
    /// SIGINT (Ctrl-C)
    Interrupt,
    /// SIGQUIT (Ctrl-\)
    Quit,
    /// SIGTERM
    Terminate,
    /// SIGHUP
    Hangup,
}

impl RelaySignal {
    /// Sent by the terminal to its whole foreground process group, so a
    /// command in lokal's group already has it when lokal does.
    pub fn from_terminal_keys(self) -> bool {
        matches!(self, RelaySignal::Interrupt | RelaySignal::Quit)
    }

    // PORTING(windows): no POSIX signals; map to CTRL_C_EVENT/CTRL_BREAK_EVENT.
    fn number(self) -> libc::c_int {
        match self {
            RelaySignal::Interrupt => libc::SIGINT,
            RelaySignal::Quit => libc::SIGQUIT,
            RelaySignal::Terminate => libc::SIGTERM,
            RelaySignal::Hangup => libc::SIGHUP,
        }
    }
}

/// `lokal run`: catches SIGINT, SIGQUIT, SIGTERM and SIGHUP, so lokal
/// outlives them and can relay them to its command. These are handlers,
/// never `SIG_IGN`: `exec` resets caught signals to their default in the
/// command, but an ignored signal would stay ignored there (a dev server
/// that ignores Ctrl-C). Create it inside a tokio runtime, before the
/// command starts.
// PORTING(windows): no POSIX signals; tokio::signal::windows (ctrl_c,
// ctrl_break, ctrl_close). A console Ctrl-C reaches every process attached
// to the console, like the foreground process group here.
pub struct RelaySignals {
    int: tokio::signal::unix::Signal,
    quit: tokio::signal::unix::Signal,
    term: tokio::signal::unix::Signal,
    hup: tokio::signal::unix::Signal,
}

impl RelaySignals {
    pub fn install() -> Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            int: signal(SignalKind::interrupt())?,
            quit: signal(SignalKind::quit())?,
            term: signal(SignalKind::terminate())?,
            hup: signal(SignalKind::hangup())?,
        })
    }

    /// The next signal lokal received. Cancel-safe.
    pub async fn recv(&mut self) -> RelaySignal {
        tokio::select! {
            _ = self.int.recv() => RelaySignal::Interrupt,
            _ = self.quit.recv() => RelaySignal::Quit,
            _ = self.term.recv() => RelaySignal::Terminate,
            _ = self.hup.recv() => RelaySignal::Hangup,
        }
    }
}

/// Sends `sig` to process `pid` (only that process, not its group).
// PORTING(windows): GenerateConsoleCtrlEvent for Ctrl-C/Ctrl-Break; otherwise
// TerminateProcess (or the job object) for SIGTERM/SIGHUP.
pub fn send_signal(pid: u32, sig: RelaySignal) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill only sends a signal to the given positive pid.
    unsafe { libc::kill(pid, sig.number()) == 0 }
}

/// Is this process in the foreground process group of its controlling
/// terminal (`tcgetpgrp` on `/dev/tty` equals `getpgrp()`)? False when
/// there is no controlling terminal.
// PORTING(windows): no controlling terminal or process groups; a console
// Ctrl-C reaches every attached process, so treat a console as "foreground".
pub fn in_terminal_foreground() -> bool {
    // SAFETY: open/tcgetpgrp/close only use the fd they get; the path is a
    // valid C string.
    unsafe {
        let fd = libc::open(
            c"/dev/tty".as_ptr(),
            libc::O_RDONLY | libc::O_NOCTTY | libc::O_CLOEXEC,
        );
        if fd < 0 {
            return false;
        }
        let fg = libc::tcgetpgrp(fd);
        libc::close(fd);
        fg >= 0 && fg == libc::getpgrp()
    }
}

/// A child's exit status as a shell reports it: the exit code, or 128 + the
/// signal number when a signal killed it.
// PORTING(windows): no signals; the exit code is all there is.
pub fn shell_exit_code(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(c), _) => c,
        (None, Some(s)) => 128 + s,
        (None, None) => 1,
    }
}

/// `root` and every process descended from it, root first, from `(pid,
/// ppid)` pairs read from the process table at one moment. Empty when `root`
/// isn't in the table (it has exited) or is 0 (the kernel). Each pid is
/// visited at most once, so bad data (a cycle, a process listed as its own
/// parent, a pid listed twice) can't make it loop.
pub fn process_tree(table: &[(u32, u32)], root: u32) -> Vec<u32> {
    if root == 0 || !table.iter().any(|(pid, _)| *pid == root) {
        return vec![];
    }
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (pid, ppid) in table {
        children.entry(*ppid).or_default().push(*pid);
    }
    let mut seen = BTreeSet::from([root]);
    let mut tree = vec![root];
    let mut i = 0;
    while i < tree.len() {
        for c in children.get(&tree[i]).into_iter().flatten() {
            if seen.insert(*c) {
                tree.push(*c);
            }
        }
        i += 1;
    }
    tree
}

/// Daemon only: raises the soft open-file limit (RLIMIT_NOFILE) to `want`,
/// or to the hard limit if that's lower, and returns the soft limit in
/// effect afterwards. launchd starts agents at 256; the daemon sizes its
/// connection caps from the result (SPEC §9.3).
// PORTING(linux): the same call works; hard limits are often 4096 or 524288.
// PORTING(windows): no per-process descriptor limit; return `want`.
// macOS refuses a soft limit above OPEN_MAX (10240) or RLIM_INFINITY for
// RLIMIT_NOFILE (setrlimit(2), COMPATIBILITY), so callers ask for at most that.
#[allow(clippy::unnecessary_cast)] // rlim_t is u64 on macOS and Linux, not on every unix
pub fn raise_open_file_limit(want: u64) -> u64 {
    #[cfg(unix)]
    {
        let mut rl = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: getrlimit/setrlimit only read or write the struct passed.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rl) } != 0 {
            return 256;
        }
        let target = (want as libc::rlim_t).min(rl.rlim_max);
        if target > rl.rlim_cur {
            let new = libc::rlimit {
                rlim_cur: target,
                rlim_max: rl.rlim_max,
            };
            if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &new) } == 0 {
                return target as u64;
            }
        }
        rl.rlim_cur as u64
    }
    #[cfg(not(unix))]
    {
        want
    }
}

pub const UNSUPPORTED: &str = "lokal currently supports macOS only";

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<u32>) -> Vec<u32> {
        v.sort_unstable();
        v
    }

    /// launchd (1) → Terminal (40) → zsh (50) → lokal (60) → the command.
    fn base() -> Vec<(u32, u32)> {
        vec![(0, 0), (1, 0), (40, 1), (50, 40), (60, 50)]
    }

    #[test]
    fn walks_a_deep_chain() {
        // npm (100) → node (101) → vite (102) → esbuild (103)
        let mut t = base();
        t.extend([(100, 60), (101, 100), (102, 101), (103, 102)]);
        assert_eq!(process_tree(&t, 100), [100, 101, 102, 103]);
        // Starting lower down only takes what's below.
        assert_eq!(process_tree(&t, 102), [102, 103]);
    }

    #[test]
    fn takes_every_child_and_skips_the_roots_siblings() {
        // The command (100) has two children, one with a child of its own.
        // lokal (60) has another child (90), and zsh started a server (95)
        // in the same process group: neither belongs to the command.
        let mut t = base();
        t.extend([
            (100, 60),
            (101, 100),
            (102, 100),
            (103, 101),
            (90, 60),
            (95, 50),
        ]);
        assert_eq!(sorted(process_tree(&t, 100)), [100, 101, 102, 103]);
    }

    #[test]
    fn a_pid_that_is_gone_has_no_tree() {
        let mut t = base();
        // Its child was already reparented to launchd, but a stale pair
        // may still name the gone pid as a parent.
        t.extend([(101, 1), (102, 555)]);
        assert!(process_tree(&t, 555).is_empty());
        assert!(process_tree(&[], 555).is_empty());
        assert!(process_tree(&t, 0).is_empty(), "never walk from the kernel");
    }

    #[test]
    fn bad_data_does_not_loop() {
        // A cycle, a process that is its own parent, and a pid listed twice.
        let mut t = base();
        t.extend([(200, 201), (201, 200), (202, 202), (203, 200), (203, 201)]);
        assert_eq!(sorted(process_tree(&t, 200)), [200, 201, 203]);
        assert_eq!(process_tree(&t, 202), [202]);
    }
}
