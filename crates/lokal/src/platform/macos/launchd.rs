//! The LaunchAgent (SPEC §8.1): launchd owns the 443/80 listening sockets
//! (VERIFY V1) and hands them to the daemon via `launch_activate_socket`.
//! Everything runs in the user's `gui/<uid>` domain; no root.

use std::ffi::{CString, c_char, c_int};
use std::net::{IpAddr, SocketAddr};
use std::os::fd::FromRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::paths::LABEL;
use crate::platform::{InstallCtx, InstallOutcome, Listeners, PrivilegedCommand, ServiceManager};
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

/// A socket in the plist's `Sockets` dictionary.
struct PlistSocket {
    /// Its key, which the daemon passes to `launch_activate_socket`.
    name: &'static str,
    /// `SockNodeName`: the IP address launchd binds.
    node: &'static str,
    port: u16,
    /// `SockFamily`.
    family: &'static str,
    kind: Kind,
}

const SOCKETS: [PlistSocket; 5] = [
    PlistSocket {
        name: "https4",
        node: "127.0.0.1",
        port: 443,
        family: "IPv4",
        kind: Kind::Https,
    },
    PlistSocket {
        name: "https6",
        node: "::1",
        port: 443,
        family: "IPv6",
        kind: Kind::Https,
    },
    PlistSocket {
        name: "http4",
        node: "127.0.0.1",
        port: 80,
        family: "IPv4",
        kind: Kind::Http,
    },
    PlistSocket {
        name: "http6",
        node: "::1",
        port: 80,
        family: "IPv6",
        kind: Kind::Http,
    },
    PlistSocket {
        name: "dns4",
        node: "127.0.0.1",
        port: crate::dns::DEFAULT_PORT,
        family: "IPv4",
        kind: Kind::Dns,
    },
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
    fn name(&self) -> &'static str {
        "launchd"
    }

    fn definition_current(&self, ctx: &InstallCtx) -> bool {
        std::fs::read_to_string(&ctx.definition).ok().as_deref()
            == Some(self.render_definition(ctx).as_str())
    }

    // launchd binds 443 and 80 for the agent (V1): nothing to change as root.
    fn port_config(&self) -> Option<String> {
        None
    }

    fn plan_install(&self, _staged: &Path) -> Vec<PrivilegedCommand> {
        vec![]
    }

    fn plan_uninstall(&self) -> Vec<PrivilegedCommand> {
        vec![]
    }

    fn render_definition(&self, ctx: &InstallCtx) -> String {
        let mut sockets = String::new();
        for PlistSocket {
            name,
            node,
            port,
            family,
            kind,
        } in SOCKETS
        {
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
            kickstart(/* kill */ true)?;
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
            kickstart(/* kill */ false)
        } else {
            bootstrap(&ctx.definition.display().to_string())
        }
    }

    fn stop(&self) -> Result<()> {
        if is_loaded() { bootout() } else { Ok(()) }
    }

    fn restart(&self) -> Result<()> {
        kickstart(/* kill */ true)
    }

    fn is_installed(&self, ctx: &InstallCtx) -> bool {
        ctx.definition.is_file()
    }

    fn is_loaded(&self) -> bool {
        is_loaded()
    }

    fn inherited_listeners(&self) -> Result<Option<Listeners>> {
        let mut l = Listeners::default();
        for PlistSocket {
            name,
            node,
            port,
            kind,
            ..
        } in SOCKETS
        {
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
            let mut got = false;
            for fd in list {
                // SAFETY: launchd hands us ownership of a listening/bound socket fd.
                let sock = unsafe { socket2::Socket::from_raw_fd(fd) };
                // An address another program held when the agent loaded:
                // launchd loads it anyway and hands over a socket it couldn't
                // bind (VERIFY V14). Dropping it closes it.
                if !launchd_bound(sock.local_addr().ok().and_then(|a| a.as_socket())) {
                    continue;
                }
                got = true;
                match kind {
                    Kind::Https => l.https.push(sock.into()),
                    Kind::Http => l.http.push(sock.into()),
                    Kind::Dns => l.dns.push(sock.into()),
                }
            }
            if !got {
                l.missing.push(socket_text(node, port, kind));
            }
        }
        Ok(Some(l))
    }
}

/// Did launchd bind this socket? One it couldn't bind, because another
/// program held the address, has port 0 (`0.0.0.0:0`, `[::]:0`; VERIFY V14).
fn launchd_bound(addr: Option<SocketAddr>) -> bool {
    addr.is_some_and(|a| a.port() != 0)
}

/// A `SOCKETS` entry's address, for `Listeners::missing`.
fn socket_text(node: &str, port: u16, kind: Kind) -> String {
    let ip: IpAddr = node.parse().expect("SOCKETS holds IP addresses");
    crate::platform::listener_text(SocketAddr::new(ip, port), kind == Kind::Dns)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// launchd hands over a socket it couldn't bind as `0.0.0.0:0` or
    /// `[::]:0` (VERIFY V14); every bound one has its port.
    #[test]
    fn a_socket_launchd_couldnt_bind_is_missing() {
        let a = |s: &str| Some(s.parse::<SocketAddr>().unwrap());
        assert!(!launchd_bound(a("0.0.0.0:0")));
        assert!(!launchd_bound(a("[::]:0")));
        assert!(!launchd_bound(None));
        assert!(launchd_bound(a("127.0.0.1:443")));
        assert!(launchd_bound(a("[::1]:80")));
    }

    #[test]
    fn names_each_socket_by_its_address() {
        let texts: Vec<String> = SOCKETS
            .iter()
            .map(|s| socket_text(s.node, s.port, s.kind))
            .collect();
        assert_eq!(
            texts,
            [
                "127.0.0.1:443",
                "[::1]:443",
                "127.0.0.1:80",
                "[::1]:80",
                &format!("127.0.0.1:{}/udp", crate::dns::DEFAULT_PORT),
            ]
        );
    }
}
