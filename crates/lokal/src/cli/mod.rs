//! CLI entry point: parse, dispatch, map errors to exit codes (SPEC §4.9).

pub mod args;
pub mod help;
pub mod output;
pub mod projects;
pub mod run;
pub mod system;
pub mod up;

use anyhow::Result;
use clap::FromArgMatches;

use args::{CaCmd, Cli, Cmd, DaemonCmd, ProjectCmd, ServiceCmd};

/// Runs the CLI and returns the process exit code.
pub fn main() -> i32 {
    let matches = match help::command().try_get_matches() {
        Ok(m) => m,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };
    if !matches!(cli.command, Some(Cmd::Daemon { .. })) {
        crate::platform::restore_default_sigpipe();
    }
    output::init(cli.no_color, cli.verbose);
    if cli.verbose > 0 && !matches!(cli.command, Some(Cmd::Daemon { .. })) {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(tracing_subscriber::EnvFilter::new("lokal=debug"))
            .with_target(false)
            .try_init();
    }
    match dispatch(cli) {
        Ok(code) => code,
        Err(e) => output::report_error(&e),
    }
}

fn dispatch(cli: Cli) -> Result<i32> {
    let Some(cmd) = cli.command else {
        return system::no_args();
    };
    match cmd {
        Cmd::Setup {
            yes,
            dry_run,
            skip_trust,
        } => crate::setup::setup(crate::setup::SetupOpts {
            yes,
            dry_run,
            skip_trust,
        }),
        Cmd::Uninstall { purge, yes } => crate::setup::uninstall(purge, yes),
        Cmd::Doctor { json } => crate::doctor::command(json),
        Cmd::Project { cmd } => match cmd {
            ProjectCmd::Create { name, domain } => {
                projects::project_create(&name, domain.as_deref())
            }
            ProjectCmd::List { json } => projects::project_list(json),
            ProjectCmd::Show { name, json } => projects::project_show(name.as_deref(), json),
            ProjectCmd::Use { name } => projects::project_use(&name),
            ProjectCmd::Remove { name, force } => projects::project_remove(&name, force),
        },
        Cmd::Service { cmd } => match cmd {
            ServiceCmd::Add {
                subdomain,
                port,
                host,
                host_header,
                p,
            } => projects::service_add(
                &subdomain,
                port,
                &host,
                host_header.into(),
                p.project.as_deref(),
            ),
            ServiceCmd::List { p, json } => projects::service_list(p.project.as_deref(), json),
            ServiceCmd::Update {
                subdomain,
                port,
                host,
                host_header,
                p,
            } => projects::service_update(
                &subdomain,
                port,
                host.as_deref(),
                host_header.map(Into::into),
                p.project.as_deref(),
            ),
            ServiceCmd::Remove { subdomain, p } => {
                projects::service_remove(&subdomain, p.project.as_deref())
            }
        },
        Cmd::Run {
            subdomain,
            port,
            host_header,
            rm,
            p,
            command,
        } => run::run(run::RunOpts {
            subdomain,
            port,
            host_header: host_header.map(Into::into),
            rm,
            project: p.project,
            command,
        }),
        Cmd::Up { subs, yes } => up::up(up::UpOpts { subs, yes }),
        Cmd::Down {
            remove_project,
            project,
        } => up::down(up::DownOpts {
            remove_project,
            project,
        }),
        Cmd::List { json } => projects::list(json),
        Cmd::Start => crate::setup::start(),
        Cmd::Stop => crate::setup::stop(),
        Cmd::Restart => crate::setup::restart(),
        Cmd::Status { json } => system::status(json),
        Cmd::Logs { follow, lines } => system::logs(follow, lines),
        Cmd::Upgrade => crate::setup::upgrade(),
        Cmd::Env { shell } => system::env(shell),
        Cmd::Ca { cmd: CaCmd::Path } => system::ca_path(),
        Cmd::Open { subdomain, project } => system::open(&subdomain, project.as_deref()),
        Cmd::Daemon {
            cmd:
                DaemonCmd::Run {
                    base_dir,
                    https_port,
                    http_port,
                    dns_port,
                    name_backend,
                    foreground,
                    max_client_connections,
                    max_upstream_connections,
                    pre_request_timeout_ms,
                    idle_timeout_ms,
                },
        } => crate::daemon::run(crate::daemon::Options {
            base_dir,
            https_port,
            http_port,
            dns_port,
            name_backend,
            foreground,
            max_client_connections,
            max_upstream_connections,
            pre_request_timeout: pre_request_timeout_ms.map(std::time::Duration::from_millis),
            idle_timeout: idle_timeout_ms.map(std::time::Duration::from_millis),
        }),
        Cmd::Internal { cmd } => system::internal(cmd),
    }
}
