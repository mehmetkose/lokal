//! `lokal doctor` (SPEC §11): every layer checked concurrently, each with a
//! fix hint. Exit 1 if any check fails.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Serialize;

use crate::ca::CaCert;
use crate::cli::output as out;
use crate::ipc;
use crate::net::{self, Trust};
use crate::paths::Paths;
use crate::platform::{self, ResolverStatus, TrustStatus};
use crate::proxy::routes::{self, Upstream};
use crate::setup::record::InstallRecord;
use crate::setup::{self, ports};
use crate::state::{Service, State, validate};
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
        self.checks
            .lock()
            .expect("no check panics while pushing")
            .push((
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
    // Projects that lokal 0.5 or earlier created on `.local` get no route,
    // certificate or name (SPEC §4.3): one `project.<name>` warning each
    // instead of the per-service checks, which could only fail.
    let unserved: Vec<(&str, &str)> = state
        .projects
        .iter()
        .filter(|(_, p)| !p.is_served())
        .map(|(name, p)| (name.as_str(), p.domain.as_str()))
        .collect();
    // Every served service, then every route (SPEC §4.11) of a served
    // project, as the route table builds them.
    let mut names: Vec<Named> = state
        .services()
        .into_iter()
        .filter(|s| validate::is_served(&s.domain))
        .map(|s| Named::new(s.fqdn, Kind::Service(s.service)))
        .collect();
    for p in state.projects.values().filter(|p| p.is_served()) {
        for (key, r) in &p.routes {
            let up = routes::rewrite(p, key, r).map(|rw| rw.upstream);
            let kind = Kind::Route {
                to: r.to.clone(),
                up,
            };
            names.push(Named::new(p.fqdn(key), kind));
        }
    }

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
                    format!("install.json and the {} exist", p.service_kind),
                    t,
                );
            } else {
                c.push(
                    0,
                    "setup.done",
                    Status::Fail,
                    format!(
                        "lokal isn't set up on {}; run 'lokal setup'",
                        platform::THIS_MACHINE
                    ),
                    t,
                );
            }
        });

        // ca.files + ca.trusted
        s.spawn(move || {
            let t = Instant::now();
            if !CaCert::exists(paths) {
                let why = if paths.ca_cert.is_file() {
                    "the CA's private key is missing; run 'lokal setup' to replace the CA (it untrusts the old one)"
                } else {
                    "no certificate authority; run 'lokal setup'"
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
                        "{} must be mode 0600 and owned by you (is {:o}, uid {:?}); run 'chmod 600' on it",
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
                    c.push(2, "ca.trusted", Status::Pass, format!("{} trusts lokal's CA", platform::OS_NAME), t);
                    trusted_ca = Some(ca);
                }
                Ok((TrustStatus::NotTrusted(why), _)) => c.push(
                    2,
                    "ca.trusted",
                    Status::Fail,
                    format!("{} doesn't trust lokal's CA ({why}); run 'lokal setup' (it will ask for your password)", platform::OS_NAME),
                    t,
                ),
                Err(e) => c.push(2, "ca.trusted", Status::Fail, format!("{e:#}"), t),
            }
            // ca.trust_scope: trusted for TLS only, not for every use (§6.3)
            let t = Instant::now();
            match trusted_ca.as_ref().map(|ca| p.trust.trust_too_broad(ca)) {
                None => c.push(2, "ca.trust_scope", Status::Skip, format!("{} doesn't trust lokal's CA", platform::OS_NAME), t),
                Some(Ok(false)) => c.push(2, "ca.trust_scope", Status::Pass, "trusted for HTTPS only", t),
                Some(Ok(true)) => c.push(
                    2,
                    "ca.trust_scope",
                    Status::Warn,
                    format!("{} trusts lokal's CA for every use, not just HTTPS; run 'lokal setup' to limit it (it will ask for your password)", platform::OS_NAME),
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
                        format!("lokal's CA should be valid for .lokal and .local only, but {why}; run 'lokal setup' to replace it (it will ask for your password)"),
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
                                "the background service runs {} but this lokal is {}; run 'lokal upgrade'",
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
                            "{e}; run 'lokal start'{}",
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
                // Who holds lokal's ports, if lokal may see it.
                let held: Vec<String> = [443, 80]
                    .into_iter()
                    .filter_map(|port| {
                        p.system
                            .port_owner(port)
                            .map(|o| format!("port {port} is held by {o}"))
                    })
                    .collect();
                let detail = if held.is_empty() {
                    "the background service isn't running; run 'lokal start'".into()
                } else {
                    format!("the background service isn't running; {}", held.join("; "))
                };
                c.push(5, "listeners", Status::Fail, detail, t);
                return;
            };
            if st.listeners.is_empty() && st.listeners_missing.is_empty() {
                c.push(
                    5,
                    "listeners",
                    Status::Fail,
                    "the daemon reports no listeners",
                    t,
                );
                return;
            }
            // Another program held some of lokal's addresses when the
            // service started (SPEC §8.1).
            if let Some(r) = ports::report(
                &ports::find(p, st),
                &st.listeners,
                p.service.port_config().is_some(),
                &|port, udp| p.system.port_lookup(port, udp),
            ) {
                let status = if r.fatal { Status::Fail } else { Status::Warn };
                c.push(
                    5,
                    "listeners",
                    status,
                    format!("{}. To fix it, {}", r.problem, r.hint),
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
            if !bad.is_empty() {
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
                return;
            }
            // A program that listens on every address on lokal's ports,
            // which loses localhost to lokal (VERIFY V14, item 7). Each
            // lookup runs lsof (about 35ms), so both run at once.
            let pid = st.pid;
            let shadowed: Vec<String> = std::thread::scope(|s| {
                let lookups: Vec<_> = [443, 80]
                    .into_iter()
                    .map(|port| {
                        s.spawn(move || ports::shadowed(p.system.port_owners(port), pid, port))
                    })
                    .collect();
                lookups
                    .into_iter()
                    .filter_map(|h| h.join().ok().flatten())
                    .collect()
            });
            if !shadowed.is_empty() {
                c.push(5, "listeners", Status::Warn, shadowed.join("; "), t);
                return;
            }
            c.push(
                5,
                "listeners",
                Status::Pass,
                format!(
                    "{} (from {})",
                    st.listeners.join(", "),
                    st.listener_source.as_deref().unwrap_or_default()
                ),
                t,
            );
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
                    let detail = if names.is_empty() {
                        // Nothing resolves .lokal here (a sandbox, or no resolver).
                        "the background service answered; the test name wasn't looked up (.lokal lookups don't reach lokal here)".to_string()
                    } else {
                        format!("{} answered", names.join(" and "))
                    };
                    c.push(6, "selftest", Status::Pass, detail, t)
                }
                Err(f) => {
                    // With an HTTPS address missing, the selftest may have
                    // reached the program that holds it: the listeners check
                    // has the cause, not trust.
                    let hint = if status
                        .as_ref()
                        .is_some_and(|st| ports::fatal(&st.listeners_missing))
                    {
                        "another program holds one of lokal's addresses: see the listeners check"
                    } else {
                        f.hint()
                    };
                    c.push(
                        6,
                        "selftest",
                        Status::Fail,
                        format!("failed at the {} layer: {}; {hint}", f.layer, f.message),
                        t,
                    )
                }
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
                    ".lokal lookups aren't sent to lokal; run 'lokal setup' (it will ask for your password)",
                    t,
                ),
                ResolverStatus::Wrong(why) => c.push(
                    8,
                    "resolver.lokal",
                    Status::Fail,
                    format!("{why}; run 'lokal setup'"),
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
                    format!(".lokal now exists in the public DNS root, and lokal hides real .lokal websites on {}. \
'lokal uninstall' removes the resolver config that sends .lokal lookups to lokal. \
Please report it: https://github.com/mehmetkose/lokal/issues", platform::THIS_MACHINE),
                    t,
                ),
                None => c.push(9, "tld.lokal", Status::Skip, "couldn't reach a DNS root server (offline?)", t),
            }
        });

        // project.<name>: a project lokal no longer serves
        for (i, (name, domain)) in unserved.iter().enumerate() {
            c.push(
                10 + i,
                format!("project.{name}"),
                Status::Warn,
                format!(
                    "{domain} isn't served: lokal serves only .lokal names since 0.6.0; {}",
                    validate::move_hint(name)
                ),
                Instant::now(),
            );
        }

        // firefox
        s.spawn(move || {
            let t = Instant::now();
            if let Some(extras) = p.trust.user_extras_status() {
                c.push(
                    7,
                    "firefox",
                    if extras.ok {
                        Status::Pass
                    } else {
                        Status::Warn
                    },
                    extras.detail,
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
        let names_ref = &names;
        s.spawn(move || {
            for (i, n) in names_ref.iter().enumerate() {
                dns_check(c, 100 + i * 4, &n.fqdn, &n.probe, paths.sandbox);
            }
        });
        for (i, n) in names.iter().enumerate() {
            let order = 100 + i * 4;
            // Scoped threads borrow these; `move` copies the references.
            let fqdn = &n.fqdn;
            let probe = &n.probe;
            let kind = &n.kind;
            s.spawn(move || {
                let t = Instant::now();
                if is_pattern(fqdn) {
                    let (status, detail) = wildcard_hosts_conflicts(hosts_text, fqdn);
                    c.push(order + 1, format!("dns.conflict.{fqdn}"), status, detail, t);
                    return;
                }
                // Addresses other than loopback that /etc/hosts gives this name.
                let mut bad: Vec<&str> = vec![];
                for line in hosts_text.lines() {
                    let line = line.split('#').next().unwrap_or("");
                    let mut fields = line.split_whitespace();
                    let Some(ip) = fields.next() else { continue };
                    let addr: Option<IpAddr> = ip.parse().ok();
                    let loopback = addr.is_some_and(|a| a.is_loopback());
                    if !loopback && fields.any(|name| name.eq_ignore_ascii_case(fqdn)) {
                        bad.push(ip);
                    }
                }
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
                            "/etc/hosts maps {fqdn} to {}; remove that line",
                            bad.join(", ")
                        ),
                        t,
                    );
                }
            });
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
                // A pattern's check says which name it covers was probed.
                let probed = if probe != fqdn {
                    format!(" for {probe}")
                } else {
                    String::new()
                };
                match net::tls_handshake(probe, addr, cfg, Duration::from_secs(2)) {
                    Ok(d) => c.push(
                        order + 2,
                        format!("tls.{fqdn}"),
                        Status::Pass,
                        format!("handshake{probed} verified in {}", util::fmt_duration(d)),
                        t,
                    ),
                    Err(e) => c.push(
                        order + 2,
                        format!("tls.{fqdn}"),
                        Status::Fail,
                        format!("{e}{probed}; run 'lokal setup'"),
                        t,
                    ),
                }
            });
            s.spawn(move || {
                let t = Instant::now();
                let (id, status, detail) = target_check(fqdn, kind);
                c.push(order + 3, id, status, detail, t);
            });
        }
    });

    let mut v = c
        .checks
        .into_inner()
        .expect("no check panics while pushing");
    v.sort_by_key(|(o, _)| *o);
    v.into_iter().map(|(_, c)| c).collect()
}

/// A name doctor checks: a service's FQDN or pattern, or a route's pattern.
struct Named {
    fqdn: String,
    /// The name the dns. and tls. checks probe. The checks' ids keep the
    /// pattern.
    probe: String,
    kind: Kind,
}

enum Kind {
    Service(Service),
    /// A route's target URL, and where it forwards to, or why the route
    /// table skips it.
    Route {
        to: String,
        up: Result<Upstream, String>,
    },
}

impl Named {
    fn new(fqdn: String, kind: Kind) -> Self {
        Named {
            probe: probe_name(&fqdn),
            fqdn,
            kind,
        }
    }
}

fn is_pattern(fqdn: &str) -> bool {
    fqdn.contains(['*', '{'])
}

/// The name doctor probes for `fqdn`: the FQDN itself, or for a wildcard
/// service or a route the pattern with each capture replaced by
/// `lokal-doctor`, a name whose handshake issues a leaf on demand
/// (SPEC §11).
fn probe_name(fqdn: &str) -> String {
    if is_pattern(fqdn) {
        validate::fill_captures(fqdn, "lokal-doctor")
    } else {
        fqdn.to_string()
    }
}

/// `upstream.<fqdn>` for a service: something listens on its port.
/// `route.<pattern>` for a route: its target service still exists, and
/// something listens where it forwards.
fn target_check(fqdn: &str, kind: &Kind) -> (String, Status, String) {
    let listening = |up: &Upstream| net::probe_tcp(&up.host, up.port, Duration::from_millis(50));
    match kind {
        Kind::Service(s) => {
            let id = format!("upstream.{fqdn}");
            if net::probe_tcp(&s.host, s.port, Duration::from_millis(50)) {
                (id, Status::Pass, format!("{} is listening", s.upstream()))
            } else {
                let detail = format!(
                    "nothing listening on {}. Is your service running?",
                    s.upstream()
                );
                (id, Status::Warn, detail)
            }
        }
        Kind::Route { to, up } => {
            let id = format!("route.{fqdn}");
            match up {
                Err(why) => (
                    id,
                    Status::Fail,
                    format!("not served: {why}; run 'lokal up' in the project's folder"),
                ),
                Ok(up) if listening(up) => (
                    id,
                    Status::Pass,
                    format!("→ {to}; {} is listening", up.authority()),
                ),
                Ok(up) => (
                    id,
                    Status::Warn,
                    format!(
                        "→ {to}, but nothing is listening on {}. Is your service running?",
                        up.authority()
                    ),
                ),
            }
        }
    }
}

/// `dns.conflict.<pattern>` for a wildcard service: `/etc/hosts` gives no
/// name it covers an address other than loopback.
fn wildcard_hosts_conflicts(hosts_text: &str, pattern: &str) -> (Status, String) {
    let mut bad: Vec<String> = vec![];
    for line in hosts_text.lines() {
        let line = line.split('#').next().unwrap_or("");
        let mut fields = line.split_whitespace();
        let Some(ip) = fields.next() else { continue };
        let addr: Option<IpAddr> = ip.parse().ok();
        if addr.is_some_and(|a| a.is_loopback()) {
            continue;
        }
        for name in fields {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            if validate::covers(pattern, &name) {
                bad.push(format!("{name} to {ip}"));
            }
        }
    }
    match bad.len() {
        0 => (Status::Pass, "no conflicting /etc/hosts entry".into()),
        1 => (
            Status::Warn,
            format!("/etc/hosts maps {}; remove that line", bad[0]),
        ),
        _ => (
            Status::Warn,
            format!("/etc/hosts maps {}; remove those lines", bad.join(", ")),
        ),
    }
}

/// `dns.<fqdn>`: getaddrinfo returns both loopbacks for `probe` (the FQDN,
/// or a name under a wildcard service's base), ideally within 20ms.
fn dns_check(c: &Collector, order: usize, fqdn: &str, probe: &str, sandbox: bool) {
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
    let (r, d) = net::resolve(probe);
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
                "resolves to {addrs:?}, expected 127.0.0.1 and ::1; check 'lokal status' names"
            ),
            t,
        ),
        Err(e) => c.push(
            order,
            id,
            Status::Fail,
            format!("{e:#}; check 'lokal status' and 'lokal logs'"),
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
                Status::Skip => out::dim("-"),
            };
            let mut lines = c.detail.lines();
            println!(
                "{mark} {:<36} {} {}",
                c.id,
                lines.next().unwrap_or(""),
                out::dim(&format!("{}ms", c.ms))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A wildcard service's checks probe a name under its base; any other
    /// service's probe its own FQDN.
    #[test]
    fn a_wildcard_is_probed_under_its_base() {
        assert_eq!(probe_name("api.p.lokal"), "api.p.lokal");
        assert_eq!(probe_name("p.lokal"), "p.lokal");
        assert_eq!(probe_name("*.p.lokal"), "lokal-doctor.p.lokal");
        assert_eq!(probe_name("*.v1.p.lokal"), "lokal-doctor.v1.p.lokal");
        assert!(validate::covers(
            "*.v1.p.lokal",
            &probe_name("*.v1.p.lokal")
        ));
    }

    /// A route's checks probe its pattern with every capture filled, a name
    /// the route itself covers.
    #[test]
    fn a_route_is_probed_with_its_captures_filled() {
        for (pattern, probe) in [
            ("{site}.p.lokal", "lokal-doctor.p.lokal"),
            ("preview.{slug}.p.lokal", "preview.lokal-doctor.p.lokal"),
            ("{a}.x.{b}.p.lokal", "lokal-doctor.x.lokal-doctor.p.lokal"),
        ] {
            assert_eq!(probe_name(pattern), probe);
            assert!(validate::covers(pattern, probe), "{pattern}");
        }
    }

    /// `route.<pattern>` fails for a route the table skips, such as one
    /// whose target service was removed by hand.
    #[test]
    fn a_route_whose_target_is_gone_fails() {
        let st = State::parse(
            r#"
version = 1

[projects.p]
domain = "p.lokal"

[projects.p.routes."{site}"]
to = "https://render.p.lokal/demosite/{site}"
managed = "lokal.toml"
"#,
        )
        .unwrap();
        let p = &st.projects["p"];
        let r = &p.routes["{site}"];
        let kind = Kind::Route {
            to: r.to.clone(),
            up: routes::rewrite(p, "{site}", r).map(|rw| rw.upstream),
        };
        let (id, status, detail) = target_check("{site}.p.lokal", &kind);
        assert_eq!(id, "route.{site}.p.lokal");
        assert_eq!(status, Status::Fail);
        assert!(
            detail.contains("render.p.lokal isn't a service of the project"),
            "{detail}"
        );
    }

    /// `dns.conflict.*.p.lokal` warns for every `/etc/hosts` name the
    /// wildcard covers that points elsewhere.
    #[test]
    fn hosts_conflicts_under_a_wildcard() {
        let hosts = "127.0.0.1 localhost\n::1 localhost\n\
127.0.0.1 a.p.lokal\n10.0.0.1 p.lokal x.q.lokal # not covered\n\
10.0.0.2 Tenant.P.lokal. other\n# 10.0.0.3 c.p.lokal\n10.0.0.4 b.a.p.lokal\n";
        let (status, detail) = wildcard_hosts_conflicts(hosts, "*.p.lokal");
        assert_eq!(status, Status::Warn);
        assert_eq!(
            detail,
            "/etc/hosts maps tenant.p.lokal to 10.0.0.2, b.a.p.lokal to 10.0.0.4; remove those lines"
        );
        let (status, detail) = wildcard_hosts_conflicts(hosts, "*.a.p.lokal");
        assert_eq!(status, Status::Warn);
        assert_eq!(
            detail,
            "/etc/hosts maps b.a.p.lokal to 10.0.0.4; remove that line"
        );
        let (status, detail) = wildcard_hosts_conflicts(hosts, "*.z.lokal");
        assert_eq!(status, Status::Pass);
        assert_eq!(detail, "no conflicting /etc/hosts entry");
    }
}
