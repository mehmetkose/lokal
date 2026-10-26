---
title: Using lokal with coding agents
description: What a coding agent needs to know to drive lokal, from AGENTS.md instructions to JSON output and exit codes.
section: guides
order: 5
sidebarLabel: Coding agents
source: [README.md, docs/SPEC.md, docs/TROUBLESHOOTING.md, crates/lokal/src/cli/help.rs, crates/lokal/src/cli/run.rs, crates/lokal/src/cli/up.rs, crates/lokal/src/error.rs]
---

lokal is built to be driven by scripts and coding agents as well as by people. Every command that sets something
up is safe to re-run, the list and status commands have a stable `--json` output, and exit codes say what went
wrong. The one thing an agent must leave to you is `lokal setup`, because it needs your password.

## Tell your agent about lokal

To tell an agent about lokal in your own project, add something like this to its `AGENTS.md` or `CLAUDE.md`:

```markdown
## Local HTTPS URLs (lokal)
- This repo's dev servers run behind lokal: https://web.myapp.lokal → localhost:3000,
  https://api.myapp.lokal → localhost:4000.
- Set up routes with `lokal project create myapp` and `lokal service add <sub> <port>` (safe to re-run).
- Or start a dev server on a port lokal picks: `lokal run <sub> -- <dev command>`, then wait for
  `✓ ready:` on stderr.
- Or, with a `lokal.toml` in the repo: run `lokal up --yes` in the background and wait for `✓ ready:` on stdout.
  Stop it with Ctrl-C or SIGTERM; `lokal down` removes its routes.
- One `lokal up` runs per project. If it says `lokal up is already running`, use that stack and don't stop it;
  `lokal logs <sub>` shows its output.
- When a .lokal URL fails, `lokal logs <sub> -n 50` shows that service's recent output (for services that
  `lokal up` started).
- When a URL fails, run `lokal doctor`. Never run `lokal setup` or sudo; ask the user.
```

Change the names and ports to match your repo.

## What the agent learns from `lokal --help`

An agent can also learn the workflow from the binary alone. lokal's root help (`lokal --help`) ends with a short
section written for people and coding agents:

```text
Using lokal in a project (for people and coding agents):
  1. lokal status                     not set up? The user runs `lokal setup` once in their Terminal
                                      (it asks for their password; don't run it with sudo yourself)
  2. lokal project create <name>      one per repo, e.g. the repo's directory name -> <name>.lokal
  3. lokal service add <sub> <port>   one per dev server: https://<sub>.<name>.lokal -> localhost:<port>
                                      `@` is the bare https://<name>.lokal; works as soon as it returns
  4. Run the dev server on that port (127.0.0.1, ::1 or localhost all work), open the https URL.
  Or skip the port: lokal run <sub> -- <dev command> picks one, routes it and starts the server.
  Or commit a lokal.toml and run lokal up: it routes and starts every service.
  When a .lokal URL fails, `lokal logs <sub> -n 50` shows that service's recent output
  (for services that `lokal up` started).
  Inspect: `lokal list --json`, `lokal status --json`. Problems: `lokal doctor` says what to fix.
  Node/Python/curl clients: eval "$(lokal env)". Vite "host not allowed": --host-header upstream.
  Steps 2-3 are safe to re-run. Changing a port: `lokal service update <sub> --port <port>`.
```

`lokal help <command>` shows each command's details and examples.

## Never run `lokal setup` or sudo

`lokal setup` trusts lokal's certificate authority in the macOS System keychain and installs
`/etc/resolver/lokal`. Both need your password, and macOS confirms the trust change in its own dialog. That's a
decision for you, not for an agent. Apart from `lokal uninstall`, setup is the only command that ever uses sudo.
Everything an agent does day to day (projects, services, `doctor`, `status`) runs without it.

So an agent should check first, and ask you if setup is missing:

```console
$ lokal status --json
{
  "schema": 1,
  "setup_done": false,
  "running": false,
  "listeners": [],
  "names": [],
  "certs_cached": 0
}
```

`"setup_done": false` means you need to run `lokal setup` yourself, once, in your own Terminal.

lokal enforces this too. When no terminal is attached to type a password into, which is the case for most agents,
setup stops instead of trying: with exit code 2 (`setup needs confirmation`), or with exit code 4 if `--yes` skipped
that question and setup needs sudo. Setting `LOKAL_FORBID_SUDO=1` in an agent's
environment makes lokal exit with code 4 instead of calling sudo even when a terminal is attached, and print the
exact commands it would have run.

## Safe to re-run

An agent can run the same setup commands every time it starts work:

```console
$ lokal project create myapp       # exists already? It becomes the current project; nothing else changes
$ lokal service add web 3000       # same settings? Nothing changes, and it still waits until the URL is ready
$ lokal service add api 4000
```

When a command returns, the URL works: the name resolves, the certificate exists and the route is live. There's
no need to sleep or retry.

A `service add` with different settings from the existing service fails (exit code 2) and names the command to
run instead, such as `lokal service update api --port 5000`. A `project create` with a different `--domain` also
fails with exit code 2.

There's one current project for the whole Mac. An agent working next to you, or across several repos, should pass
`-p <project>` to `service` commands instead of relying on it. [Projects and services](/docs/guides/projects-and-services#the-current-project)
has the details.

## Starting a dev server: `lokal run`

`lokal run` routes a name and starts the dev server in one command, on a port lokal picks:

```console
$ lokal run web -p myapp -- npm run dev
✓ https://web.myapp.lokal → localhost:51234  npm run dev  (route ready in 12ms)
  … the dev server's output …
✓ ready: https://web.myapp.lokal (1.4s)
```

- **Wait for `✓ ready:`.** lokal prints it on stderr once the server accepts connections, and the URL works from
  that line on, with no retry. lokal's own lines all go to stderr; the server's output passes through untouched.
- **Stop it** with SIGINT or SIGTERM to lokal (`kill -INT <pid>`). Without a terminal, lokal passes the signal to
  the server once, waits for it to exit, and exits with the server's exit code: 130 or 143 if the signal killed
  it.
- **A `hint:` line** means the server listens on a different port than the one lokal picked. When the server
  listens on one port, the hint names that port and the fix. When the hint lists several ports, it offers none of
  them, because one may be a debugger: find out which one is the server before you move the route. With `--port`,
  the hint never offers another port: make the server listen on the one you passed.
- **Re-running is safe.** An existing service keeps its port, so the URL stays the same. If something already
  listens on that port (an earlier run is still going), `run` exits 1 before starting anything and names the
  process. Pass `--rm` to remove the service again when the server exits.

The [command reference](/docs/reference/commands#lokal-run) has every rule.

## Starting a whole stack: `lokal up`

When a repo commits a `lokal.toml` that lists its dev servers, one command routes every service in it and starts
each one, on ports lokal picks or the file fixes:

```console
$ lokal up --yes
/Users/you/shop/lokal.toml runs these commands:
  …
  note: running these without asking (--yes); nothing is remembered
✓ https://shop.lokal → localhost:3000  npm start
✓ https://api.shop.lokal → localhost:51234  cargo run -p api
  …
  routes ready in 41ms
  … the dev servers' output, each line as name | line …
✓ ready: 4/4 services accepting connections (2.8s)
```

- **Commit a `lokal.toml`.** Then nobody, agent or person, has to pick names or ports: `lokal up` creates the
  project, routes each service and starts it. The
  [command reference](/docs/reference/commands#the-lokaltoml-file) describes the file.
- **Run `lokal up --yes` in the background and wait for `✓ ready:`** on stdout. Every URL is routed before any
  command starts, and the ready line comes once every service accepts connections or has exited. A service that
  exited first prints a line such as `api exited 1` before it, isn't counted as ready, and is named after the
  count: `✓ ready: 3/4 services accepting connections (2.8s); not ready: api (exited 1)`.
- **`web exited 0, and left localhost:4321 to node (pid …)`** means the service's command started its server in the
  background and exited, as `astro dev` does when it detects a coding agent. The server runs, and the ready line
  counts it as `detached`, but lokal can't show its output or stop it. Fix the service's `run`
  ([the setting for each tool](/docs/guides/frameworks#servers-that-detach-themselves)). Until then, stop the
  server yourself when you're done (`kill <pid>`): the next `lokal up` finds its port in use and names it.
- **A `hint:` line** means a server listens on another port than its service's, and names the fix: set
  `port = …` for it in `lokal.toml`, or put `{port}` in its `run`. When the hint lists several ports, it offers
  none of them, because one may be a debugger: find out which one is the server before you set `port`. When
  `lokal.toml` fixes the port, the hint never offers another: make the server listen on that port. Don't use
  `lokal service update` for it: the next `lokal up` sets the port back.
- **`note: web's port … was taken; retrying on …`** after `web exited 1` means the server found its port taken,
  and lokal started it again on a new one. Keep waiting for `✓ ready:`, which counts the service once.
- **`hint: web says a port is in use, but its own port … is free`** after `web exited 1` means the clash was on
  another port the server opens, most often a debugger port that another server took first. lokal doesn't retry.
  Give each server its own debugger port
  ([the setting for each tool](/docs/guides/frameworks#several-node-or-workers-servers-debugger-ports)), then run
  `lokal up` again.
- **A person approves the file.** `lokal up` runs the commands in the file, and a repo may be someone else's. So
  the first time a person runs `lokal up` in a terminal, it lists the commands and asks
  `run these commands? [y/N]`. After a `y`, `lokal up` runs that file with no terminal too, until the file
  changes. Without an approval or a terminal, it lists the commands, changes nothing and exits 1, and its hint
  asks a person to review the file. `--yes` is for agents and CI working in your own repo: it runs the commands
  without asking and remembers nothing. For a repo you didn't write, review the file and answer `y` yourself.
- **Stop it** with SIGINT or SIGTERM to lokal (`kill -INT <pid>`). lokal stops every service and everything it
  started, keeps the routes, and exits 130 or 143. `lokal down` removes the routes.
- **One `lokal up` per project.** A second one exits 1 and names the one that's running:
  `lokal up is already running for shop (pid 1234)`. `lokal down` refuses too while it runs. If it says
  `lokal up is already running`, use that stack and don't stop it: you or another agent session started it.
  `lokal logs <sub>` shows its output.
- **Read a service's output with `lokal logs <sub>`.** When a .lokal URL fails, `lokal logs <sub> -n 50` shows
  that service's recent output, also when the developer ran `lokal up` in their own terminal or another agent
  session did. Run in the repo, it reads that repo's project; `-f` follows the log. lokal's own lines in it are
  marked `lokal:` (`lokal: exited 1`). Services started with `lokal run` or by hand have no log: they print to
  their own terminal.

The [command reference](/docs/reference/commands#lokal-up) has every rule.

## Machine-readable output

These commands take `--json`:

- `lokal list`
- `lokal status`
- `lokal doctor`
- `lokal project list` and `lokal project show`
- `lokal service list`

The output is pretty-printed, has no colour codes, and always includes `"schema": 1`. It's a stable contract: it
stays backwards compatible until the schema number changes.

`lokal list --json` has every project and service:

```json
{
  "schema": 1,
  "current_project": "myapp",
  "projects": [
    {
      "name": "myapp",
      "domain": "myapp.lokal",
      "services": [
        {
          "subdomain": "api",
          "fqdn": "api.myapp.lokal",
          "url": "https://api.myapp.lokal",
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

A service's `managed` is `"lokal.toml"` when `lokal up` added it, and `null` otherwise.

`lokal status --json` always has `setup_done` and `running`. While the background service runs, it also has its
`pid`, `version`, `build_id`, `uptime_secs`, its `listeners`, and the state of each published name in `names`
(`registered`, `pending` or `failed: <reason>`).

`lokal doctor --json` lists every check with an `id`, a `status` (`pass`, `warn`, `fail` or `skip`), a `detail`
with a fix hint, and the time it took in `ms`. `ok` is `false` if any check fails. Warnings don't count:

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
      "id": "upstream.api.myapp.lokal",
      "status": "warn",
      "detail": "nothing listening on localhost:4000 — is your service running?",
      "ms": 0
    }
  ]
}
```

This example shows two checks. A real run has one for each layer, plus `dns.`, `tls.` and `upstream.` checks for
each service.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Runtime error, or `lokal doctor` found at least one `fail` |
| `2` | Usage error: an unknown command or flag, a name that breaks the [rules](/docs/guides/projects-and-services#name-rules), or a conflicting `service add` |
| `3` | Setup hasn't been run |
| `4` | sudo would be needed but isn't possible: `LOKAL_FORBID_SUDO=1` is set, or no terminal is attached |

Once its command has started, `lokal run` exits with the command's exit code instead. Once its commands have
started, `lokal up` exits with 128 + the signal that stopped it (130 for SIGINT, 143 for SIGTERM), or with 0 when
every command exited 0 on its own, and 1 otherwise.

Commands that need setup, such as `start`, `upgrade`, `env` and `open`, exit with `3` before setup.
Commands that change projects and services don't: they save the change, exit `0`, and print a note that lokal
isn't set up yet. Errors go to stderr as `error: <message>`, often followed by a `hint:` line with the fix.

## Built by a coding agent

lokal v0.1 itself was built by a coding agent working from a specification, with human sign-off for every
privileged step. The [build history](https://github.com/mehmetkose/lokal/blob/main/docs/history/README.md) has
the build log, the decisions and the measurements made on macOS. If you point an agent at the lokal repo to work
on lokal itself, it should read
[`CONTRIBUTING.md`](https://github.com/mehmetkose/lokal/blob/main/CONTRIBUTING.md) first.
