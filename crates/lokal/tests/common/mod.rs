//! Shared sandbox daemon for integration tests.
#![allow(dead_code)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
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
