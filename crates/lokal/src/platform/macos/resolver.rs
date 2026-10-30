//! `/etc/resolver/<tld>` (resolver(5)): macOS sends every lookup under
//! `.<tld>` to the nameserver and port in that file. Installing it needs
//! root and happens only in `setup`; checking it doesn't.

use std::path::Path;

use crate::platform::{PrivilegedCommand, ResolverStatus, SystemResolver};
use crate::process;

const DIR: &str = "/etc/resolver";

pub struct EtcResolver;

fn path(tld: &str) -> String {
    format!("{DIR}/{tld}")
}

impl SystemResolver for EtcResolver {
    fn unavailable(&self) -> Option<String> {
        None
    }

    fn config(&self, _tld: &str, port: u16) -> String {
        format!(
            "# Installed by lokal: .lokal names are answered by lokal's DNS server.\nnameserver 127.0.0.1\nport {port}\n"
        )
    }

    fn location(&self, tld: &str) -> String {
        path(tld)
    }

    fn plan_install(&self, tld: &str, staged: &Path) -> Vec<PrivilegedCommand> {
        vec![
            PrivilegedCommand {
                program: "/bin/mkdir".into(),
                args: vec!["-p".into(), DIR.into()],
            },
            PrivilegedCommand {
                program: "/usr/bin/install".into(),
                args: vec![
                    "-m".into(),
                    "0644".into(),
                    staged.display().to_string(),
                    path(tld),
                ],
            },
        ]
    }

    fn plan_uninstall(&self, tld: &str) -> Vec<PrivilegedCommand> {
        vec![PrivilegedCommand {
            program: "/bin/rm".into(),
            args: vec!["-f".into(), path(tld)],
        }]
    }

    fn status(&self, tld: &str, port: u16) -> ResolverStatus {
        let Ok(text) = std::fs::read_to_string(path(tld)) else {
            return ResolverStatus::Missing;
        };
        let mut ns = None;
        let mut p = None;
        for l in text.lines().map(str::trim) {
            if let Some(v) = l.strip_prefix("nameserver") {
                ns = Some(v.trim().to_string());
            } else if let Some(v) = l.strip_prefix("port") {
                p = v.trim().parse::<u16>().ok();
            }
        }
        if ns.as_deref() != Some("127.0.0.1") || p != Some(port) {
            return ResolverStatus::Wrong(format!(
                "{} points to {}:{} instead of 127.0.0.1:{port}",
                path(tld),
                ns.unwrap_or_default(),
                p.map(|x| x.to_string()).unwrap_or_else(|| "53".into())
            ));
        }
        // The file is right; is the system resolver using it?
        let active = process::run("/usr/sbin/scutil", &["--dns"])
            .map(|o| {
                o.stdout
                    .lines()
                    .any(|l| l.trim() == format!("domain   : {tld}"))
            })
            .unwrap_or(false);
        if active {
            ResolverStatus::Installed
        } else {
            ResolverStatus::Wrong(format!(
                "{} exists but macOS isn't using it yet (scutil --dns)",
                path(tld)
            ))
        }
    }
}
