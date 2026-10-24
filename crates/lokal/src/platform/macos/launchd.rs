//! The LaunchAgent (SPEC §8.1): launchd owns the 443/80 listening sockets
//! (VERIFY V1) and hands them to the daemon via `launch_activate_socket`.
//! Everything runs in the user's `gui/<uid>` domain; no root.

use std::ffi::{CString, c_char, c_int};
use std::net::{TcpListener, UdpSocket};
use std::os::fd::FromRawFd;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::paths::LABEL;
use crate::platform::{InstallCtx, InstallOutcome, Listeners, ServiceManager};
use crate::process;
use crate::util;

const LAUNCHCTL: &str = "/bin/launchctl";

unsafe extern "C" {
    /// libSystem (launch.h): `int launch_activate_socket(const char *name, int **fds, size_t *cnt);`
    fn launch_activate_socket(name: *const c_char, fds: *mut *mut c_int, cnt: *mut usize) -> c_int;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Https,
    Http,
    /// UDP, for the built-in `.lokal` DNS server.
    Dns,
}

/// Socket names in the plist: (name, node, port, family, kind).
const SOCKETS: [(&str, &str, u16, &str, Kind); 5] = [
    ("https4", "127.0.0.1", 443, "IPv4", Kind::Https),
    ("https6", "::1", 443, "IPv6", Kind::Https),
    ("http4", "127.0.0.1", 80, "IPv4", Kind::Http),
    ("http6", "::1", 80, "IPv6", Kind::Http),
    (
        "dns4",
        "127.0.0.1",
        crate::dns::DEFAULT_PORT,
        "IPv4",
        Kind::Dns,
    ),
];

pub struct Launchd;

fn domain() -> String {
    format!("gui/{}", util::current_uid())
}

fn target() -> String {
    format!("{}/{LABEL}", domain())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn launchctl(args: &[&str]) -> Result<process::CmdOutput> {
    process::run(LAUNCHCTL, args)
}

fn bootstrap(plist: &str) -> Result<()> {
    // Right after a bootout launchd may still be tearing the job down and
    // answers "Bootstrap failed: 5: Input/output error"; that clears quickly.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let out = launchctl(&["bootstrap", &domain(), plist])?;
        if out.success() {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!(
                "`launchctl bootstrap {} {plist}` failed ({}): {}{}",
                domain(),
                out.status,
                out.stdout.trim(),
                out.stderr.trim()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn bootout() -> Result<()> {
    let out = launchctl(&["bootout", &target()])?;
    // 3/113: no such process / could not find service: already gone.
    if !out.success() && is_loaded() {
        bail!(
            "`launchctl bootout {}` failed ({}): {}{}",
            target(),
            out.status,
            out.stdout.trim(),
            out.stderr.trim()
        );
    }
    // bootout returns once the job is removed; make sure before re-bootstrapping.
    let deadline = Instant::now() + Duration::from_secs(3);
    while is_loaded() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn is_loaded() -> bool {
    launchctl(&["print", &target()])
        .map(|o| o.success())
        .unwrap_or(false)
}

fn kickstart(kill: bool) -> Result<()> {
    let t = target();
    let args: Vec<&str> = if kill {
        vec!["kickstart", "-k", &t]
    } else {
        vec!["kickstart", &t]
    };
    process::run_ok(LAUNCHCTL, &args)?;
    Ok(())
}

impl ServiceManager for Launchd {
    fn render_definition(&self, ctx: &InstallCtx) -> String {
        let mut sockets = String::new();
        for (name, node, port, family, kind) in SOCKETS {
            let socktype = if kind == Kind::Dns {
                "\t\t\t<key>SockType</key>\n\t\t\t<string>dgram</string>\n"
            } else {
                ""
            };
            sockets.push_str(&format!(
                "\t\t<key>{name}</key>\n\t\t<dict>\n\t\t\t<key>SockNodeName</key>\n\t\t\t<string>{node}</string>\n\t\t\t<key>SockServiceName</key>\n\t\t\t<string>{port}</string>\n\t\t\t<key>SockFamily</key>\n\t\t\t<string>{family}</string>\n{socktype}\t\t</dict>\n"
            ));
        }
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{bin}</string>
		<string>daemon</string>
		<string>run</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<!-- names are published only while the daemon runs, so it must always run -->
	<key>KeepAlive</key>
	<true/>
	<!-- respawn a crashed daemon right away (default throttle is 10s; VERIFY V1) -->
	<key>ThrottleInterval</key>
	<integer>1</integer>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>EnvironmentVariables</key>
	<dict>
		<key>RUST_LOG</key>
		<string>info</string>
	</dict>
	<key>StandardOutPath</key>
	<string>{out}</string>
	<key>StandardErrorPath</key>
	<string>{err}</string>
	<key>Sockets</key>
	<dict>
{sockets}	</dict>
</dict>
</plist>
"#,
            label = xml_escape(&ctx.label),
            bin = xml_escape(&ctx.binary.display().to_string()),
            out = xml_escape(&ctx.stdout_log.display().to_string()),
            err = xml_escape(&ctx.stderr_log.display().to_string()),
        )
    }

    fn install(&self, ctx: &InstallCtx) -> Result<InstallOutcome> {
        util::create_private_dir(&ctx.log_dir)?;
        let want = self.render_definition(ctx);
        let have = std::fs::read_to_string(&ctx.definition).ok();
        let changed = have.as_deref() != Some(want.as_str());
        if changed {
            util::write_atomic(&ctx.definition, want.as_bytes(), 0o644)?;
        }
        let plist = ctx.definition.display().to_string();
        if !is_loaded() {
            bootstrap(&plist)?;
            Ok(InstallOutcome::Started)
        } else if changed {
            bootout()?;
            bootstrap(&plist)?;
            Ok(InstallOutcome::Reloaded)
        } else if ctx.binary_changed {
            kickstart(true)?;
            Ok(InstallOutcome::Restarted)
        } else {
            Ok(InstallOutcome::Unchanged)
        }
    }

    fn uninstall(&self, ctx: &InstallCtx) -> Result<()> {
        if is_loaded() {
            bootout()?;
        }
        match std::fs::remove_file(&ctx.definition) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn start(&self, ctx: &InstallCtx) -> Result<()> {
        if is_loaded() {
            kickstart(false)
        } else {
            bootstrap(&ctx.definition.display().to_string())
        }
    }

    fn stop(&self) -> Result<()> {
        if is_loaded() { bootout() } else { Ok(()) }
    }

    fn restart(&self) -> Result<()> {
        kickstart(true)
    }

    fn is_installed(&self, ctx: &InstallCtx) -> bool {
        ctx.definition.is_file()
    }

    fn is_loaded(&self) -> bool {
        is_loaded()
    }

    fn inherited_listeners(&self) -> Result<Option<Listeners>> {
        let mut l = Listeners::default();
        for (name, _, _, _, kind) in SOCKETS {
            let cname = CString::new(name)?;
            let mut fds: *mut c_int = std::ptr::null_mut();
            let mut cnt: usize = 0;
            // SAFETY: valid C string and out pointers; launchd allocates `fds`
            // with malloc and we free it below.
            let rc = unsafe { launch_activate_socket(cname.as_ptr(), &mut fds, &mut cnt) };
            match rc {
                0 => {}
                // ESRCH: this process was not started by launchd.
                libc::ESRCH => return Ok(None),
                // ENOENT: the plist has no socket with this name.
                libc::ENOENT => {
                    tracing::warn!("launchd has no socket named {name}");
                    continue;
                }
                e => bail!(
                    "launch_activate_socket({name}) failed: {}",
                    std::io::Error::from_raw_os_error(e)
                ),
            }
            // SAFETY: launchd returned `cnt` valid fds in `fds`.
            let list = unsafe { std::slice::from_raw_parts(fds, cnt) }.to_vec();
            // SAFETY: fds was malloc'ed by launch_activate_socket.
            unsafe { libc::free(fds as *mut libc::c_void) };
            for fd in list {
                match kind {
                    // SAFETY: launchd hands us ownership of a listening/bound socket fd.
                    Kind::Https => l.https.push(unsafe { TcpListener::from_raw_fd(fd) }),
                    Kind::Http => l.http.push(unsafe { TcpListener::from_raw_fd(fd) }),
                    Kind::Dns => l.dns.push(unsafe { UdpSocket::from_raw_fd(fd) }),
                }
            }
        }
        Ok(Some(l))
    }
}
