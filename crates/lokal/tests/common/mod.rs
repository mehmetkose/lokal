//! Shared helpers for integration tests: the sandbox daemon, the testkit
//! programs, running lokal in the background or on a pseudo-terminal, and
//! stopping what a test started even when it fails.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use testkit::probe::Opts;

pub const V4: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
pub const V6: IpAddr = IpAddr::V6(Ipv6Addr::LOCALHOST);

pub fn free_port() -> u16 {
    loop {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        // The daemon binds both loopbacks.
        if std::net::TcpListener::bind(("::1", p)).is_ok() {
            return p;
        }
    }
}

pub struct Daemon {
    pub dir: tempfile::TempDir,
    pub https: u16,
    pub http: u16,
    /// UDP port of the sandbox `.lokal` DNS server.
    pub dns: u16,
    child: Child,
}

impl Daemon {
    pub fn start(backend: &str) -> Self {
        Self::start_with(backend, &[], None)
    }

    /// With extra `daemon run` flags, and optionally an open-file limit
    /// (soft and hard) for the daemon process.
    pub fn start_with(backend: &str, extra: &[&str], nofile: Option<u64>) -> Self {
        let dir = tempfile::Builder::new().prefix("lk").tempdir().unwrap();
        let bin = assert_cmd::cargo::cargo_bin("lokal");
        let st = Command::new(&bin)
            .env("LOKAL_HOME", dir.path())
            .args(["internal", "create-ca"])
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "{}",
            String::from_utf8_lossy(&st.stderr)
        );
        // Free ports are picked by binding port 0 and releasing it, so a
        // parallel test can grab the same one; if the daemon exits at once
        // (bind failed), try again with fresh ports.
        for _attempt in 0..5 {
            let (https, http) = (free_port(), free_port());
            let dns = std::net::UdpSocket::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let mut cmd = Command::new(&bin);
            cmd.env("LOKAL_HOME", dir.path())
                .env("RUST_LOG", "info")
                .args(["daemon", "run", "--foreground", "--name-backend", backend])
                .args(["--base-dir", dir.path().to_str().unwrap()])
                .args([
                    "--https-port",
                    &https.to_string(),
                    "--http-port",
                    &http.to_string(),
                ])
                .args(["--dns-port", &dns.to_string()])
                .args(extra)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(n) = nofile {
                use std::os::unix::process::CommandExt;
                // SAFETY: only setrlimit runs between fork and exec.
                unsafe {
                    cmd.pre_exec(move || {
                        let rl = libc::rlimit {
                            rlim_cur: n as libc::rlim_t,
                            rlim_max: n as libc::rlim_t,
                        };
                        if libc::setrlimit(libc::RLIMIT_NOFILE, &rl) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }
            let child = cmd.spawn().unwrap();
            let mut child = child;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(Some(_)) = child.try_wait() {
                    break; // exited: a port was taken; retry with new ones
                }
                let out = Command::new(&bin)
                    .env("LOKAL_HOME", dir.path())
                    .args(["status", "--json"])
                    .output()
                    .unwrap();
                if String::from_utf8_lossy(&out.stdout).contains("\"running\": true") {
                    return Self {
                        dir,
                        https,
                        http,
                        dns,
                        child,
                    };
                }
                assert!(Instant::now() < deadline, "sandbox daemon did not start");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        panic!("sandbox daemon could not bind free ports in 5 attempts");
    }

    pub fn lokal(&self, args: &[&str]) -> std::process::Output {
        Command::new(assert_cmd::cargo::cargo_bin("lokal"))
            .env("LOKAL_HOME", self.dir.path())
            .env("NO_COLOR", "1")
            .args(args)
            .output()
            .unwrap()
    }

    pub fn ok(&self, args: &[&str]) -> String {
        let out = self.lokal(args);
        assert!(
            out.status.success(),
            "lokal {args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn cacert(&self) -> PathBuf {
        self.dir.path().join("ca/root.pem")
    }

    pub fn opts(&self) -> Opts {
        Opts {
            connect: Some(SocketAddr::new(V4, self.https)),
            cacert: Some(self.cacert()),
            http2: false,
        }
    }

    pub fn opts_h2(&self) -> Opts {
        Opts {
            http2: true,
            ..self.opts()
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .arg(self.child.id().to_string())
            .status();
        let _ = self.child.wait();
    }
}

/// The testkit programs that `lokal run` and `lokal up` start in these tests. `cargo test`
/// builds only testkit's library for this crate, so the binaries are built
/// once per test run, into the same target directory.
pub fn testkit_bin(name: &str) -> String {
    static DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let lokal = assert_cmd::cargo::cargo_bin("lokal");
        let dir = lokal.parent().expect("target dir").to_path_buf();
        let mut c = Command::new(env!("CARGO"));
        c.current_dir(env!("CARGO_MANIFEST_DIR")).args([
            "build",
            "--quiet",
            "-p",
            "testkit",
            "--bin",
            "echo-server",
            "--bin",
            "sig-probe",
        ]);
        if dir.ends_with("release") {
            c.arg("--release");
        }
        let out = c.output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        Ok(dir)
    });
    match dir {
        Ok(d) => d.join(name).to_string_lossy().into_owned(),
        Err(e) => panic!("cannot build the testkit binaries: {e}"),
    }
}

/// A `lokal run` started in the background, its stderr read line by line.
pub struct Running {
    pub child: Child,
    pub lines: Receiver<String>,
    /// Everything read so far from the output that `lines` comes from.
    pub seen: Arc<Mutex<String>>,
    /// The pty master, for pty runs.
    pub master: Option<OwnedFd>,
}

impl Running {
    /// Starts `lokal args…` with stdin closed, and stdout and stderr piped.
    pub fn spawn(mut cmd: Command) -> Self {
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            let mut sink = String::new();
            let _ = BufReader::new(stdout).read_to_string(&mut sink);
        });
        let stderr = child.stderr.take().unwrap();
        let (lines, seen) = Self::reader(stderr);
        Self {
            child,
            lines,
            seen,
            master: None,
        }
    }

    pub fn reader(r: impl Read + Send + 'static) -> (Receiver<String>, Arc<Mutex<String>>) {
        let (tx, rx) = channel();
        let seen = Arc::new(Mutex::new(String::new()));
        let s = seen.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(r).lines() {
                let Ok(line) = line else { break };
                s.lock().unwrap().push_str(&format!("{line}\n"));
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        (rx, seen)
    }

    /// Waits up to `limit` for a line containing `needle`.
    pub fn wait_line(&self, needle: &str, limit: Duration) -> String {
        let deadline = Instant::now() + limit;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(l) if l.contains(needle) => return l,
                Ok(_) => {}
                Err(_) => panic!(
                    "no line with {needle:?} within {limit:?}; output so far:\n{}",
                    self.seen()
                ),
            }
        }
    }

    pub fn seen(&self) -> String {
        self.seen.lock().unwrap().clone()
    }

    pub fn signal(&self, sig: libc::c_int) {
        // SAFETY: kill only sends a signal to our child's pid.
        assert_eq!(unsafe { libc::kill(self.child.id() as i32, sig) }, 0);
    }

    /// Waits up to `limit` for lokal to exit and returns its exit code.
    pub fn wait_exit(&mut self, limit: Duration) -> i32 {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                return st.code().unwrap_or(-1);
            }
            if Instant::now() > deadline {
                // Drop stops lokal with SIGTERM, which it relays; a SIGKILL
                // here would leave its command running.
                panic!(
                    "lokal didn't exit within {limit:?}; output so far:\n{}",
                    self.seen()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Running {
    /// A failed test still stops its command: SIGTERM reaches it through
    /// lokal (SIGKILL would leave it running).
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        // SAFETY: kill only sends a signal to our child's pid.
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// From <sys/ttycom.h>: `#define TIOCSCTTY _IO('t', 97)`. libc doesn't export
// it for Apple targets. Elsewhere it has its own number (Linux: 0x540E),
// which libc does export.
#[cfg(target_os = "macos")]
const TIOCSCTTY: libc::c_ulong = 0x2000_7461;
#[cfg(not(target_os = "macos"))]
use libc::TIOCSCTTY;

/// Starts `cmd` on a new pseudo-terminal, as a shell would: the pty is its
/// stdin, stdout and stderr and its controlling terminal (it leads a new
/// session), so its process group is the terminal's foreground group.
/// Returns the child and the pty's master side, which reads what the child
/// prints and types into it.
pub fn spawn_on_pty(mut cmd: Command) -> (Child, OwnedFd) {
    let (mut m, mut s) = (0, 0);
    // SAFETY: openpty writes the two fds; the other arguments may be null.
    let rc = unsafe {
        libc::openpty(
            &mut m,
            &mut s,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: openpty returned two fds that nothing else owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(m), OwnedFd::from_raw_fd(s)) };
    // openpty doesn't set close-on-exec: without it, processes that other
    // tests start meanwhile would hold the pty open. The child still gets
    // the slave as its fds 0, 1 and 2.
    for fd in [&master, &slave] {
        // SAFETY: fcntl only sets a flag on an fd this function owns.
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    cmd.stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    // SAFETY: only setsid and ioctl run between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().unwrap();
    drop(cmd); // closes the parent's copies of the slave
    (child, master)
}

/// Everything read from a pty's master side (or any pipe) so far,
/// collected by a thread. A prompt doesn't end its line, so this looks at
/// bytes, not lines.
#[derive(Clone)]
pub struct PtyOutput(Arc<Mutex<Vec<u8>>>);

impl PtyOutput {
    pub fn collect(master: &OwnedFd) -> Self {
        Self::from_reader(std::fs::File::from(master.try_clone().unwrap()))
    }

    pub fn from_reader(mut r: impl Read + Send + 'static) -> Self {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let b = buf.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            // Ends with EOF, or EIO for a pty once every copy of the slave
            // is closed.
            while let Ok(n @ 1..) = r.read(&mut chunk) {
                b.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
        });
        Self(buf)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    /// Waits up to `limit` until the output contains `needle`.
    pub fn wait_for(&self, needle: &str, limit: Duration) {
        let deadline = Instant::now() + limit;
        while !self.text().contains(needle) {
            assert!(
                Instant::now() < deadline,
                "no {needle:?} within {limit:?}; output so far:\n{}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A tag no other process on this Mac has in its command line: tests put it
/// into every long-running command they start, so [`kill_tagged`] finds
/// exactly those processes.
pub fn unique_tag() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!(
        "{}{:09}{}",
        std::process::id(),
        nanos,
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// `(pid, pgid, command)` of every process whose command line contains one
/// of `tags`.
pub fn tagged(tags: &[String]) -> Vec<(i32, i32, String)> {
    let o = Command::new("/bin/ps")
        .args(["-A", "-ww", "-o", "pid=,pgid=,command="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter(|l| tags.iter().any(|t| l.contains(t.as_str())))
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let pgid = it.next()?.parse().ok()?;
            Some((pid, pgid, it.collect::<Vec<_>>().join(" ")))
        })
        .collect()
}

/// Kills every process group that holds a process whose command line has
/// one of `tags`, and those processes: only what a test started itself. A
/// group is killed only while one of its tagged processes is in it, so its
/// pgid can't have been reused by someone else's group. Returns what it
/// found.
pub fn kill_tagged(tags: &[String]) -> Vec<(i32, i32, String)> {
    let found = tagged(tags);
    // SAFETY: getpgrp has no preconditions.
    let ours = unsafe { libc::getpgrp() };
    for (pid, pgid, _) in &found {
        // SAFETY: only processes and groups this test started (tagged), and
        // never the test's own group.
        unsafe {
            if *pgid > 1 && *pgid != ours {
                libc::killpg(*pgid, libc::SIGKILL);
            }
            libc::kill(*pid, libc::SIGKILL);
        }
    }
    found
}

/// Is anything left in process group `pgid` (`killpg(pgid, 0)` doesn't fail
/// with ESRCH)?
pub fn group_exists(pgid: i32) -> bool {
    // SAFETY: signal 0 only checks whether the group exists.
    let rc = unsafe { libc::killpg(pgid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// The pids of `pid`'s children (`pgrep -P`).
pub fn children(pid: u32) -> Vec<i32> {
    let o = Command::new("/usr/bin/pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}
