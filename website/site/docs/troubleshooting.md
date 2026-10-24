---
title: Troubleshooting
description: Fixes for certificate errors, 502 pages, names that don't resolve, port conflicts and setup problems.
section: help
order: 1
source: [docs/TROUBLESHOOTING.md, README.md, docs/SPEC.md]
---

**Run `lokal doctor` first.** It checks every layer and prints a fix for anything that fails: setup, the CA and its
trust, the background service and its build, the listeners, a full self-test, the `.lokal` resolver, and DNS, TLS
and the upstream for every service. Then look at the log:

```console
$ lokal doctor
$ lokal logs -n 200        # or -f to follow; the file is ~/Library/Logs/lokal/daemon.log
```

If nothing below helps, `lokal setup` is the repair path. It fixes whatever is missing, and asks for your password
only if a privileged step is missing.

## Node, Python or another tool rejects the certificate

Errors look like `unable to get local issuer certificate`, `self-signed certificate in certificate chain` or
`CERTIFICATE_VERIFY_FAILED`. Many tools ship their own CA bundle and ignore the macOS keychain. Run this in the
shell that starts them:

```console
$ eval "$(lokal env)"
```

This must happen **before** the process starts. Node reads `NODE_EXTRA_CA_CERTS` only at startup. Add the line to
your shell profile to make it permanent. Recent Node versions can also use the keychain directly with
`node --use-system-ca`.

[Trusting lokal from Node, Python and curl](/docs/guides/lokal-env) explains each variable.

## The browser shows a certificate error

- Run `lokal doctor`. If `ca.trusted` fails, run `lokal setup`. It re-trusts the CA and asks for your password.
- **Firefox** has its own trust store. Setup turns on Firefox's `security.enterprise_roots.enabled` preference in
  each profile. If you created a Firefox profile after setup, run `lokal setup` again.
- If only one name fails, with a handshake error rather than a certificate warning (curl:
  `tlsv1 alert access denied`), that name probably isn't configured. Check `lokal list`: lokal presents
  certificates only for configured names.

## A `502` page says the service is not responding

The page says something like `api.myapp.lokal → localhost:4000 is not responding. Is the service running?`.
lokal is working, but nothing answered on the upstream port within 2 seconds.

- Make sure your dev server is running, on the port in `lokal list`.
- `localhost` upstreams work whether the server listens on `127.0.0.1` or `::1`. If it listens on another
  address, set it: `lokal service update <sub> --host <address>`.
- `lokal doctor` shows `upstream.<name>` as `warn` when nothing is listening.

## A `404` page says the name is not configured

The page says `<name> is not configured`. The name resolves but has no route. (Over HTTPS, an unconfigured name
fails the TLS handshake instead.) The page lists the project's URLs. Add the service, or check for a typo in the
name.

## Vite says the host is not allowed

Vite shows "Blocked request. This host is not allowed" when it gets a `Host` header it doesn't recognise. Allow
your lokal names in `vite.config.js` (`server: { allowedHosts: ['.myapp.lokal'] }`), or switch the service to
send the upstream's own host with `lokal service update <sub> --host-header upstream`.

Other dev servers that check the `Host` header work the same way. See
[Using lokal with your framework](/docs/guides/frameworks#vite-and-other-dev-servers-that-check-the-host-header).

## Hot reload does not connect

lokal proxies WebSockets, so hot reload works through it. If your dev server's client connects to a hard-coded
URL such as `ws://localhost:5173` instead of the page's own origin, configure it to use the lokal URL (the page's
host, port 443, `wss://`). Check your dev server's documentation for its WebSocket URL setting.

## A `.lokal` name does not resolve

- `lokal doctor` checks `resolver.lokal`. If it fails, `/etc/resolver/lokal` is missing or wrong, so run
  `lokal setup`.
- `scutil --dns` should list a resolver for domain `lokal` with nameserver `127.0.0.1`, port 53535.
- The background service must be running (`lokal status`). It answers the DNS queries.
- Some VPN and DNS-filtering apps take over all DNS resolution. If `.lokal` names stop resolving while one is
  active, use a `.local` domain for the project instead (`lokal project create <name> --domain <name>.local`),
  which doesn't go through DNS. See [`.lokal` or `.local`?](/docs/guides/lokal-vs-local)

## A `.local` name is slow or does not resolve

- Unknown `.local` names take about 5 seconds to fail, because macOS waits for a multicast answer. Configured
  names answer in milliseconds.
- `lokal status` shows each name's state (`registered`, `pending` or `failed: <reason>`).
- If `/etc/hosts` has an entry for the same name, it can win. `lokal doctor` warns about this
  (`dns.conflict.<name>`).

## The background service is not running

Commands say `the background service isn't running — start it with 'lokal start'`. Start it and check:

```console
$ lokal start
$ lokal status
```

If it won't stay up, read `lokal logs -n 200`. You can also check launchd's view with
`launchctl print gui/$UID/dev.lokal.daemon`, and launchd's own output in `~/Library/Logs/lokal/launchd.err.log`.

## The background service runs a different build

Commands warn `the background service runs a different lokal build — run 'lokal upgrade'`. You updated the `lokal`
CLI, but the background service still runs its old copy. Switch it over:

```console
$ lokal upgrade
```

Run this after every `brew upgrade lokal`, or after you replace the binary by hand. `make install` does it for
you. See [Upgrade and uninstall](/docs/guides/upgrade-and-uninstall).

## Port 443 or 80 is already in use

Another program (another local proxy, a web server, Docker publishing port 443) holds the port. `lokal doctor`
names it in the `listeners` check, or you can look yourself:

```console
$ lsof -nP -iTCP:443 -sTCP:LISTEN
```

Stop that program or move it to another port, then run `lokal restart`.

## Setup refuses to run with sudo

Setup says `run 'lokal setup' as yourself, not with sudo`. Don't prefix it with `sudo`. lokal asks for your
password itself, for the steps that need it.

## Setup asks for your password twice

That's expected. The first prompt is sudo in Terminal. The second is a macOS dialog, because macOS confirms every
change to certificate trust settings separately.

## Setup fails with exit code 4

Setup needed sudo but couldn't ask for a password. Either no terminal is attached (for example, it ran from a
script or a coding agent), or `LOKAL_FORBID_SUDO=1` is set. Run `lokal setup` yourself in a Terminal window.

## `lokal doctor` fails `tld.lokal`

`.lokal` has become a real top-level domain in the public DNS, so lokal could hide real websites. Move your
projects to `.local` (`lokal project create <name> --domain <name>.local`).
[`.lokal` or `.local`?](/docs/guides/lokal-vs-local#using-local) shows how, and the
[security model](/docs/concepts/security-model#the-lokal-tld) explains the risk.

## Starting over

```console
$ lokal uninstall --purge   # removes everything, including your projects and the CA
$ lokal setup
```

Setup then creates a new CA, so it asks for your password again.

## Reporting a bug

Please [open an issue](https://github.com/mehmetkose/lokal/issues/new/choose) and include:

- `lokal --version` and `sw_vers`;
- `lokal doctor --json`;
- the relevant part of `lokal logs -n 200`;
- what you ran, what you expected, and what happened.

`lokal doctor --json` and the logs contain your project and service names. Remove anything you don't want to
share.

To report a security vulnerability, don't open a public issue. Report it privately through
[GitHub security advisories](https://github.com/mehmetkose/lokal/security/advisories/new) instead.
