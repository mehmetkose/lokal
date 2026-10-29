//! VERIFY V12 on Linux (SPEC §12): the Linux equivalent of V1. Can systemd
//! *user* socket units hold lokal's ports (127.0.0.1 and ::1 on 443 and 80,
//! and UDP 127.0.0.1:53535) and hand them to a user service, by name, the way
//! launchd's `Sockets` hand them to the LaunchAgent? Kept as documentation.
//! Linux only, with a running user manager (`systemctl --user`).
//!
//! It installs throwaway user units in `$XDG_CONFIG_HOME/systemd/user`:
//!   - `lokal-verify-https.socket`: `ListenStream=` 127.0.0.1 and [::1] on the
//!     HTTPS port, `FileDescriptorName=https`;
//!   - `lokal-verify-http.socket`: the same on the HTTP port, `http`;
//!   - `lokal-verify-dns.socket`: `ListenDatagram=127.0.0.1:<dns>`, `dns`;
//!   - `lokal-verify.service`: this binary's `serve`, with `Sockets=` naming
//!     all three, `Restart=always` and `RestartSec=1`.
//!
//! `FileDescriptorName=` names every fd of its socket unit, so names that tell
//! the kinds apart need one socket unit per kind.
//!
//! Then it checks:
//!   1. (recorded) the kernel's `net.ipv4.ip_unprivileged_port_start`, and
//!      whether the three socket units start; when one fails, systemd's reason,
//!      and whether this user can bind the port directly;
//!   2. activation: the first connect to 127.0.0.1:HTTPS starts the service,
//!      which answers `ok <name> <pid>` on every fd it got. All five answer,
//!      each with its own name; the service saw `LISTEN_PID` equal to its pid,
//!      `LISTEN_FDS=5` and `LISTEN_FDNAMES` (the order is recorded);
//!   3. a crash: after `kill -9` of the service, a connect made at once is
//!      answered by the new service once `RestartSec=1` has passed, because the
//!      socket stays open and the connection waits in its backlog (V1 found
//!      the same of launchd); the delay is recorded;
//!   4. `systemctl --user restart lokal-verify.service`: a connect made right
//!      after is answered by a new pid;
//!   5. the service stopped while the sockets stay: a connect starts it again.
//!
//! Finally it stops and removes the units. It never runs sudo: run it as it
//! is, then after `sudo sysctl -w net.ipv4.ip_unprivileged_port_start=80`, and
//! put the value back afterwards (`… = 1024`).
//!
//! Usage: verify_systemd_sockets [https-port http-port dns-port]
//!        (default 443 80 53535)

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const SERVICE: &str = "lokal-verify.service";
const SOCKETS: [&str; 3] = [
    "lokal-verify-https.socket",
    "lokal-verify-http.socket",
    "lokal-verify-dns.socket",
];

fn systemctl_bin() -> &'static str {
    ["/usr/bin/systemctl", "/bin/systemctl"]
        .into_iter()
        .find(|p| Path::new(p).is_file())
        .unwrap_or("/usr/bin/systemctl")
}

/// `systemctl --user <args>`: whether it succeeded, and its output.
fn systemctl(args: &[&str]) -> (bool, String) {
    let out = Command::new(systemctl_bin())
        .arg("--user")
        .args(args)
        .output()
        .expect("run systemctl");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text.trim().to_string())
}

/// The service: answers `ok <name> <pid>` on every fd systemd passed, and
/// writes what it got to `report` first.
fn serve(report: &Path) -> ! {
    let pid = std::process::id();
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let listen_pid = env("LISTEN_PID");
    let n: i32 = env("LISTEN_FDS").parse().unwrap_or(0);
    let names_var = env("LISTEN_FDNAMES");
    let names: Vec<String> = names_var.split(':').map(str::to_string).collect();
    let mut lines = vec![format!(
        "pid={pid} LISTEN_PID={listen_pid} LISTEN_FDS={n} LISTEN_FDNAMES={names_var}"
    )];
    for (i, fd) in (3..3 + n).enumerate() {
        let name = names.get(i).cloned().unwrap_or_else(|| "?".into());
        let mut ty: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: fd is one systemd passed; ty and len are valid out pointers.
        unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut ty as *mut libc::c_int).cast(),
                &mut len,
            )
        };
        if ty == libc::SOCK_STREAM {
            // SAFETY: systemd hands this process a listening socket it owns.
            let l = unsafe { TcpListener::from_raw_fd(fd) };
            lines.push(format!("fd {fd} {name} tcp {:?}", l.local_addr().ok()));
            std::thread::spawn(move || {
                for mut c in l.incoming().flatten() {
                    let _ = writeln!(c, "ok {name} {pid}");
                }
            });
        } else {
            // SAFETY: systemd hands this process a bound datagram socket it owns.
            let u = unsafe { UdpSocket::from_raw_fd(fd) };
            lines.push(format!("fd {fd} {name} udp {:?}", u.local_addr().ok()));
            std::thread::spawn(move || {
                let mut buf = [0u8; 512];
                while let Ok((_, from)) = u.recv_from(&mut buf) {
                    let _ = u.send_to(format!("ok {name} {pid}\n").as_bytes(), from);
                }
            });
        }
    }
    let tmp = report.with_extension("tmp");
    std::fs::write(&tmp, lines.join("\n") + "\n").unwrap();
    std::fs::rename(&tmp, report).unwrap();
    loop {
        std::thread::park();
    }
}

/// Connects to `addr` and reads the service's answer, with how long it took.
fn ask_tcp(addr: SocketAddr, wait: Duration) -> (Result<String, String>, Duration) {
    let t = Instant::now();
    let r = (|| {
        let mut c =
            TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| e.to_string())?;
        c.set_read_timeout(Some(wait)).map_err(|e| e.to_string())?;
        let mut s = String::new();
        c.read_to_string(&mut s).map_err(|e| e.to_string())?;
        Ok(s.trim().to_string())
    })();
    (r, t.elapsed())
}

fn ask_udp(addr: SocketAddr) -> Result<String, String> {
    let u = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    u.set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    u.send_to(b"hi", addr).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 512];
    let (n, _) = u.recv_from(&mut buf).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&buf[..n]).trim().to_string())
}

/// The pid in an `ok <name> <pid>` answer.
fn answer_pid(a: &Result<String, String>) -> Option<u32> {
    a.as_ref().ok()?.split_whitespace().nth(2)?.parse().ok()
}

fn wait_for(report: &Path) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(s) = std::fs::read_to_string(report) {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

fn first_line(path: &str) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.lines().next().map(str::to_string))
        .unwrap_or_default()
}

fn describe_system() {
    let pretty = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_string())
            })
        })
        .unwrap_or_default();
    let (_, version) = systemctl(&["--version"]);
    println!(
        "Linux {}; {pretty}; {}; uid {}",
        first_line("/proc/sys/kernel/osrelease"),
        version.lines().next().unwrap_or("?"),
        // SAFETY: getuid has no preconditions.
        unsafe { libc::getuid() }
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("serve") {
        serve(Path::new(&args[2]));
    }
    if !Path::new("/proc/self/stat").exists() {
        eprintln!("verify_systemd_sockets: Linux only (no /proc)");
        std::process::exit(2);
    }
    let port = |i: usize, d: u16| {
        args.get(i)
            .map(|s| s.parse::<u16>().expect("a port"))
            .unwrap_or(d)
    };
    let (https, http, dns) = (port(1, 443), port(2, 80), port(3, 53535));
    describe_system();
    let (running, state) = systemctl(&["is-system-running"]);
    if !running && !state.starts_with("degraded") {
        eprintln!("verify_systemd_sockets: no user manager ({state})");
        std::process::exit(2);
    }
    let start = first_line("/proc/sys/net/ipv4/ip_unprivileged_port_start");
    println!(
        "ports: https {https}, http {http}, dns {dns}/udp; ip_unprivileged_port_start = {start}; user manager: {state}"
    );

    let exe = std::env::current_exe().unwrap();
    let exe = exe.to_str().unwrap();
    assert!(
        !exe.contains(char::is_whitespace) && !exe.contains('%'),
        "this binary's path needs no unit-file escaping: {exe}"
    );
    let unit_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()).join(".config"))
        .join("systemd/user");
    std::fs::create_dir_all(&unit_dir).unwrap();
    let report = std::env::temp_dir().join(format!("lokal-verify-{}.report", std::process::id()));
    let report_s = report.to_str().unwrap().to_string();
    let socket = |name: &str, lines: &[String]| {
        format!(
            "[Unit]\nDescription=lokal VERIFY V12 ({name})\n\n[Socket]\n{}\nFileDescriptorName={name}\nService={SERVICE}\n",
            lines.join("\n")
        )
    };
    let units = [
        (
            SOCKETS[0],
            socket(
                "https",
                &[
                    format!("ListenStream=127.0.0.1:{https}"),
                    format!("ListenStream=[::1]:{https}"),
                ],
            ),
        ),
        (
            SOCKETS[1],
            socket(
                "http",
                &[
                    format!("ListenStream=127.0.0.1:{http}"),
                    format!("ListenStream=[::1]:{http}"),
                ],
            ),
        ),
        (
            SOCKETS[2],
            socket("dns", &[format!("ListenDatagram=127.0.0.1:{dns}")]),
        ),
        (
            SERVICE,
            format!(
                "[Unit]\nDescription=lokal VERIFY V12\n\n[Service]\nExecStart={exe} serve {report_s}\nSockets={}\nRestart=always\nRestartSec=1\n",
                SOCKETS.join(" ")
            ),
        ),
    ];
    for (name, text) in &units {
        std::fs::write(unit_dir.join(name), text).unwrap();
    }
    systemctl(&["daemon-reload"]);

    let mut ok = true;
    let mut check = |pass: bool, what: String| {
        ok &= pass;
        println!("{} {what}", if pass { "pass:" } else { "FAIL:" });
    };

    // 1. The sockets.
    let mut started = true;
    for s in SOCKETS {
        let (good, out) = systemctl(&["start", s]);
        if good {
            println!("1. recorded: {s} started");
        } else {
            started = false;
            let (_, status) = systemctl(&["status", "--no-pager", "-n", "3", s]);
            let why: Vec<&str> = status
                .lines()
                .filter(|l| l.contains("Failed") || l.contains("denied"))
                .collect();
            println!("1. recorded: {s} failed: {out}; {why:?}");
        }
    }
    let direct = TcpListener::bind(("127.0.0.1", https))
        .map(|_| ())
        .map_err(|e| e.to_string());
    println!("1. recorded: this user's own bind of 127.0.0.1:{https}: {direct:?}");

    if started {
        let v4 = |p: u16| SocketAddr::from(([127, 0, 0, 1], p));
        let v6 = |p: u16| SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, p));
        // 2. Activation.
        let (first, took) = ask_tcp(v4(https), Duration::from_secs(10));
        println!("2. recorded: first connect (starts the service): {first:?} after {took:?}");
        let answers = [
            ("https", first.clone()),
            ("https", ask_tcp(v6(https), Duration::from_secs(5)).0),
            ("http", ask_tcp(v4(http), Duration::from_secs(5)).0),
            ("http", ask_tcp(v6(http), Duration::from_secs(5)).0),
            ("dns", ask_udp(v4(dns))),
        ];
        let pid = answer_pid(&first);
        let all = answers.iter().all(|(name, a)| {
            a.as_ref()
                .is_ok_and(|s| s.starts_with(&format!("ok {name} ")))
                && answer_pid(a) == pid
        });
        check(
            all,
            format!("2. every socket answers with its own name, one pid: {answers:?}"),
        );
        let got = wait_for(&report).unwrap_or_default();
        let head = got.lines().next().unwrap_or("");
        let pid_ok = pid.is_some_and(|p| head.contains(&format!("pid={p} LISTEN_PID={p} ")));
        check(
            pid_ok && head.contains("LISTEN_FDS=5 "),
            format!(
                "2. the service's environment: {}",
                got.trim().replace('\n', "; ")
            ),
        );

        // 3. A crash.
        if let Some(p) = pid {
            let _ = std::fs::remove_file(&report);
            // SAFETY: kill has no memory preconditions; p is the service's pid.
            unsafe { libc::kill(p as i32, libc::SIGKILL) };
            let (a, took) = ask_tcp(v4(https), Duration::from_secs(10));
            let new = answer_pid(&a);
            check(
                new.is_some() && new != pid,
                format!("3. after kill -9, a connect made at once: {a:?} after {took:?}"),
            );
        }

        // 4. A restart.
        let before = answer_pid(&ask_tcp(v4(https), Duration::from_secs(5)).0);
        let t = Instant::now();
        let (restarted, out) = systemctl(&["restart", SERVICE]);
        let restart_took = t.elapsed();
        let (a, took) = ask_tcp(v4(https), Duration::from_secs(10));
        check(
            restarted && answer_pid(&a).is_some() && answer_pid(&a) != before,
            format!(
                "4. systemctl restart ({restart_took:?}, {out:?}), then a connect: {a:?} after {took:?}"
            ),
        );

        // 5. Stopped, sockets kept.
        let (stopped, _) = systemctl(&["stop", SERVICE]);
        let (a, took) = ask_tcp(v4(http), Duration::from_secs(10));
        check(
            stopped && a.as_ref().is_ok_and(|s| s.starts_with("ok http ")),
            format!("5. service stopped, sockets kept: a connect: {a:?} after {took:?}"),
        );
    } else {
        println!(
            "2–5. skipped: the sockets didn't start. Lower the limit (sudo sysctl -w net.ipv4.ip_unprivileged_port_start=80) and run again"
        );
    }

    // Clean up.
    let mut all: Vec<&str> = vec![SERVICE];
    all.extend(SOCKETS);
    let mut stop = vec!["stop"];
    stop.extend(&all);
    systemctl(&stop);
    let mut reset = vec!["reset-failed"];
    reset.extend(&all);
    systemctl(&reset);
    for (name, _) in &units {
        let _ = std::fs::remove_file(unit_dir.join(name));
    }
    systemctl(&["daemon-reload"]);
    let _ = std::fs::remove_file(&report);
    println!(
        "units removed; ip_unprivileged_port_start is {}",
        first_line("/proc/sys/net/ipv4/ip_unprivileged_port_start")
    );
    if !ok {
        std::process::exit(1);
    }
}
