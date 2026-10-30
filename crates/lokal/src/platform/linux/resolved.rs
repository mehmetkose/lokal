//! `.lokal` lookups on Linux: a DNS delegate file for systemd-resolved
//! (systemd.dns-delegate(5), systemd 258 and later), which sends every
//! lookup under `.<tld>` to lokal's DNS server, as `/etc/resolver/<tld>`
//! does on macOS. Installing it needs root and happens only in `setup`;
//! checking it doesn't.
//!
//! It only helps the programs that ask resolved: through nss-resolve
//! (`resolve` on the hosts line of /etc/nsswitch.conf) or resolved's stub
//! server in /etc/resolv.conf (127.0.0.53). WSL writes its own resolv.conf,
//! so there it takes nss-resolve. `unavailable` says so before setup
//! changes anything.

use std::path::Path;

use super::program;
use crate::platform::{PrivilegedCommand, ResolverStatus, SystemResolver};
use crate::process;

const DIR: &str = "/etc/systemd/dns-delegate.d";
const BUSCTL: [&str; 2] = ["/usr/bin/busctl", "/bin/busctl"];
const SYSTEMCTL: [&str; 2] = ["/usr/bin/systemctl", "/bin/systemctl"];
const NSSWITCH: &str = "/etc/nsswitch.conf";
const RESOLV_CONF: &str = "/etc/resolv.conf";
/// resolved's stub servers.
const STUBS: [&str; 2] = ["127.0.0.53", "127.0.0.54"];

pub struct Resolved;

/// The delegate's file; resolved names the delegate after it (`lokal`).
fn path(tld: &str) -> String {
    format!("{DIR}/{tld}.dns-delegate")
}

/// `busctl call` on resolved's manager object. True if it succeeds.
fn resolve1(method: &str, args: &[&str]) -> bool {
    let mut all = vec![
        "call",
        "org.freedesktop.resolve1",
        "/org/freedesktop/resolve1",
        "org.freedesktop.resolve1.Manager",
        method,
    ];
    all.extend_from_slice(args);
    process::run(&program(&BUSCTL), &all).is_ok_and(|o| o.success())
}

/// `DNS=` and `Domains=` of a delegate file, the last of each.
fn parse_delegate(text: &str) -> (Option<String>, Option<String>) {
    let mut dns = None;
    let mut domains = None;
    for l in text.lines().map(str::trim) {
        if let Some(v) = l.strip_prefix("DNS=") {
            dns = Some(v.trim().to_string());
        } else if let Some(v) = l.strip_prefix("Domains=") {
            domains = Some(v.trim().to_string());
        }
    }
    (dns, domains)
}

/// Do programs ask resolved for names, given these two files? `Err` says
/// what they ask instead.
fn lookups_use_resolved(nsswitch: &str, resolv_conf: &str) -> Result<(), String> {
    let hosts: Vec<&str> = nsswitch
        .lines()
        .map(|l| l.split('#').next().unwrap_or_default().trim())
        .find_map(|l| l.strip_prefix("hosts:"))
        .map(|v| v.split_whitespace().collect())
        .unwrap_or_default();
    let at = |s: &str| hosts.iter().position(|h| *h == s);
    // nss-resolve answers first, for every name, when it comes before dns.
    if let Some(r) = at("resolve")
        && at("dns").is_none_or(|d| r < d)
    {
        return Ok(());
    }
    let servers: Vec<&str> = resolv_conf
        .lines()
        .filter_map(|l| l.trim().strip_prefix("nameserver"))
        .map(str::trim)
        .collect();
    if at("dns").is_some() && !servers.is_empty() && servers.iter().all(|s| STUBS.contains(s)) {
        return Ok(());
    }
    Err(format!(
        "programs here don't ask systemd-resolved for names (the hosts line of {NSSWITCH} has no `resolve`, and {RESOLV_CONF} points to {})",
        if servers.is_empty() {
            "no server".to_string()
        } else {
            servers.join(", ")
        }
    ))
}

fn system_lookups() -> Result<(), String> {
    let read = |p| std::fs::read_to_string(p).unwrap_or_default();
    lookups_use_resolved(&read(NSSWITCH), &read(RESOLV_CONF))
}

impl SystemResolver for Resolved {
    fn unavailable(&self) -> Option<String> {
        let active = process::run(&program(&SYSTEMCTL), &["is-active", "systemd-resolved"])
            .is_ok_and(|o| o.stdout.trim() == "active");
        if !active {
            return Some(
                "systemd-resolved isn't running, and lokal's .lokal names need it (systemd 258 or later)"
                    .into(),
            );
        }
        if !resolve1("ListDelegates", &[]) {
            return Some(
                "this systemd-resolved has no DNS delegates, which lokal's .lokal names need (systemd 258 or later)"
                    .into(),
            );
        }
        system_lookups().err().map(|why| {
            format!(
                "{why}: add `resolve [!UNAVAIL=return]` before `dns` there (on Debian and Ubuntu, `sudo apt install libnss-resolve` does it)"
            )
        })
    }

    fn config(&self, tld: &str, port: u16) -> String {
        format!(
            "# Installed by lokal: .{tld} names are answered by lokal's DNS server.\n[Delegate]\nDNS=127.0.0.1:{port}\nDomains=~{tld}\n"
        )
    }

    fn location(&self, tld: &str) -> String {
        path(tld)
    }

    fn plan_install(&self, tld: &str, staged: &Path) -> Vec<PrivilegedCommand> {
        vec![
            PrivilegedCommand {
                program: program(&["/usr/bin/install", "/bin/install"]),
                args: vec![
                    "-D".into(),
                    "-m".into(),
                    "0644".into(),
                    staged.display().to_string(),
                    path(tld),
                ],
            },
            restart_resolved(),
        ]
    }

    fn plan_uninstall(&self, tld: &str) -> Vec<PrivilegedCommand> {
        vec![
            PrivilegedCommand {
                program: program(&["/usr/bin/rm", "/bin/rm"]),
                args: vec!["-f".into(), path(tld)],
            },
            restart_resolved(),
        ]
    }

    fn status(&self, tld: &str, port: u16) -> ResolverStatus {
        let Ok(text) = std::fs::read_to_string(path(tld)) else {
            return ResolverStatus::Missing;
        };
        let want = format!("127.0.0.1:{port}");
        let (dns, domains) = parse_delegate(&text);
        let domain_ok = domains.as_deref().is_some_and(|d| {
            d.split_whitespace()
                .any(|x| x.trim_start_matches('~') == tld)
        });
        if dns.as_deref() != Some(want.as_str()) || !domain_ok {
            return ResolverStatus::Wrong(format!(
                "{} sends {} to {} instead of {want}",
                path(tld),
                domains.unwrap_or_default(),
                dns.unwrap_or_default(),
            ));
        }
        // The file is right; has resolved loaded it?
        if !resolve1("GetDelegate", &["s", tld]) {
            return ResolverStatus::Wrong(format!(
                "{} exists but systemd-resolved hasn't loaded it (systemctl restart systemd-resolved)",
                path(tld)
            ));
        }
        match system_lookups() {
            Ok(()) => ResolverStatus::Installed,
            Err(why) => ResolverStatus::Wrong(why),
        }
    }
}

/// Delegates are read when resolved starts.
fn restart_resolved() -> PrivilegedCommand {
    PrivilegedCommand {
        program: program(&SYSTEMCTL),
        args: vec!["restart".into(), "systemd-resolved".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delegate_sends_the_tld_to_lokal() {
        let text = Resolved.config("lokal", 53535);
        assert_eq!(
            parse_delegate(&text),
            (Some("127.0.0.1:53535".into()), Some("~lokal".into()))
        );
        assert!(text.contains("[Delegate]\n"));
        assert_eq!(
            Resolved.location("lokal"),
            "/etc/systemd/dns-delegate.d/lokal.dns-delegate"
        );
        let plan = Resolved.plan_install("lokal", Path::new("/staged/lokal"));
        assert_eq!(
            plan[0].args[3..],
            ["/staged/lokal".to_string(), path("lokal")]
        );
        assert_eq!(plan[1].args, ["restart", "systemd-resolved"]);
    }

    /// nss-resolve before dns, or resolved's stub as the only server.
    #[test]
    fn knows_whether_lookups_reach_resolved() {
        let wsl = "nameserver 10.255.255.254\nsearch corp.example\n";
        let stub = "nameserver 127.0.0.53\noptions edns0 trust-ad\n";
        let plain = "passwd: files\nhosts:          files dns\n";
        let nss =
            "hosts: files mdns4_minimal [NOTFOUND=return] resolve [!UNAVAIL=return] dns # x\n";
        assert!(lookups_use_resolved(nss, wsl).is_ok());
        assert!(lookups_use_resolved(plain, stub).is_ok());
        let err = lookups_use_resolved(plain, wsl).unwrap_err();
        assert!(err.contains("10.255.255.254"), "{err}");
        // resolve after dns: dns answers (or fails) first.
        assert!(lookups_use_resolved("hosts: files dns resolve\n", wsl).is_err());
        // The stub plus another server: lookups may skip resolved.
        assert!(lookups_use_resolved(plain, &format!("{stub}nameserver 1.1.1.1\n")).is_err());
        // A commented-out line doesn't count.
        assert!(lookups_use_resolved("#hosts: resolve\nhosts: files dns\n", wsl).is_err());
    }
}
