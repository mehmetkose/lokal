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

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
// Linux's parsers, compiled everywhere so that every OS's `cargo test` runs
// their tests.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod procfs;
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
    // Linux: /usr/bin/xdg-open (platform/linux).
    // PORTING(windows): ShellExecuteW / `cmd /c start`.
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
    /// The processes that hold TCP `port`, with their process groups: each
    /// one with an unconnected socket on it, listening or only bound. Both
    /// make a server's bind fail; a connection, from the port or to it,
    /// doesn't, for a server that sets SO_REUSEADDR, so it doesn't count.
    /// Only processes lokal may inspect: another user's are missing, so an
    /// empty list doesn't mean that nothing listens. VERIFY V11 (SPEC §12).
    // Linux (platform/linux): `ss -H -t -a -n -p 'sport = :<port>'` gives the
    // state, peer and pids of each socket (another user's pids need root, as
    // here), and field 5 of /proc/<pid>/stat the pgid. A socket that's only
    // bound may be missing there (VERIFY V11 on Linux).
    // PORTING(windows): GetExtendedTcpTable (TCP_TABLE_OWNER_PID_ALL) gives the
    // pid and state of each socket. No process groups: use the job object lokal
    // puts each service in, or the pid.
    fn port_owners(&self, port: u16) -> Vec<PortOwner>;
    /// TCP ports that process `pid` and every process descended from it
    /// listen on, sorted (for `lokal run`'s wrong-port hint: the command and
    /// the processes it started). Empty if none, or unknown.
    // Linux (platform/linux): the ppid of every process from /proc/<pid>/stat,
    // walked with `process_tree`, then `ss -H -t -l -n -p` filtered to those pids.
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

impl dyn System {
    /// Which processes listen on TCP `port`, for messages (doctor, the
    /// busy-port errors): `node (pid 412)`, several joined with `, `.
    /// `None` if none that lokal may inspect. From
    /// [`System::port_owners`], so the two can't disagree.
    pub fn port_owner(&self, port: u16) -> Option<String> {
        listeners(&self.port_owners(port))
    }
}

/// `port_owner`'s text for owners a caller already has, so a
/// message that also needs them takes one lookup.
pub fn owner_text(owners: &[PortOwner]) -> Option<String> {
    listeners(owners)
}

/// A process that holds a TCP port ([`System::port_owners`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortOwner {
    pub pid: u32,
    /// Its process group.
    pub pgid: u32,
    /// Its command name (`node`).
    pub name: String,
    /// One of its sockets on the port listens. Otherwise it only holds the
    /// port: bound without listening.
    pub listening: bool,
}

/// [`System::port_owner`]'s text for `owners`: the listening ones, as
/// `node (pid 412)`. The name is cut to 9 characters, as lsof's COMMAND
/// column cut it before `port_owners` read lsof's full names, so the
/// messages stay as they were.
fn listeners(owners: &[PortOwner]) -> Option<String> {
    let names: Vec<String> = owners
        .iter()
        .filter(|o| o.listening)
        .map(|o| {
            let name: String = o.name.chars().take(9).collect();
            format!("{name} (pid {})", o.pid)
        })
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
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
    #[cfg(target_os = "linux")]
    {
        linux::platform()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
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
    #[cfg(target_os = "linux")]
    {
        linux::name_publisher(backend)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        unsupported::name_publisher(backend)
    }
}

/// The roots of the locations that `paths.rs` builds outside the base dir
/// (SPEC §5.1), for the real layout; `LOKAL_HOME` replaces all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDirs {
    /// The daemon's logs, and `lokal up`'s folders.
    pub log_dir: PathBuf,
    /// The background service's definition (the LaunchAgent plist on
    /// macOS), or `None` where lokal can't install the service yet: then
    /// lokal is never set up there.
    pub service_definition: Option<PathBuf>,
}

/// This OS's [`UserDirs`], from the user's base directories. `label` names
/// the service (`dev.lokal.daemon`).
pub fn user_dirs(dirs: &directories::BaseDirs, label: &str) -> UserDirs {
    #[cfg(target_os = "macos")]
    {
        let home = dirs.home_dir();
        UserDirs {
            log_dir: home.join("Library/Logs/lokal"),
            service_definition: Some(
                home.join("Library/LaunchAgents")
                    .join(format!("{label}.plist")),
            ),
        }
    }
    // PORTING(linux): the systemd user unit (~/.config/systemd/user) comes
    // with the background service; until then there is no definition.
    #[cfg(target_os = "linux")]
    {
        let _ = label;
        // $XDG_STATE_HOME, default ~/.local/state: directories always has
        // one on Linux.
        let state = dirs
            .state_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dirs.home_dir().join(".local/state"));
        UserDirs {
            log_dir: state.join("lokal"),
            service_definition: None,
        }
    }
    // PORTING(windows): logs under %LOCALAPPDATA%, which this is.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = label;
        UserDirs {
            log_dir: dirs.data_local_dir().join("lokal").join("logs"),
            service_definition: None,
        }
    }
}

/// This machine's host name (`gethostname`) up to its first dot: `mac` for
/// `mac.local`. Empty if it has none.
pub(crate) fn short_hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: buf is valid for its length.
    unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let h = String::from_utf8_lossy(&buf[..end]).to_string();
    h.split('.').next().unwrap_or_default().to_string()
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

/// The longest path, in bytes, that a Unix socket can be bound or reached
/// at: `sun_path` without its terminating NUL, so 103 on macOS. `lokal
/// up`'s socket is in the user's log folder, and a long user or project
/// name can make its path longer (SPEC §4.11, "Restarting a service").
// PORTING(linux): the same field, 108 bytes there (107 for the path).
// PORTING(windows): AF_UNIX sockets have a 108-byte sun_path too; a named
// pipe (\\.\pipe\…) has no such limit.
pub fn unix_socket_path_max() -> usize {
    // SAFETY: sockaddr_un is plain data, and all zero bytes are a valid one.
    let addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_path.len() - 1
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

    /// The signal's number: `lokal up` exits with 128 + it (SPEC §4.11).
    // PORTING(windows): no POSIX signals; map to CTRL_C_EVENT/CTRL_BREAK_EVENT.
    pub fn number(self) -> libc::c_int {
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

/// SIGPIPE's number. `lokal up` exits with 128 + it when its stdout or
/// stderr closes (SPEC §4.11).
// PORTING(windows): no SIGPIPE; keep 13 for the exit code a shell would show.
pub const SIGPIPE: i32 = libc::SIGPIPE;

/// `lokal up`: catches SIGPIPE, so a write to a closed stdout or stderr
/// fails with `EPIPE` instead of killing lokal, which would leave every
/// service running in its own process group (SPEC §4.11). A handler, never
/// `SIG_IGN`, as with [`RelaySignals`]: `exec` resets a caught signal, so the
/// services start with the default. Call it inside a tokio runtime; the
/// handler stays installed until lokal exits.
// PORTING(windows): no SIGPIPE; a write to a closed pipe already fails
// (ERROR_NO_DATA), so there's nothing to install.
pub fn catch_sigpipe() -> Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    // Dropping the stream keeps tokio's handler: it stays for the process.
    drop(signal(SignalKind::pipe())?);
    Ok(())
}

/// A signal `lokal up` sends to a service's whole process group (SPEC
/// §4.11, "Stopping").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupSignal {
    /// SIGINT, first
    Interrupt,
    /// SIGTERM, at 5s
    Terminate,
    /// SIGKILL, at 10s or on a second signal
    Kill,
}

impl GroupSignal {
    // PORTING(windows): no POSIX signals; see signal_group.
    fn number(self) -> libc::c_int {
        match self {
            GroupSignal::Interrupt => libc::SIGINT,
            GroupSignal::Terminate => libc::SIGTERM,
            GroupSignal::Kill => libc::SIGKILL,
        }
    }
}

/// Sends `sig` to every process in process group `pgid` (`killpg`): a
/// service that `lokal up` started with `process::spawn_shell`, whose group
/// id is its shell's pid. Refuses a pgid of 0 or 1 (`killpg` would take 0
/// as lokal's own group, and 1 is launchd's) and lokal's own group. True if
/// the signal reached at least one process.
// PORTING(linux): the same call (killpg), with the same meaning.
// PORTING(windows): no process groups: put each service in a job object and
// use TerminateJobObject for Terminate and Kill, and
// GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT) for Interrupt, which reaches a
// child started with CREATE_NEW_PROCESS_GROUP.
pub fn signal_group(pgid: u32, sig: GroupSignal) -> bool {
    let Ok(pgid) = libc::pid_t::try_from(pgid) else {
        return false;
    };
    // SAFETY: getpgrp has no preconditions.
    if pgid <= 1 || pgid == unsafe { libc::getpgrp() } {
        return false;
    }
    // SAFETY: killpg only sends a signal to the given group (> 1, not ours).
    unsafe { libc::killpg(pgid, sig.number()) == 0 }
}

/// Is anything left in process group `pgid`? `killpg(pgid, 0)` succeeds,
/// or fails with `EPERM`: the group has members lokal may not signal, which
/// includes a group whose only member is its unreaped zombie leader (VERIFY
/// V10, SPEC §12). Only `ESRCH` means empty. False for a pgid of 0 or 1.
// PORTING(linux): the same call; Linux also counts zombies as members.
// PORTING(windows): QueryInformationJobObject (JobObjectBasicAccountingInformation)
// and its ActiveProcesses count, for the service's job object.
pub fn group_alive(pgid: u32) -> bool {
    let Ok(pgid) = libc::pid_t::try_from(pgid) else {
        return false;
    };
    if pgid <= 1 {
        return false;
    }
    // SAFETY: signal 0 only checks whether the group exists.
    if unsafe { libc::killpg(pgid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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

    /// SPEC §5.1: the log dir and the service definition, per OS. macOS's
    /// must never move: the daemon log, `lokal up`'s folders and setup's
    /// check that the plist exists all use them.
    #[test]
    fn user_dirs_per_os() {
        let dirs = directories::BaseDirs::new().unwrap();
        let home = dirs.home_dir();
        let u = user_dirs(&dirs, "dev.lokal.daemon");
        if cfg!(target_os = "macos") {
            assert_eq!(u.log_dir, home.join("Library/Logs/lokal"));
            assert_eq!(
                u.service_definition,
                Some(home.join("Library/LaunchAgents/dev.lokal.daemon.plist"))
            );
        } else if cfg!(target_os = "linux") {
            let state = std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(".local/state"));
            assert_eq!(u.log_dir, state.join("lokal"));
            assert_eq!(u.service_definition, None);
        }
        if !cfg!(target_os = "macos") {
            // Nothing goes to a macOS-style ~/Library elsewhere.
            assert!(!u.log_dir.starts_with(home.join("Library")), "{u:?}");
        }
    }

    fn owner(pid: u32, name: &str, listening: bool) -> PortOwner {
        PortOwner {
            pid,
            pgid: pid,
            name: name.into(),
            listening,
        }
    }

    /// The busy-port errors and doctor name the listeners only, as `node
    /// (pid 412)`, with lsof's COMMAND column's 9 characters.
    #[test]
    fn port_owner_names_the_listeners_as_before() {
        assert_eq!(listeners(&[]), None);
        assert_eq!(listeners(&[owner(7, "node", false)]), None);
        assert_eq!(
            listeners(&[
                owner(412, "node", true),
                owner(500, "curl", false),
                owner(613, "ControlCenter", true),
            ]),
            Some("node (pid 412), ControlCe (pid 613)".into())
        );
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
