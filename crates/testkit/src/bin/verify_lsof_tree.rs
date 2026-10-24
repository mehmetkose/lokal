//! The listener lookup behind `System::tcp_listen_ports` (SPEC §4.10, VERIFY
//! V9): `lokal run`'s wrong-port hint asks which TCP ports its command, and the
//! processes the command started, listen on. No sudo; kept as documentation.
//!
//! This process listens on 127.0.0.1:0 and starts two copies of itself in its
//! own process group, as `lokal run` shares its group with its command:
//!   - a "neighbour" that listens on 127.0.0.1:0, standing in for an unrelated
//!     process in the same group (a sandbox daemon, another job of a script);
//!   - a "command" that listens on 127.0.0.1:0 and starts a grandchild that
//!     listens on 127.0.0.1:0, [::1]:0 and [::]:0 (npm → node).
//!
//! Then it checks that:
//!   1. `ps -A -o pid=,ppid=` parses as one `pid ppid` pair per line, shows the
//!      real parent of this process, the command and the grandchild, and
//!      walking it from the command finds exactly the command and the
//!      grandchild;
//!   2. `lsof -nP -a -p <command>,<grandchild> -iTCP -sTCP:LISTEN -Fn` prints
//!      the 4 ports of those two, as `n127.0.0.1:…`, `n[::1]:…` and `n*:…`,
//!      and none of this process's or the neighbour's;
//!   3. without `-a`, lsof ORs the selections and lists other listeners too
//!      (so `-a` is required);
//!   4. a pid that's gone, in the list next to a live one, doesn't stop lsof
//!      from listing the live one's ports;
//!   5. for pids that listen on nothing, lsof prints no name line.
//!
//! It also times `ps` and `lsof`.
//!
//! Usage: verify_lsof_tree
//!
//! Measured on macOS 27.0.1 (26A434), 2026-10-24, Apple Silicon (3 runs):
//!   1. pass: ps printed 579–580 lines, 0 unparsable; the walk found exactly
//!      the command and the grandchild
//!   2. pass: exactly the 4 ports, as n127.0.0.1:…, n[::1]:… and n*:… (the
//!      *:0 listener was bound on [::]); none of the neighbour's or this one's
//!   3. pass: without -a, 86 name lines, the other listeners included
//!   4. pass: lsof exits 1 for the gone pid but still prints the live pid's 3
//!      ports, so the exit status can't be used; the output can
//!   5. pass: exit 1, no name lines
//!
//!   ps -A -o pid=,ppid=: 14–26ms; lsof -a -p <2 pids>: 20–25ms (5 per run)
//!
//! It replaces `verify_lsof_group.rs`, which showed that `lsof -a -g <pgid>`
//! lists exactly a process group's listeners. A group can hold processes the
//! command never started, so the hint now asks about the command's process
//! tree instead.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Instant;

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

fn names(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| l.strip_prefix('n'))
        .map(String::from)
        .collect()
}

fn port_of(l: &TcpListener) -> u16 {
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

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--grandchild") => {
            let ls = [
                TcpListener::bind("127.0.0.1:0").unwrap(),
                TcpListener::bind("[::1]:0").unwrap(),
                TcpListener::bind("[::]:0").unwrap(),
            ];
            let ports: Vec<String> = ls.iter().map(|l| port_of(l).to_string()).collect();
            println!("{} {}", std::process::id(), ports.join(" "));
            std::io::stdout().flush().unwrap();
            park();
            return;
        }
        Some("--command") => {
            let own = TcpListener::bind("127.0.0.1:0").unwrap();
            let (gc, line) = start("--grandchild");
            // own pid, own port, then the grandchild's pid and 3 ports
            println!("{} {} {line}", std::process::id(), port_of(&own));
            std::io::stdout().flush().unwrap();
            park();
            stop(gc);
            return;
        }
        Some("--neighbour") => {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            println!("{} {}", std::process::id(), port_of(&l));
            std::io::stdout().flush().unwrap();
            park();
            return;
        }
        _ => {}
    }
    println!(
        "macOS {}",
        String::from_utf8_lossy(
            &Command::new("/usr/bin/sw_vers")
                .arg("-productVersion")
                .output()
                .unwrap()
                .stdout
        )
        .trim()
    );
    let mut ok = true;
    let mine = TcpListener::bind("127.0.0.1:0").unwrap();
    let my_port = port_of(&mine);
    let (neighbour, nline) = start("--neighbour");
    let n_port: u16 = nline.split_whitespace().nth(1).unwrap().parse().unwrap();
    let (command, cline) = start("--command");
    let f: Vec<&str> = cline.split_whitespace().collect();
    let (cmd_pid, gc_pid): (u32, u32) = (f[0].parse().unwrap(), f[2].parse().unwrap());
    let want_ports = [
        format!("127.0.0.1:{}", f[1]),
        format!("127.0.0.1:{}", f[3]),
        format!("[::1]:{}", f[4]),
        format!("*:{}", f[5]),
    ];

    // 1. the process table, and the walk from the command
    let mut ps_ms = vec![];
    let mut out = String::new();
    for _ in 0..5 {
        let t = Instant::now();
        let o = Command::new("/bin/ps")
            .args(["-A", "-o", "pid=,ppid="])
            .output()
            .unwrap();
        ps_ms.push(t.elapsed().as_millis());
        out = String::from_utf8_lossy(&o.stdout).into_owned();
    }
    let mut parent: BTreeMap<u32, u32> = BTreeMap::new();
    let mut bad = 0;
    for l in out.lines() {
        let mut it = l.split_whitespace();
        match (
            it.next().and_then(|x| x.parse().ok()),
            it.next().and_then(|x| x.parse().ok()),
            it.next(),
        ) {
            (Some(p), Some(pp), None) => {
                parent.insert(p, pp);
            }
            _ => bad += 1,
        }
    }
    let me = std::process::id();
    // SAFETY: getppid has no preconditions.
    let my_parent = unsafe { libc::getppid() } as u32;
    let mut tree = vec![cmd_pid];
    let mut i = 0;
    while i < tree.len() {
        let p = tree[i];
        for (c, pp) in &parent {
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
        && parent.get(&me) == Some(&my_parent)
        && parent.get(&cmd_pid) == Some(&me)
        && parent.get(&gc_pid) == Some(&cmd_pid)
        && tree == want_tree;
    ok &= pass1;
    println!(
        "1. {}: ps: {} lines, {bad} unparsable; tree from {cmd_pid}: {tree:?}, want {want_tree:?}",
        mark(pass1),
        parent.len()
    );

    // 2. lsof -a -p <tree>
    let list = format!("{cmd_pid},{gc_pid}");
    let args = ["-nP", "-a", "-p", &list, "-iTCP", "-sTCP:LISTEN", "-Fn"];
    let mut lsof_ms = vec![];
    for _ in 0..5 {
        let t = Instant::now();
        out = lsof(&args).1;
        lsof_ms.push(t.elapsed().as_millis());
    }
    let got = names(&out);
    let foreign =
        |n: &String| n.ends_with(&format!(":{my_port}")) || n.ends_with(&format!(":{n_port}"));
    let pass2 =
        got.len() == 4 && want_ports.iter().all(|w| got.contains(w)) && !got.iter().any(foreign);
    ok &= pass2;
    println!(
        "2. {}: -a -p {list}: {got:?}, want {want_ports:?}",
        mark(pass2)
    );

    // 3. without -a
    let (_, out) = lsof(&["-nP", "-p", &list, "-iTCP", "-sTCP:LISTEN", "-Fn"]);
    let all = names(&out);
    let pass3 = all.iter().any(foreign);
    ok &= pass3;
    println!(
        "3. {}: without -a, {} name lines, other listeners {}",
        mark(pass3),
        all.len(),
        if pass3 { "included" } else { "missing" }
    );

    // 4. a gone pid next to a live one
    let (gone_child, _) = start("--neighbour");
    let gone = gone_child.id();
    stop(gone_child);
    let list4 = format!("{gone},{gc_pid}");
    let (code4, out4) = lsof(&["-nP", "-a", "-p", &list4, "-iTCP", "-sTCP:LISTEN", "-Fn"]);
    let got4 = names(&out4);
    let pass4 = got4.len() == 3;
    ok &= pass4;
    println!(
        "4. {}: gone pid {gone} next to {gc_pid}: exit {code4}, {} name lines (want 3)",
        mark(pass4),
        got4.len()
    );

    // 5. pids that listen on nothing (this process, its listener closed)
    drop(mine);
    let me_s = me.to_string();
    let (code5, out5) = lsof(&["-nP", "-a", "-p", &me_s, "-iTCP", "-sTCP:LISTEN", "-Fn"]);
    let pass5 = names(&out5).is_empty();
    ok &= pass5;
    println!(
        "5. {}: no listeners: exit {code5}, {} name lines",
        mark(pass5),
        names(&out5).len()
    );
    println!("ps: {ps_ms:?} ms; lsof -p: {lsof_ms:?} ms");

    stop(command);
    stop(neighbour);
    std::process::exit(if ok { 0 } else { 1 });
}
