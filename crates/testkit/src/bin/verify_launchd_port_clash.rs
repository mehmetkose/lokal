//! VERIFY V14 (SPEC §12): what launchd does when an address in a
//! LaunchAgent's `Sockets` is already taken by another program, as when a web
//! server holds 127.0.0.1:443 before lokal's agent loads (SPEC §8.1, "A port
//! another program holds"). No sudo: each item loads a throwaway agent,
//! `dev.lokal.verify.portclash-<hex>`, on high ports, and boots it out and
//! deletes it afterwards. Kept as documentation.
//!
//! The agent has the shape of lokal's plist on high ports: `v4`
//! (127.0.0.1:P), `v6` (::1:P) and `u4` (UDP 127.0.0.1:Q), with `KeepAlive`
//! and `ThrottleInterval` 1. Its program is this binary in `probe` mode,
//! which calls `launch_activate_socket` for each name, writes what it got to a
//! file (the return code, the number of fds, and each fd's address from
//! getsockname), then stays running, as lokal's daemon does.
//!
//!   1. nothing else on P or Q: bootstrap exits 0, and every name hands over
//!      one socket, bound to its address;
//!   2. another process listens on 127.0.0.1:P first (with SO_REUSEADDR, as
//!      servers set it): bootstrap still exits 0 and prints nothing; `v4` still
//!      hands over one fd, but it isn't bound (getsockname 0.0.0.0:0); `v6` is
//!      bound; a connect to 127.0.0.1:P reaches the other process;
//!   3. the same with [::1]:P: `v6` isn't bound, `v4` is;
//!   4. the same with UDP 127.0.0.1:Q: `u4` isn't bound;
//!   5. after item 2's other process stops, `launchctl kickstart -k` (what
//!      `lokal restart` ran) hands over the same unbound socket, and `bootout`
//!      then `bootstrap` binds it;
//!   6. a connect to 127.0.0.1:P reaches the other process while it holds
//!      the port, and is refused once it's gone, though launchd still holds
//!      its unbound socket for `v4`. So a connect tells "taken" from "free
//!      now" (item 10: a bind can't, on 443 and 80);
//!   7. another process listens on 0.0.0.0:P first, with and without
//!      SO_REUSEADDR: every name is bound, and a connect to 127.0.0.1:P and to
//!      [::1]:P reaches launchd's socket, not the other process (the more
//!      specific address wins);
//!   8. the agent first: another process's bind of 0.0.0.0:P, and of
//!      127.0.0.1:P, with SO_REUSEADDR, fails with EADDRINUSE;
//!   9. lsof as the user: lists item 2's other process on P, and nothing for
//!      launchd's own sockets (launchd runs as root) in item 8;
//!  10. the user's bind of a free port below 1024 (SO_REUSEADDR, no listen)
//!      fails with EACCES on 127.0.0.1 and ::1 (0.0.0.0 recorded).
//!
//! Usage: verify_launchd_port_clash          (runs every item)
//!        verify_launchd_port_clash probe F  (run by launchd: write F)
//!
//! Measured on macOS 27.0.1 (26A434), 2026-10-31, Apple Silicon, 3 runs, all
//! passed:
//!   1. pass: v4 127.0.0.1:P, v6 [::1]:P, u4 127.0.0.1:Q
//!   2. pass: bootstrap exit 0, no output; v4 `rc=0 n=1 0.0.0.0:0`, v6
//!      [::1]:P; 127.0.0.1:P answers with the other process's banner
//!   3. pass: v6 `rc=0 n=1 [::]:0`, v4 127.0.0.1:P
//!   4. pass: u4 `rc=0 n=1 0.0.0.0:0`
//!   5. pass: after kickstart -k, v4 0.0.0.0:0; after bootout and bootstrap,
//!      v4 127.0.0.1:P
//!   6. pass: the other process answers while it holds the port; then
//!      `Connection refused` (61)
//!   7. pass: all bound, and both connects reach launchd's socket, for both
//!      variants
//!   8. pass: EADDRINUSE (48) for 0.0.0.0:P and for 127.0.0.1:P
//!   9. pass: lsof lists the other process's 127.0.0.1:P listener, and nothing
//!      on P while only launchd holds it
//!  10. pass: port 444: EACCES (13) on 127.0.0.1 and ::1; 0.0.0.0 binds
//!
//! An earlier version whose probe exited at once, with no KeepAlive, found
//! that `kickstart -k` of such an agent doesn't return for minutes, port
//! taken or not; with a program that stays running and KeepAlive, as lokal's,
//! it returns in about 1s (measured separately, 2026-10-31).

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{CString, c_char, c_int};
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
    use std::os::fd::FromRawFd;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    unsafe extern "C" {
        fn launch_activate_socket(
            name: *const c_char,
            fds: *mut *mut c_int,
            cnt: *mut usize,
        ) -> c_int;
    }

    const NAMES: [&str; 3] = ["v4", "v6", "u4"];

    /// `probe` mode: one line per socket name, `v4 rc=0 n=1 127.0.0.1:P`.
    fn probe(out: &Path) {
        let mut lines = vec![];
        for name in NAMES {
            let c = CString::new(name).unwrap();
            let mut fds: *mut c_int = std::ptr::null_mut();
            let mut cnt: usize = 0;
            // SAFETY: valid C string and out pointers.
            let rc = unsafe { launch_activate_socket(c.as_ptr(), &mut fds, &mut cnt) };
            let mut addrs = vec![];
            if rc == 0 {
                // SAFETY: launchd returned `cnt` fds in `fds`, malloc'ed.
                let list = unsafe { std::slice::from_raw_parts(fds, cnt) }.to_vec();
                unsafe { libc::free(fds as *mut libc::c_void) };
                for fd in list {
                    // SAFETY: launchd hands this process the fd.
                    let s = unsafe { socket2::Socket::from_raw_fd(fd) };
                    addrs.push(match s.local_addr().map(|a| a.as_socket()) {
                        Ok(Some(a)) => a.to_string(),
                        Ok(None) => "not inet".into(),
                        Err(e) => format!("error {e}"),
                    });
                }
            }
            lines.push(format!("{name} rc={rc} n={cnt} {}", addrs.join(",")));
        }
        let tmp = out.with_extension("tmp");
        std::fs::write(&tmp, lines.join("\n") + "\n").unwrap();
        std::fs::rename(tmp, out).unwrap();
        // Stay running, as lokal's daemon does: `kickstart -k` of an agent
        // whose program has exited at once waits a long time (over 20s).
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    fn uid() -> u32 {
        // SAFETY: getuid has no preconditions.
        unsafe { libc::getuid() }
    }

    fn launchctl(args: &[&str]) -> (i32, String) {
        let o = Command::new("/bin/launchctl").args(args).output().unwrap();
        (
            o.status.code().unwrap_or(-1),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
            .trim()
            .to_string(),
        )
    }

    fn rand_hex() -> String {
        let mut b = [0u8; 3];
        // SAFETY: arc4random_buf fills the buffer.
        unsafe { libc::arc4random_buf(b.as_mut_ptr().cast(), b.len()) };
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// A TCP port free on 127.0.0.1 and ::1, and a UDP port free on 127.0.0.1.
    fn ports() -> (u16, u16) {
        loop {
            let p = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            if TcpListener::bind((Ipv6Addr::LOCALHOST, p)).is_ok() {
                let q = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                    .unwrap()
                    .local_addr()
                    .unwrap()
                    .port();
                return (p, q);
            }
        }
    }

    struct Agent {
        label: String,
        plist: PathBuf,
        out: PathBuf,
        bootstrap: (i32, String),
    }

    impl Agent {
        fn load(dir: &Path, p: u16, q: u16) -> Agent {
            let label = format!("dev.lokal.verify.portclash-{}", rand_hex());
            let plist = dir.join(format!("{label}.plist"));
            let out = dir.join(format!("{label}.out"));
            let exe = std::env::current_exe().unwrap();
            let sock = |name: &str, node: &str, port: u16, fam: &str, dgram: bool| {
                let t = if dgram {
                    "<key>SockType</key><string>dgram</string>"
                } else {
                    ""
                };
                format!(
                    "<key>{name}</key><dict><key>SockNodeName</key><string>{node}</string>\
                     <key>SockServiceName</key><string>{port}</string>\
                     <key>SockFamily</key><string>{fam}</string>{t}</dict>"
                )
            };
            let socks = [
                sock("v4", "127.0.0.1", p, "IPv4", false),
                sock("v6", "::1", p, "IPv6", false),
                sock("u4", "127.0.0.1", q, "IPv4", true),
            ]
            .concat();
            std::fs::write(
                &plist,
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                     <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                     <plist version=\"1.0\"><dict><key>Label</key><string>{label}</string>\
                     <key>ProgramArguments</key><array><string>{}</string><string>probe</string><string>{}</string></array>\
                     <key>RunAtLoad</key><true/><key>KeepAlive</key><true/>\
                     <key>ThrottleInterval</key><integer>1</integer><key>Sockets</key><dict>{socks}</dict></dict></plist>\n",
                    exe.display(),
                    out.display()
                ),
            )
            .unwrap();
            let bootstrap = launchctl(&[
                "bootstrap",
                &format!("gui/{}", uid()),
                plist.to_str().unwrap(),
            ]);
            Agent {
                label,
                plist,
                out,
                bootstrap,
            }
        }

        fn target(&self) -> String {
            format!("gui/{}/{}", uid(), self.label)
        }

        /// The probe's lines, by socket name, once it has written them.
        fn probed(&self) -> Vec<String> {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Ok(s) = std::fs::read_to_string(&self.out) {
                    let _ = std::fs::remove_file(&self.out);
                    return s.lines().map(str::to_string).collect();
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            vec!["(the probe wrote nothing)".into()]
        }

        fn kickstart(&self) {
            launchctl(&["kickstart", "-k", &self.target()]);
        }

        fn unload(&self) {
            launchctl(&["bootout", &self.target()]);
            let deadline = Instant::now() + Duration::from_secs(3);
            while launchctl(&["print", &self.target()]).0 == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn reload(&self) {
            self.unload();
            launchctl(&[
                "bootstrap",
                &format!("gui/{}", uid()),
                self.plist.to_str().unwrap(),
            ]);
        }
    }

    impl Drop for Agent {
        fn drop(&mut self) {
            self.unload();
            let _ = std::fs::remove_file(&self.plist);
            let _ = std::fs::remove_file(&self.out);
        }
    }

    /// The probe line for `name`, without the name.
    fn line<'a>(lines: &'a [String], name: &str) -> &'a str {
        lines
            .iter()
            .find_map(|l| l.strip_prefix(&format!("{name} ")))
            .unwrap_or("(missing)")
    }

    /// Another program's listener, which greets every connection with
    /// `OTHER`. Dropping it closes the socket.
    struct Other {
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Other {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn other(addr: SocketAddr, reuse: bool) -> std::io::Result<Other> {
        let s = socket2::Socket::new(
            socket2::Domain::for_address(addr),
            socket2::Type::STREAM,
            None,
        )?;
        s.set_reuse_address(reuse)?;
        s.bind(&addr.into())?;
        s.listen(16)?;
        let l: TcpListener = s.into();
        l.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match l.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.write_all(b"OTHER\n");
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        Ok(Other {
            stop,
            thread: Some(thread),
        })
    }

    /// Who answers a connect: `other`, `launchd` (accepted, no banner) or the error.
    fn who(addr: SocketAddr) -> String {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(300)) {
            Err(e) => format!("connect failed: {e}"),
            Ok(mut s) => {
                s.set_read_timeout(Some(Duration::from_millis(300)))
                    .unwrap();
                let mut b = [0u8; 6];
                match s.read(&mut b) {
                    Ok(n) if b[..n].starts_with(b"OTHER") => "other".into(),
                    _ => "launchd".into(),
                }
            }
        }
    }

    /// lokal's bind check: SO_REUSEADDR, bind, no listen.
    fn bind_check(addr: SocketAddr) -> String {
        let s = socket2::Socket::new(
            socket2::Domain::for_address(addr),
            socket2::Type::STREAM,
            None,
        )
        .unwrap();
        s.set_reuse_address(true).unwrap();
        match s.bind(&addr.into()) {
            Ok(()) => "binds".into(),
            Err(e) => format!("{} ({})", e.kind(), e.raw_os_error().unwrap_or(0)),
        }
    }

    fn lsof_listeners(port: u16) -> String {
        let o = Command::new("/usr/sbin/lsof")
            .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-F", "pcn"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout)
            .replace('\n', " ")
            .trim()
            .to_string()
    }

    fn report(ok: &mut bool, item: &str, pass: bool, detail: String) {
        *ok &= pass;
        println!(
            "{:>3}. {}: {detail}",
            item,
            if pass { "pass" } else { "FAIL" }
        );
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(String::as_str) == Some("probe") {
            probe(Path::new(&args[2]));
            return;
        }
        let dir = std::env::temp_dir().join(format!("verify-portclash-{}", rand_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let v4 = |p| SocketAddr::from((Ipv4Addr::LOCALHOST, p));
        let v6 = |p| SocketAddr::from((Ipv6Addr::LOCALHOST, p));
        let any4 = |p| SocketAddr::from((Ipv4Addr::UNSPECIFIED, p));
        let mut ok = true;

        // 1. nothing else
        let (p, q) = ports();
        {
            let a = Agent::load(&dir, p, q);
            let l = a.probed();
            let pass = a.bootstrap.0 == 0
                && line(&l, "v4") == format!("rc=0 n=1 127.0.0.1:{p}")
                && line(&l, "v6") == format!("rc=0 n=1 [::1]:{p}")
                && line(&l, "u4") == format!("rc=0 n=1 127.0.0.1:{q}");
            report(
                &mut ok,
                "1",
                pass,
                format!("bootstrap {:?}; {}", a.bootstrap, l.join("; ")),
            );
        }

        // 2, 5, 6, 9: 127.0.0.1:P held first
        let (p, q) = ports();
        {
            let held = other(v4(p), true).unwrap();
            let a = Agent::load(&dir, p, q);
            let l = a.probed();
            let answer = who(v4(p));
            let pass = a.bootstrap == (0, String::new())
                && line(&l, "v4") == "rc=0 n=1 0.0.0.0:0"
                && line(&l, "v6") == format!("rc=0 n=1 [::1]:{p}")
                && answer == "other";
            report(
                &mut ok,
                "2",
                pass,
                format!(
                    "bootstrap {:?}; {}; 127.0.0.1:{p} → {answer}",
                    a.bootstrap,
                    l.join("; ")
                ),
            );
            let lsof = lsof_listeners(p);
            let held_check = who(v4(p));
            report(
                &mut ok,
                "9a",
                lsof.contains(&format!("n127.0.0.1:{p}")),
                format!("lsof: {lsof}"),
            );
            drop(held);
            let free_check = who(v4(p));
            report(
                &mut ok,
                "6",
                held_check == "other" && free_check.contains("refused"),
                format!("while held: {held_check}; after: {free_check}"),
            );
            a.kickstart();
            let after_kick = a.probed();
            a.reload();
            let after_reload = a.probed();
            report(
                &mut ok,
                "5",
                line(&after_kick, "v4") == "rc=0 n=1 0.0.0.0:0"
                    && line(&after_reload, "v4") == format!("rc=0 n=1 127.0.0.1:{p}"),
                format!(
                    "after kickstart -k: v4 {}; after bootout+bootstrap: v4 {}",
                    line(&after_kick, "v4"),
                    line(&after_reload, "v4")
                ),
            );
        }

        // 3. [::1]:P held first
        let (p, q) = ports();
        {
            let _held = other(v6(p), true).unwrap();
            let a = Agent::load(&dir, p, q);
            let l = a.probed();
            let pass = a.bootstrap.0 == 0
                && line(&l, "v6") == "rc=0 n=1 [::]:0"
                && line(&l, "v4") == format!("rc=0 n=1 127.0.0.1:{p}");
            report(&mut ok, "3", pass, l.join("; "));
        }

        // 4. UDP 127.0.0.1:Q held first
        let (p, q) = ports();
        {
            let _held = UdpSocket::bind(v4(q)).unwrap();
            let a = Agent::load(&dir, p, q);
            let l = a.probed();
            report(
                &mut ok,
                "4",
                a.bootstrap.0 == 0 && line(&l, "u4") == "rc=0 n=1 0.0.0.0:0",
                l.join("; "),
            );
        }

        // 7. 0.0.0.0:P held first, with and without SO_REUSEADDR
        for reuse in [true, false] {
            let (p, q) = ports();
            let _held = other(any4(p), reuse).unwrap();
            let a = Agent::load(&dir, p, q);
            let l = a.probed();
            let (w4, w6) = (who(v4(p)), who(v6(p)));
            let pass = line(&l, "v4") == format!("rc=0 n=1 127.0.0.1:{p}")
                && line(&l, "v6") == format!("rc=0 n=1 [::1]:{p}")
                && w4 == "launchd"
                && w6 == "launchd";
            report(
                &mut ok,
                "7",
                pass,
                format!(
                    "SO_REUSEADDR={reuse}: {}; 127.0.0.1 → {w4}, ::1 → {w6}",
                    l.join("; ")
                ),
            );
        }

        // 8, 9b: the agent first
        let (p, q) = ports();
        {
            let a = Agent::load(&dir, p, q);
            let _ = a.probed();
            let e_any = other(any4(p), true)
                .err()
                .map(|e| e.raw_os_error().unwrap_or(0));
            let e_lo = other(v4(p), true)
                .err()
                .map(|e| e.raw_os_error().unwrap_or(0));
            report(
                &mut ok,
                "8",
                e_any == Some(libc::EADDRINUSE) && e_lo == Some(libc::EADDRINUSE),
                format!("0.0.0.0:{p} → {e_any:?}; 127.0.0.1:{p} → {e_lo:?}"),
            );
            let lsof = lsof_listeners(p);
            report(&mut ok, "9b", lsof.is_empty(), format!("lsof: {lsof:?}"));
        }

        // 10. a free port below 1024
        let low = (444..1024)
            .find(|&p| TcpStream::connect_timeout(&v4(p), Duration::from_millis(50)).is_err());
        match low {
            Some(p) => {
                let (r4, r6, ra) = (bind_check(v4(p)), bind_check(v6(p)), bind_check(any4(p)));
                report(
                    &mut ok,
                    "10",
                    r4.contains("(13)") && r6.contains("(13)"),
                    format!("{p}: 127.0.0.1 {r4}, ::1 {r6}, 0.0.0.0 {ra}"),
                );
            }
            None => report(&mut ok, "10", false, "no free port below 1024".into()),
        }

        let _ = std::fs::remove_dir_all(&dir);
        let (_, list) = launchctl(&["list"]);
        let left: Vec<&str> = list
            .lines()
            .filter(|l| l.contains("dev.lokal.verify.portclash-"))
            .collect();
        report(
            &mut ok,
            "-",
            left.is_empty(),
            format!("agents left: {left:?}"),
        );
        std::process::exit(if ok { 0 } else { 1 });
    }
}

#[cfg(target_os = "macos")]
fn main() {
    macos::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("verify_launchd_port_clash: VERIFY V14 needs macOS");
    std::process::exit(2);
}
