//! The clap command tree (SPEC §4.1). The root help's command list is
//! generated from this tree at runtime (see help.rs).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::state::HostHeader;

#[derive(Debug, Parser)]
#[command(
    name = "lokal",
    version = crate::version::long_version(),
    about = "Local .lokal domains with valid HTTPS for every service you run in development.",
    max_term_width = 100
)]
pub struct Cli {
    /// Disable coloured output
    #[arg(long, global = true)]
    pub no_color: bool,

    /// More detail on stderr
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// One-time onboarding: trust lokal's certificate and start the background service
    #[command(
        visible_alias = "install",
        long_about = "One-time onboarding for this Mac. Creates lokal's certificate authority, trusts it in \
the macOS System keychain, sends .lokal lookups to lokal (/etc/resolver/lokal), installs the background \
service and runs a self-test. Trusting the certificate and installing the resolver file are the only \
steps that ask for your password.\n\nSafe to run again: it repairs whatever is missing and asks for your \
password only if one of those steps is missing.",
        after_help = "Examples:\n  lokal setup             # interactive\n  lokal setup --dry-run   # show the plan and the exact sudo commands, change nothing\n  lokal setup --yes       # don't ask for confirmation"
    )]
    Setup {
        /// Don't ask for confirmation
        #[arg(long, short = 'y')]
        yes: bool,
        /// Print the plan and the exact privileged commands, then exit
        #[arg(long)]
        dry_run: bool,
        /// Test only: everything except trusting the certificate
        #[arg(long, hide = true)]
        skip_trust: bool,
    },

    /// Undo setup (--purge also deletes projects and the certificate)
    #[command(
        after_help = "Examples:\n  lokal uninstall          # keeps your projects and the CA files\n  lokal uninstall --purge  # deletes everything"
    )]
    Uninstall {
        /// Also delete projects, the certificate authority and logs
        #[arg(long)]
        purge: bool,
        /// Don't ask for confirmation
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Check every layer and explain anything that's broken
    #[command(after_help = "Examples:\n  lokal doctor\n  lokal doctor --json")]
    Doctor {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },

    /// Manage projects (each owns a NAME.lokal domain)
    #[command(alias = "p", subcommand_required = true, arg_required_else_help = true)]
    Project {
        #[command(subcommand)]
        cmd: ProjectCmd,
    },

    /// Manage a project's services (subdomains)
    #[command(alias = "s", subcommand_required = true, arg_required_else_help = true)]
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },

    /// Pick a free port, route https://SUBDOMAIN.<project>.lokal to it, and start COMMAND
    #[command(
        long_about = "Pick a free port, route https://SUBDOMAIN.<project>.lokal to localhost:PORT, and start \
COMMAND on that port with your terminal attached. The route is ready before the command starts. lokal's own \
lines go to stderr; once the port accepts connections it prints `✓ ready: https://…`, the line scripts and \
coding agents can wait for.\n\n\
The command runs as given (never through a shell), with `{port}` in any argument replaced by the port. It \
gets PORT, HOST=127.0.0.1, LOKAL_URL (its own URL) and LOKAL_URL_<SUB> for every service in the project \
(`v1.api` → LOKAL_URL_V1_API, `@` → LOKAL_URL_APEX), so services can find each other.\n\n\
Dev servers: Next.js, Express and most Node servers read PORT (`lokal run web -- npm run dev`). Vite \
ignores it: pass `--port {port}`, and allow the name in vite.config (`server.allowedHosts`) or use \
`--host-header upstream`. Rails: `bin/rails server -p {port}`. Django: \
`python manage.py runserver 127.0.0.1:{port}`.\n\n\
If the service already exists, `run` keeps its port and settings (change them with `lokal service \
update`), and refuses to start if something else already listens on that port. Ctrl-C reaches the command \
as usual, and lokal exits with the command's exit code. The route stays afterwards: requests get lokal's \
502 page until the server runs again. --rm removes the service when the command exits, if this run added \
it.",
        after_help = "Examples:\n  lokal run web -- npm run dev                           # Next.js and others read PORT\n  lokal run web --host-header upstream -- vite --port {port}\n  lokal run api --port 4000 -- cargo run                 # a fixed port\n  lokal run docs --rm -- mkdocs serve -a 127.0.0.1:{port}"
    )]
    Run {
        /// Subdomain (`@` for the project's apex; `v1.api` is fine)
        subdomain: String,
        /// Port for a new service (default: a free one); an existing service keeps its own
        #[arg(long)]
        port: Option<u32>,
        /// For a new service: send the original Host (preserve) or localhost:PORT (upstream)
        #[arg(long, value_enum)]
        host_header: Option<HostHeaderArg>,
        /// Remove the service when the command exits, if this run added it
        #[arg(long)]
        rm: bool,
        #[command(flatten)]
        p: ProjectArg,
        /// The dev server's command and arguments, after `--`
        #[arg(last = true, required = true, value_name = "COMMAND")]
        command: Vec<String>,
    },

    /// Route every service in lokal.toml, then start their commands
    #[command(
        long_about = "Read lokal.toml from this directory or the nearest parent that has one, route every \
service in it, then start each service's `run` command and show their output, each line as `name | line`. \
One state change routes https://SUB.<project>.lokal to localhost:PORT for every service, and it is ready \
before any command starts. A service with `port` keeps it; one without keeps the port it got last time, or \
gets a free one. Its command gets the port as `{port}` and PORT, plus HOST, LOKAL_URL and LOKAL_URL_<SUB> for \
every service, as with `lokal run`, and runs with /bin/sh in its `cwd`. The project (`project` in the file, \
or the directory name) is created if needed and becomes the current project. Services that left the file \
are removed; services you added by hand stay. One with the same name as a file service is adopted when its \
route is the one the file gives it (localhost, the same port and host-header mode), so `lokal down` removes \
it from then on; with another route, it is an error that names the difference. \
With SUBs, only those services start; every service is routed.\n\n\
Before anything changes, lokal lists the file's commands and asks a person to approve them: once per file, \
and again whenever the file changes. Without a terminal to ask on, it changes nothing and exits 1. --yes \
runs them without asking and remembers nothing.\n\n\
Once every service accepts connections (or has exited), lokal prints `✓ ready: N/N services accepting \
connections`, the line scripts and coding agents can wait for, and names the services that aren't ready after \
it. A server that listens on another port gets a `hint:` naming it, and one that fails with EADDRINUSE on a \
port lokal chose is retried once on a new port. A command that exits 0 and leaves its server running in a \
session of its own (as `astro dev` does under a coding agent) is named with that server, which lokal can't \
show or stop. It runs in the foreground until the commands exit. Ctrl-C stops them all (SIGINT, SIGTERM at 5s, SIGKILL at 10s, or at once on a second \
Ctrl-C) and keeps the routes: `lokal down` removes them. One `lokal up` runs a project at a time.",
        after_help = "Examples:\n  lokal up                    # every service in lokal.toml\n  lokal up api web            # route all, start only api and web\n  lokal up --yes              # agents and CI; nothing is remembered\n  lokal down                  # remove the routes lokal up added\n  lokal down --remove-project # and the project, if nothing else is left in it\n\n\
lokal.toml:\n  project = \"shop\"\n  [services]\n  \"@\" = { run = \"npm start\", port = 3000 }\n  api = { run = \"cargo run -p api\", cwd = \"services/api\" }\n  web = { run = \"vite --port {port}\", host_header = \"upstream\" }\n  docs = \"mkdocs serve -a 127.0.0.1:{port}\""
    )]
    Up {
        /// Services to start (default: all); every service is routed either way
        #[arg(value_name = "SUB")]
        subs: Vec<String>,
        /// Run the commands without asking; nothing is remembered
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Remove the services that lokal up added
    #[command(
        long_about = "Remove the services that lokal up added or adopted in the project of the lokal.toml in this \
directory or a parent (or the project named with -p). Services you added by hand stay, unless lokal up \
adopted them. With nothing to \
remove, it says so and exits 0. While lokal up runs the project, down changes nothing and exits 1: stop it \
first.",
        after_help = "Examples:\n  lokal down\n  lokal down --remove-project   # also the project, if nothing else is left in it\n  lokal down -p shop"
    )]
    Down {
        /// Also remove the project, if nothing else is left in it
        #[arg(long)]
        remove_project: bool,
        /// Project (default: the one in lokal.toml)
        #[arg(short, long)]
        project: Option<String>,
    },

    /// Show every project with its services and URLs
    #[command(
        long_about = "Show every project with its services, URLs and upstreams. `--json` is a stable, \
machine-readable contract (schema 1) for scripts and agents.",
        visible_alias = "ls",
        after_help = "Examples:\n  lokal list\n  lokal list --json"
    )]
    List {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },

    /// Start the background service
    #[command(after_help = "Examples:\n  lokal start")]
    Start,
    /// Stop the background service (it comes back at next login)
    #[command(after_help = "Examples:\n  lokal stop")]
    Stop,
    /// Restart the background service
    #[command(after_help = "Examples:\n  lokal restart")]
    Restart,

    /// Show status, listeners and published names
    #[command(
        long_about = "Show whether the background service runs, its build, listeners and the state of every published \
name. `--json` includes `setup_done` and `running`; if setup isn't done, the user runs `lokal setup` once.",
        after_help = "Examples:\n  lokal status\n  lokal status --json"
    )]
    Status {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },

    /// Show logs (-f to follow)
    #[command(
        long_about = "Show the background service's log. With SUB, show instead the log of that service \
from the last lokal up that started it: its output from both streams, each line after a timestamp, and \
lokal's own lines about it (started, exited, stopping) marked `lokal:`. The project comes from -p, else \
from the lokal.toml in this directory or a parent, else the current project. A log stays readable after \
lokal down; the next lokal up that starts the service starts it afresh. Services started with lokal run \
or by hand have no log: they print to their own terminal.",
        after_help = "Examples:\n  lokal logs              # the background service\n  lokal logs -n 200\n  lokal logs -f\n  lokal logs api -n 50    # what lokal up's api service printed last\n  lokal logs web -f"
    )]
    Logs {
        /// Service whose `lokal up` log to show (default: the background service's log)
        #[arg(value_name = "SUB")]
        sub: Option<String>,
        /// Project of SUB (default: the one in lokal.toml, else the current project)
        #[arg(short, long, requires = "sub")]
        project: Option<String>,
        /// Keep printing new lines
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
    },

    /// Make the background service use this lokal binary
    #[command(after_help = "Examples:\n  cargo build --release && ./target/release/lokal upgrade")]
    Upgrade,

    /// Print env vars so Node, Python, curl etc. trust lokal's certificate
    #[command(
        long_about = "Print shell exports so tools with their own CA bundle (Node, Python requests/urllib, curl with \
CURL_CA_BUNDLE) trust lokal's certificate. Browsers and macOS tools already do. The bundle keeps the \
system roots, so public HTTPS keeps working.",
        after_help = "Examples:\n  eval \"$(lokal env)\"\n  lokal env --shell fish | source"
    )]
    Env {
        /// Output syntax
        #[arg(long, value_enum, default_value_t = Shell::Sh)]
        shell: Shell,
    },

    /// The certificate authority
    #[command(subcommand_required = true, arg_required_else_help = true)]
    Ca {
        #[command(subcommand)]
        cmd: CaCmd,
    },

    /// Open a service in your default browser
    #[command(after_help = "Examples:\n  lokal open api\n  lokal open web -p shop")]
    Open {
        /// Subdomain of the service (`@` for the apex)
        subdomain: String,
        /// Project (default: the current project)
        #[arg(short, long)]
        project: Option<String>,
    },

    #[command(hide = true)]
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCmd,
    },

    #[command(hide = true)]
    Internal {
        #[command(subcommand)]
        cmd: InternalCmd,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Shell {
    Sh,
    Fish,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum HostHeaderArg {
    Preserve,
    Upstream,
}

impl From<HostHeaderArg> for HostHeader {
    fn from(h: HostHeaderArg) -> Self {
        match h {
            HostHeaderArg::Preserve => HostHeader::Preserve,
            HostHeaderArg::Upstream => HostHeader::Upstream,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ProjectCmd {
    /// Create a project and its domain (NAME.lokal)
    #[command(
        visible_alias = "new",
        long_about = "Create a project: it owns NAME.lokal (or --domain NAME.local) and becomes the current project, so later `service` commands \
use it without -p. Use one project per repo (its directory name is a good NAME). Safe to re-run: if it \
already exists, it just becomes the current project (a different --domain is an error).",
        after_help = "Examples:\n  lokal project create myproject                     # myproject.lokal\n  lokal project create shop --domain shop.acme.lokal\n  lokal project create old --domain old.local         # .local (mDNS) also works"
    )]
    Create {
        /// Project name (letters, digits, '-')
        name: String,
        /// Full domain instead of NAME.lokal (must end in .lokal or .local)
        #[arg(long)]
        domain: Option<String>,
    },
    /// List all projects
    #[command(
        visible_alias = "ls",
        after_help = "Examples:\n  lokal project list\n  lokal project list --json"
    )]
    List {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Show a project's domain and services
    #[command(after_help = "Examples:\n  lokal project show\n  lokal project show shop --json")]
    Show {
        /// Project (default: the current project)
        name: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Set the current project
    #[command(
        visible_alias = "switch",
        after_help = "Examples:\n  lokal project use shop"
    )]
    Use {
        /// Project name
        name: String,
    },
    /// Delete a project
    #[command(
        visible_alias = "rm",
        after_help = "Examples:\n  lokal project remove shop\n  lokal project remove shop --force   # also removes its services"
    )]
    Remove {
        /// Project name
        name: String,
        /// Remove even if it still has services
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Args)]
pub struct ProjectArg {
    /// Project (default: the current project)
    #[arg(short, long)]
    pub project: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum ServiceCmd {
    /// Route https://SUBDOMAIN.<project>.lokal to localhost:PORT
    #[command(
        visible_alias = "new",
        long_about = "Route https://SUBDOMAIN.<project>.lokal to HOST:PORT (default localhost). \
The URL works as soon as the command returns. \
Safe to re-run: adding the same service again changes nothing; a different port, host or host-header \
mode fails and prints the `lokal service update` command to use.\n\n\
`localhost` upstreams work whether your dev server listens on 127.0.0.1 or ::1 \
(Node 17+ binds ::1 only, Python 127.0.0.1 only).\n\n\
Vite and other dev servers that check the Host header: either allow the name in the dev server \
(Vite: `server: { allowedHosts: ['.myproject.lokal'] }` in vite.config, or `allowedHosts: true`), \
or pass `--host-header upstream` so lokal sends `Host: localhost:PORT` upstream \
(the original host is still in X-Forwarded-Host).",
        after_help = "Examples:\n  lokal service add api 4000                     # https://api.<project>.lokal\n  lokal service add @ 3000                       # the apex: https://<project>.lokal\n  lokal service add v1.api 4001\n  lokal service add web 5173 --host-header upstream   # Vite without allowedHosts\n  lokal service add db-ui 8080 --host 127.0.0.1 -p shop"
    )]
    Add {
        /// Subdomain (`@` for the project's apex; `v1.api` is fine)
        subdomain: String,
        /// Upstream port
        port: u32,
        /// Upstream host
        #[arg(long, default_value = "localhost")]
        host: String,
        /// Send the original Host (preserve) or HOST:PORT (upstream)
        #[arg(long, value_enum, default_value_t = HostHeaderArg::Preserve)]
        host_header: HostHeaderArg,
        #[command(flatten)]
        p: ProjectArg,
    },
    /// List the current project's services
    #[command(
        visible_alias = "ls",
        after_help = "Examples:\n  lokal service list\n  lokal service list -p shop --json"
    )]
    List {
        #[command(flatten)]
        p: ProjectArg,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Change a service's port, host or host-header mode
    #[command(
        visible_alias = "set",
        after_help = "Examples:\n  lokal service update api --port 4001\n  lokal service update web --host-header upstream"
    )]
    Update {
        /// Subdomain of the service
        subdomain: String,
        /// New upstream port
        #[arg(long)]
        port: Option<u32>,
        /// New upstream host
        #[arg(long)]
        host: Option<String>,
        /// New host-header mode
        #[arg(long, value_enum)]
        host_header: Option<HostHeaderArg>,
        #[command(flatten)]
        p: ProjectArg,
    },
    /// Remove one or more services
    #[command(
        visible_alias = "rm",
        long_about = "Remove one or more services of a project in one change, and return once none of \
their names is served. If any of them doesn't exist, nothing is removed.",
        after_help = "Examples:\n  lokal service remove api\n  lokal service remove api web\n  lokal service remove api web -p shop"
    )]
    Remove {
        /// Subdomains of the services
        #[arg(value_name = "SUBDOMAIN", required = true)]
        subdomains: Vec<String>,
        #[command(flatten)]
        p: ProjectArg,
    },
}

#[derive(Debug, Subcommand)]
pub enum CaCmd {
    /// Print the path of the CA certificate
    #[command(
        after_help = "Examples:\n  lokal ca path\n  NODE_EXTRA_CA_CERTS=\"$(lokal ca path)\" node app.js"
    )]
    Path,
}

#[derive(Debug, Subcommand)]
pub enum DaemonCmd {
    /// Run the daemon (launchd starts this)
    Run {
        /// Base directory (sandbox)
        #[arg(long)]
        base_dir: Option<PathBuf>,
        /// Bind HTTPS directly on 127.0.0.1/::1 at this port instead of launchd sockets
        #[arg(long)]
        https_port: Option<u16>,
        /// Bind HTTP directly at this port
        #[arg(long)]
        http_port: Option<u16>,
        /// Bind the .lokal DNS server directly on 127.0.0.1 at this UDP port
        #[arg(long)]
        dns_port: Option<u16>,
        /// auto | mdns-local-only | none
        #[arg(long, default_value = "auto")]
        name_backend: String,
        /// Also log to stderr
        #[arg(long)]
        foreground: bool,
        /// Cap on client connections (tests; default: sized from the open-file limit)
        #[arg(long)]
        max_client_connections: Option<usize>,
        /// Cap on upstream connections (tests; default: sized from the open-file limit)
        #[arg(long)]
        max_upstream_connections: Option<usize>,
        /// Deadline before a connection's first request, in ms (tests; default 20000)
        #[arg(long)]
        pre_request_timeout_ms: Option<u64>,
        /// Deadline for an idle connection, in ms (tests; default 60000)
        #[arg(long)]
        idle_timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
pub enum InternalCmd {
    /// Print resolved paths as JSON
    Paths,
    /// Create the CA (sandbox/LOKAL_HOME only)
    CreateCa,
    /// Print a leaf certificate (PEM) for FQDN, signed by the CA
    IssueLeaf { fqdn: String },
    /// Print whether the OS trusts the CA
    TrustStatus,
}
