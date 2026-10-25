---
title: Using lokal with coding agents
description: What a coding agent needs to know to drive lokal, from AGENTS.md instructions to JSON output and exit codes.
section: guides
order: 4
sidebarLabel: Coding agents
source: [README.md, docs/SPEC.md, docs/TROUBLESHOOTING.md, crates/lokal/src/cli/help.rs, crates/lokal/src/cli/run.rs, crates/lokal/src/error.rs]
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
- **A `hint:` line** means the server listens on a different port than the one lokal picked. It names that port
  and the fix.
- **Re-running is safe.** An existing service keeps its port, so the URL stays the same. If something already
  listens on that port (an earlier run is still going), `run` exits 1 before starting anything and names the
  process. Pass `--rm` to remove the service again when the server exits.

The [command reference](/docs/reference/commands#lokal-run) has every rule.

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

Once its command has started, `lokal run` exits with the command's exit code instead.

Commands that need setup, such as `start`, `upgrade`, `env` and `open`, exit with `3` before setup.
Commands that change projects and services don't: they save the change, exit `0`, and print a note that lokal
isn't set up yet. Errors go to stderr as `error: <message>`, often followed by a `hint:` line with the fix.

## Built by a coding agent

lokal v0.1 itself was built by a coding agent working from a specification, with human sign-off for every
privileged step. The [build history](https://github.com/mehmetkose/lokal/blob/main/docs/history/README.md) has
the build log, the decisions and the measurements made on macOS. If you point an agent at the lokal repo to work
on lokal itself, it should read
[`CONTRIBUTING.md`](https://github.com/mehmetkose/lokal/blob/main/CONTRIBUTING.md) first.
