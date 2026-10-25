# Changelog

All notable changes to lokal are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and lokal follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor versions may change the command
line. The `--json` output (`"schema": 1`) and `state.toml` (`version = 1`) stay backwards compatible until their
schema numbers change.

## [Unreleased]

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

[Unreleased]: https://github.com/mehmetkose/lokal/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/mehmetkose/lokal/releases/tag/v0.2.0
[0.1.0]: https://github.com/mehmetkose/lokal/releases/tag/v0.1.0
