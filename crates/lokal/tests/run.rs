//! `lokal run <SUBDOMAIN> -- <COMMAND>...` (SPEC §4.10) against the sandbox
//! daemon: the route is ready when `✓ ready:` prints, the command's
//! environment and exit codes, signals reaching the command exactly once
//! (through a pty, and with no terminal), reuse, `--rm`, a busy port, and
//! the wrong-port hint.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use common::{Daemon, free_port};
use serde_json::Value;
use testkit::probe;

/// The testkit programs `lokal run` starts in these tests. `cargo test`
/// builds only testkit's library for this crate, so the binaries are built
/// once per test run, into the same target directory.
fn testkit_bin(name: &str) -> String {
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

fn lokal_cmd(home: &std::path::Path, args: &[&str]) -> Command {
    let mut c = Command::new(assert_cmd::cargo::cargo_bin("lokal"));
    c.env("LOKAL_HOME", home)
        .env("NO_COLOR", "1")
        .env_remove("LOKAL_FORBID_SUDO")
        .args(args);
    c
}

fn code(o: &std::process::Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn list(home: &std::path::Path) -> Value {
    let o = lokal_cmd(home, &["list", "--json"]).output().unwrap();
    serde_json::from_slice(&o.stdout).unwrap()
}

/// `(subdomain, port)` of every service in `project`.
fn services(home: &std::path::Path, project: &str) -> Vec<(String, u64)> {
    list(home)["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["name"] == project)
        .flat_map(|p| p["services"].as_array().unwrap().clone())
        .map(|s| {
            (
                s["subdomain"].as_str().unwrap().to_string(),
                s["port"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// A `lokal run` started in the background, its stderr read line by line.
struct Running {
    child: Child,
    lines: Receiver<String>,
    /// Everything read so far from the output that `lines` comes from.
    seen: Arc<Mutex<String>>,
    /// The pty master, for pty runs.
    master: Option<OwnedFd>,
}

impl Running {
    /// Starts `lokal args…` with stdin closed, and stdout and stderr piped.
    fn spawn(mut cmd: Command) -> Self {
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

    fn reader(r: impl Read + Send + 'static) -> (Receiver<String>, Arc<Mutex<String>>) {
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
    fn wait_line(&self, needle: &str, limit: Duration) -> String {
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

    fn seen(&self) -> String {
        self.seen.lock().unwrap().clone()
    }

    fn signal(&self, sig: libc::c_int) {
        // SAFETY: kill only sends a signal to our child's pid.
        assert_eq!(unsafe { libc::kill(self.child.id() as i32, sig) }, 0);
    }

    /// Waits up to `limit` for lokal to exit and returns its exit code.
    fn wait_exit(&mut self, limit: Duration) -> i32 {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                return st.code().unwrap_or(-1);
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
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

/// The port in lokal's route line (`✓ https://x.p.lokal → localhost:51234  …`).
fn routed_port(route_line: &str) -> u16 {
    route_line
        .split("→ localhost:")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("no port in {route_line:?}"))
}

fn https_get(d: &Daemon, url: &str) -> probe::Resp {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(probe::get(url, &d.opts()))
        .unwrap()
}

/// (a) The URL works on the first request after `✓ ready:` (SPEC §4.5: no
/// retries), and SIGTERM ends both lokal and the server.
#[test]
fn url_works_on_the_first_request_after_the_ready_line() {
    let d = Daemon::start("none");
    d.ok(&["project", "create", "p"]);
    let echo = testkit_bin("echo-server");
    let r = Running::spawn(lokal_cmd(
        d.dir.path(),
        &[
            "run", "x", "-p", "p", "--", &echo, "--port", "{port}", "--marker", "R",
        ],
    ));
    let route = r.wait_line("✓ https://x.p.lokal → localhost:", Duration::from_secs(10));
    assert!(route.contains("(route ready in"), "{route}");
    assert!(route.contains("--marker R"), "{route}");
    let port = routed_port(&route);
    let ready = r.wait_line("✓ ready:", Duration::from_secs(10));
    assert!(ready.contains("https://x.p.lokal ("), "{ready}");
    let resp = https_get(&d, "https://x.p.lokal/");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json().unwrap()["marker"], "R");
    assert_eq!(
        services(d.dir.path(), "p"),
        [("x".to_string(), port as u64)]
    );
    let mut r = r;
    r.signal(libc::SIGTERM);
    // The server died of the forwarded SIGTERM: 128 + 15.
    assert_eq!(r.wait_exit(Duration::from_secs(10)), 143, "{}", r.seen());
}

/// (b) The command's environment; lokal's own lines stay off stdout.
#[test]
fn command_gets_port_host_and_urls() {
    let d = Daemon::start("none");
    d.ok(&["project", "create", "p"]);
    d.ok(&["service", "add", "v1.api", "4000"]);
    let o = lokal_cmd(d.dir.path(), &["run", "x", "-p", "p", "--", "/usr/bin/env"])
        .output()
        .unwrap();
    let (out, err) = (text(&o.stdout), text(&o.stderr));
    assert_eq!(code(&o), 0, "{err}");
    let port = routed_port(&err);
    for want in [
        format!("PORT={port}"),
        "HOST=127.0.0.1".into(),
        "LOKAL_URL=https://x.p.lokal".into(),
        "LOKAL_URL_X=https://x.p.lokal".into(),
        "LOKAL_URL_V1_API=https://v1.api.p.lokal".into(),
    ] {
        assert!(out.lines().any(|l| l == want), "{want} missing:\n{out}");
    }
    assert!(!out.contains('✓'), "lokal's lines belong on stderr:\n{out}");
    // The route stays after the command exits.
    assert!(services(d.dir.path(), "p").contains(&("x".to_string(), port as u64)));
}

/// (c) The command's exit code; 128 + N for a signal; 127 and 126 as in a
/// shell.
#[test]
fn exit_codes_are_the_commands() {
    let d = Daemon::start("none");
    d.ok(&["project", "create", "p"]);
    let run = |cmd: &[&str]| {
        let mut args = vec!["run", "x", "--"];
        args.extend_from_slice(cmd);
        lokal_cmd(d.dir.path(), &args).output().unwrap()
    };
    assert_eq!(code(&run(&["/bin/sh", "-c", "exit 7"])), 7);
    assert_eq!(code(&run(&["/bin/sh", "-c", "kill -TERM $$"])), 143);
    let o = run(&["/nonexistent"]);
    assert_eq!(code(&o), 127);
    assert!(
        text(&o.stderr).contains("error: command not found: /nonexistent"),
        "{}",
        text(&o.stderr)
    );
    let plain = d.dir.path().join("not-executable");
    std::fs::write(&plain, "#!/bin/sh\n").unwrap();
    let o = run(&[plain.to_str().unwrap()]);
    assert_eq!(code(&o), 126);
    assert!(text(&o.stderr).contains("error: cannot run"));
}

// From <sys/ttycom.h>: `#define TIOCSCTTY _IO('t', 97)`. libc doesn't export
// it for Apple targets.
const TIOCSCTTY: libc::c_ulong = 0x2000_7461;

/// (d) Ctrl-C on the terminal: the command gets exactly one SIGINT (from the
/// terminal; lokal doesn't relay it again) and lokal exits with its code.
#[test]
fn ctrl_c_on_a_terminal_reaches_the_command_once() {
    let d = Daemon::start("none");
    d.ok(&["project", "create", "p"]);
    let probe_bin = testkit_bin("sig-probe");
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
    let mut cmd = lokal_cmd(
        d.dir.path(),
        &["run", "x", "--", &probe_bin, "--port", "{port}"],
    );
    cmd.stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    // SAFETY: only setsid and ioctl run between fork and exec. lokal becomes
    // a session leader with the pty as its controlling terminal, so its
    // process group is the terminal's foreground group, as in a shell.
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
    let reader = std::fs::File::from(master.try_clone().unwrap());
    let (lines, seen) = Running::reader(reader);
    let mut r = Running {
        child,
        lines,
        seen,
        master: Some(master),
    };
    r.wait_line("✓ ready:", Duration::from_secs(10));
    let mut tty = std::fs::File::from(r.master.as_ref().unwrap().try_clone().unwrap());
    tty.write_all(b"\x03").unwrap();
    assert_eq!(
        r.wait_exit(Duration::from_secs(10)),
        41,
        "want exactly one SIGINT:\n{}",
        r.seen()
    );
}

/// (e) No terminal (an agent's `kill -INT`): lokal relays SIGINT once, and
/// SIGTERM always.
#[test]
fn signals_without_a_terminal_are_relayed_once() {
    let d = Daemon::start("none");
    d.ok(&["project", "create", "p"]);
    let probe_bin = testkit_bin("sig-probe");
    for sig in [libc::SIGINT, libc::SIGTERM] {
        let mut cmd = lokal_cmd(
            d.dir.path(),
            &["run", "x", "--", &probe_bin, "--port", "{port}"],
        );
        // SAFETY: only setsid runs between fork and exec: a new session with
        // no controlling terminal, so the test's own terminal can't interfere.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut r = Running::spawn(cmd);
        r.wait_line("✓ ready:", Duration::from_secs(10));
        r.signal(sig);
        assert_eq!(
            r.wait_exit(Duration::from_secs(10)),
            41,
            "signal {sig}: want exactly one relayed:\n{}",
            r.seen()
        );
    }
}

/// (f) An existing service keeps its port; a different --port is a usage
/// error that names the update command.
#[test]
fn existing_service_is_reused_as_is() {
    let d = Daemon::start("none");
    d.ok(&["project", "create", "p"]);
    let p = free_port();
    d.ok(&["service", "add", "y", &p.to_string()]);
    let o = lokal_cmd(d.dir.path(), &["run", "y", "--", "/usr/bin/env"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 0, "{}", text(&o.stderr));
    assert!(text(&o.stdout).lines().any(|l| l == format!("PORT={p}")));
    // Same flags as the service: fine.
    let o = lokal_cmd(
        d.dir.path(),
        &[
            "run",
            "y",
            "--port",
            &p.to_string(),
            "--host-header",
            "preserve",
            "--",
            "/usr/bin/true",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(code(&o), 0, "{}", text(&o.stderr));
    let other = free_port();
    let o = lokal_cmd(
        d.dir.path(),
        &[
            "run",
            "y",
            "--port",
            &other.to_string(),
            "--",
            "/usr/bin/true",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(code(&o), 2);
    let err = text(&o.stderr);
    assert!(
        err.contains(&format!("use `lokal service update y --port {other}`")),
        "{err}"
    );
    let o = lokal_cmd(
        d.dir.path(),
        &[
            "run",
            "y",
            "--host-header",
            "upstream",
            "--",
            "/usr/bin/true",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(code(&o), 2);
    assert!(text(&o.stderr).contains("lokal service update y --host-header upstream"));
    assert_eq!(services(d.dir.path(), "p"), [("y".to_string(), p as u64)]);
    // A route to another machine isn't something run can start.
    d.ok(&["service", "add", "far", "4000", "--host", "10.0.0.5"]);
    let o = lokal_cmd(d.dir.path(), &["run", "far", "--", "/usr/bin/true"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 2);
    assert!(
        text(&o.stderr).contains("isn't this Mac"),
        "{}",
        text(&o.stderr)
    );
}

/// (g) --rm removes a service the run added, and keeps one that existed.
#[test]
fn rm_removes_only_what_the_run_added() {
    let d = Daemon::start("none");
    d.ok(&["project", "create", "p"]);
    let o = lokal_cmd(
        d.dir.path(),
        &["run", "tmp", "--rm", "--", "/bin/sh", "-c", "exit 3"],
    )
    .output()
    .unwrap();
    assert_eq!(code(&o), 3, "{}", text(&o.stderr));
    assert!(
        text(&o.stderr).contains("Removed tmp.p.lokal"),
        "{}",
        text(&o.stderr)
    );
    assert!(services(d.dir.path(), "p").is_empty());
    let p = free_port();
    d.ok(&["service", "add", "keep", &p.to_string()]);
    let o = lokal_cmd(
        d.dir.path(),
        &["run", "keep", "--rm", "--", "/usr/bin/true"],
    )
    .output()
    .unwrap();
    assert_eq!(code(&o), 0);
    assert!(
        text(&o.stderr).contains("kept keep.p.lokal: it existed before this run"),
        "{}",
        text(&o.stderr)
    );
    assert_eq!(
        services(d.dir.path(), "p"),
        [("keep".to_string(), p as u64)]
    );
    // A command that can't start still leaves nothing behind.
    let o = lokal_cmd(d.dir.path(), &["run", "gone", "--rm", "--", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 127);
    assert_eq!(
        services(d.dir.path(), "p"),
        [("keep".to_string(), p as u64)]
    );
}

/// (h) Something already listens on the service's port: exit 1, name it,
/// and don't start the command.
#[test]
fn busy_port_names_the_owner_and_starts_nothing() {
    let d = Daemon::start("none");
    d.ok(&["project", "create", "p"]);
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = held.local_addr().unwrap().port();
    d.ok(&["service", "add", "y", &p.to_string()]);
    let marker = d.dir.path().join("started");
    let o = lokal_cmd(
        d.dir.path(),
        &["run", "y", "--", "/usr/bin/touch", marker.to_str().unwrap()],
    )
    .output()
    .unwrap();
    let err = text(&o.stderr);
    assert_eq!(code(&o), 1, "{err}");
    assert!(
        err.contains(&format!("localhost:{p} is already in use by")),
        "{err}"
    );
    assert!(
        err.contains(&format!("(pid {})", std::process::id())),
        "{err}"
    );
    assert!(
        err.contains("lokal service update y --port <PORT>"),
        "{err}"
    );
    assert!(!marker.exists(), "the command must not start");
    // A new service on a busy --port: nothing is saved either.
    let o = lokal_cmd(
        d.dir.path(),
        &[
            "run",
            "z",
            "--port",
            &p.to_string(),
            "--",
            "/usr/bin/touch",
            marker.to_str().unwrap(),
        ],
    )
    .output()
    .unwrap();
    assert_eq!(code(&o), 1);
    assert!(!marker.exists());
    assert_eq!(services(d.dir.path(), "p"), [("y".to_string(), p as u64)]);
    drop(held);
}

/// (i) The command listens on another port: a hint names it.
#[test]
fn wrong_port_hint_names_the_port_the_command_listens_on() {
    let d = Daemon::start("none");
    d.ok(&["project", "create", "p"]);
    let echo = testkit_bin("echo-server");
    let other = free_port();
    let mut cmd = lokal_cmd(
        d.dir.path(),
        &["run", "z", "--", &echo, "--port", &other.to_string()],
    );
    // Its own process group, as a shell gives each job: the hint looks at
    // the group's listeners, and this test process (with every sandbox
    // daemon it started) would otherwise be in it.
    cmd.process_group(0);
    let t = Instant::now();
    let r = Running::spawn(cmd);
    let hint = r.wait_line("hint:", Duration::from_secs(6));
    assert!(t.elapsed() >= Duration::from_secs(3), "the hint waits 3s");
    assert!(
        hint.contains(&format!("listens on localhost:{other}, not ")),
        "{hint}"
    );
    assert!(
        hint.contains(&format!("lokal service update z --port {other}")),
        "{hint}"
    );
    assert!(hint.contains("--port {port}"), "{hint}");
    assert!(!r.seen().contains("✓ ready:"), "{}", r.seen());
    let mut r = r;
    r.signal(libc::SIGTERM);
    assert_eq!(r.wait_exit(Duration::from_secs(10)), 143);
}

/// Before setup, or with the daemon stopped: the route is saved, the usual
/// note prints, and the command still runs.
#[test]
fn without_the_background_service_the_command_still_runs() {
    let home = tempfile::Builder::new().prefix("lk").tempdir().unwrap();
    assert!(
        lokal_cmd(home.path(), &["project", "create", "p"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let echo = testkit_bin("echo-server");
    let mut r = Running::spawn(lokal_cmd(
        home.path(),
        &["run", "x", "--", &echo, "--port", "{port}"],
    ));
    let route = r.wait_line("✓ https://x.p.lokal", Duration::from_secs(10));
    assert!(route.ends_with("(saved)"), "{route}");
    r.wait_line("background service isn't running", Duration::from_secs(5));
    let port = routed_port(&route);
    let l = r.wait_line("✓ listening on", Duration::from_secs(10));
    assert_eq!(l.trim(), format!("✓ listening on localhost:{port}"));
    r.signal(libc::SIGTERM);
    assert_eq!(r.wait_exit(Duration::from_secs(10)), 143);
}
