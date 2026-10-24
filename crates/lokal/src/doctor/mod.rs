//! `lokal doctor` (SPEC §11): every layer checked concurrently, each with a
//! fix hint. Exit 1 if any check fails.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Serialize;

use crate::ca::CaCert;
use crate::cli::output::{self as out, dim};
use crate::ipc;
use crate::net::{self, Trust};
use crate::paths::Paths;
use crate::platform::{self, ResolverStatus, TrustStatus};
use crate::setup::{self, record::InstallRecord};
use crate::state::State;
use crate::util;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Skip,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub id: String,
    pub status: Status,
    pub detail: String,
    pub ms: u64,
}

struct Collector {
    checks: Mutex<Vec<(usize, Check)>>,
}

impl Collector {
    fn push(
        &self,
        order: usize,
        id: impl Into<String>,
        status: Status,
        detail: impl Into<String>,
        t: Instant,
    ) {
        self.checks.lock().expect("lock").push((
            order,
            Check {
                id: id.into(),
                status,
                detail: detail.into(),
                ms: t.elapsed().as_millis() as u64,
            },
        ));
    }
}

pub fn run(paths: &Paths) -> Vec<Check> {
    let p = platform::current();
    let c = Collector {
        checks: Mutex::new(vec![]),
    };
    let rec = InstallRecord::load(paths);
    let state = State::load(paths).unwrap_or_default();
    let services = state.services();

    // Fast sequential facts the other checks depend on.
    let t = Instant::now();
    let ping = ipc::ping(paths, Duration::from_millis(1000));
    let status: Option<ipc::Status> = ping
        .as_ref()
        .ok()
        .and_then(|_| ipc::call(paths, &ipc::Request::Status, Duration::from_secs(1)).ok());

    let hosts_text = p
        .system
        .hosts_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .unwrap_or_default();
    let tls_trust = if rec.as_ref().is_some_and(|r| r.trust_skipped) || paths.sandbox {
        Trust::CaFile(paths.ca_cert.clone())
    } else {
        Trust::Platform
    };
    let tls_cfg = net::client_config(&tls_trust).ok();
    std::thread::scope(|s| {
        let c = &c;
        let hosts_text = &hosts_text;
        let tls_cfg = &tls_cfg;
        let p = &p;
        let rec = &rec;
        let ping = &ping;
        let status = &status;

        // setup.done
        s.spawn(move || {
            let t = Instant::now();
            if paths.sandbox {
                c.push(0, "setup.done", Status::Skip, "sandbox (LOKAL_HOME)", t);
            } else if setup::is_set_up(paths) {
                c.push(
                    0,
                    "setup.done",
                    Status::Pass,
                    "install.json and the LaunchAgent exist",
                    t,
                );
            } else {
                c.push(
                    0,
                    "setup.done",
                    Status::Fail,
                    "lokal isn't set up on this Mac — run 'lokal setup'",
                    t,
                );
            }
        });

        // ca.files + ca.trusted
        s.spawn(move || {
            let t = Instant::now();
            if !CaCert::exists(paths) {
                let why = if paths.ca_cert.is_file() {
                    "the CA's private key is missing — run 'lokal setup' to replace the CA (it untrusts the old one)"
                } else {
                    "no certificate authority — run 'lokal setup'"
                };
                c.push(1, "ca.files", Status::Fail, why, t);
                c.push(2, "ca.trusted", Status::Fail, why, t);
                return;
            }
            let mode = util::file_mode(&paths.ca_key);
            let uid = util::file_uid(&paths.ca_key);
            if mode != Some(0o600) || uid != Some(util::current_uid()) {
                c.push(
                    1,
                    "ca.files",
                    Status::Fail,
                    format!(
                        "{} must be mode 0600 and owned by you (is {:o}, uid {:?}) — run 'chmod 600' on it",
                        paths.ca_key.display(),
                        mode.unwrap_or(0),
                        uid
                    ),
                    t,
                );
            } else {
                c.push(1, "ca.files", Status::Pass, format!("{} (key 0600)", paths.ca_cert.display()), t);
            }
            let t = Instant::now();
            let mut trusted_ca = None;
            match CaCert::load(paths).and_then(|ca| Ok((p.trust.is_trusted(&ca)?, ca))) {
                Ok((TrustStatus::Trusted, ca)) => {
                    c.push(2, "ca.trusted", Status::Pass, "macOS trusts lokal's CA", t);
                    trusted_ca = Some(ca);
                }
                Ok((TrustStatus::NotTrusted(why), _)) => c.push(
                    2,
                    "ca.trusted",
                    Status::Fail,
                    format!("macOS doesn't trust lokal's CA ({why}) — run 'lokal setup' (it will ask for your password)"),
                    t,
                ),
                Err(e) => c.push(2, "ca.trusted", Status::Fail, format!("{e:#}"), t),
            }
            // ca.trust_scope: trusted for TLS only, not for every use (§6.3)
            let t = Instant::now();
            match trusted_ca.as_ref().map(|ca| p.trust.trust_too_broad(ca)) {
                None => c.push(2, "ca.trust_scope", Status::Skip, "macOS doesn't trust lokal's CA", t),
                Some(Ok(false)) => c.push(2, "ca.trust_scope", Status::Pass, "trusted for HTTPS only", t),
                Some(Ok(true)) => c.push(
                    2,
                    "ca.trust_scope",
                    Status::Warn,
                    "macOS trusts lokal's CA for every use, not just HTTPS — run 'lokal setup' to limit it (it will ask for your password)",
                    t,
                ),
                Some(Err(e)) => c.push(2, "ca.trust_scope", Status::Fail, format!("{e:#}"), t),
            }
            // ca.scope: name constraints are exactly .lokal + .local
            if let Ok(ca) = CaCert::load(paths) {
                let t = Instant::now();
                match ca.scope_problem() {
                    None => c.push(2, "ca.scope", Status::Pass, "valid for .lokal and .local only", t),
                    Some(why) => c.push(
                        2,
                        "ca.scope",
                        Status::Warn,
                        format!("lokal's CA should be valid for .lokal and .local only, but {why} — run 'lokal setup' to replace it (it will ask for your password)"),
                        t,
                    ),
                }
            }
        });

        // daemon.running + daemon.build
        {
            match ping {
                Ok(pg) => {
                    c.push(
                        3,
                        "daemon.running",
                        Status::Pass,
                        format!("pid {}", pg.pid),
                        t,
                    );
                    if pg.build_id == crate::version::build_id() {
                        c.push(4, "daemon.build", Status::Pass, pg.build_id.clone(), t);
                    } else {
                        c.push(
                            4,
                            "daemon.build",
                            Status::Warn,
                            format!(
                                "the background service runs {} but this lokal is {} — run 'lokal upgrade'",
                                pg.build_id,
                                crate::version::build_id()
                            ),
                            t,
                        );
                    }
                }
                Err(e) => {
                    let logs = util::tail_lines(&paths.daemon_log, 5);
                    c.push(
                        3,
                        "daemon.running",
                        Status::Fail,
                        format!(
                            "{e} — run 'lokal start'{}",
                            if logs.is_empty() {
                                String::new()
                            } else {
                                format!("\nlast log lines:\n  {}", logs.join("\n  "))
                            }
                        ),
                        t,
                    );
                    c.push(
                        4,
                        "daemon.build",
                        Status::Skip,
                        "the background service isn't running",
                        t,
                    );
                }
            }
        }

        // listeners
        s.spawn(move || {
            let t = Instant::now();
            let Some(st) = status else {
                let owner = p.system.port_owner(443);
                let detail = match owner {
                    Some(o) => {
                        format!("the background service isn't running; port 443 is held by {o}")
                    }
                    None => "the background service isn't running — run 'lokal start'".into(),
                };
                c.push(5, "listeners", Status::Fail, detail, t);
                return;
            };
            if st.listeners.is_empty() {
                c.push(
                    5,
                    "listeners",
                    Status::Fail,
                    "the daemon reports no listeners",
                    t,
                );
                return;
            }
            if paths.sandbox {
                c.push(5, "listeners", Status::Pass, st.listeners.join(", "), t);
                return;
            }
            let mut bad = vec![];
            for ip in [
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            ] {
                let a = SocketAddr::new(ip, 443);
                if std::net::TcpStream::connect_timeout(&a, Duration::from_millis(500)).is_err() {
                    bad.push(a.to_string());
                }
            }
            if bad.is_empty() {
                c.push(
                    5,
                    "listeners",
                    Status::Pass,
                    format!(
                        "{} (from {})",
                        st.listeners.join(", "),
                        st.listener_source.clone().unwrap_or_default()
                    ),
                    t,
                );
            } else {
                let owner = p
                    .system
                    .port_owner(443)
                    .map(|o| format!("; port 443 is held by {o}"))
                    .unwrap_or_default();
                c.push(
                    5,
                    "listeners",
                    Status::Fail,
                    format!("cannot connect to {}{owner}", bad.join(", ")),
                    t,
                );
            }
        });

        // selftest
        s.spawn(move || {
            let t = Instant::now();
            if ping.is_err() {
                c.push(
                    6,
                    "selftest",
                    Status::Skip,
                    "the background service isn't running",
                    t,
                );
                return;
            }
            let trust = if rec.as_ref().is_some_and(|r| r.trust_skipped) || paths.sandbox {
                Trust::CaFile(paths.ca_cert.clone())
            } else {
                Trust::Platform
            };
            let check_lokal = !paths.sandbox
                && p.resolver
                    .status(crate::dns::ZONE, crate::dns::DEFAULT_PORT)
                    == ResolverStatus::Installed;
            match setup::selftest(paths, &trust, check_lokal) {
                Ok(ok) => {
                    let names: Vec<String> =
                        ok.fqdns.iter().map(|f| format!("https://{f}")).collect();
                    c.push(
                        6,
                        "selftest",
                        Status::Pass,
                        format!("{} answered", names.join(" and ")),
                        t,
                    )
                }
                Err(f) => c.push(
                    6,
                    "selftest",
                    Status::Fail,
                    format!(
                        "failed at the {} layer: {} — {}",
                        f.layer,
                        f.message,
                        f.hint()
                    ),
                    t,
                ),
            }
        });

        // resolver.lokal: the OS sends .lokal lookups to lokal's DNS server
        s.spawn(move || {
            let t = Instant::now();
            if paths.sandbox {
                c.push(8, "resolver.lokal", Status::Skip, "sandbox (LOKAL_HOME)", t);
                return;
            }
            match p.resolver.status(crate::dns::ZONE, crate::dns::DEFAULT_PORT) {
                ResolverStatus::Installed => c.push(
                    8,
                    "resolver.lokal",
                    Status::Pass,
                    format!(".lokal lookups go to 127.0.0.1:{}", crate::dns::DEFAULT_PORT),
                    t,
                ),
                ResolverStatus::Missing => c.push(
                    8,
                    "resolver.lokal",
                    Status::Fail,
                    ".lokal lookups aren't sent to lokal — run 'lokal setup' (it will ask for your password)",
                    t,
                ),
                ResolverStatus::Wrong(why) => c.push(
                    8,
                    "resolver.lokal",
                    Status::Fail,
                    format!("{why} — run 'lokal setup'"),
                    t,
                ),
            }
        });

        // tld.lokal: `.lokal` must not be a real public TLD
        s.spawn(move || {
            let t = Instant::now();
            match lokal_in_public_dns(Duration::from_millis(700)) {
                Some(false) => c.push(
                    9,
                    "tld.lokal",
                    Status::Pass,
                    ".lokal is not a public TLD (DNS root: NXDOMAIN)",
                    t,
                ),
                Some(true) => c.push(
                    9,
                    "tld.lokal",
                    Status::Fail,
                    ".lokal now exists in the public DNS root: lokal would hide real .lokal websites on this Mac. \
Move your projects to .local (lokal project create <name> --domain <name>.local) and tell the lokal maintainers",
                    t,
                ),
                None => c.push(9, "tld.lokal", Status::Skip, "couldn't reach a DNS root server (offline?)", t),
            }
        });

        // firefox
        s.spawn(move || {
            let t = Instant::now();
            if let Some((ok, detail)) = p.trust.user_extras_status() {
                c.push(
                    7,
                    "firefox",
                    if ok { Status::Pass } else { Status::Warn },
                    detail,
                    t,
                );
            }
        });

        // per-service checks
        let https_port = status
            .as_ref()
            .and_then(|st| st.listeners.first())
            .and_then(|l| l.rsplit(':').next())
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(443);
        // DNS lookups run one after another: concurrent getaddrinfo calls
        // inflate each other's timings (the check warns above 20ms).
        let services_ref = &services;
        s.spawn(move || {
            for (i, svc) in services_ref.iter().enumerate() {
                dns_check(c, 100 + i * 4, &svc.fqdn, paths.sandbox);
            }
        });
        for (i, svc) in services.iter().enumerate() {
            let order = 100 + i * 4;
            let fqdn = svc.fqdn.clone();
            s.spawn(move || {
                let t = Instant::now();
                let bad: Vec<String> = hosts_text
                    .lines()
                    .map(|l| l.split('#').next().unwrap_or("").trim())
                    .filter(|l| !l.is_empty())
                    .filter_map(|l| {
                        let mut it = l.split_whitespace();
                        let ip = it.next()?;
                        let names: Vec<&str> = it.collect();
                        let loopback = ip
                            .parse::<IpAddr>()
                            .map(|x| x.is_loopback())
                            .unwrap_or(false);
                        (names.iter().any(|n| n.eq_ignore_ascii_case(&fqdn)) && !loopback)
                            .then(|| ip.to_string())
                    })
                    .collect();
                if bad.is_empty() {
                    c.push(
                        order + 1,
                        format!("dns.conflict.{fqdn}"),
                        Status::Pass,
                        "no conflicting /etc/hosts entry",
                        t,
                    );
                } else {
                    c.push(
                        order + 1,
                        format!("dns.conflict.{fqdn}"),
                        Status::Warn,
                        format!(
                            "/etc/hosts maps {fqdn} to {} — remove that line",
                            bad.join(", ")
                        ),
                        t,
                    );
                }
            });
            let fqdn = svc.fqdn.clone();
            s.spawn(move || {
                let t = Instant::now();
                if ping.is_err() {
                    c.push(
                        order + 2,
                        format!("tls.{fqdn}"),
                        Status::Skip,
                        "the background service isn't running",
                        t,
                    );
                    return;
                }
                let Some(cfg) = tls_cfg.clone() else {
                    c.push(
                        order + 2,
                        format!("tls.{fqdn}"),
                        Status::Fail,
                        "cannot build a TLS client",
                        t,
                    );
                    return;
                };
                let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), https_port);
                match net::tls_handshake(&fqdn, addr, cfg, Duration::from_secs(2)) {
                    Ok(d) => c.push(
                        order + 2,
                        format!("tls.{fqdn}"),
                        Status::Pass,
                        format!("handshake verified in {}", util::fmt_duration(d)),
                        t,
                    ),
                    Err(e) => c.push(
                        order + 2,
                        format!("tls.{fqdn}"),
                        Status::Fail,
                        format!("{e} — run 'lokal setup'"),
                        t,
                    ),
                }
            });
            let fqdn = svc.fqdn.clone();
            let up = svc.service.clone();
            s.spawn(move || {
                let t = Instant::now();
                if net::probe_tcp(&up.host, up.port, Duration::from_millis(50)) {
                    c.push(
                        order + 3,
                        format!("upstream.{fqdn}"),
                        Status::Pass,
                        format!("{} is listening", up.upstream()),
                        t,
                    );
                } else {
                    c.push(
                        order + 3,
                        format!("upstream.{fqdn}"),
                        Status::Warn,
                        format!(
                            "nothing listening on {} — is your service running?",
                            up.upstream()
                        ),
                        t,
                    );
                }
            });
        }
    });

    let mut v = c.checks.into_inner().expect("lock");
    v.sort_by_key(|(o, _)| *o);
    v.into_iter().map(|(_, c)| c).collect()
}

/// `dns.<fqdn>`: getaddrinfo returns both loopbacks, ideally within 20ms.
fn dns_check(c: &Collector, order: usize, fqdn: &str, sandbox: bool) {
    let t = Instant::now();
    let id = format!("dns.{fqdn}");
    if sandbox {
        c.push(
            order,
            id,
            Status::Skip,
            "sandbox: names are not published",
            t,
        );
        return;
    }
    let (r, d) = net::resolve(fqdn);
    match r {
        Ok(addrs) if net::has_both_loopbacks(&addrs) => {
            let ms = d.as_secs_f64() * 1000.0;
            if ms > 20.0 {
                c.push(
                    order,
                    id,
                    Status::Warn,
                    format!("127.0.0.1 and ::1, but slow ({ms:.1}ms)"),
                    t,
                );
            } else {
                c.push(
                    order,
                    id,
                    Status::Pass,
                    format!("127.0.0.1 and ::1 in {ms:.1}ms"),
                    t,
                );
            }
        }
        Ok(addrs) => c.push(
            order,
            id,
            Status::Fail,
            format!(
                "resolves to {addrs:?}, expected 127.0.0.1 and ::1 — check 'lokal status' names"
            ),
            t,
        ),
        Err(e) => c.push(
            order,
            id,
            Status::Fail,
            format!("{e:#} — check 'lokal status' and 'lokal logs'"),
            t,
        ),
    }
}

/// Asks a DNS root server whether `lokal.` exists. `None` if unreachable.
fn lokal_in_public_dns(timeout: Duration) -> Option<bool> {
    use crate::dns::wire;
    // a.root-servers.net and b.root-servers.net (IPv4)
    let roots = ["198.41.0.4:53", "170.247.170.2:53"];
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(timeout)).ok()?;
    let id = u16::from_be_bytes(util::random_bytes::<2>());
    let q = wire::query(id, crate::dns::ZONE, wire::TYPE_SOA);
    for r in roots {
        if sock.send_to(&q, r).is_err() {
            continue;
        }
        let mut buf = [0u8; 1500];
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let Ok((n, _)) = sock.recv_from(&mut buf) else {
                break;
            };
            if n >= 2 && buf[0..2] == id.to_be_bytes() {
                return match wire::response_summary(&buf[..n]) {
                    Some((wire::RCODE_NXDOMAIN, _)) => Some(false),
                    Some((wire::RCODE_NOERROR, _)) => Some(true),
                    _ => None,
                };
            }
        }
    }
    None
}

pub fn command(json: bool) -> Result<i32> {
    let paths = Paths::resolve()?;
    let t = Instant::now();
    let checks = run(&paths);
    let ok = !checks.iter().any(|c| c.status == Status::Fail);
    if json {
        out::print_json(&serde_json::json!({ "schema": 1, "ok": ok, "checks": checks }));
    } else {
        for c in &checks {
            let mark = match c.status {
                Status::Pass => out::ok_mark(),
                Status::Warn => out::warn_mark(),
                Status::Fail => out::fail_mark(),
                Status::Skip => dim("-"),
            };
            let mut lines = c.detail.lines();
            println!(
                "{mark} {:<36} {} {}",
                c.id,
                lines.next().unwrap_or(""),
                dim(&format!("{}ms", c.ms))
            );
            for l in lines {
                println!("    {l}");
            }
        }
        let fails = checks.iter().filter(|c| c.status == Status::Fail).count();
        let warns = checks.iter().filter(|c| c.status == Status::Warn).count();
        println!();
        if ok {
            println!(
                "{} all good ({} warning(s)) in {}",
                out::ok_mark(),
                warns,
                util::fmt_duration(t.elapsed())
            );
        } else {
            println!(
                "{} {fails} check(s) failed, {warns} warning(s)",
                out::fail_mark()
            );
        }
    }
    Ok(if ok { 0 } else { 1 })
}
