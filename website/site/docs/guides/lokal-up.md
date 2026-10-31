---
title: A whole repo with lokal up
description: Commit a lokal.toml that lists your dev servers, and one command routes every one of them and starts it.
section: guides
order: 2
sidebarLabel: A whole repo (lokal up)
source: [README.md, docs/SPEC.md, crates/lokal/src/cli/up.rs, crates/lokal/src/project_file.rs]
---

A repo with several dev servers can commit one `lokal.toml` that lists them. Then `lokal up` gets the whole stack
running behind its domains: it routes every service at once, starts each dev server on its port, and shows all
their output in one terminal. Ctrl-C stops them all.

Pick the command that fits how the server starts:

| You want to | Use |
|---|---|
| Route a server you start yourself, or one lokal can't start, such as a container | [`lokal service add`](/docs/guides/projects-and-services#services) |
| Start one dev server on a port lokal picks | [`lokal run`](/docs/reference/commands#lokal-run) |
| Start every dev server in a repo | `lokal up` |

## Write a `lokal.toml`

Put the file at the repo root and commit it:

```toml
project = "shop"                 # optional; default: the directory name

[services]
"@"  = { run = "npm start", port = 3000 }                    # https://shop.lokal, fixed port
api  = { run = "cargo run -p api", cwd = "services/api" }    # lokal picks a free port
web  = { run = "vite --port {port}", host_header = "upstream" }
docs = "mkdocs serve -a 127.0.0.1:{port}"                    # short form: the string is the command
```

- **Each key under `[services]` is a subdomain.** `"@"` is the project's domain itself, and nested names such as
  `v1.api` work, as in `service add`.
- **`run` is required:** the shell command that starts the dev server. `{port}` in it becomes the port, for
  servers that don't read `PORT`. A plain string is short for `{ run = "…" }`.
- **`port` is optional.** Without it, lokal picks a free port the first time and keeps it from one `lokal up` to
  the next.
- **`cwd`** is the directory the command runs in, relative to `lokal.toml`. **`host_header = "upstream"`** is for
  dev servers that reject unknown hosts, such as Vite ([Frameworks](/docs/guides/frameworks)). **`env`** adds
  environment variables.
- **`project`** defaults to the name of the directory that holds `lokal.toml`, and **`domain`** to
  `<project>.lokal`.

There's no `host` key, because lokal starts these servers on your Mac. A server it doesn't start, such as a
container, stays with `lokal service add`. Every key and rule is in the
[command reference](/docs/reference/commands#the-lokaltoml-file). A mistake in the file stops `lokal up` with the
file, line and column, and an unknown key only gets a warning.

## Start the stack

Run `lokal up` anywhere in the repo. It finds `lokal.toml` in the current directory or the nearest parent that
has one:

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
```

1. **It routes every service first,** in one change, so every URL works before any server starts. Until a server
   is ready, its URL shows lokal's 502 page. `lokal up` creates the project if it's missing and makes it the
   current project.
2. **It starts each `run`** with `/bin/sh` in the service's `cwd`. The command gets the same variables as
   [`lokal run`](/docs/reference/commands#lokal-run): `PORT`, `HOST`, `LOKAL_URL`, and a `LOKAL_URL_<SUB>` for every
   service, such as `LOKAL_URL_API`, so one service can find another's URL.
3. **It prefixes each line** of output with the service's name.
4. **It prints `✓ ready: 4/4 services accepting connections`** once every server accepts connections. From then
   on, every URL works.

A service that exits on its own is reported as `api exited 1`, and the others keep running. The ready line then
counts it out and names it: `✓ ready: 3/4 services accepting connections (2.8s); not ready: api (exited 1)`.
`lokal up` stays in the foreground until every server has exited or you stop it.

A command that starts its server in the background and exits 0 leaves a server that lokal can't show or stop.
lokal says so, names the server, and counts it as `detached`:

```text
web exited 0, and left localhost:4321 to node (pid 25898), which runs outside lokal up: lokal logs web won't show its output, and stopping lokal up won't stop it.
hint: web's run must keep its server in the foreground. See https://mehmetkose.github.io/lokal/docs/guides/frameworks#servers-that-detach-themselves
```

[Servers that detach themselves](/docs/guides/frameworks#servers-that-detach-themselves) shows how to keep each
tool's server in the foreground.

## The first run: approving the commands

A `lokal.toml` runs commands, and the repo may be someone else's. So the first time you run a file, `lokal up` lists
every command in it and asks before it changes anything:

```text
/Users/you/shop/lokal.toml runs these commands:
  @     npm start
  api   cargo run -p api                     in services/api
  web   vite --port {port}
  docs  mkdocs serve -a 127.0.0.1:{port}
run these commands? [y/N]
```

- Answer `y`, and lokal remembers the approval for this file. It doesn't ask again until the file changes, and then
  it lists the commands again.
- With no terminal to ask on, such as in a script, it lists the commands, changes nothing and exits 1:
  `lokal.toml has commands nobody has approved yet`.
- `lokal up --yes` runs the commands without asking and remembers nothing. Use it for agents and CI in your own
  repos, not to skip reading someone else's file.

Approving a file means trusting the repo, because the commands run the repo's own code: `npm start` runs the
scripts in `package.json`. The [security model](/docs/concepts/security-model#commands-from-a-lokaltoml) explains
what lokal stores.

## Start only some services

```console
$ lokal up api web            # route every service, start only api and web
```

Every service in the file is still routed, and the ones this `lokal up` doesn't start are marked `(not started)`.
That's how you run one server yourself, for example in a debugger, while `lokal up` runs the rest. Start it on the
port in the route line, or fix its port with `port` in `lokal.toml`.

Only one `lokal up` runs a project at a time. A second one exits with
`lokal up is already running for shop (pid 1234)`, and its hint points at the running one: to start or restart a
service there, use [`lokal restart`](#restart-one-service).

## Ports

- **A fixed `port`** is used as it is. If something already listens there, `lokal up` stops before it changes or
  starts anything, and names the process: `localhost:3000 is already in use by node (pid 412); not starting @`.
- **A picked port** stays the same from one `lokal up` to the next. If the server exits with `EADDRINUSE` before
  it's ready, because something took the port after lokal chose it, lokal moves the service to a new free port and
  starts it again, once.
- **A port in use that isn't the service's.** A server also says `address already in use` when another port it
  opens is taken, such as its debugger's. lokal checks the service's own port when the server says so, and moves
  the service only when something else holds that port. Otherwise it doesn't retry, which couldn't help, and
  after the exit line it says where to look, for a fixed `port` too:

  ```text
  w  | workerd/server/server.c++:1882: bind(): Address already in use; port 9229
  w exited 1
  hint: w says a port is in use, but its own port 53840 is free. Another port it opens may be shared with another service, such as a debugger port (9229): give each server its own. See https://mehmetkose.github.io/lokal/docs/guides/frameworks#several-node-or-workers-servers-debugger-ports
  ```
- **A server on the wrong port.** Some servers ignore `PORT` and listen on their own default. lokal checks what
  each server listens on 3 seconds after it starts, then every 2 seconds. Once two checks in a row agree, and the
  server still isn't on its service's port, lokal prints the fix:

  ```text
  hint: web listens on localhost:5173, not 51234. Set port = 5173 for web in lokal.toml, or put {port} in its run: vite --port {port}
  ```

  A server that listens on several ports, such as its own and a debugger's, gets a list and no port to switch to
  (`If one of them is its server, set port to it for web in lokal.toml`). Check which one serves the app before
  you set it. When `lokal.toml` fixes the port, the hint never suggests another one:

  ```text
  hint: web isn't listening on 3000, the port lokal.toml gives it (it listens on localhost:5173). Make its server listen on 3000, or put {port} in its run: vite --port {port}
  ```

[Frameworks](/docs/guides/frameworks#let-lokal-pick-the-port-lokal-run) lists the `{port}` flag for common dev
servers.

lokal routes one port per service. Ports a server opens for itself, such as a Node or Workers debugger on 9229,
can clash when `lokal up` starts several servers at once.
[Debugger ports](/docs/guides/frameworks#several-node-or-workers-servers-debugger-ports) has the setting for each
tool.

## Read a service's logs

Each service that `lokal up` starts also writes its output to a log file, with a timestamp on each line and
lokal's own lines (started, exited, stopping) marked `lokal:`. Read it from any terminal, also after `lokal down`:

```console
$ lokal logs api -n 50        # the last 50 lines
$ lokal logs api -f           # follow new lines
```

The files are in `~/Library/Logs/lokal/up/<project>/` (on Linux, `~/.local/state/lokal/up/<project>/`). Each `lokal up` starts a service's log afresh, and a
service keeps at most about 10 MB. Plain `lokal logs`, with no service, still shows the background service's log.

## Restart one service

A dev server sometimes needs a restart: it crashed, it serves errors after its dependencies changed, or it reads a
config file only when it starts. Restart it from any terminal, while `lokal up` keeps the others running:

```console
$ lokal restart web
✓ restarted https://web.shop.lokal (1.2s)
```

`lokal up` stops that one server as Ctrl-C would, starts its command again on the same port, and `lokal restart`
returns once the URL works again. In `lokal up`'s terminal, it shows as `restarting web` and then
`web ready (1.2s)`, and the server's log goes on in the same file.

- **It also starts a service that isn't running:** one that exited, or one you left out of `lokal up api`.
- **If the server doesn't come back,** `lokal restart` exits 1 with `error: web exited 1 before it was ready` and
  the last lines of that run's log. If something else listens on the port, it names it, and starts nothing.
- **It runs the command `lokal up` started with,** the one you approved. After you change `lokal.toml`,
  `lokal restart` says so: stop `lokal up` and start it again to use the new file.
- **Several at once:** `lokal restart web api`. Outside the repo, name the project: `lokal restart web -p shop`.

`lokal restart` with no service still restarts lokal's background service.

## Stop and clean up

```console
^C
stopping 4 services… done. Routes kept; 'lokal down' removes them.
$ lokal down
✓ Removed 4 services from lokal.toml  (done in 30ms)
```

- **Ctrl-C** stops every service and everything it started, background jobs included: SIGINT first, SIGTERM after
  5 seconds, SIGKILL after 10. A second Ctrl-C kills them at once. SIGTERM and closing the terminal stop them the
  same way.
- **A server that detached** keeps running. After `done`, lokal names each one that still runs, such as
  `1 server keeps running outside lokal up: web (node, pid 25898)`. Stop it yourself (`kill 25898`), or the next
  `lokal up` finds its port in use, and says which service left it there.
- **The routes stay,** with their ports. Their URLs show lokal's 502 page until the servers run again.
- **`lokal down`** removes the services that `lokal up` added or [adopted](#services-you-added-before-the-file).
  Other services you added by hand stay.
  `lokal down --remove-project` also removes the project, if nothing else is left in it.

`lokal up` exits with 130 after Ctrl-C and 143 after SIGTERM. If every server exits on its own, it exits 0 when all
of them exited 0, and 1 otherwise. Servers that detached don't change that.

## Services that `lokal up` manages

`lokal list` marks the services that came from a `lokal.toml`:

```console
$ lokal list
shop  shop.lokal  (current)
  @    https://shop.lokal      → localhost:3000  (lokal.toml)
  api  https://api.shop.lokal  → localhost:51234  (lokal.toml)
  db   https://db.shop.lokal   → localhost:5432
```

- The file is the source for these services. `lokal up` updates them when the file changes, and removes the ones
  that left it.
- You can still change one with `service update` or remove it with `service remove`, but the next `lokal up` sets
  it back, and lokal says so: `note: lokal.toml manages api; the next lokal up sets it back`.
- In `--json` output, these services have `"managed": "lokal.toml"`, and all others `null`.

### Services you added before the file

A project often has services you added with `lokal service add`, or that `lokal run` left, before the repo got a
`lokal.toml`. When the file lists one by the same name and gives it the same route, `lokal up` adopts it:

- **The same route** means host `localhost`, the same `host_header` (`preserve` unless the file says otherwise),
  and the file's `port`. If the file gives it no `port`, the one it has counts as the same, as long as no other
  service in the file fixes that port. It keeps that port.
- **Nothing moves.** The URL stays as it is, and `lokal up` only marks the service as the file's. From then on it's
  managed like the others, so `lokal down` removes it. lokal says so once, after the route lines:

  ```text
    note: adopted api, web (added by hand or by lokal run, with the routes lokal.toml gives them); lokal down removes them now
  ```

- **A different route** stops `lokal up` before it changes anything, because taking the service over would move
  its URL. The error says what differs, and the hint is one command that removes every such service. Run it, then
  `lokal up` again, and the services come back with the file's routes:

  ```text
  error: service web already exists with another route than lokal.toml gives it (host 127.0.0.1, not localhost), and lokal.toml doesn't manage it (added by hand, or left by lokal run)
  hint: lokal service remove web -p shop
  ```

## Monorepos and nested files

`lokal up` uses the first `lokal.toml` it finds, going up from the current directory, and nothing merges. A nested
`lokal.toml` is a project of its own. For one stack in a monorepo, put one file at the root and give each service a
`cwd`:

```toml
[services]
api   = { run = "pnpm dev", cwd = "apps/api" }
web   = { run = "pnpm dev --port {port}", cwd = "apps/web" }
admin = { run = "pnpm dev --port {port}", cwd = "apps/admin" }
```

## In scripts and coding agents

Run `lokal up --yes` in the background and wait for `✓ ready:` on its stdout. Every URL is routed before any
command starts, and the ready line comes once every service accepts connections or has exited. Stop it with
SIGTERM: lokal stops every service and exits 143. When a URL fails, `lokal logs <service> -n 50` shows that
service's recent output, and `lokal restart <service>` restarts it and returns once the URL works again.

Each `run` must keep its server in the foreground. Astro 7 moves `astro dev` into the background when a coding
agent runs it: lokal reports `web exited 0, and left localhost:4321 to node (pid …)`, and the server keeps running
where lokal can't show or stop it. The ready line counts it as `detached`.
[Servers that detach themselves](/docs/guides/frameworks#servers-that-detach-themselves) has the fix.

[Using lokal with coding agents](/docs/guides/coding-agents#starting-a-whole-stack-lokal-up) explains each line an
agent may see, and has a snippet for your repo's `AGENTS.md`.
