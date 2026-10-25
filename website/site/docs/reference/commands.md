---
title: Command reference
description: Every lokal command and flag, with defaults, aliases, JSON output, exit codes and environment variables.
section: reference
order: 1
sidebarLabel: Commands
source: [docs/SPEC.md, crates/lokal/src/cli/args.rs, crates/lokal/src/cli/projects.rs, crates/lokal/src/cli/run.rs, crates/lokal/src/cli/up.rs, crates/lokal/src/project_file.rs, crates/lokal/src/cli/system.rs, crates/lokal/src/setup/mod.rs, crates/lokal/src/error.rs, docs/PACKAGING.md]
---

This page lists every lokal command, grouped the way `lokal --help` groups them. The binary documents itself
too: `lokal --help` lists every command, and `lokal help <command>` shows a command's flags and examples.

```console
$ lokal --help
$ lokal help service add
```

## Synopsis

```text
lokal setup [--yes] [--dry-run]                                    alias: install
lokal doctor [--json]
lokal uninstall [--purge] [--yes]

lokal project create <NAME> [--domain <DOMAIN>]                    alias: project new
lokal project list [--json]                                        alias: project ls
lokal project show [NAME] [--json]
lokal project use <NAME>                                           alias: project switch
lokal project remove <NAME> [--force]                              alias: project rm

lokal service add <SUBDOMAIN> <PORT> [--host <HOST>] [--host-header <MODE>] [-p <PROJECT>]
                                                                   alias: service new
lokal service list [-p <PROJECT>] [--json]                         alias: service ls
lokal service update <SUBDOMAIN> [--port <PORT>] [--host <HOST>] [--host-header <MODE>] [-p <PROJECT>]
                                                                   alias: service set
lokal service remove <SUBDOMAIN> [-p <PROJECT>]                    alias: service rm
lokal run <SUBDOMAIN> [--port <PORT>] [--host-header <MODE>] [--rm] [-p <PROJECT>] -- <COMMAND>...
lokal up [SUB]... [--yes]
lokal down [--remove-project] [-p <PROJECT>]
lokal list [--json]                                                alias: ls

lokal start
lokal stop
lokal restart
lokal status [--json]
lokal logs [-f] [-n <LINES>]
lokal upgrade

lokal env [--shell <sh|fish>]
lokal ca path
lokal open <SUBDOMAIN> [-p <PROJECT>]

lokal help [COMMAND...]
```

`p` is short for `project` and `s` for `service`, so `lokal s add api 4000` does the same as
`lokal service add api 4000`. These two aliases don't appear in the help output.

Running `lokal` with no arguments prints the root help. Before setup has run, it first prints a one-line welcome
and `Get started: lokal setup`.

## Global options

These work with every command, before or after it. The exception is `--version`, which works only on `lokal`
itself.

| Option | What it does |
|---|---|
| `-h`, `--help` | Print help. On a subcommand, `-h` prints a summary and `--help` the full text. Both end with examples |
| `-V`, `--version` | Print the version and build: `lokal <version> (<git hash> <build time>)` |
| `--no-color` | Turn off coloured output. `NO_COLOR` does the same, and colour is off anyway when stdout isn't a terminal |
| `-v`, `--verbose` | Print debug detail to stderr. Repeatable |

```console
$ lokal --version
lokal 0.1.0 (af13985 2026-10-20T23:26:47Z)
```

## Get started

### `lokal setup`

```text
lokal setup [--yes] [--dry-run]
```

Alias: `lokal install`.

One-time onboarding for this Mac. Setup creates lokal's certificate authority, trusts it in the macOS System
keychain, installs `/etc/resolver/lokal` so `.lokal` lookups go to lokal, installs the background service and runs
a self-test. Trusting the certificate and installing the resolver file are the only steps that ask for your
password.

| Flag | What it does |
|---|---|
| `-y`, `--yes` | Don't ask `Continue? [Y/n]`. Password prompts still appear |
| `--dry-run` | Print the plan and the exact privileged commands, then exit without changing anything |

- Setup is safe to re-run. It repairs whatever is missing, and asks for your password only if a privileged step
  is missing.
- Run it as yourself. It refuses to run as root.
- If it needs sudo but can't ask for a password (no terminal is attached, or `LOKAL_FORBID_SUDO=1` is set), it
  stops before it changes anything and exits with code 4.
- With no terminal and no `--yes`, it can't ask for confirmation and exits with code 2.

```console
$ lokal setup --dry-run      # show the plan and the exact sudo commands, change nothing
$ lokal setup
```

[Setting up lokal](/docs/setup) walks through each step and the password prompts.

### `lokal doctor`

```text
lokal doctor [--json]
```

Checks every layer at once and explains how to fix anything that's broken. On a healthy Mac it finishes in under
a second.

| Check | What it checks |
|---|---|
| `setup.done` | Setup has run: `install.json` and the LaunchAgent exist |
| `ca.files` | The CA files exist, and the key is mode 0600 and owned by you |
| `ca.trusted` | macOS trusts lokal's CA |
| `ca.trust_scope` | macOS trusts it for HTTPS only, not for email, code signing or anything else |
| `ca.scope` | The CA's name constraints are exactly `.lokal` and `.local`, with every IP address excluded |
| `daemon.running` | The background service answers |
| `daemon.build` | It runs the same build as this `lokal` |
| `listeners` | Port 443 accepts connections on `127.0.0.1` and `::1`. If the port is taken, it names the process |
| `selftest` | The full setup self-test: names, connection, TLS verified by macOS, and an HTTP request |
| `resolver.lokal` | `/etc/resolver/lokal` points at `127.0.0.1:53535`, and macOS uses it |
| `tld.lokal` | `.lokal` isn't a real top-level domain. Asks a DNS root server, and reports `skip` when you're offline |
| `firefox` | If Firefox is installed, whether it's set up to trust lokal |
| `dns.<fqdn>` | Each service's name resolves to `127.0.0.1` and `::1`. Warns if the lookup takes over 20ms |
| `dns.conflict.<fqdn>` | `/etc/hosts` has no entry that points the name somewhere else |
| `tls.<fqdn>` | A TLS handshake for the name verifies |
| `upstream.<fqdn>` | Something is listening on the service's port. Only a warning: your dev server may just be stopped |

Each check reports `pass`, `warn`, `fail` or `skip`. `lokal doctor` exits with code 1 if any check fails.
Warnings don't change the exit code. The `tld.lokal` query is the only network traffic lokal sends off your Mac.

```console
$ lokal doctor
$ lokal doctor --json
```

### `lokal uninstall`

```text
lokal uninstall [--purge] [--yes]
```

Undoes setup. It stops and removes the background service, untrusts and deletes lokal's CA from the System
keychain, removes `/etc/resolver/lokal` and removes lokal's Firefox setting. Untrusting the CA and removing the
resolver file ask for your password.

| Flag | What it does |
|---|---|
| `--purge` | Also delete your projects, the CA files and the logs |
| `-y`, `--yes` | Don't ask `Continue? [y/N]` |

A plain uninstall keeps `state.toml` and the CA files, so running `lokal setup` later restores everything as it
was. Uninstall succeeds, with warnings, if some pieces are already gone. Like setup, it refuses to run as root.
[Files and paths](/docs/reference/files-and-paths#what-uninstall-removes) lists exactly what each form removes.

```console
$ lokal uninstall            # keeps your projects and the CA files
$ lokal uninstall --purge    # deletes everything
```

## Projects

A project owns one domain, `<name>.lokal` by default. [Projects and services](/docs/guides/projects-and-services)
covers the naming rules in more depth.

### `lokal project create`

```text
lokal project create <NAME> [--domain <DOMAIN>]
```

Alias: `lokal project new`.

| Argument or flag | What it does |
|---|---|
| `<NAME>` | Project name: letters, digits and `-`, up to 63 characters. Names are lowercased |
| `--domain <DOMAIN>` | Use this domain instead of `<NAME>.lokal`. It must end in `.lokal` or `.local` |

The new project becomes the [current project](#how-the-project-is-chosen). A project's domain can't equal,
contain or sit under another project's domain.

`project create` is safe to re-run. If the project already exists, it just becomes the current project. Passing
a different `--domain` for an existing project is an error.

```console
$ lokal project create shop
✓ Created shop.lokal (current project)  (done in 6ms)
  Next: lokal service add api 4000
$ lokal project create store --domain store.acme.lokal
$ lokal project create legacy --domain legacy.local      # .local (mDNS) also works
```

### `lokal project list`

```text
lokal project list [--json]
```

Alias: `lokal project ls`.

Lists every project with its domain and number of services, and marks the current one. `--json` prints the same
document as [`lokal list --json`](#lokal-list).

```console
$ lokal project list
shop  shop.lokal  2 service(s)  (current)
```

### `lokal project show`

```text
lokal project show [NAME] [--json]
```

Shows a project's domain and its services. Without `NAME`, it picks the project the same way `service` commands
do (see [How the project is chosen](#how-the-project-is-chosen)).

```console
$ lokal project show
$ lokal project show shop --json
```

### `lokal project use`

```text
lokal project use <NAME>
```

Alias: `lokal project switch`.

Sets the current project. `service` commands, `project show` and `open` use it when you don't pass `-p`.

```console
$ lokal project use shop
✓ Current project: shop (shop.lokal)
```

### `lokal project remove`

```text
lokal project remove <NAME> [--force]
```

Alias: `lokal project rm`.

| Flag | What it does |
|---|---|
| `--force` | Remove the project even if it still has services, and remove those services too |

Without `--force`, removing a project that still has services fails. If you remove the current project, no
project is current afterwards. The command returns once the project's names are no longer served.

```console
$ lokal project remove shop
$ lokal project remove shop --force   # also removes its services
```

### How the project is chosen

`service` commands, `run`, `project show` and `open` work on one project. lokal picks it in this order:

1. the project you pass with `-p` (or `--project`);
2. the current project;
3. the only project, if exactly one exists.

Otherwise the command fails with `no project selected: run 'lokal project use <NAME>' or pass -p <NAME>`.

[`lokal up`](#lokal-up) takes its project from the repo's `lokal.toml` instead, and [`lokal down`](#lokal-down)
from `-p` or, without it, from `lokal.toml`.

## Services

A service is a name under a project's domain, routed to a port on your Mac.

### `lokal service add`

```text
lokal service add <SUBDOMAIN> <PORT> [--host <HOST>] [--host-header <MODE>] [-p <PROJECT>]
```

Alias: `lokal service new`.

Routes `https://<SUBDOMAIN>.<project domain>` to `<HOST>:<PORT>`. The URL works as soon as the command returns.

| Argument or flag | Default | What it does |
|---|---|---|
| `<SUBDOMAIN>` | | The name under the project's domain. `@` is the domain itself, and nested names such as `v1.api` work. A full name such as `api.shop.lokal` is accepted too |
| `<PORT>` | | Upstream port, from 1 to 65535. Ports 443 and 80 on a loopback host are rejected, because they're lokal itself |
| `--host <HOST>` | `localhost` | Upstream host name or IP address, without a scheme or port. lokal warns if it isn't a loopback address |
| `--host-header <MODE>` | `preserve` | `preserve` sends the original `Host` header upstream. `upstream` sends `Host: <HOST>:<PORT>`, for dev servers such as Vite that check it |
| `-p`, `--project <PROJECT>` | current project | The project to add the service to |

```console
$ lokal service add api 4000
✓ api.myapp.lokal → localhost:4000  (ready in 8ms)
  https://api.myapp.lokal
$ lokal service add @ 3000                              # https://myapp.lokal
$ lokal service add v1.api 4001
$ lokal service add web 5173 --host-header upstream     # Vite without allowedHosts
$ lokal service add db-ui 8080 --host 127.0.0.1 -p shop
```

- `service add` is safe to re-run. Adding an identical service changes nothing, still waits until the URL works,
  and prints `already configured, ready in …`. Adding an existing name with a different port, host or host-header
  mode fails, and the error names the `lokal service update` command to run instead.
- `localhost` upstreams work whether your dev server listens on `127.0.0.1` or only on `::1`.
- If nothing is listening on the port yet, the command still succeeds and adds a note, such as
  `note: nothing is listening on localhost:4000 yet`.

[Frameworks](/docs/guides/frameworks) covers Vite's `allowedHosts` and the `--host-header upstream` option.

### `lokal service list`

```text
lokal service list [-p <PROJECT>] [--json]
```

Alias: `lokal service ls`.

Lists one project's services, with their URLs and upstreams.

```console
$ lokal service list
$ lokal service list -p shop --json
```

### `lokal service update`

```text
lokal service update <SUBDOMAIN> [--port <PORT>] [--host <HOST>] [--host-header <MODE>] [-p <PROJECT>]
```

Alias: `lokal service set`.

Changes only what you pass. You must pass at least one of `--port`, `--host` and `--host-header`. Like
`service add`, it returns once the change is live.

| Flag | What it does |
|---|---|
| `--port <PORT>` | New upstream port |
| `--host <HOST>` | New upstream host |
| `--host-header <MODE>` | New host-header mode: `preserve` or `upstream` |
| `-p`, `--project <PROJECT>` | The project (default: the current project) |

```console
$ lokal service update api --port 4001
$ lokal service update web --host-header upstream
```

### `lokal service remove`

```text
lokal service remove <SUBDOMAIN> [-p <PROJECT>]
```

Alias: `lokal service rm`.

Removes the service's route and name. The command returns once the name is no longer served.

```console
$ lokal service remove api
```

### `lokal run`

```text
lokal run <SUBDOMAIN> [--port <PORT>] [--host-header <MODE>] [--rm] [-p <PROJECT>] -- <COMMAND>...
```

Starts a dev server behind its URL without you choosing a port. lokal picks a free port, routes
`https://<SUBDOMAIN>.<project domain>` to `localhost:<port>`, waits until the route is ready, and then starts the
command on that port with your terminal attached.

| Argument or flag | Default | What it does |
|---|---|---|
| `<SUBDOMAIN>` | | The name under the project's domain, as in `service add`. `@` is the domain itself |
| `--port <PORT>` | a free port | The port for a new service. A service that already exists keeps its own port |
| `--host-header <MODE>` | `preserve` | For a new service: `preserve` or `upstream`, as in `service add` |
| `--rm` | | Remove the service when the command exits, if this run added it |
| `-p`, `--project <PROJECT>` | current project | The project |
| `-- <COMMAND>...` | | The dev server's command and its arguments. The `--` is required, because `run` has a `--port` of its own |

```console
$ lokal run web -- npm run dev
✓ https://web.myapp.lokal → localhost:51234  npm run dev  (route ready in 12ms)
  … your dev server's output …
✓ ready: https://web.myapp.lokal (1.4s)
$ lokal run web --host-header upstream -- vite --port {port}
$ lokal run api --port 4000 -- cargo run
$ lokal run docs --rm -- mkdocs serve -a 127.0.0.1:{port}
```

- **The command** runs as you typed it, never through a shell. `{port}` in any argument is replaced with the
  port, for servers that don't read `PORT`. It keeps your terminal, so colours and keyboard shortcuts work. It gets
  these environment variables:

  | Variable | Value |
  |---|---|
  | `PORT` | The port |
  | `HOST` | `127.0.0.1`, the address to listen on |
  | `LOKAL_URL` | The command's own URL, such as `https://web.myapp.lokal` |
  | `LOKAL_URL_<SUB>` | One for every service in the project, this one included: `LOKAL_URL_API`, `LOKAL_URL_V1_API` for `v1.api`, `LOKAL_URL_APEX` for `@` |

  If two services map to the same variable (`v1.api` and `v1-api`), lokal sets neither and prints a warning.
- **lokal's own lines go to stderr.** Your dev server's output passes through untouched. `✓ ready: …` appears once
  the port accepts connections, so a script or agent can wait for that line: the URL works from then on. If the
  background service isn't running, or lokal isn't set up, the route is saved, a note says so, the command starts
  anyway, and the line reads `✓ listening on localhost:<port>` instead.
- **A server on the wrong port.** If, 3 seconds after the start, the command (or a process it started, such as
  the `node` that `npm run dev` starts) listens on some other port and not on lokal's, lokal prints a `hint:` line
  once, with the two ways to fix it: point the route at that port with `lokal service update`, or pass
  `--port {port}` to the server.
- **An existing service** is reused as it is: same port, host and host-header mode, so the port stays the same
  from one run to the next, and a route you added by hand gets its server started. A different `--port` or
  `--host-header` fails with exit code 2 and names the `lokal service update` command to run. A service that
  routes to another machine can't be run.
- **A port that's already in use** stops `run` before anything starts (exit code 1). The error names the process
  that listens there and how to move the route. lokal never moves a port by itself.
- **Ctrl-C** reaches your dev server exactly once, as it does without lokal. When there's no terminal, for example
  when an agent runs `kill -INT` on lokal, lokal passes SIGINT on to the server. SIGTERM and SIGHUP are always
  passed on.
- **The exit code** is the command's. If a signal stopped it, the code is 128 + the signal number: 130 for
  SIGINT, 143 for SIGTERM. A command that isn't found exits 127, one that can't be executed 126.
- **Afterwards** the route stays, and requests get lokal's 502 page until the server runs again. With `--rm`, lokal
  removes the service, but only if this run added it.

[Frameworks](/docs/guides/frameworks#let-lokal-pick-the-port-lokal-run) lists the `{port}` patterns for common dev
servers.

### `lokal up`

```text
lokal up [SUB]... [--yes]
```

Gets a repo's whole stack running behind its domains. lokal reads the repo's `lokal.toml`, routes every service in
it, waits until every URL works, and then starts each service's dev server and shows their output. It stays in the
foreground until the servers exit or you stop it. `lokal up` is the many-service form of [`lokal run`](#lokal-run):
its commands get the same ports, `{port}` and environment variables.

| Argument or flag | What it does |
|---|---|
| `[SUB]...` | The services to start (default: all of them). Every service in the file is routed either way |
| `-y`, `--yes` | Run the commands without asking, and remember nothing. For agents and CI in your own repos (see [the trust check](#the-trust-check)) |

```console
$ lokal up
✓ https://shop.lokal → localhost:3000  npm start
✓ https://api.shop.lokal → localhost:51234  cargo run -p api
✓ https://web.shop.lokal → localhost:51240  vite --port 51240
✓ https://docs.shop.lokal → localhost:51251  mkdocs serve -a 127.0.0.1:51251
  routes ready in 41ms
@    | > shop@1.0.0 start
api  |    Compiling api v0.1.0 (/Users/you/shop/services/api)
web  |   VITE v5.4.10  ready in 312 ms
  … the dev servers' own output …
✓ ready: 4/4 services accepting connections (2.8s)
^C
stopping 4 services… done. Routes kept; 'lokal down' removes them.
$ lokal up api web            # route every service, start only api and web
$ lokal up --yes              # agents and CI: no question, nothing remembered
```

#### The `lokal.toml` file

A repo commits one `lokal.toml`, usually at its root. It works like a Procfile with domains: each service has a
name, which becomes its subdomain, and the command that starts its dev server.

```toml
# lokal.toml at the repo root
project = "shop"                 # optional; default: the directory name
# domain = "shop.lokal"          # optional; default: <project>.lokal

[services]
"@"  = { run = "npm start", port = 3000 }                    # https://shop.lokal, fixed port
api  = { run = "cargo run -p api", cwd = "services/api" }    # lokal picks a free port
web  = { run = "vite --port {port}", host_header = "upstream" }
docs = "mkdocs serve -a 127.0.0.1:{port}"                    # short form: the string is the command
```

| Key | Default | What it is |
|---|---|---|
| `project` | the directory's name | The project name, following the same rules as `project create`. The default comes from the directory that holds `lokal.toml`, lowercased, with other characters turned into `-`, and lokal prints a note saying so |
| `domain` | `<project>.lokal` | The project's domain. It must end in `.lokal` or `.local` |
| `[services]` | | Required: one entry per service. The key is the subdomain. `"@"` is the domain itself, and nested names such as `v1.api` work |

Each service is a table with these keys, or just a string, which is short for `{ run = "…" }`:

| Key | Default | What it does |
|---|---|---|
| `run` | | Required. The shell command that starts the dev server. `{port}` in it becomes the port |
| `port` | a free port | A fixed port. Without it, lokal picks a free port the first time and keeps it from one `lokal up` to the next |
| `cwd` | the directory of `lokal.toml` | The directory the command runs in, relative to `lokal.toml`. It must exist |
| `host_header` | `"preserve"` | `"preserve"` or `"upstream"`, as `--host-header` in [`service add`](#lokal-service-add) |
| `env` | | Extra environment variables for the command, as a table of strings. `PORT`, `HOST`, `LOKAL_URL` and the `LOKAL_URL_*` variables are lokal's, so setting one is an error |

- Every service needs `run`, so `api = 4000` is an error. There's no `host` key: lokal starts these servers on your
  Mac, so the upstream is always `localhost`. Servers that lokal doesn't start, such as a container, stay with
  [`lokal service add`](#lokal-service-add).
- `lokal up` looks for `lokal.toml` in the current directory, then in each parent, and the first one it finds wins.
  Nothing merges: a nested `lokal.toml` is a project of its own. In a monorepo, put one file at the root, with a
  `cwd` for each service.
- A symlinked `lokal.toml` counts as the file it points to: its directory, its project name and its approval.
- An error in the file is a usage error (exit code 2) that names the file, line and column:
  `error: <path>:<line>:<col>: <message>`. An unknown key only gets a warning, so a typo is flagged and a file
  written for a newer lokal still runs.

#### What `lokal up` does

1. **Finds and checks `lokal.toml`.** A SUB that isn't in the file is a usage error that lists the file's
   services.
2. **Runs [the trust check](#the-trust-check).** It comes before anything changes, so a refusal routes nothing and
   starts nothing.
3. **Takes the project's lock,** so only one `lokal up` runs a project at a time. A second one exits 1 with
   `lokal up is already running for shop (pid 1234)`.
4. **Routes every service** in one state change and waits until every name is ready, so every URL works before any
   command starts. It creates the project if it's missing and makes it the current project. Services that left the
   file are removed. Services you added by hand stay.
5. **Prints one line per service,** with `{port}` filled in:
   `✓ https://api.shop.lokal → localhost:51234  cargo run -p api`. A service this `lokal up` doesn't start gets a
   dim `(not started)`. Then comes `routes ready in 41ms`.
6. **Starts the commands** and shows their output, then prints [the ready line](#output-and-the-ready-line). It
   runs until every command has exited or you [stop it](#stopping).

- **The commands.** Each one runs as `/bin/sh -c '<run>'`, with `{port}` replaced by the port and nothing else
  added. It runs as you, in its `cwd`, in a process group of its own, with stdin from `/dev/null`. It gets your
  environment plus the variables [`lokal run`](#lokal-run) sets (`PORT`, `HOST=127.0.0.1`, `LOKAL_URL` and a
  `LOKAL_URL_<SUB>` for every service in the project), and then the file's `env`.
- **Ports.** A `port` in the file is used as it is. A service without one keeps the port it got last time, so its
  port stays the same from one `lokal up` to the next. A new service gets a free port, picked the way `lokal run`
  picks one.
- **A port that's already in use** stops `lokal up` before it changes or starts anything (exit code 1). The error
  names the process that listens there, such as
  `localhost:3000 is already in use by node (pid 412); not starting @`, and the hint says how to fix it. lokal never
  moves a port by itself. Services this `lokal up` doesn't start aren't checked: the server there may be your own.
- **A service you added by hand** with the same name as one in the file is a usage error (exit code 2). `lokal up`
  never takes over or overwrites it, and the hint names the `lokal service remove` command that clears the way.
  Usually it's a route left by an earlier `lokal run`.
- **A different domain.** If the file sets `domain` and the project already exists with another one, `lokal up`
  fails, as `project create` does with a different `--domain`.
- **Before setup,** or while the background service isn't running, `lokal up` saves the routes, prints a note
  saying what to run, and starts the commands anyway, so the servers still work on `localhost`.

#### The trust check

A `lokal.toml` runs commands, and the repo may be someone else's. So before anything changes, `lokal up` lists every
command in the file, with its `cwd` and `env`, and asks a person to approve them:

```text
/Users/you/shop/lokal.toml runs these commands:
  @     npm start
  api   cargo run -p api                     in services/api
  web   vite --port {port}
  docs  mkdocs serve -a 127.0.0.1:{port}
run these commands? [y/N]
```

- It asks the first time it runs a file, and again whenever the file changes. `y` records the approval and goes on.
  Anything else prints `Not approved; nothing was started.` and exits 1.
- Once a person has approved the file, `lokal up` runs it without asking, also with no terminal, until the file
  changes.
- With no terminal to ask on and no `--yes`, it lists the commands, changes nothing and exits 1:
  `lokal.toml has commands nobody has approved yet: <path>`, or `lokal.toml changed since it was approved: <path>`.
  The hint asks a person to review the file and run `lokal up` in a terminal.
- `--yes` runs the commands without asking. It still lists them, and it records no approval, so a stored approval
  always means a person said yes.
- The listing shows control characters and invisible characters escaped, and puts each extra line of a multi-line
  value on a line of its own starting with `↳`, so nothing in the file can hide what runs.

Approving a file means trusting the repo, because the commands run the repo's own code: `npm start` runs the
scripts in `package.json`. The [security model](/docs/concepts/security-model#commands-from-a-lokaltoml) explains
what lokal stores.

#### Output and the ready line

- Each line a service prints shows up as `name | line`, with the names padded to the same width, and coloured when
  colour is on. A service's stdout lines go to lokal's stdout, and its stderr lines to lokal's stderr. Lines longer
  than 16 KiB are split.
- lokal's own lines go to stdout, unlike `lokal run`'s, because no single command owns stdout here. `warning:` and
  `error:` lines go to stderr.
- A service that exits on its own is reported as `api exited 1`, and the others keep running. If its shell can't
  start, lokal prints `api: cannot start: <error>` on stderr.
- **The ready line.** lokal probes the port of each service it started. Once every one of them accepts connections
  or has exited, it prints `✓ ready: 4/4 services accepting connections (2.8s)`, timed from the first start. The
  second number counts the services this `lokal up` started, and the first those that accept connections, so a
  service that exited doesn't count. This is the line a script or a coding agent waits for. Until a service is
  ready, its URL gets lokal's 502 page.
- If the routes didn't take, because lokal isn't set up or the background service isn't running, the URLs don't
  work, so the line reads `✓ listening: 4/4 services on localhost` instead.

#### Stopping

The first Ctrl-C (SIGINT), SIGTERM, SIGHUP (the terminal closed) or SIGQUIT stops every service:

1. lokal prints `stopping 4 services…` and sends SIGINT to each service's process group, SIGTERM at 5 seconds to
   any that are left, and SIGKILL at 10 seconds. A second signal sends SIGKILL at once.
2. It waits until every process group is empty, not just until each shell has exited, so the background jobs a
   command started, which ignore SIGINT, stop too.
3. It prints `done. Routes kept; 'lokal down' removes them.`

- The services aren't in lokal's process group, so the terminal's Ctrl-C reaches only lokal, and lokal signals each
  service once.
- A closed stdout or stderr, as in `lokal up | head`, stops the services the same way.
- The routes stay. Requests get lokal's 502 page until the servers run again, and [`lokal down`](#lokal-down)
  removes the routes.
- If lokal itself is killed with SIGKILL, it can't stop anything, and the commands keep running. The next
  `lokal up` finds their ports in use and names the process that holds each one.

#### How `lokal up` exits

`lokal up` exits with 128 + the first signal it got: 130 for Ctrl-C, 143 for SIGTERM, and 141 when its stdout or
stderr closed. If every command exits on its own, it exits 0 when all of them exited 0, and 1 otherwise.

Errors before any command starts keep the usual [exit codes](#exit-codes): 2 for a usage error (an error in
`lokal.toml`, an unknown SUB, a service you added by hand, a different domain), and 1 for no `lokal.toml`, a refused
trust check, another `lokal up` running the project, or a port in use.

#### Services that `lokal up` manages

- `lokal list`, `project show` and `service list` mark them with a dim `(lokal.toml)`, and their `managed` field in
  [JSON output](#json-output) is `"lokal.toml"`.
- `service update` and `service remove` still work on them, with a note that the next `lokal up` sets the service
  back, or adds it back.
- `lokal run <SUB>` reuses such a service as it is. While `lokal up` runs it, its port is in use, so `lokal run`
  refuses and names the process.

### `lokal down`

```text
lokal down [--remove-project] [-p <PROJECT>]
```

Removes the services that [`lokal up`](#lokal-up) added. Services you added by hand stay.

| Flag | What it does |
|---|---|
| `--remove-project` | Also remove the project, if nothing else is left in it |
| `-p`, `--project <PROJECT>` | The project (default: the one in `lokal.toml`) |

```console
$ lokal down
✓ Removed 3 services from lokal.toml  (done in 30ms)
$ lokal down --remove-project   # also the project, if nothing else is left in it
$ lokal down -p shop
```

- Without `-p`, the project comes from the `lokal.toml` that `lokal up` would find. Only its project name matters,
  so a file with other errors still names it. With no file and no `-p`, `down` exits 1 and suggests
  `lokal down -p <NAME>`.
- It removes the services in one change and returns once their names are no longer served.
- `--remove-project` also removes the project when nothing is left in it, and clears the current project if it was
  this one. If services you added by hand remain, it keeps the project and says so.
- With nothing to remove, it says so and exits 0, so a repeated cleanup doesn't fail.
- While `lokal up` runs the project, `down` changes nothing and exits 1:
  `lokal up is running for shop (pid 1234); stop it first (Ctrl-C in its terminal)`.
- If you rename the project in `lokal.toml`, the old project's services stay until `lokal down -p <old name>`.

### `lokal list`

```text
lokal list [--json]
```

Alias: `lokal ls`.

Shows every project with its services, URLs and upstreams. `--json` is a stable contract for scripts and coding
agents (see [JSON output](#json-output)).

```console
$ lokal list
myproject  myproject.lokal  (current)
  api  https://api.myproject.lokal  → localhost:4000
  web  https://web.myproject.lokal  → localhost:3000
shop  shop.lokal
  (no services)
```

## Background service

The background service is a LaunchAgent (`dev.lokal.daemon`) that runs as you. `start`, `stop`, `restart` and
`upgrade` need setup to have run. Before that, they exit with code 3. None of them asks for a password.

### `lokal start`

```text
lokal start
```

Starts the background service and waits until it answers. If it's already running, `start` says so and does
nothing.

### `lokal stop`

```text
lokal stop
```

Stops the background service. It stays installed, and starts again at your next login or with `lokal start`.
While it's stopped, lokal URLs don't work.

### `lokal restart`

```text
lokal restart
```

Restarts the background service and waits until the new process answers. launchd holds the ports, so connections
that arrive during the restart wait in its queue instead of failing.

### `lokal status`

```text
lokal status [--json]
```

Shows whether the background service is running, its build, its listeners and the state of every published name.
It also works before setup, and then says that lokal isn't set up yet. `--json` includes `setup_done` and
`running`, which scripts and coding agents can use to check that lokal is ready.

```console
$ lokal status
$ lokal status --json
```

### `lokal logs`

```text
lokal logs [-f] [-n <LINES>]
```

Shows the background service's log, `~/Library/Logs/lokal/daemon.log`.

| Flag | Default | What it does |
|---|---|---|
| `-f`, `--follow` | | Keep printing new lines |
| `-n`, `--lines <LINES>` | `50` | Number of lines to show |

```console
$ lokal logs -n 200
$ lokal logs -f
```

### `lokal upgrade`

```text
lokal upgrade
```

Makes the background service run this `lokal` binary. It copies the binary to the service's own copy, restarts
the service and waits until the new build answers. Run it after every `brew upgrade lokal`, or after you replace
the binary. Until you do, lokal warns:
`the background service runs a different lokal build — run 'lokal upgrade'`.

```console
$ lokal upgrade
✓ background service now running lokal 0.1.0 (70ec3e3) — ready in 1.2s
```

## Utilities

### `lokal env`

```text
lokal env [--shell <sh|fish>]
```

Prints shell exports so tools that ship their own certificate bundle (Node, Python, curl with `CURL_CA_BUNDLE`)
trust lokal's certificate. Browsers and macOS tools already do.

| Flag | Default | What it does |
|---|---|---|
| `--shell <SHELL>` | `sh` | Output syntax: `sh` (also right for zsh and bash) or `fish` |

```console
$ lokal env
export NODE_EXTRA_CA_CERTS='/Users/you/Library/Application Support/lokal/ca/root.pem'
export SSL_CERT_FILE='/Users/you/Library/Application Support/lokal/ca/bundle.pem'
export REQUESTS_CA_BUNDLE='/Users/you/Library/Application Support/lokal/ca/bundle.pem'
export CURL_CA_BUNDLE='/Users/you/Library/Application Support/lokal/ca/bundle.pem'
$ eval "$(lokal env)"                 # zsh, bash
$ lokal env --shell fish | source     # fish
```

`bundle.pem` holds the macOS system roots plus lokal's CA, so public HTTPS sites keep working. `lokal env`
rebuilds it when it's older than 30 days. [Node, Python and curl](/docs/guides/lokal-env) has the details.

### `lokal ca path`

```text
lokal ca path
```

Prints the path of lokal's CA certificate.

```console
$ lokal ca path
/Users/you/Library/Application Support/lokal/ca/root.pem
$ NODE_EXTRA_CA_CERTS="$(lokal ca path)" node app.js
```

### `lokal open`

```text
lokal open <SUBDOMAIN> [-p <PROJECT>]
```

Opens the service's `https://` URL in your default browser. `@` opens the project's own domain.

```console
$ lokal open api
$ lokal open web -p shop
```

### `lokal help`

```text
lokal help [COMMAND...]
```

The same as `--help`. `lokal help` prints the root help, and `lokal help service add` prints the full help for
that command, with examples.

## JSON output

`list`, `status`, `doctor`, `project list`, `project show` and `service list` take `--json`. That output is a
stable contract:

- It is pretty-printed, has no colour codes, and always contains `"schema": 1`.
- Fields may be added, but they aren't renamed or removed without a new schema number.

### `lokal list --json`

`lokal project list --json` prints the same document.

```json
{
  "schema": 1,
  "current_project": "myproject",
  "projects": [
    {
      "name": "myproject",
      "domain": "myproject.lokal",
      "services": [
        {
          "subdomain": "api",
          "fqdn": "api.myproject.lokal",
          "url": "https://api.myproject.lokal",
          "host": "localhost",
          "port": 4000,
          "host_header": "preserve",
          "managed": null
        }
      ]
    }
  ]
}
```

`current_project` is `null` when no project is current. A service's `managed` is `"lokal.toml"` when
[`lokal up`](#lokal-up) added it, and `null` otherwise.

### `lokal project show --json`

One project, in the same shape as an entry of `projects` above, plus whether it's the current project.

```json
{
  "schema": 1,
  "current": true,
  "project": {
    "name": "myproject",
    "domain": "myproject.lokal",
    "services": [
      {
        "subdomain": "api",
        "fqdn": "api.myproject.lokal",
        "url": "https://api.myproject.lokal",
        "host": "localhost",
        "port": 4000,
        "host_header": "preserve",
        "managed": null
      }
    ]
  }
}
```

### `lokal service list --json`

```json
{
  "schema": 1,
  "project": "myproject",
  "domain": "myproject.lokal",
  "services": [
    {
      "subdomain": "api",
      "fqdn": "api.myproject.lokal",
      "url": "https://api.myproject.lokal",
      "host": "localhost",
      "port": 4000,
      "host_header": "preserve",
      "managed": null
    }
  ]
}
```

### `lokal status --json`

```json
{
  "schema": 1,
  "setup_done": true,
  "running": true,
  "pid": 1234,
  "version": "0.1.0",
  "build_id": "0.1.0+abc1234.20261010T120000Z",
  "uptime_secs": 42,
  "listeners": [
    "127.0.0.1:443",
    "[::1]:443",
    "127.0.0.1:80",
    "[::1]:80",
    "127.0.0.1:53535/udp"
  ],
  "listener_source": "launchd",
  "name_backend": "mdns-local-only+dns",
  "names": [
    {
      "fqdn": "api.myproject.lokal",
      "state": "registered"
    }
  ],
  "certs_cached": 2
}
```

A name's `state` is `registered`, `pending` or `failed: <reason>`. When the background service isn't running,
`running` is `false`, `listeners` and `names` are empty, and the fields that describe the running process (`pid`,
`version`, `build_id`, `uptime_secs`, `listener_source`, `name_backend`) are left out. Check `setup_done` first:
if it's `false`, the user needs to run `lokal setup`.

### `lokal doctor --json`

```json
{
  "schema": 1,
  "ok": true,
  "checks": [
    {
      "id": "ca.trusted",
      "status": "pass",
      "detail": "macOS trusts lokal's CA",
      "ms": 31
    },
    {
      "id": "upstream.api.myproject.lokal",
      "status": "warn",
      "detail": "nothing listening on localhost:4000 — is your service running?",
      "ms": 50
    }
  ]
}
```

`ok` is `false` when at least one check has `"status": "fail"`. Warnings don't affect it.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Runtime error, or `lokal doctor` found at least one failing check |
| `2` | Usage error: an unknown command or flag, a value that fails validation, such as an invalid name or port, or an error in `lokal.toml` |
| `3` | Setup hasn't run. Run `lokal setup` |
| `4` | Sudo would be needed but can't run: `LOKAL_FORBID_SUDO=1` is set, or there's no terminal to ask for the password |

Human output goes to stdout, except `lokal run`'s own lines, which go to stderr because the dev server it starts
owns stdout. `lokal up`'s own lines go to stdout, and each service's lines keep their stream: its stdout on stdout,
its stderr on stderr. Errors go to stderr as `error: <message>`, often followed by `hint: <suggestion>`. Apart from
`setup`, `uninstall` and `lokal up`'s [trust check](#the-trust-check), no command ever prompts.

Once its command has started, `lokal run` exits with the command's exit code instead (see
[`lokal run`](#lokal-run)). Once its commands have started, `lokal up` exits with 128 + the signal that stopped it
(130 for Ctrl-C, 143 for SIGTERM), or, when the commands all exit on their own, with 0 if every one exited 0 and 1
otherwise (see [How `lokal up` exits](#how-lokal-up-exits)).

Before setup has run, `project` and `service` commands still save your changes and exit 0. They print a note that
lokal isn't set up yet, and the URLs start working once you run `lokal setup`.

## Environment variables

| Variable | What it does |
|---|---|
| `NO_COLOR` | Any non-empty value turns off coloured output, like `--no-color` |
| `LOKAL_HOME` | Test hook. `LOKAL_HOME=<dir>` moves lokal's data into `<dir>` and its logs into `<dir>/logs`, as a sandbox. lokal then never touches the real background service: `setup`, `uninstall`, `start`, `stop`, `restart` and `upgrade` refuse to run. lokal's own integration tests use it |
| `LOKAL_FORBID_SUDO` | Test hook. With `LOKAL_FORBID_SUDO=1`, a command that would need sudo fails with exit code 4 instead of asking for a password. The live test uses it to prove that re-running setup on a set-up Mac needs no password |

`lokal env` prints `NODE_EXTRA_CA_CERTS`, `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE` and `CURL_CA_BUNDLE` for your other
tools. lokal itself doesn't read them.
