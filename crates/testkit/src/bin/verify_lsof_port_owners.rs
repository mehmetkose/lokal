//! The port-owner lookup behind `System::port_owners` (SPEC §4.11 "A port
//! that's taken after all", VERIFY V11): when a service's line says a port is
//! in use, `lokal up` asks who holds the service's own port, with each
//! holder's process group, to tell the service's own processes from others.
//! No sudo; kept as documentation.
//!
//! This process (the "experiment") sets up, on ports the kernel picks:
//!   - A: a child in a process group of its own (`process_group(0)`, as `lokal
//!     up` starts a service) that listens on 127.0.0.1:P and [::1]:P; the
//!     experiment keeps a client connection to 127.0.0.1:P open;
//!   - B: a child in the experiment's own process group that listens on
//!     127.0.0.1:Q;
//!   - R: a socket of the experiment's that is bound to 127.0.0.1:R but
//!     doesn't listen (as `a_kept_port_that_a_server_cant_bind_moves_too`
//!     holds a port).
//!
//! Then it checks, with `lsof -nP -iTCP:<port> -F pgcnT`:
//!   1. for P: A is listed once as a process (`p`), with `g` its own pgid
//!      (= its pid, not the experiment's), `c` its full command name, and its
//!      two listeners as `n127.0.0.1:P` and `n[::1]:P` with `TST=LISTEN`; its
//!      accepted connection is `n127.0.0.1:P->127.0.0.1:X` (`TST=ESTABLISHED`);
//!      the experiment's client socket is listed too, as
//!      `n127.0.0.1:X->127.0.0.1:P`, so a lookup must keep only the sockets
//!      whose local port is P;
//!   2. the `g` field equals `getpgid(pid)` and `ps -o pgid= -p <pid>`, for A,
//!      for B and for the experiment;
//!   3. for Q: B with `g` the experiment's pgid;
//!   4. for R: the experiment, `n127.0.0.1:R`, with a state other than
//!      LISTEN (recorded). A server's bind of 127.0.0.1:R (std's
//!      `TcpListener::bind`, which sets SO_REUSEADDR) fails with
//!      `EADDRINUSE`, while a connect to R is refused: only lsof sees that R
//!      is held;
//!   5. for a port nothing holds: no output (exit status recorded);
//!   6. the processes that `lsof -nP -iTCP:P -sTCP:LISTEN` (the old
//!      `port_owner` form) lists are exactly those with a `TST=LISTEN`
//!      socket on local port P in the new form;
//!   7. with a port argument: a loopback TCP listener that `netstat -anp tcp`
//!      shows and lsof doesn't (another user's, such as root's), found by
//!      hand: a connect succeeds and lsof prints nothing for its port;
//!   8. a connected socket doesn't hold its local port for a server: with the
//!      experiment's connections to A open (local ports X on 127.0.0.1 and Y
//!      on ::1), a server's bind and listen on 127.0.0.1:X and [::1]:Y with
//!      SO_REUSEADDR (as std, Node, Go and Python's servers set it)
//!      succeeds; without SO_REUSEADDR it fails with `EADDRINUSE`. So only
//!      unconnected sockets (listening, or only bound) count as holding a
//!      port.
//!   9. lokal's bind check (recorded): binding 127.0.0.1 and ::1 with
//!      SO_REUSEADDR, without listening, on A's port, on R, on the connected
//!      sockets' local ports X and Y, and on a free port: `EADDRINUSE` exactly
//!      where a server's bind fails.
//!
//! It also times the lookup.
//!
//! Usage: verify_lsof_port_owners [PORT]
//!
//! Measured on macOS 27.0.1 (26A434), 2026-10-26, Apple Silicon, lsof 4.91,
//! `verify_lsof_port_owners 8021` (3 runs, all passed):
//!   1. pass: lsof exits 0; A is one `p` record with `g` = its pid and `c` its
//!      full name (`verify_lsof_port_owners`, 23 characters); its sockets are
//!      `127.0.0.1:P` and `[::1]:P` (`TST=LISTEN`) and
//!      `127.0.0.1:P->127.0.0.1:X` (`TST=ESTABLISHED`); the experiment is
//!      listed too, for its client socket `127.0.0.1:X->127.0.0.1:P`
//!   2. pass: `g` = getpgid = `ps -o pgid=` for all three processes
//!   3. pass: B's `g` is the experiment's pgid
//!   4. pass: the bound socket is listed with `TST=CLOSED`; a server's bind
//!      fails with EADDRINUSE (48); a connect isn't refused but times out
//!      (50ms): the kernel drops the SYN, so a probe finds nothing listening
//!   5. pass: no output, exit 1
//!   6. pass: the same pid; the old form's COMMAND column cuts the name to 9
//!      characters (`verify_ls`), the `c` field doesn't
//!   7. pass: port 8021 (netstat: LISTEN on 127.0.0.1 and ::1, not in lsof):
//!      a connect succeeds; lsof exits 1 with no output
//!   8. pass: binding a connected socket's local port works with SO_REUSEADDR,
//!      on 127.0.0.1 and on ::1, and fails with EADDRINUSE (48) without it
//!   9. recorded 2026-10-27, same Mac, 3 runs: the bind check gives
//!      EADDRINUSE (48) for A's port on both addresses and for R on
//!      127.0.0.1 (R is IPv4 only, so ::1 binds), and binds on X, Y and a
//!      free port: it fails exactly where a server's bind does (items 4, 8)
//!
//!   lsof -nP -iTCP:<port> -F pgcnT: 31–54ms (5 per run, the first the
//!   slowest)

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// One process in `lsof -F pgcnT` output, with its sockets as `(name, state)`.
#[derive(Debug, Default, Clone)]
struct Proc {
    pid: i32,
    pgid: Option<i32>,
    cmd: String,
    socks: Vec<(String, String)>,
}

fn lsof(args: &[&str]) -> (i32, String) {
    let o = Command::new("/usr/sbin/lsof")
        .args(args)
        .output()
        .expect("run lsof");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
    )
}

fn parse(out: &str) -> Vec<Proc> {
    let mut procs: Vec<Proc> = vec![];
    for l in out.lines() {
        let (tag, rest) = l.split_at(l.len().min(1));
        match tag {
            "p" => procs.push(Proc {
                pid: rest.parse().unwrap_or(-1),
                ..Proc::default()
            }),
            "g" => {
                if let Some(p) = procs.last_mut() {
                    p.pgid = rest.parse().ok();
                }
            }
            "c" => {
                if let Some(p) = procs.last_mut() {
                    p.cmd = rest.to_string();
                }
            }
            "f" => {
                if let Some(p) = procs.last_mut() {
                    p.socks.push((String::new(), String::new()));
                }
            }
            "n" => {
                if let Some(s) = procs.last_mut().and_then(|p| p.socks.last_mut()) {
                    s.0 = rest.to_string();
                }
            }
            "T" => {
                if let (Some(st), Some(s)) = (
                    rest.strip_prefix("ST="),
                    procs.last_mut().and_then(|p| p.socks.last_mut()),
                ) {
                    s.1 = st.to_string();
                }
            }
            _ => {}
        }
    }
    procs
}

fn owners(port: u16) -> (i32, Vec<Proc>) {
    let sel = format!("-iTCP:{port}");
    let (code, out) = lsof(&["-nP", &sel, "-F", "pgcnT"]);
    (code, parse(&out))
}

/// The local port of an lsof TCP name: `127.0.0.1:5173`, `[::1]:5173`,
/// `*:5173`, or the part before `->` of a connection.
fn local_port(name: &str) -> Option<u16> {
    let local = name.split("->").next()?;
    local.rsplit_once(':')?.1.parse().ok()
}

fn ps_pgid(pid: i32) -> Option<i32> {
    let o = Command::new("/bin/ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&o.stdout).trim().parse().ok()
}

fn getpgid(pid: i32) -> i32 {
    // SAFETY: getpgid only reads the process table.
    unsafe { libc::getpgid(pid) }
}

/// lokal's bind check (SPEC §4.11, `net::bind_blocked`): bind
/// `127.0.0.1:<port>` and `[::1]:<port>` with SO_REUSEADDR, as servers set it,
/// without listening, and release both at once. `(v4, v6)`: `Ok` or the errno.
fn bind_check(port: u16) -> (Result<(), i32>, Result<(), i32>) {
    let bind = |addr: SocketAddr| -> Result<socket2::Socket, i32> {
        let s = socket2::Socket::new(
            socket2::Domain::for_address(addr),
            socket2::Type::STREAM,
            None,
        )
        .map_err(|e| e.raw_os_error().unwrap_or(-1))?;
        s.set_reuse_address(true)
            .and_then(|()| s.bind(&addr.into()))
            .map_err(|e| e.raw_os_error().unwrap_or(-1))?;
        Ok(s)
    };
    let v4 = bind(SocketAddr::from(([127, 0, 0, 1], port)));
    let v6 = bind(SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)));
    (v4.map(drop), v6.map(drop))
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

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--a") => {
            let v4 = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = v4.local_addr().unwrap().port();
            let v6 = TcpListener::bind(("::1", port)).unwrap();
            println!("{} {port}", std::process::id());
            std::io::stdout().flush().unwrap();
            // Accept the experiment's connection, and keep it.
            let (_conn, _) = v4.accept().unwrap();
            park();
            drop(v6);
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
    let sw = |arg: &str| {
        String::from_utf8_lossy(
            &Command::new("/usr/bin/sw_vers")
                .arg(arg)
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string()
    };
    println!("macOS {} ({})", sw("-productVersion"), sw("-buildVersion"));
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
    let client_port = client.local_addr().unwrap().port();
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
    // Let A accept the connection.
    std::thread::sleep(Duration::from_millis(100));

    // 1. P: A's listeners and accepted connection, and our client socket
    let mut times = vec![];
    let mut got = (0, vec![]);
    for _ in 0..5 {
        let t = Instant::now();
        got = owners(p);
        times.push(t.elapsed().as_millis());
    }
    let (code1, procs) = got;
    let pa = procs.iter().find(|x| x.pid == a_pid);
    let pme = procs.iter().find(|x| x.pid == me);
    let want_listen: BTreeSet<String> = [format!("127.0.0.1:{p}"), format!("[::1]:{p}")].into();
    let a_listen: BTreeSet<String> = pa
        .map(|x| {
            x.socks
                .iter()
                .filter(|s| s.1 == "LISTEN")
                .map(|s| s.0.clone())
                .collect()
        })
        .unwrap_or_default();
    let a_conn = format!("127.0.0.1:{p}->127.0.0.1:{client_port}");
    let a_has_conn = pa.is_some_and(|x| {
        x.socks
            .iter()
            .any(|s| s.0 == a_conn && s.1 == "ESTABLISHED")
    });
    let my_conn = format!("127.0.0.1:{client_port}->127.0.0.1:{p}");
    let me_listed = pme.is_some_and(|x| x.socks.iter().any(|s| s.0 == my_conn));
    let pass1 = pa.is_some_and(|x| x.pgid == Some(a_pid) && !x.cmd.is_empty())
        && procs.iter().filter(|x| x.pid == a_pid).count() == 1
        && a_listen == want_listen
        && a_has_conn
        && me_listed
        && local_port(&my_conn) == Some(client_port)
        && local_port(&a_conn) == Some(p);
    ok &= pass1;
    println!(
        "1. {}: port {p}: exit {code1}; A (pid {a_pid}): {:?}; experiment (pid {me}, pgid {my_pgid}): {:?}",
        mark(pass1),
        pa,
        pme
    );

    // 2. g = getpgid = ps
    let g = |pid: i32, procs: &[Proc]| procs.iter().find(|x| x.pid == pid).and_then(|x| x.pgid);
    let (_, procs_q) = owners(q);
    let checks = [
        (a_pid, g(a_pid, &procs), a_pid),
        (me, g(me, &procs), my_pgid),
        (b_pid, g(b_pid, &procs_q), my_pgid),
    ];
    let pass2 = checks.iter().all(|(pid, lsof_g, want)| {
        *lsof_g == Some(*want) && getpgid(*pid) == *want && ps_pgid(*pid) == Some(*want)
    });
    ok &= pass2;
    println!(
        "2. {}: (pid, lsof g, want): {checks:?}; ps: {:?}",
        mark(pass2),
        checks.iter().map(|c| ps_pgid(c.0)).collect::<Vec<_>>()
    );

    // 3. Q: B, in our group
    let pb = procs_q.iter().find(|x| x.pid == b_pid);
    let pass3 = pb.is_some_and(|x| {
        x.pgid == Some(my_pgid)
            && x.socks
                .iter()
                .any(|s| s.0 == format!("127.0.0.1:{q}") && s.1 == "LISTEN")
    });
    ok &= pass3;
    println!("3. {}: port {q}: B: {pb:?}", mark(pass3));

    // 4. R: bound, not listening
    let (code4, procs_r) = owners(r);
    let pr = procs_r.iter().find(|x| x.pid == me);
    let r_state = pr
        .and_then(|x| x.socks.iter().find(|s| s.0 == format!("127.0.0.1:{r}")))
        .map(|s| s.1.clone());
    let bind = TcpListener::bind(("127.0.0.1", r));
    let bind_err = bind.as_ref().err().and_then(|e| e.raw_os_error());
    let connect = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], r)),
        Duration::from_millis(50),
    );
    let pass4 = pr.is_some_and(|x| x.pgid == Some(my_pgid))
        && r_state.as_deref().is_some_and(|s| s != "LISTEN")
        && bind_err == Some(libc::EADDRINUSE)
        && connect.is_err();
    ok &= pass4;
    println!(
        "4. {}: port {r} bound, not listening: exit {code4}; state {r_state:?}; a server's bind: {:?}; connect: {:?}",
        mark(pass4),
        bind.map(|_| "ok").map_err(|e| e.to_string()),
        connect.map(|_| "ok").map_err(|e| e.to_string()),
    );

    // 5. a port nothing holds
    let free = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let (code5, procs5) = owners(free);
    let pass5 = procs5.is_empty();
    ok &= pass5;
    println!(
        "5. {}: port {free}, nothing there: exit {code5}, {} processes",
        mark(pass5),
        procs5.len()
    );

    // 6. the old -sTCP:LISTEN form lists the same processes
    let sel = format!("-iTCP:{p}");
    let (_, old) = lsof(&["-nP", &sel, "-sTCP:LISTEN"]);
    let old_pids: BTreeSet<i32> = old
        .lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().nth(1)?.parse().ok())
        .collect();
    let new_pids: BTreeSet<i32> = procs
        .iter()
        .filter(|x| {
            x.socks
                .iter()
                .any(|s| s.1 == "LISTEN" && local_port(&s.0) == Some(p))
        })
        .map(|x| x.pid)
        .collect();
    let old_names: Vec<String> = old
        .lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next().map(String::from))
        .collect();
    let pass6 = old_pids == new_pids && old_pids == [a_pid].into();
    ok &= pass6;
    println!(
        "6. {}: -sTCP:LISTEN pids {old_pids:?} (COMMAND column {old_names:?}), -F listeners {new_pids:?} (c {:?})",
        mark(pass6),
        pa.map(|x| &x.cmd)
    );

    // 7. a listener lsof can't see: a port given on the command line
    match std::env::args().nth(1).and_then(|a| a.parse::<u16>().ok()) {
        Some(port) => {
            let conn = [
                SocketAddr::from(([127, 0, 0, 1], port)),
                SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)),
            ]
            .iter()
            .any(|a| TcpStream::connect_timeout(a, Duration::from_millis(50)).is_ok());
            let (code7, procs7) = owners(port);
            let pass7 = conn && procs7.is_empty();
            ok &= pass7;
            println!(
                "7. {}: port {port}, another user's listener: connect {conn}, lsof exit {code7}, {} processes",
                mark(pass7),
                procs7.len()
            );
        }
        None => println!("7. skipped: no port given"),
    }

    // 8. a connected socket's local port, for a server
    let client6 = TcpStream::connect(("::1", p)).unwrap();
    let y = client6.local_addr().unwrap().port();
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
    let x_addr = SocketAddr::from(([127, 0, 0, 1], client_port));
    let y_addr = SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, y));
    let got8 = [
        bind_listen(x_addr, true),
        bind_listen(y_addr, true),
        bind_listen(x_addr, false),
        bind_listen(y_addr, false),
    ];
    let pass8 = got8[0].is_ok()
        && got8[1].is_ok()
        && got8[2] == Err(libc::EADDRINUSE)
        && got8[3] == Err(libc::EADDRINUSE);
    ok &= pass8;
    println!(
        "8. {}: connected sockets' local ports {x_addr} and {y_addr}: with SO_REUSEADDR {:?} {:?}; without {:?} {:?}",
        mark(pass8),
        got8[0],
        got8[1],
        got8[2],
        got8[3]
    );

    // 9. lokal's bind check on each kind of socket (recorded)
    let free9 = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    println!(
        "9. recorded: bind check (SO_REUSEADDR, no listen; v4, v6): listener {p}: {:?}; bound only {r}: {:?}; \
connections' local ports {client_port} (v4) and {y} (v6): {:?} {:?}; free {free9}: {:?} (errno {} is EADDRINUSE)",
        bind_check(p),
        bind_check(r),
        bind_check(client_port).0,
        bind_check(y).1,
        bind_check(free9),
        libc::EADDRINUSE
    );
    println!("lsof -nP -iTCP:<port> -F pgcnT: {times:?} ms");
    drop(client6);
    drop(client);
    drop(r_sock);
    stop(b);
    stop(a);
    std::process::exit(if ok { 0 } else { 1 });
}
