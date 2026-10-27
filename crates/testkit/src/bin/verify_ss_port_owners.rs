//! VERIFY V11 on Linux (SPEC §12): the port-owner lookup behind Linux's
//! `System::port_owners`, `ss -H -t -a -n -p 'sport = :<port>'` plus field 5
//! of `/proc/<pid>/stat`, with the scenario of `verify_lsof_port_owners.rs`.
//! When a service's line says a port is in use, `lokal up` asks who holds the
//! service's own port, with each holder's process group, to tell the
//! service's own processes from others (SPEC §4.11 "A port that's taken
//! after all"). No sudo; kept as documentation. Linux only.
//!
//! This process (the "experiment") sets up, on ports the kernel picks:
//!   - A: a child in a process group of its own (`process_group(0)`, as `lokal
//!     up` starts a service) that listens on 127.0.0.1:P and [::1]:P; the
//!     experiment keeps a client connection to 127.0.0.1:P open;
//!   - B: a child in the experiment's own process group that listens on
//!     127.0.0.1:Q;
//!   - R: a socket of the experiment's that is bound to 127.0.0.1:R but
//!     doesn't listen, without SO_REUSEADDR (as `hold()` in `tests/up.rs`
//!     holds a port).
//!
//! Then it checks:
//!   1. for P: A is listed, with its two listeners (`LISTEN`, local
//!      `127.0.0.1:P` and `[::1]:P`, peer port `*`) and its accepted
//!      connection (`ESTAB`, `127.0.0.1:P` to `127.0.0.1:X`), each with
//!      `users:(("<name>",pid=<A>,fd=…))`; the experiment's client socket
//!      (local port X) isn't, since the filter is on the local port;
//!   2. field 5 of `/proc/<pid>/stat` equals `getpgid(pid)` and `ps -o pgid=`,
//!      for A, B and the experiment;
//!   3. for Q: B, whose pgid is the experiment's;
//!   4. for R, bound and not listening (recorded): what plain `ss -a` shows,
//!      whether `/proc/net/tcp` has it, whether this ss has `-B`
//!      (`--bound-inactive`) and what it shows; a server's bind of
//!      127.0.0.1:R (std's, with SO_REUSEADDR) must fail with `EADDRINUSE`,
//!      and a connect to R is refused or times out;
//!   5. for a port nothing holds: no output (exit status recorded);
//!   6. `ss -H -t -l -n -p 'sport = :P'` lists exactly A's two listeners;
//!   7. the name in `users:` is the kernel's `comm`, which is also in
//!      `/proc/<pid>/stat`: this binary's 21-character name, cut to 15
//!      (recorded); and the loopback listeners that ss shows without
//!      `users:` (another user's, such as systemd-resolved's), where a
//!      connect succeeds (recorded);
//!   8. a connected socket's local port, for a server (recorded): with the
//!      experiment's connections to A open (local ports X on 127.0.0.1 and Y
//!      on ::1), a server's bind and listen on 127.0.0.1:X and [::1]:Y with
//!      and without SO_REUSEADDR.
//!
//! It also times the lookup.
//!
//! Usage: verify_ss_port_owners
//!
//! Results: SPEC §12, V11, the Linux lines (and below once measured on a
//! real machine).

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SS: [&str; 4] = ["/usr/bin/ss", "/usr/sbin/ss", "/bin/ss", "/sbin/ss"];

/// One socket from an ss line.
#[derive(Debug, Clone)]
struct Sock {
    state: String,
    local: String,
    peer: String,
    /// `(pid, name)` from `users:(…)`.
    users: Vec<(i32, String)>,
}

fn ss_bin() -> &'static str {
    SS.into_iter()
        .find(|p| Path::new(p).is_file())
        .expect("no ss in /usr/bin, /usr/sbin, /bin or /sbin")
}

/// `(exit status, stdout, stderr)` of ss with `args`.
fn ss(args: &[&str]) -> (i32, String, String) {
    let o = Command::new(ss_bin()).args(args).output().expect("run ss");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn parse(out: &str) -> Vec<Sock> {
    out.lines()
        .filter_map(|l| {
            let (cols, users) = match l.find("users:(") {
                Some(i) => (&l[..i], &l[i + "users:(".len()..]),
                None => (l, ""),
            };
            let mut it = cols.split_whitespace();
            let mut state = it.next()?;
            if state == "tcp" {
                state = it.next()?;
            }
            let (_, _) = (it.next()?, it.next()?);
            let local = it.next()?.to_string();
            let peer = it.next()?.to_string();
            let users = users
                .split("(\"")
                .skip(1)
                .filter_map(|e| {
                    let (name, rest) = e.split_once("\",pid=")?;
                    let pid = rest.split(',').next()?.parse().ok()?;
                    Some((pid, name.to_string()))
                })
                .collect();
            Some(Sock {
                state: state.to_string(),
                local,
                peer,
                users,
            })
        })
        .collect()
}

fn owners(port: u16) -> (i32, Vec<Sock>) {
    let filter = format!("sport = :{port}");
    let (code, out, _) = ss(&["-H", "-t", "-a", "-n", "-p", &filter]);
    (code, parse(&out))
}

fn port_of(addr: &str) -> Option<u16> {
    addr.rsplit_once(':')?.1.parse().ok()
}

fn has_user(s: &Sock, pid: i32) -> bool {
    s.users.iter().any(|u| u.0 == pid)
}

/// `(comm, pgid)` from `/proc/<pid>/stat`, split at the last `)`.
fn proc_stat(pid: i32) -> Option<(String, i32)> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = s.find('(')?;
    let close = s.rfind(')')?;
    let mut rest = s[close + 1..].split_whitespace();
    let (_state, _ppid) = (rest.next()?, rest.next()?);
    Some((s[open + 1..close].to_string(), rest.next()?.parse().ok()?))
}

fn ps_pgid(pid: i32) -> Option<i32> {
    let o = Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&o.stdout).trim().parse().ok()
}

fn getpgid(pid: i32) -> i32 {
    // SAFETY: getpgid only reads the process table.
    unsafe { libc::getpgid(pid) }
}

/// Lines of /proc/net/tcp{,6} whose local port is `port`.
fn proc_net_tcp(port: u16) -> Vec<String> {
    let hex = format!(":{port:04X}");
    ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .flat_map(|t| {
            t.lines()
                .skip(1)
                .filter(|l| {
                    l.split_whitespace()
                        .nth(1)
                        .is_some_and(|a| a.ends_with(&hex))
                })
                .map(|l| l.trim().to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn park() {
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

/// Starts `self <mode>`, in its own process group when `own_group`, and
/// reads its first line.
fn start(mode: &str, own_group: bool) -> (Child, String) {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.arg(mode).stdin(Stdio::piped()).stdout(Stdio::piped());
    if own_group {
        cmd.process_group(0);
    }
    let mut c = cmd.spawn().unwrap();
    let mut line = String::new();
    BufReader::new(c.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .unwrap();
    (c, line.trim().to_string())
}

fn stop(mut c: Child) {
    drop(c.stdin.take());
    let _ = c.wait();
}

fn mark(ok: bool) -> &'static str {
    if ok { "pass" } else { "FAIL" }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// The machine: kernel, distribution, ss's version and what /bin/sh is.
fn describe_system() {
    let uname = Command::new("uname")
        .arg("-srm")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let pretty = ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .find_map(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_default();
    let (_, v, _) = ss(&["-V"]);
    let sh = std::fs::canonicalize("/bin/sh")
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into());
    println!(
        "{uname}; {pretty}; {} ({}); /bin/sh -> {sh}",
        first_line(&v),
        ss_bin()
    );
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--a") => {
            let v4 = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = v4.local_addr().unwrap().port();
            let v6 = TcpListener::bind(("::1", port)).unwrap();
            println!("{} {port}", std::process::id());
            std::io::stdout().flush().unwrap();
            // Accept the experiment's two connections, and keep them.
            let (_c4, _) = v4.accept().unwrap();
            let (_c6, _) = v6.accept().unwrap();
            park();
            return;
        }
        Some("--b") => {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            println!("{} {}", std::process::id(), l.local_addr().unwrap().port());
            std::io::stdout().flush().unwrap();
            park();
            return;
        }
        _ => {}
    }
    if !Path::new("/proc/self/stat").exists() {
        eprintln!("verify_ss_port_owners: Linux only (no /proc)");
        std::process::exit(2);
    }
    describe_system();
    let mut ok = true;
    let me = std::process::id() as i32;
    // SAFETY: getpgrp has no preconditions.
    let my_pgid = unsafe { libc::getpgrp() };

    let (a, aline) = start("--a", true);
    let af: Vec<i32> = aline
        .split_whitespace()
        .map(|x| x.parse().unwrap())
        .collect();
    let (a_pid, p) = (af[0], af[1] as u16);
    let client = TcpStream::connect(("127.0.0.1", p)).unwrap();
    let x = client.local_addr().unwrap().port();
    let client6 = TcpStream::connect(("::1", p)).unwrap();
    let y = client6.local_addr().unwrap().port();
    let (b, bline) = start("--b", false);
    let bf: Vec<i32> = bline
        .split_whitespace()
        .map(|x| x.parse().unwrap())
        .collect();
    let (b_pid, q) = (bf[0], bf[1] as u16);
    let r_sock = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
    r_sock
        .bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())
        .unwrap();
    let r = r_sock.local_addr().unwrap().as_socket().unwrap().port();
    // Let A accept the connections.
    std::thread::sleep(Duration::from_millis(100));

    // 1. P: A's listeners and accepted connection
    let mut times = vec![];
    let mut got = (0, vec![]);
    for _ in 0..5 {
        let t = Instant::now();
        got = owners(p);
        times.push(t.elapsed().as_micros() as f64 / 1000.0);
    }
    let (code1, socks) = got;
    let listen: Vec<&Sock> = socks
        .iter()
        .filter(|s| s.state == "LISTEN" && has_user(s, a_pid))
        .collect();
    let listen_local: Vec<&str> = listen.iter().map(|s| s.local.as_str()).collect();
    let conn = format!("127.0.0.1:{x}");
    let a_conn = socks
        .iter()
        .any(|s| s.state == "ESTAB" && s.peer == conn && has_user(s, a_pid));
    let pass1 = code1 == 0
        && listen.len() == 2
        && listen_local.contains(&format!("127.0.0.1:{p}").as_str())
        && listen_local.contains(&format!("[::1]:{p}").as_str())
        && listen.iter().all(|s| s.peer.ends_with(":*"))
        && a_conn
        && !socks.iter().any(|s| has_user(s, me))
        && socks.iter().all(|s| port_of(&s.local) == Some(p));
    ok &= pass1;
    println!(
        "1. {}: port {p}: exit {code1}; {} sockets: {:?}",
        mark(pass1),
        socks.len(),
        socks
            .iter()
            .map(|s| format!("{} {} -> {} {:?}", s.state, s.local, s.peer, s.users))
            .collect::<Vec<_>>()
    );

    // 2. pgid: /proc = getpgid = ps
    let checks = [(a_pid, a_pid), (me, my_pgid), (b_pid, my_pgid)];
    let got2: Vec<(i32, Option<i32>, i32, Option<i32>)> = checks
        .iter()
        .map(|(pid, _)| {
            (
                *pid,
                proc_stat(*pid).map(|s| s.1),
                getpgid(*pid),
                ps_pgid(*pid),
            )
        })
        .collect();
    let pass2 = checks
        .iter()
        .zip(&got2)
        .all(|((_, want), (_, proc, g, ps))| {
            *proc == Some(*want) && g == want && *ps == Some(*want)
        });
    ok &= pass2;
    println!(
        "2. {}: (pid, /proc pgid, getpgid, ps): {got2:?}",
        mark(pass2)
    );

    // 3. Q: B, in our group
    let (_, socks_q) = owners(q);
    let pb = socks_q
        .iter()
        .find(|s| s.state == "LISTEN" && has_user(s, b_pid));
    let pass3 = pb.is_some() && proc_stat(b_pid).map(|s| s.1) == Some(my_pgid);
    ok &= pass3;
    println!("3. {}: port {q}: B: {pb:?}", mark(pass3));

    // 4. R: bound, not listening
    let (code4, socks_r) = owners(r);
    let proc_r = proc_net_tcp(r);
    let filter_r = format!("sport = :{r}");
    let (code_b, out_b, err_b) = ss(&["-H", "-t", "-B", "-n", "-p", &filter_r]);
    let (code_ab, out_ab, err_ab) = ss(&["-H", "-t", "-a", "-B", "-n", "-p", &filter_r]);
    let bind = TcpListener::bind(("127.0.0.1", r));
    let bind_err = bind.as_ref().err().and_then(|e| e.raw_os_error());
    let connect = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], r)),
        Duration::from_millis(50),
    );
    let pass4 = bind_err == Some(libc::EADDRINUSE) && connect.is_err();
    ok &= pass4;
    println!(
        "4. {}: port {r} bound, not listening: ss -a: exit {code4}, {} sockets {:?}; /proc/net/tcp{{,6}}: {proc_r:?}; \
ss -B: exit {code_b}, stdout {:?}, stderr {:?}; ss -a -B: exit {code_ab}, stdout {:?}, stderr {:?}; \
a server's bind: {:?}; connect: {:?}",
        mark(pass4),
        socks_r.len(),
        socks_r,
        out_b.trim(),
        first_line(&err_b),
        out_ab.trim(),
        first_line(&err_ab),
        bind.map(|_| "ok").map_err(|e| e.to_string()),
        connect.map(|_| "ok").map_err(|e| e.to_string()),
    );

    // 5. a port nothing holds
    let free = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let (code5, socks5) = owners(free);
    let pass5 = socks5.is_empty();
    ok &= pass5;
    println!(
        "5. {}: port {free}, nothing there: exit {code5}, {} sockets",
        mark(pass5),
        socks5.len()
    );

    // 6. the -l form lists exactly A's listeners
    let filter_p = format!("sport = :{p}");
    let (_, out6, _) = ss(&["-H", "-t", "-l", "-n", "-p", &filter_p]);
    let l6 = parse(&out6);
    let pass6 = l6.len() == 2 && l6.iter().all(|s| s.state == "LISTEN" && has_user(s, a_pid));
    ok &= pass6;
    println!("6. {}: -l: {:?}", mark(pass6), l6);

    // 7. names, and listeners whose process ss doesn't show (recorded)
    let ss_name = listen
        .first()
        .and_then(|s| s.users.first())
        .map(|u| u.1.clone());
    let proc_name = proc_stat(a_pid).map(|s| s.0);
    let (_, all_l, _) = ss(&["-H", "-t", "-l", "-n", "-p"]);
    let hidden: Vec<String> = parse(&all_l)
        .into_iter()
        .filter(|s| s.users.is_empty())
        .map(|s| s.local)
        .collect();
    let hidden_connect: Vec<(String, bool)> = hidden
        .iter()
        .filter(|l| l.starts_with("127.") || l.starts_with("[::1]"))
        .filter_map(|l| {
            let host = l.rsplit_once(':')?.0;
            let host = host.split('%').next()?;
            let addr: SocketAddr = format!("{host}:{}", port_of(l)?).parse().ok()?;
            let c = TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok();
            Some((l.clone(), c))
        })
        .collect();
    println!(
        "7. recorded: users: name {ss_name:?}, /proc comm {proc_name:?}; listeners without users: {hidden:?}; \
loopback ones, connect: {hidden_connect:?}"
    );

    // 8. a connected socket's local port, for a server (recorded)
    let bind_listen = |addr: SocketAddr, reuse: bool| -> Result<(), i32> {
        let domain = if addr.is_ipv4() {
            socket2::Domain::IPV4
        } else {
            socket2::Domain::IPV6
        };
        let s = socket2::Socket::new(domain, socket2::Type::STREAM, None).unwrap();
        s.set_reuse_address(reuse).unwrap();
        s.bind(&addr.into())
            .and_then(|()| s.listen(1))
            .map_err(|e| e.raw_os_error().unwrap_or(-1))
    };
    let x_addr = SocketAddr::from(([127, 0, 0, 1], x));
    let y_addr = SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, y));
    let got8 = [
        bind_listen(x_addr, true),
        bind_listen(y_addr, true),
        bind_listen(x_addr, false),
        bind_listen(y_addr, false),
    ];
    let filter_x = format!("sport = :{x}");
    let (_, out8, _) = ss(&["-H", "-t", "-a", "-n", "-p", &filter_x]);
    println!(
        "8. recorded: connected sockets' local ports {x_addr} and {y_addr}: with SO_REUSEADDR {:?} {:?}; \
without {:?} {:?} (errno {} is EADDRINUSE); ss for {x}: {:?}",
        got8[0],
        got8[1],
        got8[2],
        got8[3],
        libc::EADDRINUSE,
        parse(&out8)
    );

    println!("ss -H -t -a -n -p 'sport = :<port>': {times:?} ms");
    drop(client6);
    drop(client);
    drop(r_sock);
    stop(b);
    stop(a);
    std::process::exit(if ok { 0 } else { 1 });
}
