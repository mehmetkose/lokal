# Troubleshooting

**Start with `lokal doctor`.** It checks every layer and prints a fix for anything that fails: setup, the CA and
its trust, the background service and its build, the listeners, a full self-test, the `.lokal` resolver, and
DNS, TLS and the upstream for every service. Then look at the log:

```console
$ lokal doctor
$ lokal logs -n 200        # or -f to follow; the file is ~/Library/Logs/lokal/daemon.log
```

On Linux, the log is `~/.local/state/lokal/daemon.log`.

If nothing below helps, `lokal setup` is the repair path: it fixes whatever is missing and asks for your
password only if a privileged step is missing.

This page describes macOS first. Problems that are specific to Linux (a preview) are under [On Linux](#on-linux).

## Certificates

### Node, Python or another tool says the certificate isn't trusted

Errors look like `unable to get local issuer certificate`, `self-signed certificate in certificate chain` or
`CERTIFICATE_VERIFY_FAILED`. Many tools ship their own CA bundle and ignore the macOS keychain (or, on Linux, the
system certificate store). Run this in the shell that starts them:

```console
$ eval "$(lokal env)"
```

This must happen **before** the process starts. Node reads `NODE_EXTRA_CA_CERTS` only at startup. Add the line to
your shell profile to make it permanent. Recent Node versions can also use the keychain directly with
`node --use-system-ca`.

### The browser shows a certificate error

- Run `lokal doctor`. If `ca.trusted` fails, run `lokal setup`. It re-trusts the CA and asks for your password.
- **Firefox** has its own trust store. Setup turns on Firefox's `security.enterprise_roots.enabled` preference in
  each profile. If you created a Firefox profile after setup, run `lokal setup` again.
- **On Linux**, Chrome and Firefox use their own certificate databases. See
  [Browsers don't trust lokal on Linux](#browsers-dont-trust-lokal-on-linux).
- If only one name fails, with a handshake error rather than a certificate warning (curl:
  `tlsv1 alert access denied`), that name probably isn't configured. Check `lokal list`: lokal presents
  certificates only for configured names.

## Pages and responses

### `502`: "… is not responding. Is the service running?"

lokal is working, but nothing answered on the upstream port within 2 seconds.

- Make sure your dev server is running, on the port in `lokal list`.
- If `lokal up` started it, `lokal logs <sub> -n 50` shows its recent output: whether it crashed, exited
  (`lokal: exited 1`) or is still starting.
- `localhost` upstreams work whether the server listens on `127.0.0.1` or `::1`. If it listens on another
  address, set it: `lokal service update <sub> --host <address>`.
- `lokal doctor` shows `upstream.<name>` as `warn` when nothing is listening.

### `404`: "… is not configured"

The name resolves but has no route. (Over HTTPS, an unconfigured name fails the TLS handshake instead.) The page
lists the project's URLs. Add the service, or check for a typo in the name.

### Vite says "Blocked request. This host is not allowed"

Vite rejects `Host` headers it doesn't recognise. Allow your lokal names in `vite.config.js`
(`server: { allowedHosts: ['.myapp.lokal'] }`), or use `lokal service update <sub> --host-header upstream`. Other
dev servers that check the `Host` header work the same way.

### Hot reload (WebSocket) doesn't connect

lokal proxies WebSockets, so hot reload works through it. If your dev server's client connects to a hard-coded
URL such as `ws://localhost:5173` instead of the page's own origin, configure it to use the lokal URL (the page's
host, port 443, `wss://`). Check your dev server's documentation for its WebSocket URL setting.

## Names

### A `.lokal` name doesn't resolve

- `lokal doctor` checks `resolver.lokal`. If it fails, `/etc/resolver/lokal` is missing or wrong, so run
  `lokal setup`.
- `scutil --dns` should list a resolver for domain `lokal` with nameserver `127.0.0.1`, port 53535.
- The background service must be running (`lokal status`). It answers the DNS queries.
- Some VPN and DNS-filtering apps take over all DNS resolution. If `.lokal` names stop resolving while one is
  active, use a `.local` domain for the project instead (`lokal project create <name> --domain <name>.local`),
  which doesn't go through DNS.
- On Linux, `.lokal` lookups go through systemd-resolved instead. See
  [`.lokal` names don't resolve on Linux](#lokal-names-dont-resolve-on-linux).

### A `.local` name is slow, or doesn't resolve

- On Linux, `.local` names don't work yet: lokal doesn't publish them there. Use `.lokal`.
- Unknown `.local` names take about 5 seconds to fail, because macOS waits for a multicast answer. Configured
  names answer in milliseconds.
- `lokal status` shows each name's state (`registered`, `pending` or `failed: <reason>`).
- If `/etc/hosts` has an entry for the same name, it can win. `lokal doctor` warns about this
  (`dns.conflict.<name>`).

## The background service

### "the background service isn't running"

```console
$ lokal start
$ lokal status
```

If it won't stay up, read `lokal logs -n 200`. You can also check launchd's view with
`launchctl print gui/$UID/dev.lokal.daemon` and launchd's own output in `~/Library/Logs/lokal/launchd.err.log`.
On Linux, systemd runs it instead: see [The background service on Linux](#the-background-service-on-linux).

### "the background service runs a different lokal build"

You updated the `lokal` CLI, but the background service still runs its old copy. Switch it over with:

```console
$ lokal upgrade
```

`make install` does this for you.

### Port 443 or 80 is already in use

Another program (another local proxy, a web server, Docker publishing port 443) holds the port. `lokal doctor`
names it in the `listeners` check, or you can look yourself:

```console
$ lsof -nP -iTCP:443 -sTCP:LISTEN
```

On Linux, use `ss -ltnp 'sport = :443'`. It names another user's process, such as a system web server, only when
you run it with `sudo`.

Stop that program or move it to another port, then run `lokal restart`.

## Setup

### "run 'lokal setup' as yourself, not with sudo"

Don't prefix it with `sudo`. lokal asks for your password itself, for the steps that need it.

### Setup asks for my password twice

That's expected on macOS. The first prompt is sudo in Terminal. The second is a macOS dialog, because macOS
confirms every change to certificate trust settings separately. On Linux, setup asks once.

### Setup fails with exit code 4

Setup needed sudo but couldn't ask for a password. Either no terminal is attached (for example, it ran from a
script or a coding agent), or `LOKAL_FORBID_SUDO=1` is set. Run `lokal setup` yourself in a Terminal window.

### `lokal doctor` fails `tld.lokal`

`.lokal` has become a real top-level domain in the public DNS, so lokal could hide real websites. Move your
projects to `.local` (`lokal project create <name> --domain <name>.local`). See the
[security model](SECURITY-MODEL.md#the-lokal-tld). On Linux, where `.local` names don't work yet, there's no such
move.

## On Linux

Linux is a preview. [`README.md`](../README.md#linux-preview) lists what setup needs there, and
[`PORTING.md`](PORTING.md#linux-whats-done) describes what it changes.

### Setup stops before it changes anything

Setup checks that each step can work on this machine first. If one can't, it stops with
`lokal can't be set up here yet:` and the reason, and nothing is changed (`lokal setup --dry-run` prints the plan,
then the reason, and exits 1):

- **`systemd-resolved isn't running`.** lokal sends `.lokal` lookups through systemd-resolved, version 258 or
  later. Ubuntu and Fedora run it by default.
- **`this systemd-resolved has no DNS delegates`.** systemd is older than 258, such as on Ubuntu 24.04 (255) or
  Debian 13 (257). Setup can't run there yet. Ubuntu 26.04 and Fedora 43 have systemd 258 or later.
- **`programs here don't ask systemd-resolved for names`.** The hosts line of `/etc/nsswitch.conf` has no
  `resolve`, and `/etc/resolv.conf` doesn't point at resolved's stub, so programs would never see lokal's names.
  On Debian and Ubuntu, `sudo apt install libnss-resolve` adds `resolve` to the hosts line. Under WSL, which
  writes its own `resolv.conf`, you need it.
- **`no system certificate store: install the ca-certificates package`.** Install `ca-certificates`.

Then run `lokal setup` again.

### apt or dnf refuses the package

The `.deb` and `.rpm` require systemd 258 or later, which setup needs, so apt and dnf refuse them where setup
couldn't run. apt's message names `systemd (>= 258)`. Ubuntu 24.04 has systemd 255, and Debian 13 has 257. Use
Ubuntu 26.04, Fedora 43 or later.

### Browsers don't trust lokal on Linux

Chrome, Chromium and Firefox on Linux don't read the system certificate store. Setup adds lokal's CA to their own
databases with `certutil`, and `lokal doctor` checks them in its `firefox` check:

- **`certutil is missing, so browsers don't trust lokal yet`.** Install `libnss3-tools` (Debian, Ubuntu) or
  `nss-tools` (Fedora), then run `lokal setup` again.
- **`Chrome has no certificate database yet`.** Chrome is installed but hasn't run yet. Run `lokal setup` again,
  which creates the database.
- **`<database> don't trust lokal's CA; run 'lokal setup'`.** A browser profile was created after setup, such as a
  new Firefox profile. Run `lokal setup` again.

### `.lokal` names don't resolve on Linux

`lokal doctor` checks the delegate in `resolver.lokal`:

- **`.lokal lookups aren't sent to lokal`.** `/etc/systemd/dns-delegate.d/lokal.dns-delegate` is missing. Run
  `lokal setup`.
- **`… exists but systemd-resolved hasn't loaded it`.** resolved reads delegates when it starts. Run
  `sudo systemctl restart systemd-resolved`, or `lokal setup`.
- **`programs here don't ask systemd-resolved for names`.** See
  [Setup stops before it changes anything](#setup-stops-before-it-changes-anything).

To look yourself, compare resolved's answer with the one other programs get:

```console
$ resolvectl query api.myapp.lokal    # asks systemd-resolved directly
$ getent ahosts api.myapp.lokal       # looks the name up the way other programs do
```

Both should give `127.0.0.1` and `::1`. If `resolvectl` does and `getent` doesn't, programs on this machine don't
ask resolved: install `libnss-resolve`. If neither does, run `lokal doctor`. It checks the delegate and the
background service, which answers the DNS queries.

### `.local` names don't work on Linux

lokal doesn't publish `.local` names on Linux yet. You can create a project under `.local`, but its names won't
resolve. Use `.lokal`, the default.

### The background service on Linux

The background service is a set of systemd user units in `~/.config/systemd/user/`: `dev.lokal.daemon.service`
and its three socket units, `dev.lokal.daemon-https.socket`, `dev.lokal.daemon-http.socket` and
`dev.lokal.daemon-dns.socket`. To see systemd's view:

```console
$ systemctl --user status dev.lokal.daemon.service dev.lokal.daemon-https.socket
```

`lokal logs` shows the service's log, `~/.local/state/lokal/daemon.log`. Its stdout and stderr are in the same
folder, in `launchd.out.log` and `launchd.err.log`.

If the HTTPS and HTTP socket units fail with `Permission denied`, user services may no longer bind ports below
1024. `sysctl net.ipv4.ip_unprivileged_port_start` should print 80 or lower. `lokal setup` sets it again.

## Starting over

```console
$ lokal uninstall --purge   # removes everything, including your projects and the CA
$ lokal setup
```

## Reporting a bug

Please [open an issue](https://github.com/mehmetkose/lokal/issues/new/choose) and include:

- `lokal --version` and `sw_vers` (on Linux, `cat /etc/os-release`);
- `lokal doctor --json`;
- the relevant part of `lokal logs -n 200`;
- what you ran, what you expected, and what happened.

`lokal doctor --json` and the logs contain your project and service names. Remove anything you don't want to
share.
