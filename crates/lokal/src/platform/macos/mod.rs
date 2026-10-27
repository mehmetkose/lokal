//! macOS: keychain trust (`security`), LaunchAgent with launchd socket
//! activation, mDNSResponder LocalOnly records, Firefox profiles, `open`.

mod firefox;
mod launchd;
mod mdns;
mod resolver;
mod trust;

use std::path::PathBuf;

use anyhow::{Result, bail};

use super::*;
use crate::process;

pub fn platform() -> Platform {
    Platform {
        trust: Box::new(trust::Keychain),
        resolver: Box::new(resolver::EtcResolver),
        service: Box::new(launchd::Launchd),
        desktop: Box::new(Mac),
        system: Box::new(Mac),
        default_name_backend: mdns::BACKEND,
    }
}

pub fn name_publisher(backend: &str) -> Result<Box<dyn NamePublisher>> {
    match backend {
        mdns::BACKEND | "auto" => Ok(Box::new(mdns::MdnsPublisher::start()?)),
        other => bail!("unknown name backend `{other}` (use mdns-local-only or none)"),
    }
}

struct Mac;

impl Desktop for Mac {
    fn open_url(&self, url: &str) -> Result<()> {
        process::run_ok("/usr/bin/open", &[url])?;
        Ok(())
    }
}

impl System for Mac {
    fn is_root(&self) -> bool {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }

    fn user_and_host(&self) -> (String, String) {
        let user = std::env::var("USER")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "user".into());
        let host = process::run("/usr/sbin/scutil", &["--get", "LocalHostName"])
            .ok()
            .filter(|o| o.success())
            .map(|o| o.stdout.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(short_hostname);
        (user, host)
    }

    fn run_privileged(&self, cmd: &PrivilegedCommand, prompt: &str) -> Result<()> {
        let mut args: Vec<&str> = vec!["-p", prompt, cmd.program.as_str()];
        args.extend(cmd.args.iter().map(String::as_str));
        let status = process::run_interactive("/usr/bin/sudo", &args)?;
        if !status.success() {
            bail!("`{}` failed ({status})", cmd.display());
        }
        Ok(())
    }

    fn port_owners(&self, port: u16) -> Vec<PortOwner> {
        // One record per process: pid, pgid, full command name, then each
        // socket's name and state. lsof also lists connections from or to
        // the port, which `owners_of` drops, and exits 1 with no output
        // when nothing is there. VERIFY V11 (SPEC §12), measured on macOS
        // 27.0.1 (26A434).
        let Ok(out) = process::run(
            "/usr/sbin/lsof",
            &["-nP", &format!("-iTCP:{port}"), "-F", "pgcnT"],
        ) else {
            return vec![];
        };
        owners_of(&out.stdout, port)
    }

    fn tcp_listen_ports(&self, pid: u32) -> Vec<u16> {
        // One read of the process table, then lsof about exactly the
        // command's tree. Not its process group: without job control that
        // also holds processes the command never started. VERIFY V9 (SPEC
        // §12), measured on macOS 27.0.1 (26A434).
        let Ok(ps) = process::run("/bin/ps", &["-A", "-o", "pid=,ppid="]) else {
            return vec![];
        };
        let pids = process_tree(&ps_pairs(&ps.stdout), pid);
        if pids.is_empty() {
            return vec![];
        }
        let list = pids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        // `-a` ANDs the selections; without it lsof ORs them and lists every
        // TCP listener. lsof exits 1 when a listed pid has exited meanwhile,
        // but still prints the others, so only the output counts.
        let Ok(out) = process::run(
            "/usr/sbin/lsof",
            &["-nP", "-a", "-p", &list, "-iTCP", "-sTCP:LISTEN", "-Fn"],
        ) else {
            return vec![];
        };
        listen_ports(&out.stdout)
    }

    fn hosts_file(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/etc/hosts"))
    }

    fn os_version(&self) -> String {
        let v = process::run("/usr/bin/sw_vers", &["-productVersion"])
            .map(|o| o.stdout.trim().to_string())
            .unwrap_or_default();
        format!("macOS {v}")
    }
}

/// `(pid, ppid)` pairs from `ps -A -o pid=,ppid=`. A line that isn't
/// exactly two numbers is skipped.
fn ps_pairs(out: &str) -> Vec<(u32, u32)> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pair = (it.next()?.parse().ok()?, it.next()?.parse().ok()?);
            it.next().is_none().then_some(pair)
        })
        .collect()
}

/// Ports from `lsof -F n` name lines (`n127.0.0.1:5173`, `n[::1]:5173`,
/// `n*:5173`), sorted and without duplicates. Other lines (`p<pid>`,
/// `f<fd>`) are skipped.
fn listen_ports(lsof_fn: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = lsof_fn
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .filter_map(local_port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// The local port of an lsof TCP name: `127.0.0.1:5173`, `[::1]:5173`,
/// `*:5173`, or the part before `->` of a connection
/// (`127.0.0.1:5173->127.0.0.1:60123`).
fn local_port(name: &str) -> Option<u16> {
    let local = name.split("->").next()?;
    local.rsplit_once(':')?.1.parse().ok()
}

/// The processes in `lsof -F pgcnT` output that hold `port`: those with an
/// unconnected socket on it (`n127.0.0.1:5173`, `n[::1]:5173`, `n*:5173`),
/// listening or only bound, `listening` if one of those is `TST=LISTEN`. A
/// connection (`…->…`) doesn't count, whichever end the port is: a server
/// that sets SO_REUSEADDR still binds the local port of one (VERIFY V11).
/// A process without a `g` line gets pgid 0, which is no service's group.
fn owners_of(lsof_f: &str, port: u16) -> Vec<PortOwner> {
    // Each process, and whether it holds the port.
    let mut procs: Vec<(PortOwner, bool)> = vec![];
    // The current socket's local port is `port`.
    let mut on_port = false;
    for l in lsof_f.lines() {
        let mut chars = l.chars();
        let Some(tag) = chars.next() else {
            continue;
        };
        let rest = chars.as_str();
        if tag == 'p' {
            on_port = false;
            // An unparsable pid makes a record that is dropped, so its
            // lines don't go to the process before it.
            procs.push((
                PortOwner {
                    pid: rest.parse().unwrap_or(0),
                    pgid: 0,
                    name: String::new(),
                    listening: false,
                },
                false,
            ));
            continue;
        }
        let Some((owner, holds)) = procs.last_mut() else {
            continue;
        };
        match tag {
            'g' => owner.pgid = rest.parse().unwrap_or(0),
            'c' => owner.name = rest.to_string(),
            'f' => on_port = false,
            'n' => {
                on_port = !rest.contains("->") && local_port(rest) == Some(port);
                *holds |= on_port;
            }
            'T' if on_port && rest == "ST=LISTEN" => owner.listening = true,
            _ => {}
        }
    }
    procs
        .into_iter()
        .filter(|(o, holds)| *holds && o.pid != 0)
        .map(|(o, _)| o)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ps_pid_ppid_lines() {
        let out = "    0     0\n    1     0\n  612     1\n91423 91400\n\nPID PPID\n7 x\n8 9 10\n";
        assert_eq!(ps_pairs(out), [(0, 0), (1, 0), (612, 1), (91423, 91400)]);
    }

    #[test]
    fn parses_lsof_listen_lines() {
        let out = "p22100\nf3\nn127.0.0.1:52885\np22101\nf3\nn[::1]:52884\nf4\nn*:5173\nf5\nn[::1]:5173\n";
        assert_eq!(listen_ports(out), [5173, 52884, 52885]);
        assert!(listen_ports("").is_empty());
        assert!(listen_ports("p1\nf3\nnlocalhost:http\n").is_empty());
    }

    /// `lsof -nP -iTCP:55788 -F pgcnT` as VERIFY V11 recorded it (a listener
    /// with an accepted connection, and the client of that connection), plus
    /// a process that only holds the port (bound, `CLOSED`) and one that
    /// listens on another port.
    #[test]
    fn parses_lsof_port_owners() {
        let out = "p50433\ng50433\ncverify_lsof_port_owners\n\
f3\nn127.0.0.1:55788\nTST=LISTEN\nTQR=0\nTQS=0\n\
f4\nn[::1]:55788\nTST=LISTEN\nTQR=0\nTQS=0\n\
f5\nn127.0.0.1:55788->127.0.0.1:55789\nTST=ESTABLISHED\nTQR=0\nTQS=0\n\
p50427\ng50191\ncverify_lsof_port_owners\n\
f6\nn127.0.0.1:55789->127.0.0.1:55788\nTST=ESTABLISHED\nTQR=0\nTQS=0\n\
p50440\ng50191\ncholder\n\
f7\nn127.0.0.1:55788\nTST=CLOSED\n\
p50450\ng50450\ncnode\n\
f8\nn*:55790\nTST=LISTEN\n";
        let o = |pid, pgid, name: &str, listening| PortOwner {
            pid,
            pgid,
            name: name.into(),
            listening,
        };
        assert_eq!(
            owners_of(out, 55788),
            [
                o(50433, 50433, "verify_lsof_port_owners", true),
                o(50440, 50191, "holder", false),
            ]
        );
        assert_eq!(owners_of(out, 55790), [o(50450, 50450, "node", true)]);
        // The client's local port: a connection holds no port.
        assert!(owners_of(out, 55789).is_empty());
        assert!(owners_of("", 55788).is_empty());
        // A record whose pid doesn't parse is dropped with its lines; one
        // without a `g` line has pgid 0.
        assert!(owners_of("px\ng1\ncnode\nf3\nn*:80\nTST=LISTEN\n", 80).is_empty());
        assert_eq!(
            owners_of("p9\ncnode\nf3\nn*:80\nTST=LISTEN\n", 80),
            [o(9, 0, "node", true)]
        );
    }
}
