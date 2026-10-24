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
            .unwrap_or_else(|| {
                let mut buf = [0u8; 256];
                // SAFETY: buf is valid for its length.
                unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
                let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
                let h = String::from_utf8_lossy(&buf[..end]).to_string();
                h.split('.').next().unwrap_or("mac").to_string()
            });
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

    fn port_owner(&self, port: u16) -> Option<String> {
        let out = process::run(
            "/usr/sbin/lsof",
            &["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN"],
        )
        .ok()?;
        let owners: Vec<String> = out
            .stdout
            .lines()
            .skip(1)
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                Some(format!("{} (pid {})", it.next()?, it.next()?))
            })
            .collect();
        if owners.is_empty() {
            None
        } else {
            let mut o = owners;
            o.dedup();
            Some(o.join(", "))
        }
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
