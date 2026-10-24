//! `lokal run <SUBDOMAIN> -- <COMMAND>...` (SPEC §4.10): pick a port (or
//! reuse the service's), route it with one locked write and one reload that
//! waits until it's ready, then start the command with the terminal
//! attached and supervise it: the ready line, the wrong-port hint, signals
//! and the exit code. lokal's own lines go to stderr; the command owns
//! stdout.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tokio::time::MissedTickBehavior;

use super::output::{self as out, dim, ok_mark};
use super::projects::{self, Applied};
use crate::error::UserError;
use crate::paths::Paths;
use crate::platform::{self, RelaySignals};
use crate::state::{HostHeader, Service, State, validate};
use crate::{net, process, runner, util};

/// How often the port is probed until the command listens on it.
const PROBE_EVERY: Duration = Duration::from_millis(50);
const PROBE_TIMEOUT: Duration = Duration::from_millis(50);
/// The wrong-port hint looks at the command's listeners from this long
/// after launch, then every [`HINT_EVERY`], until the port is ready.
const HINT_AFTER: Duration = Duration::from_secs(3);
const HINT_EVERY: Duration = Duration::from_secs(2);

pub struct RunOpts {
    pub subdomain: String,
    pub port: Option<u32>,
    pub host_header: Option<HostHeader>,
    pub rm: bool,
    pub project: Option<String>,
    pub command: Vec<String>,
}

/// What the locked state write decided.
enum Route {
    /// Routed (added or reused): start the command.
    Go {
        project: String,
        sub: String,
        fqdn: String,
        svc: Service,
        created: bool,
        env: Vec<(String, String)>,
        warnings: Vec<String>,
    },
    /// Something already listens on the port. Nothing was written.
    Busy {
        sub: String,
        svc: Service,
        existed: bool,
    },
}

pub fn run(o: RunOpts) -> Result<i32> {
    let t = Instant::now();
    let paths = Paths::resolve()?;
    let route = State::update(&paths, |st| route(st, &o))?;
    let (project, sub, fqdn, svc, created, env, warnings) = match route {
        Route::Busy { sub, svc, existed } => bail!(busy_error(&sub, &svc, existed, &o)),
        Route::Go {
            project,
            sub,
            fqdn,
            svc,
            created,
            env,
            warnings,
        } => (project, sub, fqdn, svc, created, env, warnings),
    };
    let cleanup = |paths: &Paths| {
        if !o.rm {
            return;
        }
        if !created {
            out::enote(&format!("kept {fqdn}: it existed before this run"));
            return;
        }
        if let Err(e) = remove_created(paths, &project, &sub, &fqdn) {
            eprintln!("{} couldn't remove {fqdn}: {e:#}", out::yellow("warning:"));
        }
    };

    // The route is ready before the command starts.
    let applied = match projects::apply(&paths, std::slice::from_ref(&fqdn), &[]) {
        Ok(a) => a,
        Err(e) => {
            cleanup(&paths);
            return Err(e);
        }
    };
    let argv: Vec<String> = o
        .command
        .iter()
        .map(|a| runner::substitute_port(a, svc.port))
        .collect();
    eprintln!(
        "{} https://{fqdn} → {}  {}{}",
        ok_mark(),
        svc.upstream(),
        process::display_command(&argv[0], &argv[1..]),
        dim(&projects::timing(&applied, "route ready", t))
    );
    for w in &warnings {
        eprintln!("  {} {w}", out::yellow("warning:"));
    }
    if let Some(n) = projects::note_text(&applied) {
        out::enote(n);
    }

    let job = Job {
        argv,
        env,
        original: &o.command,
        sub: &sub,
        project_flag: o
            .project
            .as_ref()
            .map(|_| format!(" -p {project}"))
            .unwrap_or_default(),
        fqdn: &fqdn,
        svc: &svc,
        url_works: matches!(applied, Applied::Ready),
    };
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")
        .and_then(|rt| {
            let r = rt.block_on(supervise(&job));
            // An lsof for the hint may still be running; don't wait for it.
            rt.shutdown_background();
            r
        });
    cleanup(&paths);
    result
}

/// Inside the state lock: pick or reuse the port, refuse a busy one, and
/// add the service if it's new.
fn route(st: &mut State, o: &RunOpts) -> Result<Route> {
    let pname = st.resolve_project(o.project.as_deref())?;
    let taken: BTreeSet<u16> = st.services().iter().map(|e| e.service.port).collect();
    let p = st
        .projects
        .get_mut(&pname)
        .expect("resolved project exists");
    let sub = validate::subdomain(&o.subdomain, &p.domain)?;
    let fqdn = p.fqdn(&sub);
    let (svc, created) = match p.services.get(&sub) {
        Some(have) => {
            if !net::is_loopback_host(&have.host) {
                bail!(
                    UserError::usage(format!(
                        "service `{sub}` routes to {}, which isn't this Mac; `lokal run` starts its command here",
                        have.upstream()
                    ))
                    .hint(format!(
                        "use another subdomain, or point it here: lokal service update {sub} --host localhost"
                    ))
                );
            }
            // Reuse it as it is: a stable port across runs, and a route added
            // by hand gets its server started.
            let mut want = have.clone();
            if let Some(port) = o.port {
                want.port = validate::port(port, &have.host)?;
            }
            if let Some(hh) = o.host_header {
                want.host_header = hh;
            }
            if want != *have {
                bail!(
                    UserError::usage(format!(
                        "service `{sub}` already exists ({}); use `{}`",
                        projects::describe(have),
                        projects::update_command(&sub, have, &want)
                    ))
                    .hint(
                        "or leave the flag out: lokal run uses the service's own port and settings"
                    )
                );
            }
            (have.clone(), false)
        }
        None => {
            let port = match o.port {
                Some(port) => validate::port(port, "localhost")?,
                None => net::pick_port(&taken)?,
            };
            let svc = Service {
                host: "localhost".into(),
                port,
                host_header: o.host_header.unwrap_or_default(),
            };
            (svc, true)
        }
    };
    // Never move a port silently: a hand-picked port may be deliberate.
    if net::probe_tcp(&svc.host, svc.port, Duration::from_millis(50)) {
        return Ok(Route::Busy {
            sub,
            svc,
            existed: !created,
        });
    }
    if created {
        p.services.insert(sub.clone(), svc.clone());
    }
    let (env, warnings) = runner::child_env(&p.domain, &p.services, &sub, svc.port);
    Ok(Route::Go {
        project: pname,
        sub,
        fqdn,
        svc,
        created,
        env,
        warnings,
    })
}

fn busy_error(sub: &str, svc: &Service, existed: bool, o: &RunOpts) -> UserError {
    let by = platform::current()
        .system
        .port_owner(svc.port)
        .map(|owner| format!(" by {owner}"))
        .unwrap_or_default();
    let pflag = o
        .project
        .as_ref()
        .map(|p| format!(" -p {p}"))
        .unwrap_or_default();
    let hint = if existed {
        format!(
            "stop that server first, or move the route: lokal service update {sub} --port <PORT>{pflag} \
(or lokal service remove {sub}{pflag}, and lokal run picks a free port)"
        )
    } else {
        "stop that server first, pass another --port, or leave --port out and lokal run picks a free port"
            .to_string()
    };
    UserError::runtime(format!(
        "{} is already in use{by}; not starting the command",
        svc.upstream()
    ))
    .hint(hint)
}

/// `--rm`: removes the service this run added (one reload that waits until
/// it's gone). A service someone already removed is left alone.
fn remove_created(paths: &Paths, project: &str, sub: &str, fqdn: &str) -> Result<()> {
    let t = Instant::now();
    let removed = State::update(paths, |st| {
        Ok(st
            .projects
            .get_mut(project)
            .and_then(|p| p.services.remove(sub))
            .is_some())
    })?;
    if !removed {
        return Ok(());
    }
    let a = projects::apply(paths, &[], &[fqdn.to_string()])?;
    eprintln!(
        "{} Removed {fqdn}{}",
        ok_mark(),
        dim(&projects::timing(&a, "done", t))
    );
    if let Some(n) = projects::note_text(&a) {
        out::enote(n);
    }
    Ok(())
}

/// The command and what `supervise` needs to report on it.
struct Job<'a> {
    /// After `{port}` substitution.
    argv: Vec<String>,
    env: Vec<(String, String)>,
    /// As typed, for the hint's suggestions.
    original: &'a [String],
    sub: &'a str,
    /// ` -p <project>` if the user passed one, for the hint's suggestions.
    project_flag: String,
    fqdn: &'a str,
    svc: &'a Service,
    /// The daemon took the route, so the URL works once the port does.
    url_works: bool,
}

/// Starts the command and waits for it, printing the ready line and the
/// wrong-port hint, and relaying signals. Returns the command's exit code
/// as a shell reports it.
async fn supervise(job: &Job<'_>) -> Result<i32> {
    // Before the command starts, so lokal survives a signal from then on.
    let mut signals = RelaySignals::install().context("cannot catch signals")?;
    let started = Instant::now();
    let mut child = match process::spawn_inherited(&job.argv, &job.env) {
        Ok(c) => c,
        Err(e) => return spawn_error(&job.argv[0], e),
    };
    let pid = child.id().unwrap_or(0);
    let mut probe = tokio::time::interval(PROBE_EVERY);
    probe.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let hint_timer = tokio::time::sleep(HINT_AFTER);
    tokio::pin!(hint_timer);
    let mut listeners: Option<tokio::task::JoinHandle<Vec<u16>>> = None;
    let mut ready = false;
    let mut hinted = false;
    loop {
        tokio::select! {
            status = child.wait() => {
                let status = status.context("cannot wait for the command")?;
                return Ok(platform::shell_exit_code(&status));
            }
            sig = signals.recv() => {
                // The command shares lokal's process group, so the terminal
                // already sent it Ctrl-C or Ctrl-\ when lokal is in the
                // foreground; relaying those would deliver them twice.
                if !(sig.from_terminal_keys() && platform::in_terminal_foreground()) {
                    platform::send_signal(pid, sig);
                }
            }
            _ = probe.tick(), if !ready => {
                if net::probe_tcp_async(&job.svc.host, job.svc.port, PROBE_TIMEOUT).await {
                    ready = true;
                    listeners = None;
                    print_ready(job, started.elapsed());
                }
            }
            () = &mut hint_timer, if !ready && !hinted && listeners.is_none() && pid != 0 => {
                // The command and the processes it started (npm → node →
                // vite), on a blocking thread: ps and lsof take ~40ms.
                listeners = Some(tokio::task::spawn_blocking(move || {
                    platform::current().system.tcp_listen_ports(pid)
                }));
                hint_timer.as_mut().reset(tokio::time::Instant::now() + HINT_EVERY);
            }
            ports = async { listeners.as_mut().expect("guarded by is_some").await },
                if listeners.is_some() =>
            {
                listeners = None;
                let ports = ports.unwrap_or_default();
                // Listening on nothing yet (still compiling) says nothing.
                if !ready && !ports.is_empty() && !ports.contains(&job.svc.port) {
                    hinted = true;
                    print_hint(job, &ports);
                }
            }
        }
    }
}

fn print_ready(job: &Job<'_>, took: Duration) {
    if job.url_works {
        eprintln!(
            "{} ready: https://{} {}",
            ok_mark(),
            job.fqdn,
            dim(&format!("({})", util::fmt_duration(took)))
        );
    } else {
        eprintln!("{} listening on {}", ok_mark(), job.svc.upstream());
    }
}

fn print_hint(job: &Job<'_>, ports: &[u16]) {
    let cmd = process::display_command(&job.original[0], &job.original[1..]);
    let on = ports
        .iter()
        .map(|p| format!("localhost:{p}"))
        .collect::<Vec<_>>()
        .join(", ");
    let (sub, pflag) = (job.sub, &job.project_flag);
    let mut hint = format!(
        "{cmd} listens on {on}, not {}. Point the route there: lokal service update {sub} --port {}{pflag}",
        job.svc.port, ports[0]
    );
    if !job
        .original
        .iter()
        .any(|a| a.contains(runner::PORT_PLACEHOLDER))
    {
        hint.push_str(&format!(
            ", or pass the port: lokal run {sub}{pflag} -- {cmd} --port {}",
            runner::PORT_PLACEHOLDER
        ));
    }
    eprintln!("{} {hint}", out::yellow("hint:"));
}

/// A command that can't start exits like it would in a shell: 127 when it
/// isn't found, 126 when it can't be executed.
fn spawn_error(program: &str, e: std::io::Error) -> Result<i32> {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            eprintln!("{} command not found: {program}", out::red("error:"));
            Ok(127)
        }
        std::io::ErrorKind::PermissionDenied => {
            eprintln!(
                "{} cannot run {program}: permission denied",
                out::red("error:")
            );
            Ok(126)
        }
        _ => Err(anyhow!(e).context(format!("cannot run {program}"))),
    }
}
