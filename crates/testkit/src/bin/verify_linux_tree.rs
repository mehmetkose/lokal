//! VERIFY V9 on Linux (SPEC §12): the listener lookup behind Linux's
//! `System::tcp_listen_ports`, with the scenario of `verify_lsof_tree.rs`.
//! `lokal run`'s wrong-port hint asks which TCP ports its command, and the
//! processes the command started, listen on (SPEC §4.10). On Linux, lokal
//! reads `(pid, ppid)` from `/proc/<n>/stat` for every process, walks it
//! from the command, and keeps the listeners in `ss -H -t -l -n -p` whose
//! `users:` has a pid in that tree. No sudo; kept as documentation. Linux
//! only.
//!
//! This process listens on 127.0.0.1:0 and starts two copies of itself in its
//! own process group, as `lokal run` shares its group with its command:
//!   - a "neighbour" that listens on 127.0.0.1:0, standing in for an unrelated
//!     process in the same group (a sandbox daemon, another job of a script);
//!   - a "command" that listens on 127.0.0.1:0 and starts a grandchild that
//!     listens on 127.0.0.1:0, [::1]:0 and [::]:0 (npm → node).
//!
//! Then it checks that:
//!   1. every numeric entry of `/proc` has a `/proc/<n>/stat` that parses
//!      (split at the last `)`), or is gone by the time it's read (counted);
//!      the table shows the real parent of this process, the command and
//!      the grandchild; and walking it from the command finds exactly the
//!      command and the grandchild;
//!   2. the listeners in `ss -H -t -l -n -p` whose `users:` has one of those
//!      pids are exactly their 4 ports (how `[::]:0` is shown is recorded),
//!      and none of this process's or the neighbour's;
//!   3. the same ss output lists the neighbour's and this process's
//!      listeners too, each with its own pid (so the pid filter is needed);
//!   4. a pid that's gone has no `/proc/<pid>/stat` (ENOENT), so the walk
//!      from it is empty;
//!   5. for pids that listen on nothing, no listener has their pid.
//!
//! It also times the `/proc` read and ss.
//!
//! Usage: verify_linux_tree
//!
//! Measured 2026-10-27 on GitHub's `ubuntu-latest` (Linux 6.17.0-1022-azure,
//! x86_64) and `ubuntu-24.04-arm` (Linux 7.0.0-1017-azure, aarch64), both
//! Ubuntu 24.04.5 LTS, iproute2 6.1.0 (/usr/bin/ss), /bin/sh -> dash; 3 runs
//! each, all passed:
//!   1. pass: 166 (x86_64) and 167 (arm64) entries in /proc, every one
//!      parsed, none gone; the walk found exactly the command and the
//!      grandchild
//!   2. pass: exactly the 4 ports, as 127.0.0.1:…, [::1]:… and *:… (the
//!      [::]:0 listener, dual-stack); none of the neighbour's or this one's
//!   3. pass: 10 listeners in all, the neighbour's and this one's under their
//!      own pids
//!   4. pass: a gone pid's /proc/<pid>/stat fails with ENOENT
//!   5. pass: no listener has the pid
//!
//!   /proc read: 2.3–2.7ms (x86_64), 1.3–1.8ms (arm64); ss -l -p: 11.0–11.7ms
//!   and 5.1–5.5ms (5 per run)

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Instant;

const SS: [&str; 4] = ["/usr/bin/ss", "/usr/sbin/ss", "/bin/ss", "/sbin/ss"];

fn ss_bin() -> &'static str {
    SS.into_iter()
        .find(|p| Path::new(p).is_file())
        .expect("no ss in /usr/bin, /usr/sbin, /bin or /sbin")
}

fn ss(args: &[&str]) -> String {
    let o = Command::new(ss_bin()).args(args).output().expect("run ss");
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// `(local address, pids)` of each listening socket in `ss -l -p` output.
fn listeners(out: &str) -> Vec<(String, Vec<u32>)> {
    out.lines()
        .filter_map(|l| {
            let (cols, users) = match l.find("users:(") {
                Some(i) => (&l[..i], &l[i..]),
                None => (l, ""),
            };
            let mut it = cols.split_whitespace();
            let mut state = it.next()?;
            if state == "tcp" {
                state = it.next()?;
            }
            if state != "LISTEN" {
                return None;
            }
            let local = it.nth(2)?.to_string();
            let pids = users
                .split(",pid=")
                .skip(1)
                .filter_map(|p| p.split(',').next()?.parse().ok())
                .collect();
            Some((local, pids))
        })
        .collect()
}

fn port_of(addr: &str) -> Option<u16> {
    addr.rsplit_once(':')?.1.parse().ok()
}

/// `(pid, ppid)` from `/proc/<pid>/stat`: `Err` with the errno if it can't
/// be read, `Ok(None)` if it doesn't parse.
fn stat(pid: u32) -> Result<Option<(u32, u32)>, i32> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|e| e.raw_os_error().unwrap_or(-1))?;
    let parsed = (|| {
        let close = s.rfind(')')?;
        let pid = s[..s.find('(')?].trim().parse().ok()?;
        let mut rest = s[close + 1..].split_whitespace();
        let _state = rest.next()?;
        Some((pid, rest.next()?.parse().ok()?))
    })();
    Ok(parsed)
}

fn port_of_listener(l: &TcpListener) -> u16 {
    l.local_addr().unwrap().port()
}

/// Waits on stdin until the parent closes it.
fn park() {
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

/// Starts `self <mode>` in this process group and reads its first line.
fn start(mode: &str) -> (Child, String) {
    let mut c = Command::new(std::env::current_exe().unwrap())
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
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
    let v = ss(&["-V"]);
    let sh = std::fs::canonicalize("/bin/sh")
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into());
    println!(
        "{uname}; {pretty}; {} ({}); /bin/sh -> {sh}",
        v.lines().next().unwrap_or(""),
        ss_bin()
    );
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--grandchild") => {
            let ls = [
                TcpListener::bind("127.0.0.1:0").unwrap(),
                TcpListener::bind("[::1]:0").unwrap(),
                TcpListener::bind("[::]:0").unwrap(),
            ];
            let ports: Vec<String> = ls.iter().map(|l| port_of_listener(l).to_string()).collect();
            println!("{} {}", std::process::id(), ports.join(" "));
            std::io::stdout().flush().unwrap();
            park();
            return;
        }
        Some("--command") => {
            let own = TcpListener::bind("127.0.0.1:0").unwrap();
            let (gc, line) = start("--grandchild");
            // own pid, own port, then the grandchild's pid and 3 ports
            println!("{} {} {line}", std::process::id(), port_of_listener(&own));
            std::io::stdout().flush().unwrap();
            park();
            stop(gc);
            return;
        }
        Some("--neighbour") => {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            println!("{} {}", std::process::id(), port_of_listener(&l));
            std::io::stdout().flush().unwrap();
            park();
            return;
        }
        _ => {}
    }
    if !Path::new("/proc/self/stat").exists() {
        eprintln!("verify_linux_tree: Linux only (no /proc)");
        std::process::exit(2);
    }
    describe_system();
    let mut ok = true;
    let mine = TcpListener::bind("127.0.0.1:0").unwrap();
    let my_port = port_of_listener(&mine);
    let (neighbour, nline) = start("--neighbour");
    let nf: Vec<u32> = nline
        .split_whitespace()
        .map(|x| x.parse().unwrap())
        .collect();
    let (n_pid, n_port) = (nf[0], nf[1] as u16);
    let (command, cline) = start("--command");
    let f: Vec<u32> = cline
        .split_whitespace()
        .map(|x| x.parse().unwrap())
        .collect();
    let (cmd_pid, gc_pid) = (f[0], f[2]);
    let want_ports: BTreeSet<u16> = [f[1], f[3], f[4], f[5]].iter().map(|p| *p as u16).collect();

    // 1. the process table from /proc, and the walk from the command
    let mut proc_ms = vec![];
    let mut table: BTreeMap<u32, u32> = BTreeMap::new();
    let (mut entries, mut gone, mut bad) = (0, 0, 0);
    for _ in 0..5 {
        let t = Instant::now();
        table.clear();
        (entries, gone, bad) = (0, 0, 0);
        for e in std::fs::read_dir("/proc").unwrap() {
            let Some(pid) = e
                .ok()
                .and_then(|e| e.file_name().to_str()?.parse::<u32>().ok())
            else {
                continue;
            };
            entries += 1;
            match stat(pid) {
                Ok(Some((p, pp))) => {
                    table.insert(p, pp);
                }
                Ok(None) => bad += 1,
                Err(_) => gone += 1,
            }
        }
        proc_ms.push(t.elapsed().as_micros() as f64 / 1000.0);
    }
    let me = std::process::id();
    // SAFETY: getppid has no preconditions.
    let my_parent = unsafe { libc::getppid() } as u32;
    let mut tree = vec![cmd_pid];
    let mut i = 0;
    while i < tree.len() {
        let p = tree[i];
        for (c, pp) in &table {
            if *pp == p && !tree.contains(c) {
                tree.push(*c);
            }
        }
        i += 1;
    }
    tree.sort_unstable();
    let mut want_tree = vec![cmd_pid, gc_pid];
    want_tree.sort_unstable();
    let pass1 = bad == 0
        && table.get(&me) == Some(&my_parent)
        && table.get(&cmd_pid) == Some(&me)
        && table.get(&gc_pid) == Some(&cmd_pid)
        && tree == want_tree;
    ok &= pass1;
    println!(
        "1. {}: /proc: {entries} entries, {} parsed, {bad} unparsable, {gone} gone; tree from {cmd_pid}: {tree:?}, \
want {want_tree:?}",
        mark(pass1),
        table.len()
    );

    // 2. ss -l, filtered to the tree's pids
    let mut ss_ms = vec![];
    let mut out = String::new();
    for _ in 0..5 {
        let t = Instant::now();
        out = ss(&["-H", "-t", "-l", "-n", "-p"]);
        ss_ms.push(t.elapsed().as_micros() as f64 / 1000.0);
    }
    let all = listeners(&out);
    let tree_set: BTreeSet<u32> = tree.iter().copied().collect();
    let mine_l: Vec<&(String, Vec<u32>)> = all
        .iter()
        .filter(|(_, pids)| pids.iter().any(|p| tree_set.contains(p)))
        .collect();
    let got_ports: BTreeSet<u16> = mine_l.iter().filter_map(|(a, _)| port_of(a)).collect();
    let pass2 =
        got_ports == want_ports && !got_ports.contains(&my_port) && !got_ports.contains(&n_port);
    ok &= pass2;
    println!(
        "2. {}: listeners of {tree:?}: {:?}, want ports {want_ports:?}",
        mark(pass2),
        mine_l.iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>()
    );

    // 3. the others are there too, under their own pids
    let has = |port: u16, pid: u32| {
        all.iter()
            .any(|(a, pids)| port_of(a) == Some(port) && pids.contains(&pid))
    };
    let pass3 = has(n_port, n_pid) && has(my_port, me);
    ok &= pass3;
    println!(
        "3. {}: {} listeners in all, the neighbour's ({n_port}) and this one's ({my_port}) among them",
        mark(pass3),
        all.len()
    );

    // 4. a gone pid
    let (gone_child, _) = start("--neighbour");
    let gone_pid = gone_child.id();
    stop(gone_child);
    let st = stat(gone_pid);
    let pass4 = st == Err(libc::ENOENT);
    ok &= pass4;
    println!(
        "4. {}: gone pid {gone_pid}: /proc/<pid>/stat {st:?} (ENOENT is {})",
        mark(pass4),
        libc::ENOENT
    );

    // 5. pids that listen on nothing (this process, its listener closed)
    drop(mine);
    let out5 = ss(&["-H", "-t", "-l", "-n", "-p"]);
    let left: Vec<String> = listeners(&out5)
        .into_iter()
        .filter(|(_, pids)| pids.contains(&me))
        .map(|(a, _)| a)
        .collect();
    let pass5 = left.is_empty();
    ok &= pass5;
    println!("5. {}: no listeners: {left:?}", mark(pass5));
    println!("/proc read: {proc_ms:?} ms; ss -l -p: {ss_ms:?} ms");

    stop(command);
    stop(neighbour);
    std::process::exit(if ok { 0 } else { 1 });
}
