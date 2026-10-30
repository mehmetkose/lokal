//! `status`, `logs` (the background service's, and `lokal up` services'),
//! `env`, `ca path`, `open`, and the no-argument welcome.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};

use super::args::Shell;
use super::output::{self as out, dim, ok_mark};
use crate::ca::CaCert;
use crate::error::UserError;
use crate::ipc::{self, IpcError};
use crate::paths::Paths;
use crate::platform;
use crate::project_file;
use crate::setup;
use crate::state::{State, validate};

pub fn no_args() -> Result<i32> {
    let paths = Paths::resolve()?;
    if !paths.sandbox && !setup::is_set_up(&paths) {
        println!(
            "lokal gives every service you run in development its own https://<name>.lokal URL with a valid certificate."
        );
        println!("Get started: lokal setup\n");
    }
    println!("{}", super::help::root_help_text(&super::help::command()));
    Ok(0)
}

pub fn status(json: bool) -> Result<i32> {
    let paths = Paths::resolve()?;
    let setup_done = paths.sandbox || setup::is_set_up(&paths);
    let st: Option<ipc::Status> =
        match ipc::call(&paths, &ipc::Request::Status, Duration::from_secs(2)) {
            Ok(s) => Some(s),
            Err(IpcError::NotRunning) => None,
            Err(e) => bail!(UserError::runtime(format!(
                "cannot reach the background service: {e}"
            ))),
        };
    if json {
        let v = match &st {
            Some(s) => serde_json::to_value(s)?,
            None => serde_json::to_value(ipc::Status {
                schema: 1,
                setup_done,
                running: false,
                pid: None,
                version: None,
                build_id: None,
                uptime_secs: None,
                listeners: vec![],
                listener_source: None,
                name_backend: None,
                names: vec![],
                certs_cached: 0,
            })?,
        };
        out::print_json(&v);
        return Ok(0);
    }
    let Some(s) = st else {
        if setup_done {
            println!(
                "{} background service is not running — start it with 'lokal start'",
                out::fail_mark()
            );
        } else {
            println!(
                "{} lokal isn't set up on {} yet — run 'lokal setup'",
                out::fail_mark(),
                platform::THIS_MACHINE
            );
        }
        return Ok(0);
    };
    let build = s.build_id.clone().unwrap_or_default();
    println!(
        "{} running (pid {}, lokal {}, up {}s)",
        out::green("●"),
        s.pid.unwrap_or(0),
        build,
        s.uptime_secs.unwrap_or(0)
    );
    if build != crate::version::build_id() {
        println!(
            "  {} the background service runs a different lokal build — run 'lokal upgrade'",
            out::yellow("warning:")
        );
    }
    println!(
        "  listeners: {} ({})",
        s.listeners.join(", "),
        s.listener_source.clone().unwrap_or_default()
    );
    let registered = s.names.iter().filter(|n| n.state == "registered").count();
    println!(
        "  names:     {registered}/{} registered ({})",
        s.names.len(),
        s.name_backend.clone().unwrap_or_default()
    );
    let w = s.names.iter().map(|n| n.fqdn.len()).max().unwrap_or(0);
    for n in &s.names {
        let state = if n.state == "registered" {
            dim(&n.state)
        } else {
            out::red(&n.state)
        };
        println!("    {:<w$}  {state}", n.fqdn);
    }
    println!("  certificates cached: {}", s.certs_cached);
    Ok(0)
}

/// `lokal logs [SUB [-p <PROJECT>]] [-f] [-n <LINES>]` (SPEC §8.3, §4.11).
/// Without SUB, the background service's `daemon.log`, as before.
pub fn logs(sub: Option<&str>, project: Option<&str>, follow: bool, n: usize) -> Result<i32> {
    let paths = Paths::resolve()?;
    if let Some(sub) = sub {
        return service_logs(&paths, sub, project, follow, n);
    }
    let path = &paths.daemon_log;
    if !path.exists() {
        if !paths.sandbox && !setup::is_set_up(&paths) {
            bail!(UserError::not_set_up());
        }
        println!("{}", dim(&format!("(no log yet at {})", path.display())));
        if !follow {
            return Ok(0);
        }
    }
    for l in crate::util::tail_lines(path, n) {
        println!("{l}");
    }
    if !follow {
        return Ok(0);
    }
    follow_file(path)
}

/// The project of a `lokal up` service that `lokal logs <SUB>` and `lokal
/// restart <SUB>` name (SPEC §4.11), and the apex SUB is normalised
/// against: `-p`, else the `lokal.toml` found from here, else §4.4. An
/// agent in one repo thus reaches that repo's services, not those of
/// whichever project ran `up` last. The apex is the project's domain, or
/// the file's apex once the project is gone from state.
pub(crate) fn sub_project(st: &State, project: Option<&str>) -> Result<(String, String)> {
    let (project, file_apex) = match project {
        Some(p) => {
            let p = validate::normalize(p);
            // Not a name any project can have (nor a folder of logs).
            if !validate::is_label(&p) {
                bail!(
                    UserError::usage(format!("`{p}` isn't a project name"))
                        .hint("pass a project's name, as 'lokal project list' shows it")
                );
            }
            (p, None)
        }
        None => {
            let cwd = std::env::current_dir().context("cannot read the current directory")?;
            match Paths::find_project_file(&cwd) {
                // Only the name (and apex): a file with other errors still
                // names its project.
                Some(file) => {
                    let named = project_file::load_name(&file)?;
                    (named.project, Some(named.apex))
                }
                None => (st.resolve_project(None)?, None),
            }
        }
    };
    let apex = match (st.projects.get(&project), file_apex) {
        (Some(p), _) => p.domain.clone(),
        (None, Some(apex)) => apex,
        (None, None) => validate::apex(&project, None)?,
    };
    Ok((project, apex))
}

/// The log of a service that `lokal up` started (SPEC §4.11, "Logs"): the
/// last `n` lines across `<sub>.log.1` and `<sub>.log`, then, with
/// `follow`, what gets added. It needs no setup, since `up` doesn't either.
fn service_logs(
    paths: &Paths,
    raw_sub: &str,
    project: Option<&str>,
    follow: bool,
    n: usize,
) -> Result<i32> {
    let st = State::load(paths)?;
    let (project, apex) = sub_project(&st, project)?;
    let known = st.projects.get(&project);
    let sub = validate::subdomain(raw_sub, &apex)?;
    let log = paths.up_log(&project, &sub);
    let rotated = paths.up_log_rotated(&project, &sub);
    // A log stays readable after its service is gone (after `down`).
    if !log.exists() && !rotated.exists() {
        match known {
            Some(p) if p.services.contains_key(&sub) => bail!(
                UserError::runtime(format!(
                    "no log for {sub} yet: only services that lokal up starts have one"
                ))
                .hint("services started with lokal run or by hand print to their own terminal")
            ),
            Some(_) => bail!(
                UserError::usage(format!("no service {sub} in project {project}")).hint(format!(
                    "run 'lokal service list -p {project}' to see its services"
                ))
            ),
            None => bail!(
                UserError::usage(format!("no service {sub} in project {project}")).hint(format!(
                    "there is no project {project}: run 'lokal project list' to see your projects"
                ))
            ),
        }
    }
    for l in crate::util::tail_lines_across(&[&rotated, &log], n) {
        println!("{l}");
    }
    if !follow {
        return Ok(0);
    }
    follow_file(&log)
}

/// Prints what gets added to `path`, until lokal is stopped. When the file
/// gets shorter (truncated, or replaced after a rotation), it starts again
/// from the top. Lines written in the last moments before a rotation can be
/// missed.
fn follow_file(path: &Path) -> Result<i32> {
    let mut pos = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut stdout = std::io::stdout();
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        if meta.len() < pos {
            pos = 0; // rotated or truncated
        }
        if meta.len() > pos
            && let Ok(mut f) = std::fs::File::open(path)
        {
            f.seek(SeekFrom::Start(pos))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            pos += buf.len() as u64;
            stdout.write_all(&buf)?;
            stdout.flush()?;
        }
    }
}

const BUNDLE_MAX_AGE: Duration = Duration::from_secs(30 * 86_400);

/// `env` and `ca path` hand out lokal's CA as a trust anchor, so they need
/// setup in place, not just the CA files: a plain `lokal uninstall` keeps
/// those, and the `eval "$(lokal env)"` profile line must then export nothing.
fn require_ca_in_use(paths: &Paths) -> Result<()> {
    if !CaCert::exists(paths) || !(paths.sandbox || setup::is_set_up(paths)) {
        bail!(UserError::not_set_up());
    }
    Ok(())
}

pub fn env(shell: Shell) -> Result<i32> {
    let paths = Paths::resolve()?;
    require_ca_in_use(&paths)?;
    let stale = std::fs::metadata(&paths.ca_bundle)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_none_or(|age| age > BUNDLE_MAX_AGE);
    if stale {
        let ca = CaCert::load(&paths)?;
        setup::write_bundle(&paths, &platform::current(), &ca)?;
    }
    let ca = paths.ca_cert.display().to_string();
    let bundle = paths.ca_bundle.display().to_string();
    let q = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
    let vars = [
        ("NODE_EXTRA_CA_CERTS", &ca),
        ("SSL_CERT_FILE", &bundle),
        ("REQUESTS_CA_BUNDLE", &bundle),
        ("CURL_CA_BUNDLE", &bundle),
    ];
    for (k, v) in vars {
        match shell {
            Shell::Sh => println!("export {k}={}", q(v)),
            Shell::Fish => println!("set -gx {k} {}", q(v)),
        }
    }
    Ok(0)
}

pub fn ca_path() -> Result<i32> {
    let paths = Paths::resolve()?;
    require_ca_in_use(&paths)?;
    println!("{}", paths.ca_cert.display());
    Ok(0)
}

pub fn open(raw_sub: &str, project: Option<&str>) -> Result<i32> {
    let paths = Paths::resolve()?;
    let st = State::load(&paths)?;
    let name = st.resolve_project(project)?;
    let p = &st.projects[&name];
    let sub = validate::subdomain(raw_sub, &p.domain)?;
    if !p.services.contains_key(&sub) {
        bail!(
            UserError::usage(format!("no service `{sub}` in project `{name}`"))
                .hint("run 'lokal service list' to see the services")
        );
    }
    if !paths.sandbox && !setup::is_set_up(&paths) {
        bail!(UserError::not_set_up());
    }
    let url = format!("https://{}", p.fqdn(&sub));
    platform::current().desktop.open_url(&url)?;
    println!("{} opened {url}", ok_mark());
    Ok(0)
}

pub fn internal(cmd: super::args::InternalCmd) -> Result<i32> {
    use super::args::InternalCmd;
    let p = Paths::resolve()?;
    match cmd {
        InternalCmd::Paths => {}
        InternalCmd::CreateCa => {
            if !p.sandbox {
                bail!(UserError::usage(
                    "internal create-ca needs LOKAL_HOME (use 'lokal setup')"
                ));
            }
            let (user, host) = platform::current().system.user_and_host();
            let (ca, created) = CaCert::load_or_create(&p, &user, &host)?;
            println!(
                "{} {} ({})",
                if created { "created" } else { "exists" },
                ca.path.display(),
                ca.sha256
            );
            return Ok(0);
        }
        InternalCmd::IssueLeaf { fqdn } => {
            print!("{}", CaCert::load(&p)?.issuer()?.issue_pem(&fqdn)?);
            return Ok(0);
        }
        InternalCmd::TrustStatus => {
            let st = platform::current().trust.is_trusted(&CaCert::load(&p)?)?;
            println!("{st:?}");
            return Ok(if st == platform::TrustStatus::Trusted {
                0
            } else {
                1
            });
        }
    }
    out::print_json(&serde_json::json!({
        "base": p.base, "state": p.state, "ca_cert": p.ca_cert, "ca_key": p.ca_key,
        "ca_bundle": p.ca_bundle, "daemon_bin": p.daemon_bin, "socket": p.socket,
        "install_record": p.install_record, "trusted_dir": p.trusted_dir, "log_dir": p.log_dir,
        "daemon_log": p.daemon_log, "plist": p.plist, "sandbox": p.sandbox,
    }));
    Ok(0)
}
