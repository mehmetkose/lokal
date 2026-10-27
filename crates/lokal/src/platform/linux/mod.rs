//! Linux: the pieces that need no privileges. `System` reads the process
//! table from `/proc` and sockets with `ss` (iproute2), and `Desktop` opens
//! URLs with `xdg-open`. Trust, the background service and name resolution
//! aren't ported yet: they're `Unsupported`, so lokal reports that it isn't
//! set up, and `lokal setup` fails (SPEC §10.1).
//
// PORTING(linux): trust (the system store and NSS), the background service
// (systemd, ports 443/80) and name resolution (`.lokal`, maybe `.local`)
// replace the `Unsupported` parts in later changes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::procfs::{self, ProcStat};
use super::unsupported::Unsupported;
use super::*;
use crate::process;

pub fn platform() -> Platform {
    Platform {
        trust: Box::new(Unsupported),
        resolver: Box::new(Unsupported),
        service: Box::new(Unsupported),
        desktop: Box::new(Linux),
        system: Box::new(Linux),
        default_name_backend: "none",
    }
}

pub fn name_publisher(backend: &str) -> Result<Box<dyn NamePublisher>> {
    super::unsupported::name_publisher(backend)
}

struct Linux;

impl Desktop for Linux {
    fn open_url(&self, url: &str) -> Result<()> {
        process::run_ok("/usr/bin/xdg-open", &[url])?;
        Ok(())
    }
}

/// Where ss may be, in order. An absolute path, so `$PATH` can't swap the
/// program, as macOS runs `/usr/sbin/lsof`.
const SS: [&str; 4] = ["/usr/bin/ss", "/usr/sbin/ss", "/bin/ss", "/sbin/ss"];

/// Runs ss with `args` and returns its output, or `None` if there is no ss
/// or it fails to run.
fn ss(args: &[&str]) -> Option<String> {
    let bin = SS.into_iter().find(|p| Path::new(p).is_file())?;
    process::run(bin, args).ok().map(|o| o.stdout)
}

/// `/proc/<pid>/stat`, parsed. `None` if the process is gone (or hidden).
fn proc_stat(pid: u32) -> Option<ProcStat> {
    let bytes = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    procfs::parse_stat(&String::from_utf8_lossy(&bytes))
}

/// `(pid, ppid)` of every process in `/proc` at one moment. One that exits
/// while it's read is left out.
fn process_table() -> Vec<(u32, u32)> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(proc_stat)
        .map(|s| (s.pid, s.ppid))
        .collect()
}

impl System for Linux {
    fn is_root(&self) -> bool {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }

    fn user_and_host(&self) -> (String, String) {
        let user = std::env::var("USER")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "user".into());
        let host = Some(short_hostname())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "localhost".into());
        (user, host)
    }

    fn run_privileged(&self, _cmd: &PrivilegedCommand, _prompt: &str) -> Result<()> {
        // Nothing privileged is ported yet: setup fails before it gets here.
        bail!(UNSUPPORTED)
    }

    fn port_owners(&self, port: u16) -> Vec<PortOwner> {
        // Every TCP socket on the port, in every state, with the processes
        // that have it open (only the user's own: another user's need
        // root). ss reads the kernel's socket tables, which leave out a
        // socket that's only bound, and iproute2 6.1 has no -B to ask for
        // them (VERIFY V11 on Linux, SPEC §12). The filter is one argument:
        // no shell.
        let filter = format!("sport = :{port}");
        let Some(out) = ss(&["-H", "-t", "-a", "-n", "-p", &filter]) else {
            return vec![];
        };
        procfs::ss_port_holders(&out, port)
            .into_iter()
            // The process group from /proc; a process that's gone since ss
            // listed it holds nothing any more.
            .filter_map(|(pid, name, listening)| {
                Some(PortOwner {
                    pid,
                    pgid: proc_stat(pid)?.pgid,
                    name,
                    listening,
                })
            })
            .collect()
    }

    fn tcp_listen_ports(&self, pid: u32) -> Vec<u16> {
        // One read of the process table, then ss's listeners filtered to
        // exactly the command's tree, as on macOS (VERIFY V9 on Linux).
        let pids: BTreeSet<u32> = process_tree(&process_table(), pid).into_iter().collect();
        if pids.is_empty() {
            return vec![];
        }
        let Some(out) = ss(&["-H", "-t", "-l", "-n", "-p"]) else {
            return vec![];
        };
        procfs::ss_listen_ports(&out, &pids)
    }

    fn hosts_file(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/etc/hosts"))
    }

    fn os_version(&self) -> String {
        // os-release(5): /etc/os-release, else /usr/lib/os-release.
        ["/etc/os-release", "/usr/lib/os-release"]
            .iter()
            .find_map(|p| procfs::pretty_name(&std::fs::read_to_string(p).ok()?))
            .unwrap_or_else(|| format!("Linux {}", kernel_release()))
    }
}

/// The kernel's release (`6.8.0-1015-azure`), from `uname`.
fn kernel_release() -> String {
    // SAFETY: utsname is plain data, and all zero bytes are a valid one.
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: uname only writes into the struct it gets.
    if unsafe { libc::uname(&mut u) } != 0 {
        return String::new();
    }
    // SAFETY: on success, release is a NUL-terminated string within the array.
    unsafe { std::ffi::CStr::from_ptr(u.release.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}
