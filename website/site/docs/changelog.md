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
[compare `v0.1.0` with `main`](https://github.com/mehmetkose/lokal/compare/v0.1.0...HEAD).

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
