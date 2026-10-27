//! What "stopped" means for `lokal up`'s shells (SPEC §4.11 "Stopping",
//! VERIFY V10). No sudo; kept as documentation.
//!
//! `lokal up` starts each `run` as `/bin/sh -c <run>` in a new process group
//! (pgid = the shell's pid) and signals the group, because the real server is
//! often a grandchild (`sh` → `npm` → `node`). This checks, with
//! `/bin/sh -c 'sleep 30 & exec sleep 31'` started with `process_group(0)`:
//!   1. both sleeps have the shell's pid as their pgid (`ps -o pid=,pgid=`);
//!   2. `killpg(pgid, SIGINT)` ends the leader (`exec sleep 31`) but not the
//!      background `sleep 30`: a non-interactive shell starts asynchronous
//!      lists with SIGINT ignored (POSIX), and `exec` keeps an ignored
//!      disposition;
//!   3. `killpg(pgid, 0)` still returns 0 while the leader is an unreaped
//!      zombie, and after it is reaped, while the background job is left;
//!   4. after `killpg(pgid, SIGTERM)` and a short wait, `killpg(pgid, 0)`
//!      fails with `ESRCH` (the orphaned job is reaped by launchd);
//!   5. what `killpg(pgid, 0)` returns while the leader is an unreaped zombie
//!      and nothing else is left in the group (`/bin/sh -c 'exec sleep 32'`).
//!
//! Usage: verify_process_groups
//!
//! Measured on macOS 27.0.1 (26A434), 2026-10-25, Apple Silicon, /bin/sh is
//! bash 3.2.57 (/private/var/select/sh -> /bin/bash), 3 runs, all passed:
//!   1. pass: `sleep 31` (the leader, after exec) and `sleep 30` both have
//!      the shell's pid as their pgid; the background sleep's parent is the
//!      leader
//!   2. pass: killpg(SIGINT) = 0; the leader died of signal 2 (a zombie after
//!      38–41ms); the background sleep was still alive 200ms later
//!   3. pass: killpg(pgid, 0) = 0 with the leader an unreaped zombie, and
//!      again after reaping it; the background sleep, reparented to launchd
//!      (ppid 1), is still in the group
//!   4. pass: after killpg(SIGTERM), killpg(pgid, 0) fails with ESRCH within
//!      5–7ms
//!   5. recorded: with the unreaped zombie leader as the group's only member,
//!      killpg(pgid, 0) fails with EPERM, not ESRCH; ESRCH once it is reaped.
//!      So EPERM must count as alive, and lokal reaps each shell before it
//!      calls the group empty
//!
//! Measured 2026-10-27 on GitHub's `ubuntu-latest` (Linux 6.17.0-1022-azure,
//! x86_64) and `ubuntu-24.04-arm` (Linux 7.0.0-1017-azure, aarch64), both
//! Ubuntu 24.04.5 LTS, /bin/sh -> /usr/bin/dash, 3 runs each, all passed:
//!   1-4. as on macOS: the leader a zombie after 6–10ms, the background sleep
//!      alive 200ms later and re-parented to pid 1, ESRCH 5.1ms after SIGTERM
//!   5. recorded: with the unreaped zombie leader as the group's only member,
//!      killpg(pgid, 0) returns 0 (not EPERM as on macOS); ESRCH once it is
//!      reaped. lokal counts 0 and EPERM as alive, so nothing changes

use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// `(pid, pgid, ppid, stat, command)` of every process in group `pgid`.
fn members(pgid: i32) -> Vec<(i32, i32, i32, String, String)> {
    let o = Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,pgid=,ppid=,stat=,command="])
        .output()
        .expect("run ps");
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let pg = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            let stat = it.next()?.to_string();
            let cmd = it.collect::<Vec<_>>().join(" ");
            Some((pid, pg, ppid, stat, cmd))
        })
        .filter(|m| m.1 == pgid)
        .collect()
}

/// `killpg(pgid, sig)` and the errno when it fails.
fn killpg(pgid: i32, sig: libc::c_int) -> (i32, Option<i32>) {
    // SAFETY: killpg only sends a signal (or 0, a check) to the given group.
    let rc = unsafe { libc::killpg(pgid, sig) };
    let errno = (rc != 0).then(|| std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
    (rc, errno)
}

fn show(r: (i32, Option<i32>)) -> String {
    match r {
        (0, _) => "0".into(),
        (rc, Some(e)) if e == libc::ESRCH => format!("{rc} (ESRCH)"),
        (rc, Some(e)) if e == libc::EPERM => format!("{rc} (EPERM)"),
        (rc, e) => format!("{rc} (errno {e:?})"),
    }
}

/// Polls every 5ms for up to `limit` until `f` is true; returns how long it took.
fn until(limit: Duration, mut f: impl FnMut() -> bool) -> Option<Duration> {
    let t = Instant::now();
    while t.elapsed() < limit {
        if f() {
            return Some(t.elapsed());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

fn is_zombie(pgid: i32, pid: i32) -> bool {
    members(pgid)
        .iter()
        .any(|m| m.0 == pid && m.3.starts_with('Z'))
}

fn start(script: &str) -> Child {
    Command::new("/bin/sh")
        .args(["-c", script])
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("start /bin/sh")
}

/// The OS: `sw_vers` on macOS; elsewhere (Linux) `uname -srm` and
/// `PRETTY_NAME` from os-release.
fn os() -> String {
    if let Ok(o) = Command::new("/usr/bin/sw_vers").output() {
        return String::from_utf8_lossy(&o.stdout).replace('\n', " ");
    }
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
    format!("{uname}; {pretty}")
}

fn main() {
    let sw = os();
    // macOS's /bin/sh runs the shell /private/var/select/sh names; on Linux
    // /bin/sh is a link (dash on Debian and Ubuntu).
    let sh = std::fs::read_link("/private/var/select/sh")
        .or_else(|_| std::fs::canonicalize("/bin/sh"))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into());
    println!("verify_process_groups: {}; /bin/sh -> {sh}", sw.trim());
    let mut failed = 0;
    let mut check = |name: &str, ok: bool, detail: String| {
        println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failed += 1;
        }
    };

    // 1. The group: the leader and its background job.
    let mut child = start("sleep 30 & exec sleep 31");
    let pgid = child.id() as i32;
    let found = until(Duration::from_secs(2), || {
        let m = members(pgid);
        m.len() == 2 && m.iter().any(|x| x.4 == "sleep 31") && m.iter().any(|x| x.4 == "sleep 30")
    });
    let m = members(pgid);
    let bg = m.iter().find(|x| x.4 == "sleep 30").map(|x| x.0);
    check(
        "1 pgid",
        found.is_some() && m.iter().all(|x| x.1 == pgid),
        format!("leader {pgid}; members (pid pgid ppid stat command) {m:?}"),
    );
    let Some(bg) = bg else {
        eprintln!("no background sleep found; stopping");
        let _ = killpg(pgid, libc::SIGKILL);
        let _ = child.wait();
        std::process::exit(1);
    };

    // 2. SIGINT to the group: the leader dies, the background job doesn't.
    let r = killpg(pgid, libc::SIGINT);
    let zombie = until(Duration::from_secs(2), || is_zombie(pgid, pgid));
    // 3a. While the leader is an unreaped zombie (and the job is alive).
    let while_zombie = killpg(pgid, 0);
    std::thread::sleep(Duration::from_millis(200));
    // SAFETY: kill(pid, 0) only checks that the pid exists.
    let bg_alive = unsafe { libc::kill(bg, 0) } == 0;
    let status = child.wait().expect("reap the leader");
    use std::os::unix::process::ExitStatusExt;
    check(
        "2 SIGINT",
        r.0 == 0 && status.signal() == Some(libc::SIGINT) && bg_alive,
        format!(
            "killpg(SIGINT) = {}; leader {status:?} (zombie after {zombie:?}); background sleep {bg} {} 200ms later",
            show(r),
            if bg_alive { "alive" } else { "gone" }
        ),
    );
    // 3b. After the leader is reaped.
    let after_reap = killpg(pgid, 0);
    let m = members(pgid);
    check(
        "3 group outlives its leader",
        while_zombie.0 == 0 && after_reap.0 == 0,
        format!(
            "killpg(pgid, 0) with the leader a zombie = {}, after reaping it = {}; left {m:?}",
            show(while_zombie),
            show(after_reap)
        ),
    );

    // 4. SIGTERM ends the job; the group is then empty.
    let r = killpg(pgid, libc::SIGTERM);
    let gone = until(Duration::from_secs(2), || {
        killpg(pgid, 0).1 == Some(libc::ESRCH)
    });
    check(
        "4 SIGTERM empties the group",
        r.0 == 0 && gone.is_some(),
        format!(
            "killpg(SIGTERM) = {}; killpg(pgid, 0) = ESRCH after {gone:?}",
            show(r)
        ),
    );

    // 5. A group whose only member is the unreaped zombie leader.
    let mut child = start("exec sleep 32");
    let pgid = child.id() as i32;
    until(Duration::from_secs(2), || {
        members(pgid).iter().any(|x| x.4 == "sleep 32")
    });
    let r = killpg(pgid, libc::SIGTERM);
    let zombie = until(Duration::from_secs(2), || is_zombie(pgid, pgid));
    let only_zombie = killpg(pgid, 0);
    let m = members(pgid);
    let status = child.wait().expect("reap");
    let reaped = killpg(pgid, 0);
    check(
        "5 zombie leader only (recorded)",
        r.0 == 0 && zombie.is_some() && reaped.1 == Some(libc::ESRCH),
        format!(
            "killpg(SIGTERM) = {}; zombie after {zombie:?} ({m:?}); killpg(pgid, 0) then = {}; after reaping ({status:?}) = {}",
            show(r),
            show(only_zombie),
            show(reaped)
        ),
    );

    std::process::exit(if failed > 0 { 1 } else { 0 });
}
