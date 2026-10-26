---
title: Files and paths
description: Every file and setting lokal creates or changes, what each one is for, and which ones lokal uninstall removes.
section: reference
order: 2
source: [docs/SECURITY-MODEL.md, docs/SPEC.md, docs/ARCHITECTURE.md, docs/PACKAGING.md, crates/lokal/src/paths.rs, crates/lokal/src/cli/up_log.rs, crates/lokal/src/approval.rs, crates/lokal/src/setup/mod.rs, crates/lokal/src/platform/macos/launchd.rs, crates/lokal/src/platform/macos/resolver.rs]
---

lokal keeps almost everything in your home folder, owned by you. It changes two system settings, both during
`lokal setup`: it trusts its certificate authority in the System keychain, and it installs `/etc/resolver/lokal`.
Nothing else outside your home folder changes.

On this page, `<base>` stands for lokal's data directory, `~/Library/Application Support/lokal`. lokal creates it
with mode 0700, so only you can read it.

## Overview

| Path | What it is | Removed by |
|---|---|---|
| `<base>/state.toml` | Your projects and services | `uninstall --purge` |
| `<base>/ca/root.pem` | The CA certificate (mode 0644) | `uninstall --purge` |
| `<base>/ca/root-key.pem` | The CA's private key (mode 0600) | `uninstall --purge` |
| `<base>/ca/bundle.pem` | The macOS system roots plus lokal's CA, for `lokal env` | `uninstall --purge` |
| `<base>/bin/lokal` | The background service's own copy of the `lokal` binary | `uninstall` |
| `<base>/lokal.sock` | The socket the CLI uses to talk to the background service (mode 0600) | `uninstall` |
| `<base>/install.json` | A record of what setup did, which uninstall follows | `uninstall` |
| `<base>/resolver/lokal` | The resolver file as setup prepares it, before copying it to `/etc/resolver/lokal` | `uninstall` |
| `<base>/state.toml.lock`, `<base>/daemon.lock` | Lock files | `uninstall --purge` |
| `<base>/trusted/` | Your approvals of `lokal.toml` files, for `lokal up`: one file each (folder 0700, files 0600) | `uninstall --purge` |
| `~/Library/Logs/lokal/` | The background service's logs | `uninstall --purge` |
| `~/Library/Logs/lokal/up/<project>/up.lock` | `lokal up`'s lock for a project, holding its pid (0600, in a 0700 folder) | `uninstall --purge` |
| `~/Library/Logs/lokal/up/<project>/<sub>.log` | The output of a service that `lokal up` started, and `<sub>.log.1`, its older part (0600) | `uninstall --purge` |
| `~/Library/LaunchAgents/dev.lokal.daemon.plist` | The LaunchAgent that runs the background service | `uninstall` |
| `/etc/resolver/lokal` | Sends `.lokal` lookups to lokal. Owned by root | `uninstall` |
| `/Library/Keychains/System.keychain` | lokal's CA, trusted as a root. Changed as root | `uninstall` |
| `user.js` in each Firefox profile | One preference line, only if Firefox is installed | `uninstall` |
| `lokal.toml` in your repo | Your repo's services and their commands, for `lokal up`. lokal only reads it | never: it's yours |

The two root-owned changes are the only reason setup and uninstall ask for your password. The
[security model](/docs/concepts/security-model#privileged-commands) lists the exact commands.

## The state file

`<base>/state.toml` holds your projects and services. The first `lokal project create` writes it, and every
`project` and `service` command updates it, as do `lokal run`, `lokal up` and `lokal down`.

```toml
version = 1
current_project = "myproject"

[projects.myproject]
domain = "myproject.lokal"
created_at = "2026-10-10T12:00:00Z"

[projects.myproject.services.api]
host = "localhost"
port = 4000
host_header = "preserve"   # or "upstream"

[projects.myproject.services."@"]
host = "localhost"
port = 3000
host_header = "preserve"

[projects.myproject.services.web]
host = "localhost"
port = 51234
host_header = "preserve"
managed = "lokal.toml"     # only on services that lokal up added or adopted
```

- `current_project` is the project that `service` commands use when you don't pass `-p`.
- Each `[projects.<name>]` table has the project's `domain` and when it was created.
- Each `[projects.<name>.services.<subdomain>]` table routes one name to `host:port`. `"@"` is the project's own
  domain. `host_header` defaults to `preserve` if it's missing.
- `managed = "lokal.toml"` marks a service that `lokal up` added or adopted. A later `lokal up` updates or removes
  it, and `lokal down` removes it. Without it, the service was added by hand or by `lokal run`: `lokal down` never
  touches it, and `lokal up` only adopts it, when the `lokal.toml` gives it the same route
  ([details](/docs/guides/lokal-up#services-you-added-before-the-file)).
- A newer lokal may add fields. lokal keeps fields it doesn't know when it rewrites the file, so an older lokal
  doesn't erase a newer one's settings. lokal 0.3.0 and earlier drop them.

Only the CLI writes this file, and the background service only reads it. Each write takes an exclusive lock on
`state.toml.lock`, writes `state.toml.tmp`, syncs it to disk and renames it into place, so the file is never
half-written, and parallel commands never lose a change.

### Editing it by hand

You can edit `state.toml` by hand. The background service watches the file and applies your changes without a
CLI command. Some things to know:

- Hand edits skip the checks the CLI makes: the naming rules, overlapping domains, and ports 443 and 80. Prefer
  the CLI when you can.
- If the file doesn't parse, the background service logs the error (see `lokal logs`) and keeps serving what it
  had.
- Fields lokal doesn't know stay. But the CLI rewrites the whole file on each change, so comments don't survive
  the next `project` or `service` command.
- A file with a newer `version` than your lokal supports fails with
  `state file was written by a newer lokal; run a newer lokal`.

## The certificate authority

`<base>/ca/` holds lokal's CA, which setup creates as you:

- `root.pem` is the CA certificate, mode 0644. `lokal ca path` prints its path.
- `root-key.pem` is the CA's private key, mode 0600. lokal never copies, logs or sends it anywhere.
- `bundle.pem` is the macOS system roots plus lokal's CA. Setup writes it, and `lokal env` rebuilds it when it's
  older than 30 days. `lokal env` points `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE` and `CURL_CA_BUNDLE` at it.
- `retired/` appears only while setup replaces an outdated CA. It holds the old certificate (never its key)
  until the old CA is out of the keychain. Setup deletes it once that's done, and so does uninstall.

Leaf certificates, one per service, are issued in memory by the background service. They're never written to
disk or to the keychain.

## The background service

### The LaunchAgent

`~/Library/LaunchAgents/dev.lokal.daemon.plist` defines the background service, a per-user LaunchAgent with the
label `dev.lokal.daemon`. Setup writes it, and `lokal upgrade` rewrites it if it changed. It tells launchd to:

- run `<base>/bin/lokal daemon run` as you, when you log in (`RunAtLoad`), and keep it running (`KeepAlive`);
- restart it within a second if it crashes (`ThrottleInterval` 1, instead of launchd's default of 10 seconds);
- send its stdout and stderr to `~/Library/Logs/lokal/launchd.out.log` and `launchd.err.log`;
- open these sockets and hand them to it:

| Socket | Address | Protocol | Used for |
|---|---|---|---|
| `https4` | `127.0.0.1:443` | TCP | HTTPS |
| `https6` | `[::1]:443` | TCP | HTTPS |
| `http4` | `127.0.0.1:80` | TCP | Redirects to HTTPS |
| `http6` | `[::1]:80` | TCP | Redirects to HTTPS |
| `dns4` | `127.0.0.1:53535` | UDP | lokal's DNS server for `.lokal` |

Because launchd binds the ports, the background service never needs root, and every listener is on loopback.

### The binary copy

The LaunchAgent doesn't run the `lokal` you installed. It runs `<base>/bin/lokal`, a copy that `lokal setup` and
`lokal upgrade` make of the binary you run them with. So the CLI can live anywhere (`/opt/homebrew/bin`,
`~/.local/bin`), and nothing depends on its path.

It also means updating the package doesn't update the background service. After `brew upgrade lokal`, run
`lokal upgrade`. Until you do, lokal warns that the background service runs a different build.

### The IPC socket and the lock

- `<base>/lokal.sock` is a Unix socket, mode 0600, that the CLI uses to ask the background service for its status
  and to reload after a change. Only your user account can connect to it.
- `<base>/daemon.lock` makes sure only one copy of the background service runs at a time.

## Logs

`~/Library/Logs/lokal/` holds:

- `daemon.log`, the background service's log, with one line per proxied request. It rotates daily to
  `daemon.log.1` and `daemon.log.2`, so three files are kept. `lokal logs` shows it, and `lokal logs -f` follows it.
- `launchd.out.log` and `launchd.err.log`, the service's stdout and stderr as launchd captures them.
- `up/<project>/up.lock`, the lock that [`lokal up`](#lokal-ups-files) holds while it runs a project.
- `up/<project>/<sub>.log`, the output of each service that `lokal up` started. `lokal logs <sub>` shows it.
- `up/<project>/detached.json`, the servers that `lokal up`'s commands left running outside it.

## `lokal up`'s files

- **`lokal.toml`** is in your repo, not in lokal's folders. `lokal up` and `lokal down` look for it in the current
  directory, then in each parent, and use the first one they find. lokal only reads it. The
  [command reference](/docs/reference/commands#the-lokaltoml-file) describes the format.
- **`<base>/trusted/`** (mode 0700) holds one approval per `lokal.toml` that a person approved (mode 0600). Its
  name is the SHA-256 of the `lokal.toml`'s real path, and it holds the SHA-256 of the file's bytes and nothing
  else. When the file changes, the two no longer match, and `lokal up` asks again. The
  [security model](/docs/concepts/security-model#commands-from-a-lokaltoml) explains the check.
- **`~/Library/Logs/lokal/up/<project>/`** (mode 0700) holds `up.lock` (mode 0600). While `lokal up` runs a
  project, it holds a lock on that file and writes its pid into it, so that a second `lokal up`, or a `lokal down`,
  can say which process is running. The kernel releases the lock when lokal exits, also if it dies, so a stale file
  never blocks anything. lokal never signals the pid in it.
- **`~/Library/Logs/lokal/up/<project>/<sub>.log`** (mode 0600; `@.log` for the apex, `v1.api.log` for `v1.api`)
  holds the output of each service that `lokal up` started, each line after a timestamp, and lokal's own lines
  about it, marked `lokal:`. Each `lokal up` starts the log of each service it starts afresh, and at 5 MB the log
  moves to `<sub>.log.1`, so a service keeps at most about 10 MB. The logs stay after `lokal down`, until the next
  `lokal up` or `uninstall --purge`. [`lokal logs <sub>`](/docs/reference/commands#lokal-logs) reads them, and the
  [command reference](/docs/reference/commands#logs) has the format.
- **`~/Library/Logs/lokal/up/<project>/detached.json`** (mode 0600) lists the servers that a command started in
  the background and left running outside `lokal up`
  ([a server that detaches](/docs/reference/commands#a-server-that-detaches)): the service, the port, the pid and
  the command name of each. It's only for messages: when the next `lokal up` finds such a port still in use, the
  error says which service left the server there. lokal never signals a pid from it, because pids get reused. A
  `lokal up` drops the entries of the services it starts.

## System files

### The resolver file

`/etc/resolver/lokal` tells macOS to send every lookup under `.lokal` to lokal's DNS server. Setup installs it
with `sudo /usr/bin/install -m 0644`, so it's owned by root and readable by everyone:

```text
# Installed by lokal: .lokal names are answered by lokal's DNS server.
nameserver 127.0.0.1
port 53535
```

`.local` names need no file. lokal registers them with mDNSResponder instead.

### The System keychain

Setup adds lokal's CA certificate to `/Library/Keychains/System.keychain` and trusts it as a root for
HTTPS only (`security add-trusted-cert -d -r trustRoot -p ssl`). That's the only keychain change lokal makes, and after setup
nothing touches the keychain again. Uninstall untrusts the CA and deletes it (`security remove-trusted-cert` and
`security delete-certificate`).

### Firefox

Firefox keeps its own certificate store. If `/Applications/Firefox.app` exists, setup appends one line to
`user.js` in each profile under `~/Library/Application Support/Firefox/Profiles/`:

```js
user_pref("security.enterprise_roots.enabled", true); // added by lokal
```

Uninstall removes exactly that line. The [security model](/docs/concepts/security-model#firefox) explains what the
preference does.

### What lokal doesn't touch

lokal doesn't change `/etc/hosts`, `pf`, the Application Firewall, other launchd jobs or any other keychain item.
`lokal doctor` reads `/etc/hosts`, but only to warn about entries that conflict with your lokal names.

## What uninstall removes

| | `lokal uninstall` | `lokal uninstall --purge` |
|---|---|---|
| Background service: the LaunchAgent and `<base>/bin/lokal` | removed | removed |
| CA trust and the CA certificate in the System keychain | removed | removed |
| `/etc/resolver/lokal` | removed | removed |
| lokal's Firefox line | removed | removed |
| `install.json`, `lokal.sock`, `resolver/`, `ca/retired/` | removed | removed |
| `state.toml` (your projects and services) | kept | deleted |
| Approvals in `trusted/` | kept | deleted |
| CA files: `root.pem`, `root-key.pem`, `bundle.pem` | kept | deleted |
| Logs in `~/Library/Logs/lokal/` | kept | deleted |

A plain uninstall keeps your projects and the CA files, so running `lokal setup` later restores everything with
the same CA. Once the CA is no longer trusted, its key on disk can't make anything trusted. `--purge` deletes the
whole data directory, `~/Library/Application Support/lokal/`, and the log directory.

The `lokal` command itself stays where you installed it. Remove it with `brew uninstall lokal`, with
`make uninstall` if you built from source, or by deleting the file. Run `lokal uninstall` first: removing the
package doesn't undo setup. [Upgrade and uninstall](/docs/guides/upgrade-and-uninstall) has the full steps.

## The test sandbox

`LOKAL_HOME=<dir>` moves every path on this page that's under `<base>` into `<dir>`, and the logs into
`<dir>/logs`. There's no LaunchAgent in a sandbox, and the commands that manage the real background service
(`setup`, `uninstall`, `start`, `stop`, `restart`, `upgrade`) refuse to run. lokal's integration tests use it. It
isn't meant for daily use.
