//! Validation rules of SPEC §4.3. Every input is trimmed and lowercased
//! first. Errors are usage errors (exit 2) with a hint where useful.

use std::net::IpAddr;

use anyhow::Result;

use super::State;
use crate::error::UserError;

/// TLD of new projects: `.lokal`, served by lokal's own DNS server.
pub const DEFAULT_TLD: &str = "lokal";
/// Usable TLDs: `.lokal` (built-in DNS server) and `.local` (mDNS). The
/// CA's name constraints (`ca::PERMITTED_TLDS`) allow exactly these.
pub const SUPPORTED_TLDS: &[&str] = &["lokal", "local"];

pub fn normalize(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

/// 1–63 chars of `[a-z0-9-]`, not starting or ending with `-`.
pub fn is_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

fn label_error(what: &str, s: &str) -> UserError {
    UserError::usage(format!(
        "invalid {what} `{s}`: use 1–63 letters, digits or '-', not starting or ending with '-'"
    ))
}

pub fn project_name(input: &str) -> Result<String, UserError> {
    let s = normalize(input);
    if s.contains('.') {
        return Err(UserError::usage(format!("project name `{s}` can't contain dots"))
            .hint(format!("use a plain name and pass the full domain: lokal project create <NAME> --domain {s}")));
    }
    if !is_label(&s) {
        return Err(label_error("project name", &s));
    }
    Ok(s)
}

/// The apex for a new project: `<name>.lokal`, or a validated `--domain`.
pub fn apex(name: &str, domain: Option<&str>) -> Result<String, UserError> {
    let Some(d) = domain else {
        return Ok(format!("{name}.{DEFAULT_TLD}"));
    };
    let d = normalize(d);
    let d = d.trim_end_matches('.').to_string();
    let labels: Vec<&str> = d.split('.').collect();
    let tld = *labels.last().unwrap_or(&"");
    if !SUPPORTED_TLDS.contains(&tld) {
        return Err(UserError::usage(
            "only .lokal and .local are supported (other TLDs could shadow real websites)",
        ));
    }
    if labels.len() < 2 {
        return Err(UserError::usage(format!(
            "invalid domain `{d}`: it needs a name before .{tld}"
        )));
    }
    if d.len() > 253 {
        return Err(UserError::usage(format!(
            "domain `{d}` is longer than 253 characters"
        )));
    }
    for l in &labels {
        if !is_label(l) {
            return Err(label_error("domain label", l));
        }
    }
    Ok(d)
}

/// `a` equals, contains or sits under `b` (label-wise).
pub fn apex_overlaps(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

pub fn check_overlap(state: &State, apex: &str, except: Option<&str>) -> Result<(), UserError> {
    for (name, p) in &state.projects {
        if Some(name.as_str()) == except {
            continue;
        }
        if apex_overlaps(apex, &p.domain) {
            return Err(UserError::usage(format!(
                "{apex} overlaps project `{name}` ({})",
                p.domain
            ))
            .hint("pick a domain that neither equals, contains nor sits under another project's domain"));
        }
    }
    Ok(())
}

/// `@`, or dot-joined labels. A full FQDN under the apex is accepted and
/// stripped (`api.myproject.local` → `api`).
pub fn subdomain(input: &str, apex: &str) -> Result<String, UserError> {
    let mut s = normalize(input);
    s = s.trim_end_matches('.').to_string();
    if s == "@" || s == apex {
        return Ok("@".into());
    }
    if let Some(stripped) = s.strip_suffix(&format!(".{apex}")) {
        s = stripped.to_string();
    }
    if s.is_empty() {
        return Err(UserError::usage("subdomain can't be empty").hint("use @ for the apex"));
    }
    for l in s.split('.') {
        if !is_label(l) {
            return Err(label_error("subdomain label", l));
        }
    }
    let fqdn = format!("{s}.{apex}");
    if fqdn.len() > 253 {
        return Err(UserError::usage(format!(
            "{fqdn} is longer than 253 characters"
        )));
    }
    Ok(s)
}

/// Host: hostname or IP, brackets stripped. Returns the host and an
/// optional warning (non-loopback).
pub fn host(input: &str) -> Result<(String, Option<String>), UserError> {
    let s = normalize(input);
    if s.contains("://") {
        return Err(
            UserError::usage(format!("host `{s}` must not include a scheme"))
                .hint("pass just the host name, e.g. --host localhost"),
        );
    }
    let unbracketed = s
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .map(str::to_string);
    let h = match unbracketed {
        Some(inner) => {
            if inner.parse::<std::net::Ipv6Addr>().is_err() {
                return Err(UserError::usage(format!("`{s}` is not an IPv6 address")));
            }
            inner
        }
        None => {
            if s.starts_with('[') {
                return Err(UserError::usage(format!("host `{s}` must not include a port"))
                    .hint("pass the port separately: lokal service add <SUBDOMAIN> <PORT> --host <HOST>"));
            }
            s
        }
    };
    if h.parse::<IpAddr>().is_err() {
        if h.contains(':') {
            return Err(
                UserError::usage(format!("host `{h}` must not include a port")).hint(
                    "pass the port separately: lokal service add <SUBDOMAIN> <PORT> --host <HOST>",
                ),
            );
        }
        let name = h.trim_end_matches('.');
        if name.is_empty() || name.len() > 253 || !name.split('.').all(is_label) {
            return Err(UserError::usage(format!(
                "`{h}` is not a valid host name or IP address"
            )));
        }
    }
    let warn = (!crate::net::is_loopback_host(&h))
        .then(|| format!("{h} is not a loopback address; lokal will proxy to it over the network"));
    Ok((h, warn))
}

pub fn port(port: u32, host: &str) -> Result<u16, UserError> {
    if port == 0 || port > 65535 {
        return Err(UserError::usage(format!(
            "port {port} is out of range (1–65535)"
        )));
    }
    if (port == 443 || port == 80) && crate::net::is_loopback_host(host) {
        return Err(UserError::usage(format!(
            "port {port} on {host} is lokal itself; requests would loop back into lokal"
        ))
        .hint("point the service at the port your dev server listens on"));
    }
    Ok(port as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert!(is_label("api"));
        assert!(is_label("a-1"));
        assert!(!is_label("-a"));
        assert!(!is_label("a-"));
        assert!(!is_label(""));
        assert!(!is_label(&"a".repeat(64)));
        assert!(is_label(&"a".repeat(63)));
        assert!(!is_label("a_b"));
    }

    #[test]
    fn project_names() {
        assert_eq!(project_name("  MyProj ").unwrap(), "myproj");
        let e = project_name("my.proj").unwrap_err();
        assert!(e.hint.unwrap().contains("--domain"));
    }

    #[test]
    fn domains() {
        assert_eq!(apex("x", None).unwrap(), "x.lokal");
        assert_eq!(apex("x", Some("x.LOKAL")).unwrap(), "x.lokal");
        assert_eq!(apex("x", Some("App.Local")).unwrap(), "app.local");
        assert_eq!(apex("x", Some("a.b.local")).unwrap(), "a.b.local");
        // The CA can't sign anything else, so nothing else is accepted.
        for d in ["x.com", "x.test", "x.internal"] {
            assert!(
                apex("x", Some(d))
                    .unwrap_err()
                    .message
                    .contains("only .lokal and .local"),
                "{d}"
            );
        }
        for t in crate::ca::PERMITTED_TLDS {
            assert!(SUPPORTED_TLDS.contains(&t), "{t}");
        }
        assert_eq!(SUPPORTED_TLDS.len(), crate::ca::PERMITTED_TLDS.len());
        assert!(apex("x", Some("local")).is_err());
        assert!(apex("x", Some("lokal")).is_err());
        assert!(
            apex(
                "x",
                Some(&format!("{}.local", vec!["a".repeat(63); 4].join(".")))
            )
            .is_err()
        );
    }

    #[test]
    fn overlap() {
        assert!(apex_overlaps("app.local", "api.app.local"));
        assert!(apex_overlaps("api.app.local", "app.local"));
        assert!(apex_overlaps("app.local", "app.local"));
        assert!(!apex_overlaps("myapp.local", "app.local"));
    }

    #[test]
    fn subdomains() {
        assert_eq!(subdomain("api", "p.local").unwrap(), "api");
        assert_eq!(subdomain("API.p.local", "p.local").unwrap(), "api");
        assert_eq!(subdomain("v1.api", "p.local").unwrap(), "v1.api");
        assert_eq!(subdomain("@", "p.local").unwrap(), "@");
        assert_eq!(subdomain("p.local", "p.local").unwrap(), "@");
        assert!(subdomain("a..b", "p.local").is_err());
        assert!(subdomain("-a", "p.local").is_err());
    }

    #[test]
    fn hosts() {
        assert_eq!(host("localhost").unwrap(), ("localhost".into(), None));
        assert_eq!(host("[::1]").unwrap().0, "::1");
        assert_eq!(host("::1").unwrap().0, "::1");
        assert!(
            host("http://localhost")
                .unwrap_err()
                .message
                .contains("scheme")
        );
        assert!(host("localhost:3000").unwrap_err().message.contains("port"));
        assert!(host("[::1]:3000").unwrap_err().message.contains("port"));
        assert!(host("192.168.1.5").unwrap().1.is_some());
        assert!(host("bad host").is_err());
    }

    #[test]
    fn ports() {
        assert!(port(0, "localhost").is_err());
        assert!(port(70000, "localhost").is_err());
        assert!(port(443, "localhost").is_err());
        assert!(port(80, "127.0.0.1").is_err());
        assert!(port(443, "10.0.0.2").is_ok());
        assert_eq!(port(4000, "localhost").unwrap(), 4000);
    }
}
