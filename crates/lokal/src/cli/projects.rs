//! Project and service commands. Every mutating command takes the fast
//! path (SPEC §4.5): atomic locked write → IPC `reload` that waits until
//! the affected FQDNs are ready → print the result with the measured time.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde_json::json;

use super::output::{self as out, dim, ok_mark};
use crate::error::UserError;
use crate::ipc;
use crate::paths::Paths;
use crate::setup::{self, DaemonState};
use crate::state::{
    HostHeader, MANAGED_BY_FILE, Project, Service, State, no_such_project, validate,
};
use crate::util;

const RELOAD_TIMEOUT: Duration = Duration::from_millis(2500);

/// How the daemon took a change.
pub(super) enum Applied {
    Ready,
    NotSetUp,
    NotRunning,
}

/// Sends `reload` and checks that `ready` FQDNs are live and `gone` ones
/// are not.
pub(super) fn apply(paths: &Paths, ready: &[String], gone: &[String]) -> Result<Applied> {
    match setup::daemon_state(paths) {
        DaemonState::NotSetUp => Ok(Applied::NotSetUp),
        DaemonState::NotRunning => Ok(Applied::NotRunning),
        DaemonState::Broken(e) => bail!(
            UserError::runtime(format!("cannot reach the background service: {e}"))
                .hint("run 'lokal doctor'")
        ),
        DaemonState::Running(ping) => {
            setup::warn_build(&ping);
            let r: ipc::Reload =
                ipc::call(paths, &ipc::Request::Reload, RELOAD_TIMEOUT).map_err(|e| {
                    UserError::runtime(format!(
                        "the background service didn't apply the change: {e}"
                    ))
                    .hint("run 'lokal doctor'")
                })?;
            if !r.ok {
                bail!(UserError::runtime(format!(
                    "the background service couldn't apply the change: {}",
                    r.error.unwrap_or_default()
                )));
            }
            let mut problems = vec![];
            for f in ready {
                if let Some(x) = r.failed.iter().find(|x| &x.fqdn == f) {
                    problems.push(format!("{f}: {}", x.error));
                } else if !r.ready.contains(f) {
                    problems.push(format!("{f}: not ready"));
                }
            }
            for f in gone {
                if r.ready.contains(f) {
                    problems.push(format!("{f}: still served"));
                }
            }
            if !problems.is_empty() {
                bail!(
                    UserError::runtime(format!("not ready:\n  {}", problems.join("\n  ")))
                        .hint("run 'lokal doctor' and 'lokal logs'")
                );
            }
            Ok(Applied::Ready)
        }
    }
}

/// The SPEC §4.5 note for a change the daemon didn't take, if any.
pub(super) fn note_text(a: &Applied) -> Option<&'static str> {
    match a {
        Applied::Ready => None,
        Applied::NotSetUp => {
            Some("lokal isn't set up on this Mac yet — run 'lokal setup' to make these URLs work")
        }
        Applied::NotRunning => {
            Some("the background service isn't running — start it with 'lokal start'")
        }
    }
}

pub(super) fn print_notes(a: &Applied) {
    if let Some(n) = note_text(a) {
        out::note(n);
    }
}

pub(super) fn timing(a: &Applied, verb: &str, t: Instant) -> String {
    match a {
        Applied::Ready => format!("  ({verb} in {})", util::fmt_duration(t.elapsed())),
        _ => "  (saved)".into(),
    }
}

fn service_json(p: &Project, sub: &str, s: &Service) -> serde_json::Value {
    let fqdn = p.fqdn(sub);
    json!({
        "subdomain": sub,
        "fqdn": fqdn,
        "url": format!("https://{fqdn}"),
        "host": s.host,
        "port": s.port,
        "host_header": s.host_header.as_str(),
        "managed": s.managed,
    })
}

fn project_json(name: &str, p: &Project) -> serde_json::Value {
    json!({
        "name": name,
        "domain": p.domain,
        "services": p.services.iter().map(|(sub, s)| service_json(p, sub, s)).collect::<Vec<_>>(),
    })
}

fn list_json(st: &State) -> serde_json::Value {
    json!({
        "schema": 1,
        "current_project": st.current_project,
        "projects": st.projects.iter().map(|(n, p)| project_json(n, p)).collect::<Vec<_>>(),
    })
}

fn print_project(name: &str, p: &Project, current: bool) {
    let cur = if current {
        format!("  {}", dim("(current)"))
    } else {
        String::new()
    };
    println!("{}  {}{cur}", out::bold(name), p.domain);
    if p.services.is_empty() {
        println!("  {}", dim("(no services)"));
        return;
    }
    let rows: Vec<(String, String, &Service)> = p
        .services
        .iter()
        .map(|(sub, s)| (sub.clone(), format!("https://{}", p.fqdn(sub)), s))
        .collect();
    let w1 = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let w2 = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
    for (sub, url, s) in rows {
        let by = if s.is_managed() {
            format!("  {}", dim(&format!("({MANAGED_BY_FILE})")))
        } else {
            String::new()
        };
        println!("  {sub:<w1$}  {url:<w2$}  → {}{by}", s.upstream());
    }
}

/// The note for changing a service by hand that `lokal up` manages: the
/// next `up` puts the file's version back (SPEC §4.11).
fn managed_note(sub: &str, s: &Service, next_up: &str) {
    if s.is_managed() {
        out::note(&format!(
            "{MANAGED_BY_FILE} manages {sub}; the next lokal up {next_up}"
        ));
    }
}

// ───────────────────────────── projects ─────────────────────────────

/// An existing project asked for with another domain (SPEC §4.3; also
/// `lokal up`, §4.11). The caller adds the hint.
pub(super) fn domain_mismatch(name: &str, have: &str, want: &str) -> UserError {
    UserError::usage(format!(
        "project `{name}` already exists with domain {have}, not {want}"
    ))
}

pub fn project_create(raw: &str, domain: Option<&str>) -> Result<i32> {
    let t = Instant::now();
    let paths = Paths::resolve()?;
    let name = validate::project_name(raw)?;
    let apex = validate::apex(&name, domain)?;
    // Re-running with the same arguments is fine: it just makes the project current.
    let existed = State::update(&paths, |st| {
        if let Some(p) = st.projects.get(&name) {
            if domain.is_some() && p.domain != apex {
                bail!(domain_mismatch(&name, &p.domain, &apex).hint(format!(
                    "use a different project name, or remove it first: lokal project remove {name}"
                )));
            }
            let d = p.domain.clone();
            st.current_project = Some(name.clone());
            return Ok(Some(d));
        }
        validate::check_overlap(st, &apex, None)?;
        st.projects.insert(
            name.clone(),
            Project {
                domain: apex.clone(),
                created_at: util::now_rfc3339(),
                services: Default::default(),
                extra: Default::default(),
            },
        );
        st.current_project = Some(name.clone());
        Ok(None)
    })?;
    let a = apply(&paths, &[], &[])?;
    if let Some(d) = existed {
        println!("{} {d} already exists (current project)", ok_mark());
        print_notes(&a);
        return Ok(0);
    }
    println!(
        "{} Created {apex} (current project){}",
        ok_mark(),
        dim(&timing(&a, "done", t))
    );
    println!("  Next: lokal service add api 4000");
    print_notes(&a);
    Ok(0)
}

pub fn project_list(json: bool) -> Result<i32> {
    let paths = Paths::resolve()?;
    let st = State::load(&paths)?;
    if json {
        out::print_json(&list_json(&st));
        return Ok(0);
    }
    if st.projects.is_empty() {
        println!("No projects yet. Create one: lokal project create myproject");
        return Ok(0);
    }
    for (n, p) in &st.projects {
        let cur = if st.current_project.as_deref() == Some(n) {
            format!("  {}", dim("(current)"))
        } else {
            String::new()
        };
        println!("{n}  {}  {} service(s){cur}", p.domain, p.services.len());
    }
    Ok(0)
}

pub fn project_show(name: Option<&str>, json: bool) -> Result<i32> {
    let paths = Paths::resolve()?;
    let st = State::load(&paths)?;
    let name = st.resolve_project(name)?;
    let p = &st.projects[&name];
    if json {
        out::print_json(&json!({
            "schema": 1,
            "current": st.current_project.as_deref() == Some(name.as_str()),
            "project": project_json(&name, p),
        }));
    } else {
        print_project(
            &name,
            p,
            st.current_project.as_deref() == Some(name.as_str()),
        );
    }
    Ok(0)
}

pub fn project_use(raw: &str) -> Result<i32> {
    let paths = Paths::resolve()?;
    let name = validate::normalize(raw);
    let domain = State::update(&paths, |st| {
        let Some(p) = st.projects.get(&name) else {
            bail!(no_such_project(&name));
        };
        let d = p.domain.clone();
        st.current_project = Some(name.clone());
        Ok(d)
    })?;
    println!("{} Current project: {name} ({domain})", ok_mark());
    Ok(0)
}

pub fn project_remove(raw: &str, force: bool) -> Result<i32> {
    let t = Instant::now();
    let paths = Paths::resolve()?;
    let name = validate::normalize(raw);
    let (domain, gone) = State::update(&paths, |st| {
        let Some(p) = st.projects.get(&name) else {
            bail!(no_such_project(&name));
        };
        if !p.services.is_empty() && !force {
            bail!(
                UserError::runtime(format!(
                    "project `{name}` still has {} service(s)",
                    p.services.len()
                ))
                .hint(format!(
                    "remove them first, or run 'lokal project remove {name} --force'"
                ))
            );
        }
        let gone: Vec<String> = p.services.keys().map(|s| p.fqdn(s)).collect();
        let domain = p.domain.clone();
        st.projects.remove(&name);
        if st.current_project.as_deref() == Some(name.as_str()) {
            st.current_project = None;
        }
        Ok((domain, gone))
    })?;
    let a = apply(&paths, &[], &gone)?;
    let extra = if gone.is_empty() {
        String::new()
    } else {
        format!(" and {} service(s)", gone.len())
    };
    println!(
        "{} Removed {domain}{extra}{}",
        ok_mark(),
        dim(&timing(&a, "done", t))
    );
    print_notes(&a);
    Ok(0)
}

// ───────────────────────────── services ─────────────────────────────

fn upstream_note(host: &str, port: u16) {
    if !crate::net::probe_tcp(host, port, Duration::from_millis(50)) {
        let up = Service {
            host: host.into(),
            port,
            host_header: HostHeader::Preserve,
            managed: None,
            extra: Default::default(),
        }
        .upstream();
        out::note(&format!("nothing is listening on {up} yet"));
    }
}

/// `localhost:4000`, or `localhost:4000, host header upstream`.
pub(super) fn describe(s: &Service) -> String {
    match s.host_header {
        HostHeader::Preserve => s.upstream(),
        HostHeader::Upstream => format!("{}, host header upstream", s.upstream()),
    }
}

/// The `service update` command that turns `have` into `want`.
pub(super) fn update_command(sub: &str, have: &Service, want: &Service) -> String {
    let mut cmd = format!("lokal service update {sub}");
    if want.port != have.port {
        cmd.push_str(&format!(" --port {}", want.port));
    }
    if want.host != have.host {
        cmd.push_str(&format!(" --host {}", want.host));
    }
    if want.host_header != have.host_header {
        cmd.push_str(&format!(" --host-header {}", want.host_header.as_str()));
    }
    cmd
}

pub fn service_add(
    raw_sub: &str,
    port: u32,
    raw_host: &str,
    hh: HostHeader,
    project: Option<&str>,
) -> Result<i32> {
    let t = Instant::now();
    let paths = Paths::resolve()?;
    let (fqdn, host, port, warn, existed) = State::update(&paths, |st| {
        let pname = st.resolve_project(project)?;
        let p = st
            .projects
            .get_mut(&pname)
            .expect("resolved project exists");
        let sub = validate::subdomain(raw_sub, &p.domain)?;
        let (host, warn) = validate::host(raw_host)?;
        let port = validate::port(port, &host)?;
        let want = Service {
            host: host.clone(),
            port,
            host_header: hh,
            managed: None,
            extra: Default::default(),
        };
        // Re-adding an identical service is fine (scripts and agents re-run
        // setup commands); anything different must go through `update`. The
        // route is what counts: a service `lokal up` added keeps its marker.
        if let Some(existing) = p.services.get(&sub) {
            if existing.same_route(&want) {
                return Ok((p.fqdn(&sub), host, port, warn, true));
            }
            bail!(UserError::usage(format!(
                "service `{sub}` already exists ({}); use `{}`",
                describe(existing),
                update_command(&sub, existing, &want)
            )));
        }
        p.services.insert(sub.clone(), want);
        Ok((p.fqdn(&sub), host, port, warn, false))
    })?;
    let a = apply(&paths, std::slice::from_ref(&fqdn), &[])?;
    let up = Service {
        host: host.clone(),
        port,
        host_header: hh,
        managed: None,
        extra: Default::default(),
    }
    .upstream();
    let note = match (&a, existed) {
        (Applied::Ready, true) => timing(&a, "already configured, ready", t),
        (_, true) => "  (already configured)".into(),
        _ => timing(&a, "ready", t),
    };
    println!("{} {fqdn} → {up}{note}", ok_mark());
    println!("  https://{fqdn}");
    if let Some(w) = warn {
        println!("  {} {w}", out::yellow("warning:"));
    }
    upstream_note(&host, port);
    print_notes(&a);
    Ok(0)
}

pub fn service_update(
    raw_sub: &str,
    port: Option<u32>,
    raw_host: Option<&str>,
    hh: Option<HostHeader>,
    project: Option<&str>,
) -> Result<i32> {
    let t = Instant::now();
    if port.is_none() && raw_host.is_none() && hh.is_none() {
        bail!(UserError::usage("nothing to change").hint("pass --port, --host or --host-header"));
    }
    let paths = Paths::resolve()?;
    let (sub, fqdn, svc, warn) = State::update(&paths, |st| {
        let pname = st.resolve_project(project)?;
        let p = st
            .projects
            .get_mut(&pname)
            .expect("resolved project exists");
        let sub = validate::subdomain(raw_sub, &p.domain)?;
        let fqdn = p.fqdn(&sub);
        let Some(s) = p.services.get(&sub).cloned() else {
            bail!(
                UserError::usage(format!("no service `{sub}` in project `{pname}`"))
                    .hint(format!("add it with 'lokal service add {sub} <PORT>'"))
            );
        };
        let mut next = s.clone();
        let mut warn = None;
        if let Some(h) = raw_host {
            let (h, w) = validate::host(h)?;
            next.host = h;
            warn = w;
        }
        let port_to_check = port.unwrap_or(next.port as u32);
        next.port = validate::port(port_to_check, &next.host)?;
        if let Some(h) = hh {
            next.host_header = h;
        }
        p.services.insert(sub.clone(), next.clone());
        Ok((sub, fqdn, next, warn))
    })?;
    let a = apply(&paths, std::slice::from_ref(&fqdn), &[])?;
    println!(
        "{} {fqdn} → {}{}{}",
        ok_mark(),
        svc.upstream(),
        if svc.host_header == HostHeader::Upstream {
            "  (host header: upstream)"
        } else {
            ""
        },
        timing(&a, "ready", t)
    );
    if let Some(w) = warn {
        println!("  {} {w}", out::yellow("warning:"));
    }
    upstream_note(&svc.host, svc.port);
    managed_note(&sub, &svc, "sets it back");
    print_notes(&a);
    Ok(0)
}

pub fn service_remove(raw_sub: &str, project: Option<&str>) -> Result<i32> {
    let t = Instant::now();
    let paths = Paths::resolve()?;
    let (sub, fqdn, removed) = State::update(&paths, |st| {
        let pname = st.resolve_project(project)?;
        let p = st
            .projects
            .get_mut(&pname)
            .expect("resolved project exists");
        let sub = validate::subdomain(raw_sub, &p.domain)?;
        let Some(removed) = p.services.remove(&sub) else {
            bail!(UserError::usage(format!(
                "no service `{sub}` in project `{pname}`"
            )));
        };
        Ok((sub.clone(), p.fqdn(&sub), removed))
    })?;
    let a = apply(&paths, &[], std::slice::from_ref(&fqdn))?;
    println!(
        "{} Removed {fqdn}{}",
        ok_mark(),
        dim(&timing(&a, "done", t))
    );
    managed_note(&sub, &removed, "adds it back");
    print_notes(&a);
    Ok(0)
}

pub fn service_list(project: Option<&str>, json: bool) -> Result<i32> {
    let paths = Paths::resolve()?;
    let st = State::load(&paths)?;
    let name = st.resolve_project(project)?;
    let p = &st.projects[&name];
    if json {
        out::print_json(&json!({
            "schema": 1,
            "project": name,
            "domain": p.domain,
            "services": p.services.iter().map(|(sub, s)| service_json(p, sub, s)).collect::<Vec<_>>(),
        }));
    } else {
        print_project(
            &name,
            p,
            st.current_project.as_deref() == Some(name.as_str()),
        );
    }
    Ok(0)
}

pub fn list(json: bool) -> Result<i32> {
    let paths = Paths::resolve()?;
    let st = State::load(&paths)?;
    if json {
        out::print_json(&list_json(&st));
        return Ok(0);
    }
    if st.projects.is_empty() {
        println!("No projects yet. Create one: lokal project create myproject");
        return Ok(0);
    }
    for (n, p) in &st.projects {
        print_project(n, p, st.current_project.as_deref() == Some(n.as_str()));
    }
    Ok(0)
}
