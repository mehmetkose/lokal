//! Process-group listener lookup behind `System::tcp_listen_ports` (SPEC
//! §4.10, VERIFY V9): `lokal run`'s wrong-port hint asks which TCP ports its
//! process group listens on. No sudo; kept as documentation.
//!
//! Starts a copy of itself in a new process group that listens on
//! 127.0.0.1:0, [::1]:0 and *:0, while this process (another group) listens
//! on 127.0.0.1:0 too. Then checks that
//! `lsof -nP -a -g <pgid> -iTCP -sTCP:LISTEN -Fn`:
//!   1. prints `n127.0.0.1:<p>`, `n[::1]:<p>` and `n*:<p>` name lines for
//!      exactly the group's three listeners, and not this process's;
//!   2. without `-a` ORs the selections and also lists this process's
//!      listener (so `-a` is required);
//!   3. for a group that listens on nothing, prints no name line (exit 1).
//!
//! It also times one lookup.
//!
//! Usage: verify_lsof_group
//!
//! Measured on macOS 27.0.1 (26A434), 2026-10-24, Apple Silicon (2 runs):
//!   1. pass: exactly the group's 3 ports, as n127.0.0.1:…, n[::1]:… and n*:…
//!      (the *:0 listener was bound on [::])
//!   2. pass: without -a, 77 name lines (every TCP listener lsof can see),
//!      including this process's port
//!   3. pass: exit 1, no name lines
//!
//!   one lookup: 21–22ms (5 per run)

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
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

fn ports(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| l.strip_prefix('n'))
        .map(String::from)
        .collect()
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--child") {
        let ls = [
            TcpListener::bind("127.0.0.1:0").unwrap(),
            TcpListener::bind("[::1]:0").unwrap(),
            TcpListener::bind("[::]:0").unwrap(),
        ];
        let names: Vec<String> = ls
            .iter()
            .map(|l| l.local_addr().unwrap().port().to_string())
            .collect();
        println!("{}", names.join(" "));
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line); // until the parent is done
        return;
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
    let mine = TcpListener::bind("127.0.0.1:0").unwrap();
    let my_port = mine.local_addr().unwrap().port();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--child")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let pgid = child.id().to_string();
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .unwrap();
    let want: Vec<String> = line.split_whitespace().map(String::from).collect();
    let mut ok = true;

    // 1. the group's listeners, and only those
    let args = ["-nP", "-a", "-g", &pgid, "-iTCP", "-sTCP:LISTEN", "-Fn"];
    let mut times = vec![];
    let mut out = String::new();
    for _ in 0..5 {
        let t = Instant::now();
        out = lsof(&args).1;
        times.push(t.elapsed().as_millis());
    }
    let got = ports(&out);
    let shapes = [
        format!("127.0.0.1:{}", want[0]),
        format!("[::1]:{}", want[1]),
        format!("*:{}", want[2]),
    ];
    let pass1 = got.len() == 3
        && shapes.iter().all(|s| got.contains(s))
        && !got.iter().any(|n| n.ends_with(&format!(":{my_port}")));
    ok &= pass1;
    println!(
        "1. {}: group listeners {got:?}, want {shapes:?}",
        if pass1 { "pass" } else { "FAIL" }
    );

    // 2. without -a the selections are ORed
    let (_, out) = lsof(&["-nP", "-g", &pgid, "-iTCP", "-sTCP:LISTEN", "-Fn"]);
    let all = ports(&out);
    let pass2 = all.iter().any(|n| n.ends_with(&format!(":{my_port}")));
    ok &= pass2;
    println!(
        "2. {}: without -a, {} name lines; this process's :{my_port} {}",
        if pass2 { "pass" } else { "FAIL" },
        all.len(),
        if pass2 { "included" } else { "missing" }
    );

    // 3. a group that listens on nothing (this process's own group: its
    // only listener is closed first)
    drop(mine);
    // SAFETY: getpgrp has no preconditions.
    let own = unsafe { libc::getpgrp() }.to_string();
    let (code, out) = lsof(&["-nP", "-a", "-g", &own, "-iTCP", "-sTCP:LISTEN", "-Fn"]);
    let pass3 = ports(&out).is_empty();
    ok &= pass3;
    println!(
        "3. {}: exit {code}, {} name lines",
        if pass3 {
            "pass"
        } else {
            "FAIL (another process in this group listens?)"
        },
        ports(&out).len()
    );
    println!("one lookup: {times:?} ms");

    drop(child.stdin.take());
    let _ = child.wait();
    std::process::exit(if ok { 0 } else { 1 });
}
