---
title: Changelog
description: Every notable change to lokal, release by release.
section: project
order: 2
source: [CHANGELOG.md]
---

All notable changes to lokal are listed here. Downloads and release notes for each version are on
[GitHub Releases](https://github.com/mehmetkose/lokal/releases).

lokal follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor versions may change
the command line. The `--json` output (`"schema": 1`) and `state.toml` (`version = 1`) stay backwards compatible
until their schema numbers change.

After you upgrade, run `lokal upgrade` so the background service switches to the new version. See
[Upgrading and uninstalling](/docs/guides/upgrade-and-uninstall).

## Unreleased

To see what's on `main` but not in a release,
[compare `v0.5.1` with `main`](https://github.com/mehmetkose/lokal/compare/v0.5.1...HEAD).

## 0.5.1

Released 2026-10-31. [Release on GitHub](https://github.com/mehmetkose/lokal/releases/tag/v0.5.1).

### Added

- Linux release builds, for x86_64 and arm64. Each release now has a `.deb` for Ubuntu 26.04 and later, an `.rpm`
  for Fedora 43 and later, and a tarball for other distributions, all with SHA-256 checksums. The Homebrew formula
  (`brew install mehmetkose/tap/lokal`) works on Linux too. The Linux binaries need glibc 2.39 or later. See
  [Linux (preview)](/docs/installation#linux-preview).
- The `.deb` and `.rpm` require systemd 258 or later, which `lokal setup` needs, so apt and dnf refuse them where
  setup couldn't run, such as Ubuntu 24.04 and Debian 13. They also install `certutil` as a recommended package,
  so Chrome and Firefox trust lokal. Like the Homebrew formula, they don't run setup. Run `lokal upgrade` after
  installing a newer one, and `lokal uninstall` before removing the package.

## 0.5.0

Released 2026-10-31. [Release on GitHub](https://github.com/mehmetkose/lokal/releases/tag/v0.5.0).

### Added

- lokal runs on Linux, as a preview: x86_64 and arm64, built [from source](/docs/installation#from-source) with
  `make install`. There are no Linux release builds or packages yet. On Linux, `lokal setup` trusts lokal's CA in
  the system certificate store and in Chrome's and Firefox's certificate databases, sends `.lokal` lookups to
  lokal through a systemd-resolved DNS delegate, and runs the background service as systemd user units that hold
  ports 443 and 80. It needs systemd 258 or later with systemd-resolved running, such as Ubuntu 26.04, and stops
  before it changes anything if a piece is missing. Only `.lokal` names work on Linux; `.local` names don't yet.
  Tested on Ubuntu 26.04 under WSL2. See [Linux (preview)](/docs/installation#linux-preview).
- On Linux, a user service can't bind ports below `net.ipv4.ip_unprivileged_port_start`, 1024 by default, so
  setup lowers it to 80 with `/etc/sysctl.d/50-lokal.conf`. This lets every local user bind ports 80–1023.
  `lokal uninstall` removes the file and sets the kernel's default, 1024, back.

### Changed

- When a service of [`lokal up`](/docs/guides/lokal-up) says a port is in use, lokal also tries to bind the
  service's own port itself (without listening, released at once) to tell whether it's taken. A port held by a
  socket that neither a connect nor lsof shows, such as another user's that is bound without listening, now gets
  the retry on a new port instead of the hint about debugger ports.

## 0.4.0

Released 2026-10-26. [Release on GitHub](https://github.com/mehmetkose/lokal/releases/tag/v0.4.0).

### Added

- [`lokal restart <SUB>...`](/docs/reference/commands#lokal-restart) restarts services of a running `lokal up`,
  and returns once their URLs work again: `✓ restarted https://web.shop.lokal (1.2s)`. The `lokal up` stops the
  service's process group as Ctrl-C would, checks that nothing else took its port, and starts its command again
  on the same port, with the same log. It also starts a service that isn't running, because it exited or
  `lokal up` wasn't asked to start it. When the new run exits first, `lokal restart` exits 1 and shows that run's
  last log lines; when something holds the port, it names it. It runs only the commands `lokal up` started with,
  so a changed `lokal.toml` isn't read (it says so). Plain `lokal restart`, with no SUB, still restarts the
  background service. See [Restart one service](/docs/guides/lokal-up#restart-one-service).
- A second `lokal up` for a project that is already running points at `lokal restart`.
- [`lokal service remove`](/docs/reference/commands#lokal-service-remove) takes several names:
  `lokal service remove api web`. It removes them all in one change, and removes nothing if any name doesn't
  exist.
- `lokal up` notices a server that its command started in a session of its own before exiting 0, as `astro dev`
  does under a coding agent, and `pg_ctl start`, `docker compose up -d` and `pm2 start` do. It says that
  `lokal logs` won't show that server's output and stopping `lokal up` won't stop it, and links to
  [Servers that detach themselves](/docs/guides/frameworks#servers-that-detach-themselves). Before it exits, it
  names the servers that still run, and the next `lokal up` that finds such a port busy says which service left
  it running. lokal never stops these servers itself.

### Changed

- `lokal up` adopts a service it doesn't manage when its route is the one `lokal.toml` gives it: host
  `localhost`, the same host-header mode, and the file's `port`, or, when the file gives none, a port that no
  other service in the file fixes. Moving a project whose services you added by hand to a `lokal.toml`, or using
  one after `lokal run`, just works, and no URL moves. `lokal down` removes adopted services, and lokal says so
  when it adopts them.
- A service `lokal up` doesn't manage whose route differs from the file's is still an error, which now says what
  differs. Its hint is one command for all of them, `lokal service remove api web -p shop`, instead of one per
  service joined with `;`.
- `lokal up`'s ready line names the services it doesn't count as ready, and the servers that detached:
  `✓ ready: 4/6 services accepting connections (6.5s); not ready: api (exited 0), web (exited 1)`. A script that
  waits for `✓ ready: N/N` keeps working; one that matches the whole line needs to change.

### Fixed

- `lokal up`'s ready line counted a service whose command exited 0 by a race, so the same stack could print
  `1/2` or `2/2`. It now counts by what the command left on its port.
- The wrong-port hint of `lokal run` and `lokal up` waits until a server's listeners settle, so a server that
  opens helper listeners before its own port, such as Astro on Cloudflare's Vite plugin, no longer gets a hint a
  moment before it's ready. The hint offers a port only when the server listens on exactly one, and never offers
  to replace a port that `lokal.toml` fixes or that `lokal run --port` gave.
- lokal keeps fields it doesn't know when it rewrites `state.toml`, so an older lokal editing state no longer
  erases a newer lokal's settings. lokal 0.3.0 and earlier still drop them.
- `lokal up` retries a service on a new port only when its own port is taken. A server that reports a clash on
  another port, such as workerd's debugger port 9229, now gets a hint that links to
  [debugger ports](/docs/guides/frameworks#several-node-or-workers-servers-debugger-ports) instead of a retry
  that can't help.

## 0.3.0

Released 2026-10-25. [Release on GitHub](https://github.com/mehmetkose/lokal/releases/tag/v0.3.0).

### Added

- `lokal.toml`, [`lokal up`](/docs/reference/commands#lokal-up) and [`lokal down`](/docs/reference/commands#lokal-down).
  A repo commits a `lokal.toml` that lists its services and their dev server commands. `lokal up` routes every
  service in it at once (each URL works before any command starts), then starts each command with `/bin/sh` in a
  process group of its own, in the service's `cwd`, with `PORT`, `{port}`, `LOKAL_URL` and `LOKAL_URL_<SUB>` as
  `lokal run` gives them. Each line of output is prefixed with the service's name,
  `✓ ready: N/N services accepting connections` prints once every service accepts connections, and Ctrl-C stops
  every service and everything it started, keeping the routes. Before running anything from a file, `lokal up`
  lists its commands and asks a person to approve them, once per file and again whenever the file changes; `--yes`
  runs them without asking and remembers nothing. `lokal up <SUB>...` starts only those services. One `lokal up`
  runs a project at a time. `lokal down` removes the services that `lokal up` added.
- Every service in the `--json` output of `list`, `project list`, `project show` and `service list` has a
  `managed` field: `"lokal.toml"` for a service that `lokal up` added from a repo's `lokal.toml`, `null`
  otherwise. `"schema"` stays 1.
- A log file for each service that `lokal up` starts, `~/Library/Logs/lokal/up/<project>/<sub>.log`: its output
  from both streams, each line after a timestamp in `daemon.log`'s format, and lokal's own lines about it
  (`lokal: started on port …`, `lokal: exited 1`, `lokal: stopping (SIGINT)`). Each `lokal up` starts it afresh,
  and it moves to `<sub>.log.1` at 5 MB. [`lokal logs <SUB>`](/docs/reference/commands#lokal-logs) reads it, also
  from another terminal and after `lokal down`; the project comes from `-p`, the `lokal.toml` found from the
  current directory, or the current project. Plain `lokal logs` still shows the background service's log. When a
  .lokal URL fails, `lokal logs <sub> -n 50` shows that service's recent output.
- `lokal up` gives a service whose server listens on another port the hint `lokal run` gives, with the fix for
  `lokal.toml`: `hint: web listens on localhost:5173, not 51234. Set port = 5173 for web in lokal.toml, or put
  {port} in its run: vite --port {port}`. A service without a fixed `port` whose server exits with `EADDRINUSE`
  or `address already in use` before it's ready (something took the port after lokal chose it) is moved to a new
  free port and started again, once: `note: web's port 51234 was taken; retrying on 51302`. Both go into the
  service's log too.

## 0.2.0

Released 2026-10-25. [Release on GitHub](https://github.com/mehmetkose/lokal/releases/tag/v0.2.0).

### Added

- [`lokal run <SUBDOMAIN> -- <COMMAND>`](/docs/reference/commands#lokal-run): picks a free port (or reuses the
  service's), routes `https://<SUBDOMAIN>.<project>.lokal` to it, and starts your dev server on it with the terminal
  attached. The command gets `PORT`, `HOST`, `LOKAL_URL` and a `LOKAL_URL_<SUB>` for every service in the project,
  and `{port}` in any argument becomes the port. lokal prints `✓ ready: https://…` on stderr once the port accepts
  connections, hints when the server listens on a different port, lets Ctrl-C reach the server exactly once, and
  exits with the server's exit code. A port that's already in use stops it before anything starts. `--rm` removes
  the service when the command exits, if the run added it.

## 0.1.0

The first public release. macOS 14 Sonoma or later. Tested on Apple Silicon. Intel builds are published but
haven't been tested on an Intel Mac. [Release on GitHub](https://github.com/mehmetkose/lokal/releases/tag/v0.1.0).

### Added

- `lokal setup`: one-time onboarding. It creates a name-constrained CA (`.lokal` and `.local` only, no IP
  addresses), trusts it in the System keychain, installs `/etc/resolver/lokal`, installs the per-user LaunchAgent
  with launchd-owned sockets on 443 and 80, and runs a self-test. It is idempotent, `--dry-run` shows the exact
  sudo commands, and running it again repairs a broken install.
- `.lokal` names through a built-in DNS server (wildcard loopback answers, TTL 0), and `.local` names through
  mDNSResponder local-only records.
- Projects and services: `project create|list|show|use|remove`, `service add|list|update|remove`, `list`. Every
  change is live when the command returns (`ready in Nms`), and setup-type commands are safe to re-run.
- HTTPS reverse proxy: HTTP/1.1 and HTTP/2, WebSockets, server-sent events, streaming bodies, `localhost`
  upstreams on IPv4 or IPv6, `X-Forwarded-*` headers, `--host-header upstream` for Host-checking dev servers, 308
  redirects from HTTP, and 404, 421 and 502 pages.
- `lokal doctor`: checks every layer concurrently and gives a fix hint for each problem, including a check that
  `.lokal` isn't a public TLD.
- `lokal env` (Node, Python and curl trust), `ca path`, `open`, `status`, `logs`, `start`, `stop`, `restart`,
  `upgrade`, `uninstall [--purge]`.
- `--json` output for `list`, `status`, `doctor`, `project list|show` and `service list`.
- Prebuilt binaries for Apple Silicon and Intel Macs on GitHub Releases, with SHA-256 checksums, and a Homebrew
  tap: `brew install mehmetkose/tap/lokal`.
