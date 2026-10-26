# Changelog

All notable changes to lokal are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and lokal follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor versions may change the command
line. The `--json` output (`"schema": 1`) and `state.toml` (`version = 1`) stay backwards compatible until their
schema numbers change.

## [Unreleased]

## [0.4.0] - 2026-10-26

### Added

- `lokal restart <SUB>... [-p <PROJECT>]` restarts services of a running `lokal up`, and returns once their URLs
  work again: `✓ restarted https://web.shop.lokal (1.2s)`. The `lokal up` stops the service's process group as
  Ctrl-C would, checks that nothing else took its port, and starts its command again on the same port, with the
  same log. It also starts a service that isn't running, because it exited or `lokal up` wasn't asked to start
  it. When the new run exits first, `lokal restart` exits 1 and shows that run's last log lines; when something
  holds the port, it names it, as `lokal up`'s busy-port error does. It runs only the commands `lokal up` started
  with, so a changed `lokal.toml` isn't read (it says so). `lokal up` takes these requests on a socket next to its
  lock, `up.sock` (0600), which accepts nothing else. Plain `lokal restart`, with no SUB, still restarts the
  background service.
- A second `lokal up` for a project that is already running points at `lokal restart`:
  `hint: restart a service: lokal restart <SUB> -p shop; see its output: lokal logs <SUB>`.
- `lokal service remove` takes several names: `lokal service remove api web`. It removes them all in one change
  and returns once none of them is served. If any name doesn't exist, it removes nothing.
- `lokal up` notices a server that its command started in a session of its own before exiting 0, as `astro dev`
  does under a coding agent, and `pg_ctl start`, `docker compose up -d` and `pm2 start` do. lokal can't show such
  a server's output or stop it, and now says so: `web exited 0, and left localhost:9903 to node (pid 25898), which
  runs outside lokal up: lokal logs web won't show its output, and stopping lokal up won't stop it.`, followed by
  a hint that links to the frameworks guide. A server can bind after its command has exited, so after an exit 0
  lokal keeps probing the port for up to 2 seconds before it reports the exit; a command that exits 0 without a
  server holds the ready line that much longer. Before it exits, also after Ctrl-C, lokal names the servers that
  still run: `2 servers keep running outside lokal up: web (node, pid 25898), api (node, pid 25901)`. It records
  them next to its lock, so that the next `lokal up` that finds such a port busy says
  `…, which web left running at the last lokal up` and how to stop it. lokal never stops these servers itself.

### Changed

- `lokal up` adopts a service it doesn't manage when its route is the one `lokal.toml` gives it: host
  `localhost`, the same host-header mode, and the file's `port`, or, when the file gives none, a port that no
  other service in the file fixes, which the service then keeps. So moving a project whose services you added by
  hand to a `lokal.toml`, or using one after `lokal run`, just works, and no URL moves. lokal says so once:
  `note: adopted api, web (added by hand or by lokal run, with the routes lokal.toml gives them); lokal down
  removes them now`. It used to stop with an error, and the only way past it was to remove the services so that
  `lokal up` could add the same routes back.
- A service `lokal up` doesn't manage whose route differs from the file's is still an error, and the error now
  says what differs, such as `(host 127.0.0.1, not localhost)`, or `(api: port 4000, not 9904; web: …)` for
  several. Its hint is one command for all of them, `lokal service remove api web -p shop`, instead of one
  command per service joined with `;`, so a script that split the old hint on `;` needs to change.
- `lokal up`'s ready line names the services it doesn't count as ready after the count:
  `✓ ready: 4/6 services accepting connections (6.5s); not ready: api (exited 0), web (exited 1)`. Servers that
  detached count, and are named too: `; detached: web, render`. A script that waits for `✓ ready: N/N` keeps
  working; one that matches the whole line needs to change.

### Fixed

- `lokal up`'s ready line counted a service whose command exited 0 by a race: as ready if its port had accepted a
  connection before lokal handled the exit, and not otherwise, so the same stack could print `1/2` or `2/2`. A
  service that has exited now holds the ready line until its exit line is printed, and counts by what its
  command left on its port.
- The wrong-port hint of `lokal run` and `lokal up` waits until a server's listeners settle: it prints only when
  two lookups in a row, 2 seconds apart, find the same ports. A server that opens helper listeners before its
  own port, such as Astro on Cloudflare's Vite plugin (Miniflare's inspector and loopback ports), no longer gets
  a hint a moment before it's ready. A server that really listens elsewhere gets it at 5 seconds instead of 3.
- The hint offers a port to switch to only when the server listens on exactly one. With several, it lists up to
  three, then how many more, and offers none, because one may be a debugger. It never offers to replace a port
  that `lokal.toml` fixes or that `lokal run --port` gave: it says the server isn't listening there instead.
- lokal keeps fields it doesn't know when it rewrites `state.toml`, at the top level, on a project and on a
  service, so an older lokal editing state no longer erases a newer lokal's settings. lokal 0.3.0 and earlier
  still drop them.
- `lokal up` retries a service on a new port only when its own port is taken. It used to read any
  `EADDRINUSE` or `address already in use` line as "this service's port was taken", also when the clash was on
  another port, such as workerd's debugger port 9229, and then said `note: w's port 53840 was taken` about a free
  port and retried in vain. Now, at the first such line, lokal checks the service's port: it's taken when a
  process outside the service's process group listens on it or has it bound, or when it accepts connections and
  no owner shows (another user's). Otherwise there's no retry, and after the exit line a hint says so: `hint: w says a port is in use, but its own port 53840 is free. Another port it opens may be shared
  with another service, such as a debugger port (9229): give each server its own. See
  https://mehmetkose.github.io/lokal/docs/guides/frameworks#several-node-or-workers-servers-debugger-ports`. A
  service with a fixed `port` gets the same hint, where it used to get only `exited 1`.

## [0.3.0] - 2026-10-25

### Added

- `lokal.toml`, `lokal up` and `lokal down`. A repo commits a `lokal.toml` that lists its services and their dev
  server commands. `lokal up` routes every service in it at once (each URL works before any command starts),
  then starts each command with `/bin/sh` in a process group of its own, in the service's `cwd`, with `PORT`,
  `{port}`, `LOKAL_URL` and `LOKAL_URL_<SUB>` as `lokal run` gives them. Each line of output is prefixed with the
  service's name, `✓ ready: N/N services accepting connections` prints once every service accepts connections,
  and Ctrl-C stops every service and everything it started, keeping the routes. Before running anything from a
  file, `lokal up` lists its commands and asks a person to approve them, once per file and again whenever the
  file changes; `--yes` runs them without asking and remembers nothing. `lokal up <SUB>...` starts only those
  services. One `lokal up` runs a project at a time. `lokal down` removes the services that `lokal up` added.
- Every service in the `--json` output of `list`, `project list`, `project show` and `service list` has a
  `managed` field: `"lokal.toml"` for a service that `lokal up` added from a repo's `lokal.toml`, `null`
  otherwise. `"schema"` stays 1.
- A log file for each service that `lokal up` starts, `~/Library/Logs/lokal/up/<project>/<sub>.log`: its output
  from both streams, each line after a timestamp in `daemon.log`'s format, and lokal's own lines about it
  (`lokal: started on port …`, `lokal: exited 1`, `lokal: stopping (SIGINT)`). Each `lokal up` starts it afresh,
  and it moves to `<sub>.log.1` at 5 MB. `lokal logs <SUB> [-p <PROJECT>] [-n <LINES>] [-f]` reads it, also from
  another terminal and after `lokal down`; the project comes from `-p`, the `lokal.toml` found from the current
  directory, or the current project. Plain `lokal logs` still shows the background service's log. When a .lokal
  URL fails, `lokal logs <sub> -n 50` shows that service's recent output.
- `lokal up` gives a service whose server listens on another port the hint `lokal run` gives, with the fix for
  `lokal.toml`: `hint: web listens on localhost:5173, not 51234. Set port = 5173 for web in lokal.toml, or put
  {port} in its run: vite --port {port}`. A service without a fixed `port` whose server exits with `EADDRINUSE`
  or `address already in use` before it's ready (something took the port after lokal chose it) is moved to a new
  free port and started again, once: `note: web's port 51234 was taken; retrying on 51302`. Both go into the
  service's log too.

## [0.2.0] - 2026-10-25

### Added

- `lokal run <SUBDOMAIN> -- <COMMAND>`: picks a free port (or reuses the service's), routes
  `https://<SUBDOMAIN>.<project>.lokal` to it, and starts your dev server on it with the terminal attached. The
  command gets `PORT`, `HOST`, `LOKAL_URL` and a `LOKAL_URL_<SUB>` for every service in the project, and `{port}` in
  any argument becomes the port. lokal prints `✓ ready: https://…` on stderr once the port accepts connections,
  hints when the server listens on a different port, lets Ctrl-C reach the server exactly once, and exits with the
  server's exit code. A port that's already in use stops it before anything starts. `--rm` removes the service
  when the command exits, if the run added it.

## [0.1.0] - 2026-10-24

The first public release. macOS 14 Sonoma or later. Tested on Apple Silicon. Intel builds are published but
haven't been tested on an Intel Mac. Licensed under Apache-2.0.

### Added

- `lokal setup`: one-time onboarding. It creates a name-constrained CA (`.lokal` and `.local` only, no IP
  addresses), trusts it in the System keychain for HTTPS only, installs `/etc/resolver/lokal`, installs the per-user
  LaunchAgent with launchd-owned sockets on 443 and 80, and runs a self-test. It is idempotent, `--dry-run`
  shows the exact sudo commands, and running it again repairs a broken install.
- `.lokal` names through a built-in DNS server (wildcard loopback answers, TTL 0), and `.local` names through
  mDNSResponder local-only records.
- Projects and services: `project create|list|show|use|remove`, `service add|list|update|remove`, `list`. Every
  change is live when the command returns (`ready in Nms`), and setup-type commands are safe to re-run.
- HTTPS reverse proxy: HTTP/1.1 and HTTP/2, WebSockets, server-sent events, streaming bodies, `localhost`
  upstreams on IPv4 or IPv6, `X-Forwarded-*` headers, `--host-header upstream` for Host-checking dev servers,
  308 redirects from HTTP, and 404, 421 and 502 pages.
- `lokal doctor`: checks every layer concurrently and gives a fix hint for each problem, including a check that
  `.lokal` isn't a public TLD.
- `lokal env` (Node, Python and curl trust), `ca path`, `open`, `status`, `logs`, `start`, `stop`, `restart`,
  `upgrade`, `uninstall [--purge]`.
- `--json` output for `list`, `status`, `doctor`, `project list|show` and `service list`.
- Prebuilt binaries for Apple Silicon and Intel Macs on GitHub Releases, with SHA-256 checksums, and a Homebrew
  tap: `brew install mehmetkose/tap/lokal`.

### Security

- The CA is trusted for HTTPS only (`add-trusted-cert -p ssl`), so macOS doesn't accept a certificate it signs
  for email (S/MIME), code signing or TLS client authentication. `lokal doctor` checks this (`ca.trust_scope`).
- `lokal env` and `lokal ca path` work only while lokal is set up. `lokal uninstall` deletes `bundle.pem` and
  reminds you to remove an `eval "$(lokal env)"` line from your shell profile, so Node, Python and curl stop
  trusting the CA once the keychain does.
- `lokal uninstall` and `lokal setup` find the CAs to untrust by certificate, so a CA whose key file was deleted
  (for example after a suspected leak) is still untrusted, and setup never writes a new CA over it. If the CA
  files are gone altogether, both find the CA that `install.json` records in the System keychain. `lokal doctor`
  names a missing key.
- Idle connections can't use up the background service's file descriptors. A connection that isn't proxying is
  closed after a deadline (20s before its first request, 60s idle). Client and upstream connections have
  separate caps, sized from an open-file limit of 10240; when the client cap is full, the connection idle
  longest makes room, and a request that would exceed the upstream cap gets a `503`. HTTP/2 is chosen by ALPN
  only, and port 80 speaks HTTP/1.1 only. Proxied requests and WebSockets have no timeout.

[Unreleased]: https://github.com/mehmetkose/lokal/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/mehmetkose/lokal/releases/tag/v0.4.0
[0.3.0]: https://github.com/mehmetkose/lokal/releases/tag/v0.3.0
[0.2.0]: https://github.com/mehmetkose/lokal/releases/tag/v0.2.0
[0.1.0]: https://github.com/mehmetkose/lokal/releases/tag/v0.1.0
